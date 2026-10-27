use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use gstreamer::{self as gst, glib, prelude::*};

use super::device::{self, LocalDevices};
use super::live::{self, LinkTracker, LiveInput};
use super::InputSource;
use crate::api::types::{LinkState, SourceCapabilitiesDto, SourceType, WhipSourceConfig};
use crate::pipeline::{loopback_http, loopback_port, make_el};

/// A WHIP ingest at `http://<node>:<port>/whip/endpoint`. `whipserversrc`
/// can't check a token, so it listens on loopback behind a [`Gate`].
/// Plays black and silence while nobody publishes.
pub struct WhipSource {
    id: String,
    name: String,
    config: WhipSourceConfig,
    devices: Arc<LocalDevices>,
    tracker: Arc<LinkTracker>,
    gate: OnceLock<Gate>,
}

impl WhipSource {
    pub fn new(
        id: String,
        name: String,
        config: WhipSourceConfig,
        devices: Arc<LocalDevices>,
    ) -> Self {
        Self {
            id,
            name,
            config,
            devices,
            tracker: Arc::default(),
            gate: OnceLock::new(),
        }
    }
}

impl InputSource for WhipSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }

    fn source_type(&self) -> SourceType {
        SourceType::Whip
    }

    fn capabilities(&self) -> Option<SourceCapabilitiesDto> {
        None
    }

    fn fingerprint(&self) -> String {
        serde_json::to_string(&self.config).unwrap_or_default()
    }

    fn build_bin(&self) -> Result<gst::Element> {
        let cfg = &self.config;
        let upstream = loopback_port().context("pick a loopback port")?;
        match self.gate.get() {
            Some(gate) => gate.upstream.store(upstream, Ordering::Relaxed),
            None => {
                let gate = Gate::start(cfg.port, cfg.token.clone(), upstream)?;
                let _ = self.gate.set(gate);
            }
        }
        let server = make_el("whipserversrc", &format!("whip-{}", self.id))?;
        let signaller = server.property::<glib::Object>("signaller");
        signaller.set_property("host-addr", format!("http://127.0.0.1:{upstream}"));
        // LAN ingest: don't contact a public STUN server.
        signaller.set_property("stun-server", None::<String>);
        let bin = live::build_bin(LiveInput {
            id: &self.id,
            source: server,
            audio: self.devices.audio(&cfg.audio).context("audio")?,
            format: cfg.format.as_ref(),
            tracker: &self.tracker,
            direct: true,
            latency: None,
        })?;
        Ok(bin.upcast())
    }

    fn link(&self) -> Option<LinkState> {
        Some(self.tracker.state(true))
    }
}

/// The publish port: refuses requests without the source's bearer token and
/// passes the rest to `whipserversrc` on loopback. Lives as long as the
/// source, across monitor restarts; `upstream` follows the current bin.
struct Gate {
    upstream: Arc<AtomicU16>,
    task: tokio::task::JoinHandle<()>,
}

struct GateState {
    token: String,
    upstream: Arc<AtomicU16>,
}

