//! TLS: every node serves HTTPS with its own self-signed certificate, and a
//! controller trusts a node by its key, pinned when the node is approved.
//!
//! HTTPS and plain HTTP share the API port: a connection's first byte tells a
//! TLS handshake from a request. Plain HTTP is answered only from this machine
//! and for `/whep` (open, for players that can't trust a self-signed
//! certificate); anything else is redirected to HTTPS.
//!
//! The key is kept for good and the certificate renewed before it runs out:
//! browsers on macOS refuse one valid for more than 825 days. A pin is the
//! SHA-256 of the key (the certificate's SubjectPublicKeyInfo), so renewing
//! never breaks it.

use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Context as _, Result};
use axum::extract::{connect_info::Connected, ConnectInfo, Request};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::serve::{IncomingStream, Listener};
use chrono::{DateTime, Datelike, Utc};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::api::types::IdentityDto;
use crate::db;

const KEY_CONFIG: &str = "tls_key";
const CERT_CONFIG: &str = "tls_cert";
const EXPIRES_CONFIG: &str = "tls_cert_expires";
/// Under the 825 days macOS allows.
const VALID_DAYS: i64 = 800;
/// Renewed at startup once it has less than this left.
const RENEW_DAYS: i64 = 60;
/// A connection that hasn't sent its first byte, or finished its handshake,
/// in this long is dropped.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// The first byte of a TLS record carrying a handshake.
const TLS_HANDSHAKE: u8 = 0x16;
const IDENTIFY_TIMEOUT: Duration = Duration::from_secs(3);

/// This node's certificate and key.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    /// What controllers pin this node by.
    pub fingerprint: String,
}

/// This node's key and certificate: made on first run, the certificate
/// renewed when it's close to running out.
pub async fn load_identity(pool: &SqlitePool, hostname: Option<&str>) -> Result<Identity> {
    let key_pem = match db::config_get(pool, KEY_CONFIG).await? {
        Some(pem) => pem,
        None => {
            let pem = rcgen::KeyPair::generate()?.serialize_pem();
            db::config_set(pool, KEY_CONFIG, &pem).await?;
            info!("made this node's TLS key");
            pem
        }
    };
    let key_pair = rcgen::KeyPair::from_pem(&key_pem).context("read the TLS key")?;

    let expires = db::config_get(pool, EXPIRES_CONFIG)
        .await?
        .and_then(|e| DateTime::parse_from_rfc3339(&e).ok())
        .map(|e| e.with_timezone(&Utc));
    let cert_pem = match db::config_get(pool, CERT_CONFIG).await? {
        Some(pem) if expires.is_some_and(|e| e - Utc::now() > chrono::Duration::days(RENEW_DAYS)) => {
            pem
        }
        _ => {
            let (pem, expires) = issue(&key_pair, hostname)?;
            db::config_set(pool, CERT_CONFIG, &pem).await?;
            db::config_set(pool, EXPIRES_CONFIG, &expires.to_rfc3339()).await?;
            info!(%expires, "issued this node's TLS certificate");
            pem
        }
    };

    let cert = CertificateDer::from_pem_slice(cert_pem.as_bytes()).context("read the TLS certificate")?;
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).context("read the TLS key")?;
    let fingerprint = fingerprint(&cert).context("read this node's TLS certificate")?;
    Ok(Identity {
        cert,
        key,
        fingerprint,
    })
}

/// A certificate for `key`, named for this machine, and when it runs out.
fn issue(key: &rcgen::KeyPair, hostname: Option<&str>) -> Result<(String, DateTime<Utc>)> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Some(host) = hostname {
        names.push(format!("{host}.local"));
        names.push(host.to_string());
    }
    let mut params = rcgen::CertificateParams::new(names)?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Capture Room");
    let now = Utc::now();
    let ymd = |d: DateTime<Utc>| rcgen::date_time_ymd(d.year(), d.month() as u8, d.day() as u8);
    params.not_before = ymd(now - chrono::Duration::days(1));
    let expires = now + chrono::Duration::days(VALID_DAYS);
    params.not_after = ymd(expires);
    // macOS wants a server certificate to say so.
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    Ok((params.self_signed(key)?.pem(), expires))
}