impl Gate {
    fn start(port: u16, token: String, upstream: u16) -> Result<Self> {
        let rt = tokio::runtime::Handle::try_current().context("no tokio runtime")?;
        let upstream = Arc::new(AtomicU16::new(upstream));
        let state = Arc::new(GateState {
            token,
            upstream: upstream.clone(),
        });
        let app = axum::Router::new().fallback(forward).with_state(state);
        let task = rt.spawn(async move {
            // A source replacing this one may still hold the port for a moment.
            let mut warned = false;
            let listener = loop {
                match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                    Ok(l) => break l,
                    Err(e) if !warned => {
                        tracing::warn!("WHIP port {port}: {e}; retrying");
                        warned = true;
                    }
                    Err(_) => {}
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            };
            if let Err(e) = axum::serve(listener, app).await {
                tracing::warn!("WHIP port {port}: {e}");
            }
        });
        Ok(Self { upstream, task })
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Hop-by-hop and per-connection headers, which don't pass through.
const NOT_FORWARDED: [header::HeaderName; 5] = [
    header::HOST,
    header::AUTHORIZATION,
    header::CONNECTION,
    header::CONTENT_LENGTH,
    header::TRANSFER_ENCODING,
];

/// Check the token (CORS preflights carry none), then forward.
async fn forward(State(gate): State<Arc<GateState>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    if parts.method != Method::OPTIONS && !authorized(&parts.headers, &gate.token) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response();
    }
    let Ok(body) = axum::body::to_bytes(body, 1 << 20).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let mut headers = parts.headers;
    for h in &NOT_FORWARDED {
        headers.remove(h);
    }
    let url = format!(
        "http://127.0.0.1:{}{}",
        gate.upstream.load(Ordering::Relaxed),
        parts.uri.path_and_query().map_or("/", |p| p.as_str())
    );
    let resp = match loopback_http()
        .request(parts.method, url)
        .headers(headers)
        .body(body)
        .send()
        .await
    {
        Ok(resp) => resp,
        // No monitor running, so nothing listening.
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let status = resp.status();
    let mut headers = resp.headers().clone();
    for h in &NOT_FORWARDED {
        headers.remove(h);
    }
    let body = resp.bytes().await.unwrap_or_default();
    (status, headers, body).into_response()
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|p| crate::auth::constant_time_eq(p.trim().as_bytes(), token.as_bytes()))
}

/// Check a config before it's saved, making a token if it has none.
pub fn validate(cfg: &mut WhipSourceConfig, devices: &LocalDevices) -> Result<(), String> {
    cfg.token = cfg.token.trim().to_string();
    if cfg.token.is_empty() {
        cfg.token = crate::auth::new_token();
    }
    if cfg.port < 1024 {
        return Err("use a port from 1024 up".into());
    }
    if cfg.token.len() < 16
        || cfg.token.len() > 128
        || !cfg
            .token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
    {
        return Err("the token needs 16 to 128 letters, digits or -._~+/=".into());
    }
    live::validate_format(cfg.format.as_ref())?;
    device::validate_audio(&cfg.audio, devices)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::api::types::{AudioPlan, MonitorSettingsDto};
    use crate::pipeline::monitor::MonitorPipeline;

    #[test]
    fn validate_rejects_zero_rate() {
        let cfg = |fps_den| WhipSourceConfig {
            port: 8443,
            token: "0123456789abcdef".into(),
            audio: AudioPlan::default(),
            format: Some(crate::api::types::LiveVideoFormat {
                width: 1920,
                height: 1080,
                fps_num: 30,
                fps_den,
            }),
        };
        gst::init().unwrap();
        let devices = LocalDevices::none();
        assert!(validate(&mut cfg(1), &devices).is_ok());
        assert!(validate(&mut cfg(0), &devices).is_err());
        let mut short = WhipSourceConfig {
            token: "short".into(),
            ..cfg(1)
        };
        assert!(validate(&mut short, &devices).is_err());
        let mut blank = WhipSourceConfig {
            token: " ".into(),
            ..cfg(1)
        };
        assert!(validate(&mut blank, &devices).is_ok(), "a token is made");
    }

    /// `whipclientsink` blocks on its own runtime while changing state.
    fn off_runtime(p: &gst::Element, state: gst::State) {
        std::thread::scope(|s| {
            s.spawn(|| p.set_state(state).unwrap());
        });
    }

    /// Publish, stop, and publish again: the source goes live, waits, and goes
    /// live again. Publishers without the token are refused.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real-time (~25 s); run with --ignored"]
    async fn publish_stop_publish() {
        gst::init().unwrap();
        gstfallbackswitch::plugin_register_static().unwrap();
        gstrswebrtc::plugin_register_static().unwrap();
        let port = 7743;
        let token = crate::auth::new_token();
        let src = WhipSource::new(
            "w".into(),
            "w".into(),
            WhipSourceConfig {
                port,
                token: token.clone(),
                audio: AudioPlan::default(),
                format: None,
            },
            Arc::new(LocalDevices::start()),
        );
        let mon = MonitorPipeline::new(
            &src,
            &MonitorSettingsDto::default(),
            &gst::SystemClock::obtain(),
        )
        .unwrap();
        let publish = || {
            let p = gst::parse::launch(&format!(
                "videotestsrc is-live=1 pattern=ball ! video/x-raw,width=640,height=360,framerate=30/1 ! queue ! ws. \
                 audiotestsrc is-live=1 ! queue ! ws. \
                 whipclientsink name=ws signaller::whip-endpoint=http://127.0.0.1:{port}/whip/endpoint \
                 signaller::auth-token={token}"
            ))
            .unwrap();
            off_runtime(&p, gst::State::Playing);
            p
        };
        let sleep = |s| tokio::time::sleep(Duration::from_secs(s));

        sleep(2).await;
        assert_eq!(src.link(), Some(LinkState::Waiting));
        let offer = |auth: Option<&str>| {
            let mut req = loopback_http()
                .post(format!("http://127.0.0.1:{port}/whip/endpoint"))
                .header("content-type", "application/sdp")
                .body("v=0\r\n");
            if let Some(auth) = auth {
                req = req.header("authorization", auth);
            }
            req.send()
        };
        for auth in [None, Some("Bearer wrong"), Some(token.as_str())] {
            let status = offer(auth).await.unwrap().status();
            assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "{auth:?}");
        }
        let p = publish();
        sleep(8).await;
        assert_eq!(src.link(), Some(LinkState::Live), "{:?}", mon.error());
        off_runtime(&p, gst::State::Null);
        drop(p);
        sleep(5).await;
        assert_eq!(src.link(), Some(LinkState::Waiting));
        let p = publish();
        sleep(10).await;
        assert_eq!(src.link(), Some(LinkState::Live), "{:?}", mon.error());
        off_runtime(&p, gst::State::Null);
        mon.stop().unwrap();
    }
}