/// The pin for a certificate: the SHA-256 of its key, in hex.
pub fn fingerprint(cert: &[u8]) -> Option<String> {
    let der = CertificateDer::from(cert);
    let cert = webpki::EndEntityCert::try_from(&der).ok()?;
    Some(hex::encode(Sha256::digest(
        cert.subject_public_key_info().as_ref(),
    )))
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn server_config(identity: &Identity) -> Result<Arc<rustls::ServerConfig>> {
    let mut config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.clone()], identity.key.clone_key())?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// The keys of the nodes this controller approved: the only ones it sends
/// its access token to. They all hold the token already, so any of them
/// will do for any request; a request still names the node it's for.
#[derive(Debug, Default)]
pub struct Pins(RwLock<HashSet<String>>);

impl Pins {
    pub fn insert(&self, fingerprint: &str) {
        self.0.write().unwrap().insert(fingerprint.to_string());
    }

    pub fn remove(&self, fingerprint: &str) {
        self.0.write().unwrap().remove(fingerprint);
    }

    /// Trust none: the controller was turned off.
    pub fn clear(&self) {
        self.0.write().unwrap().clear();
    }

    pub(crate) fn contains(&self, fingerprint: &str) -> bool {
        self.0.read().unwrap().contains(fingerprint)
    }
}

/// Checks a server's certificate against [`Pins`], or with `None` takes any
/// (to learn who a node is, never to send it the token). Either way the
/// handshake has to be signed by the certificate's key.
#[derive(Debug)]
struct Verifier {
    pins: Option<Arc<Pins>>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let Some(pins) = &self.pins else {
            return Ok(ServerCertVerified::assertion());
        };
        match fingerprint(end_entity) {
            Some(fp) if pins.contains(&fp) => Ok(ServerCertVerified::assertion()),
            Some(_) => Err(rustls::Error::General(
                "not the key of an approved node".into(),
            )),
            None => Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn client_config(pins: Option<Arc<Pins>>) -> Arc<rustls::ClientConfig> {
    let provider = provider();
    let algorithms = provider.signature_verification_algorithms;
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Verifier { pins, algorithms }))
        .with_no_client_auth();
    Arc::new(config)
}

/// How this node talks to other nodes.
pub struct Peers {
    pub pins: Arc<Pins>,
    /// To approved nodes only: requests carry the access token.
    pub http: reqwest::Client,
    /// To any node, for its identity: never carries the token.
    identify: reqwest::Client,
    ws: Arc<rustls::ClientConfig>,
}

impl Peers {
    pub fn new() -> Result<Self> {
        let pins = Arc::new(Pins::default());
        let ws = client_config(Some(Arc::clone(&pins)));
        let http = reqwest::Client::builder()
            .use_preconfigured_tls((*ws).clone())
            .build()?;
        let identify = reqwest::Client::builder()
            .use_preconfigured_tls((*client_config(None)).clone())
            .tls_info(true)
            .build()?;
        Ok(Self {
            pins,
            http,
            identify,
            ws,
        })
    }

    /// For WebSockets to approved nodes.
    pub fn ws_connector(&self) -> tokio_tungstenite::Connector {
        tokio_tungstenite::Connector::Rustls(Arc::clone(&self.ws))
    }

    /// Who the node at `url` says it is, and the key it holds.
    pub async fn identify(&self, url: &str) -> Result<(IdentityDto, String)> {
        let resp = self
            .identify
            .get(format!("{url}/api/v1/auth/identity"))
            .timeout(IDENTIFY_TIMEOUT)
            .send()
            .await?
            .error_for_status()?;
        let fingerprint = resp
            .extensions()
            .get::<reqwest::tls::TlsInfo>()
            .and_then(|info| info.peer_certificate())
            .and_then(fingerprint)
            .context("the node has no TLS certificate")?;
        Ok((resp.json().await?, fingerprint))
    }
}

/// A connection's peer address, and whether it came over TLS.
#[derive(Debug, Clone, Copy)]
pub struct Conn {
    pub addr: SocketAddr,
    pub tls: bool,
}

impl Connected<IncomingStream<'_, TlsListener>> for Conn {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Self {
        *stream.remote_addr()
    }
}

/// The API port: TLS and plain connections, told apart by their first byte.
pub struct TlsListener {
    accepted: mpsc::Receiver<(Io, Conn)>,
    local: Conn,
}

impl TlsListener {
    pub fn new(tcp: TcpListener, config: Arc<rustls::ServerConfig>) -> io::Result<Self> {
        let local = Conn {
            addr: tcp.local_addr()?,
            tls: true,
        };
        let (tx, accepted) = mpsc::channel(64);
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        tokio::spawn(async move {
            loop {
                match tcp.accept().await {
                    // Handshakes run beside each other: a slow client mustn't
                    // hold up the rest.
                    Ok((stream, addr)) => {
                        let tx = tx.clone();
                        let acceptor = acceptor.clone();
                        tokio::spawn(async move {
                            if let Some(io) = classify(stream, &acceptor, addr).await {
                                let tls = matches!(io, Io::Tls(_));
                                let _ = tx.send((io, Conn { addr, tls })).await;
                            }
                        });
                    }
                    // Out of file descriptors and the like: wait, as axum does.
                    Err(e) => {
                        warn!(error = %e, "accepting a connection failed");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
        Ok(Self { accepted, local })
    }
}

impl Listener for TlsListener {
    type Io = Io;
    type Addr = Conn;

    async fn accept(&mut self) -> (Io, Conn) {
        match self.accepted.recv().await {
            Some(conn) => conn,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Conn> {
        Ok(self.local)
    }
}

async fn classify(
    stream: TcpStream,
    acceptor: &tokio_rustls::TlsAcceptor,
    addr: SocketAddr,
) -> Option<Io> {
    let mut first = [0u8; 1];
    let read = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first)).await;
    if !matches!(read, Ok(Ok(1))) {
        return None;
    }
    if first[0] != TLS_HANDSHAKE {
        return Some(Io::Plain(stream));
    }
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
        Ok(Ok(tls)) => Some(Io::Tls(Box::new(tls))),
        // Mostly a browser that hasn't accepted the certificate yet.
        Ok(Err(e)) => {
            debug!(%addr, error = %e, "TLS handshake failed");
            None
        }
        Err(_) => None,
    }
}

pub enum Io {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl AsyncRead for Io {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Io::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Io {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Io::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Io::Tls(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Io::Plain(s) => s.is_write_vectored(),
            Io::Tls(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_flush(cx),
            Io::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Io::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Io::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Whether a plain-HTTP request is answered rather than sent to HTTPS: it
/// comes from this machine, or it's for `/whep`.
fn plain_allowed(conn: &Conn, path: &str) -> bool {
    conn.addr.ip().is_loopback() || path.starts_with("/whep/")
}

/// Middleware: plain HTTP from another machine is redirected to HTTPS, so
/// the token never crosses the network in the clear.
pub async fn redirect_plain(
    ConnectInfo(conn): ConnectInfo<Conn>,
    req: Request,
    next: Next,
) -> Response {
    if conn.tls || plain_allowed(&conn, req.uri().path()) {
        return next.run(req).await;
    }
    let Some(host) = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
    else {
        return (StatusCode::BAD_REQUEST, "use HTTPS").into_response();
    };
    let path = req.uri().path_and_query().map_or("/", |p| p.as_str());
    Redirect::permanent(&format!("https://{host}{path}")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(ip: [u8; 4], tls: bool) -> Conn {
        Conn {
            addr: SocketAddr::from((ip, 50000)),
            tls,
        }
    }

    #[test]
    fn plain_http_only_locally_or_for_whep() {
        assert!(plain_allowed(&conn([127, 0, 0, 1], false), "/api/v1/nodes"));
        assert!(plain_allowed(&conn([192, 168, 1, 5], false), "/whep/program"));
        assert!(!plain_allowed(&conn([192, 168, 1, 5], false), "/api/v1/nodes"));
        assert!(!plain_allowed(&conn([192, 168, 1, 5], false), "/"));
        assert!(!plain_allowed(&conn([192, 168, 1, 5], false), "/whepx"));
    }

    #[test]
    fn a_renewed_certificate_keeps_its_pin() {
        let key = rcgen::KeyPair::generate().unwrap();
        let pin = |pem: String| {
            fingerprint(&CertificateDer::from_pem_slice(pem.as_bytes()).unwrap()).unwrap()
        };
        let first = pin(issue(&key, Some("a")).unwrap().0);
        let renewed = pin(issue(&key, Some("b")).unwrap().0);
        let other = pin(issue(&rcgen::KeyPair::generate().unwrap(), Some("a")).unwrap().0);
        assert_eq!(first, renewed);
        assert_ne!(first, other);
        assert_eq!(first.len(), 64);
    }
}
