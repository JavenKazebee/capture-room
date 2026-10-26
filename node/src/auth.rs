//! Access token: one shared secret for a whole cluster. Every node makes its
//! own the first time it starts, so none is ever open; every `/api` route
//! and `/ws/…` needs it.
//!
//! Browsers sign in once and carry it in an HttpOnly cookie, so `<img>`,
//! `<video>`, downloads and the WebSocket work without code of their own. On
//! the node's own machine a one-time link signs them in (the menu bar's Open
//! Capture Room). Nodes send it to each other as `Authorization: Bearer`.
//!
//! A controller adds a node with the node's pairing code, which the node's
//! menu bar shows: the code lets the controller give it the cluster's token.
//! The UI assets, `/api/v1/auth/…` and `/whep` (the program, like RTSP) stay
//! open.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{ConnectInfo, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    AccessTokenDto, AccessTokenFailure, AccessTokenRequest, AccessTokenResultDto, AuthStatusDto,
    IdentityDto, PairRequest, SignInRequest,
};
use crate::db;
use crate::state::AppState;
use crate::tls::Conn;

const CONFIG_KEY: &str = "access_token";
const PAIRING_KEY: &str = "pairing_code";
const LINKS_KEY: &str = "sign_in_links";
const MIN_LEN: usize = 16;
const MAX_LEN: usize = 128;
/// Chrome's cap on a cookie's lifetime: an operator's browser stays signed in.
const COOKIE_MAX_AGE: u64 = 400 * 24 * 60 * 60;
/// Slows guessing; a token is long enough that this is a backstop.
const WRONG_TOKEN_DELAY: Duration = Duration::from_secs(1);
/// Wrong pairing codes before the node makes a new one: with ~40 bits per
/// code, guessing one this way is hopeless.
const PAIRING_ATTEMPTS: u32 = 5;
/// How long a sign-in link works: long enough for a browser to open.
const LINK_TTL_SECS: u64 = 60;
/// How long the link a node without a menu bar prints works: long enough to
/// notice it in the terminal.
const TERMINAL_LINK_TTL_SECS: u64 = 10 * 60;

/// New tokens: letters and digits that can't be mistaken for each other.
const TOKEN_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
const TOKEN_LEN: usize = 32;
/// Pairing codes are read off one screen and typed into another: capitals
/// and digits only, none that look alike.
const PAIRING_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const PAIRING_LEN: usize = 8;

/// A peer refused this node's token: it has a different one (or none of ours).
#[derive(Debug)]
pub struct AccessDenied;

impl std::fmt::Display for AccessDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("that node uses a different access token")
    }
}

impl std::error::Error for AccessDenied {}

/// This node's token, and the pairing code a controller adds it with.
pub struct AccessToken {
    /// `None` only until [`load`]: nothing gets in then.
    token: std::sync::RwLock<Option<String>>,
    changed: tokio::sync::watch::Sender<()>,
    pairing: Mutex<Pairing>,
}

#[derive(Default)]
struct Pairing {
    code: String,
    failures: u32,
}

/// What a pairing code someone presented turned out to be.
#[derive(Debug, PartialEq)]
enum Attempt {
    Right,
    Wrong,
    /// Wrong, and the code has had all its tries: make a new one.
    Exhausted,
}

impl Default for AccessToken {
    fn default() -> Self {
        Self {
            token: Default::default(),
            changed: tokio::sync::watch::channel(()).0,
            pairing: Default::default(),
        }
    }
}

impl AccessToken {
    /// The token itself, for the menu bar's Copy Access Token.
    pub fn get(&self) -> Option<String> {
        self.token.read().unwrap().clone()
    }

    fn set(&self, token: String) {
        *self.token.write().unwrap() = Some(token);
        self.changed.send_replace(());
    }

    /// Fires when the token changes: WebSockets signed in with the old one
    /// close, and their clients reconnect with (or sign in for) the new one.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<()> {
        self.changed.subscribe()
    }

    /// Whether `presented` is this node's token.
    fn accepts(&self, presented: Option<&str>) -> bool {
        match (self.token.read().unwrap().as_deref(), presented) {
            (Some(token), Some(p)) => constant_time_eq(p.as_bytes(), token.as_bytes()),
            _ => false,
        }
    }

    /// The `Authorization` header for requests to peers.
    pub fn bearer(&self) -> Option<HeaderValue> {
        let token = self.token.read().unwrap();
        let mut value = HeaderValue::from_str(&format!("Bearer {}", token.as_deref()?)).ok()?;
        value.set_sensitive(true);
        Some(value)
    }

    /// The pairing code as people see it (`ABCD-EFGH`); empty before [`load`].
    pub fn pairing_code(&self) -> String {
        format_pairing_code(&self.pairing.lock().unwrap().code)
    }

    fn set_pairing_code(&self, code: String) {
        *self.pairing.lock().unwrap() = Pairing { code, failures: 0 };
    }

    /// Check a pairing code, counting wrong ones. A code that's had all its
    /// tries accepts nothing until it's replaced.
    fn attempt(&self, presented: &str) -> Attempt {
        let mut pairing = self.pairing.lock().unwrap();
        if pairing.code.is_empty() || pairing.failures >= PAIRING_ATTEMPTS {
            return Attempt::Wrong;
        }
        let presented = normalize_pairing_code(presented);
        if constant_time_eq(presented.as_bytes(), pairing.code.as_bytes()) {
            return Attempt::Right;
        }
        pairing.failures += 1;
        if pairing.failures >= PAIRING_ATTEMPTS {
            Attempt::Exhausted
        } else {
            Attempt::Wrong
        }
    }
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `len` characters from `alphabet`, from the OS's random source. Bytes that
/// would favour some characters over others are skipped.
fn random_string(alphabet: &[u8], len: usize) -> String {
    let limit = 256 - 256 % alphabet.len();
    let mut out = String::with_capacity(len);
    let mut bytes = [0u8; 64];
    while out.len() < len {
        getrandom::fill(&mut bytes).expect("the OS has a random source");
        for &b in &bytes {
            if (b as usize) < limit && out.len() < len {
                out.push(alphabet[b as usize % alphabet.len()] as char);
            }
        }
    }
    out
}

/// A new access token (about 187 bits).
pub fn new_token() -> String {
    random_string(TOKEN_ALPHABET, TOKEN_LEN)
}

fn new_pairing_code() -> String {
    random_string(PAIRING_ALPHABET, PAIRING_LEN)
}

/// `ABCDEFGH` as `ABCD-EFGH`.
fn format_pairing_code(code: &str) -> String {
    match code.len() {
        PAIRING_LEN => format!("{}-{}", &code[..4], &code[4..]),
        _ => code.to_string(),
    }
}

/// A code as typed: any case, with or without the dash or spaces.
fn normalize_pairing_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Read the stored token and pairing code into `state`, making them the
/// first time the node starts.
pub async fn load(state: &AppState) -> anyhow::Result<()> {
    let token = match db::config_get(&state.db, CONFIG_KEY).await? {
        Some(token) => token,
        None => {
            let token = new_token();
            db::config_set(&state.db, CONFIG_KEY, &token).await?;
            info!("made this node's access token");
            token
        }
    };
    state.access_token.set(token);
    let code = match db::config_get(&state.db, PAIRING_KEY).await? {
        Some(code) => code,
        None => replace_pairing_code(&state.db).await?,
    };
    state.access_token.set_pairing_code(code);
    Ok(())
}

/// Store a new pairing code; the old one stops working.
async fn replace_pairing_code(pool: &SqlitePool) -> anyhow::Result<String> {
    let code = new_pairing_code();
    db::config_set(pool, PAIRING_KEY, &code).await?;
    Ok(code)
}

async fn renew_pairing_code(state: &AppState) {
    match replace_pairing_code(&state.db).await {
        Ok(code) => state.access_token.set_pairing_code(code),
        Err(e) => warn!(error = %e, "couldn't store a new pairing code"),
    }
}

/// Routes anyone can call: what a browser needs to sign in, and what a
/// controller needs to pair.
pub fn open_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/auth", get(get_auth))
        .route("/api/v1/auth/identity", get(get_identity))
        .route("/api/v1/auth/sign-in", post(post_sign_in))
        .route("/api/v1/auth/sign-out", post(post_sign_out))
        .route("/api/v1/auth/link", get(get_sign_in_link))
        .route("/api/v1/auth/pair", post(post_pair))
}

/// Shows this node's token, and sets it here and on every node this
/// controller has.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/api/v1/access-token",
        get(get_access_token).put(put_access_token),
    )
}

/// Part of the node API: sets this node's token only.
pub fn node_router() -> Router<Arc<AppState>> {
    Router::new().route("/access-token", put(put_node_access_token))
}

/// The routes under `/api` that work without the token.
const OPEN_PATHS: [&str; 6] = [
    "/api/v1/auth",
    "/api/v1/auth/identity",
    "/api/v1/auth/sign-in",
    "/api/v1/auth/sign-out",
    "/api/v1/auth/link",
    "/api/v1/auth/pair",
];

/// Whether `path` needs the token: all of `/api` and `/ws` but the open routes.
fn guarded(path: &str) -> bool {
    (path.starts_with("/api/") && !OPEN_PATHS.contains(&path))
        || path == "/ws"
        || path.starts_with("/ws/")
}

/// Middleware: refuse `/api` and `/ws` without the token, except the open
/// routes. Page loads stay open so the UI can show its sign-in.
pub async fn require(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if guarded(req.uri().path())
        && !state
            .access_token
            .accepts(presented(&state, req.headers()).as_deref())
    {
        return (
            StatusCode::UNAUTHORIZED,
            "sign in with this node's access token",
        )
            .into_response();
    }
    next.run(req).await
}

/// The token a request carries: a bearer header, else the sign-in cookie.
fn presented(state: &AppState, headers: &HeaderMap) -> Option<String> {
    if let Some(bearer) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return Some(bearer.trim().to_string());
    }
    let name = cookie_name(state);
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// Cookies are per host, not per port: two nodes on one machine keep theirs
/// apart by port.
fn cookie_name(state: &AppState) -> String {
    format!("capture_room_token_{}", state.port)
}

/// `Set-Cookie` signing this browser in with `token`, or out with `None`.
/// Over TLS it's `Secure`, so the browser never sends it in the clear; plain
/// HTTP is only answered from this machine.
fn cookie(state: &AppState, conn: Conn, token: Option<&str>) -> HeaderValue {
    let name = cookie_name(state);
    let secure = if conn.tls { "; Secure" } else { "" };
    let value = match token {
        Some(t) => format!(
            "{name}={t}; Path=/; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE}{secure}"
        ),
        None => format!("{name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}"),
    };
    HeaderValue::from_str(&value).expect("token characters are checked")
}

async fn get_identity(State(state): State<Arc<AppState>>) -> Json<IdentityDto> {
    Json(IdentityDto {
        id: state.node_id.clone(),
        name: state.node_name(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        fingerprint: state.fingerprint.clone(),
    })
}

async fn get_auth(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Json<AuthStatusDto> {
    Json(AuthStatusDto {
        required: true,
        signed_in: state
            .access_token
            .accepts(presented(&state, &headers).as_deref()),
    })
}

async fn post_sign_in(
    State(state): State<Arc<AppState>>,
    ConnectInfo(conn): ConnectInfo<Conn>,
    Json(req): Json<SignInRequest>,
) -> Response {
    let token = req.token.trim();
    if !state.access_token.accepts(Some(token)) {
        tokio::time::sleep(WRONG_TOKEN_DELAY).await;
        return (
            StatusCode::UNAUTHORIZED,
            "that isn't this node's access token",
        )
            .into_response();
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, cookie(&state, conn, Some(token)));
    resp
}

async fn post_sign_out(
    State(state): State<Arc<AppState>>,
    ConnectInfo(conn): ConnectInfo<Conn>,
) -> Response {
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, cookie(&state, conn, None));
    resp
}

/// A sign-in link waiting to be opened, by the SHA-256 of its secret.
#[derive(Serialize, Deserialize)]
struct Link {
    hash: String,
    expires: u64,
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn link_hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// Change the current sign-in links. The service and the app opened from the
/// desktop both do, so each change is one transaction.
async fn update_links<T>(
    pool: &SqlitePool,
    update: impl FnOnce(&mut Vec<Link>) -> T,
) -> anyhow::Result<T> {
    db::config_update(pool, LINKS_KEY, |json| {
        let now = unix_secs();
        let mut links: Vec<Link> = json
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        links.retain(|l| l.expires > now);
        let out = update(&mut links);
        Ok((serde_json::to_string(&links)?, out))
    })
    .await
}

/// A link that signs a browser on this machine in once, within a minute:
/// what Open Capture Room opens. Kept in the database (hashed), so the app
/// opened from the Finder, another process, can make one for the service.
pub async fn sign_in_link(pool: &SqlitePool, port: u16) -> anyhow::Result<String> {
    make_sign_in_link(pool, port, LINK_TTL_SECS).await
}

/// A node with no menu bar (run from a terminal, as `pnpm dev` does) prints
/// a sign-in link instead: nothing else signs a browser in without the token.
/// Printed, not logged, so it stands apart from the log lines and stays out
/// of the Logs view.
pub async fn print_sign_in_link(state: &AppState) {
    match make_sign_in_link(&state.db, state.port, TERMINAL_LINK_TTL_SECS).await {
        Ok(url) => println!(
            "\n  Sign in on this machine (once, within 10 minutes):\n  {url}\n  \
             Or print the token: capture-room access-token show\n"
        ),
        Err(e) => warn!(error = %e, "couldn't make a sign-in link"),
    }
}

async fn make_sign_in_link(pool: &SqlitePool, port: u16, ttl_secs: u64) -> anyhow::Result<String> {
    let secret = new_token();
    let link = Link {
        hash: link_hash(&secret),
        expires: unix_secs() + ttl_secs,
    };
    update_links(pool, |links| {
        links.push(link);
        // A few at once is plenty: the menu clicked again before the first opened.
        let excess = links.len().saturating_sub(8);
        links.drain(..excess);
    })
    .await?;
    Ok(format!(
        "http://localhost:{port}/api/v1/auth/link?secret={secret}"
    ))
}

/// Use up the link with `secret`: whether it was there and still current.
async fn take_sign_in_link(pool: &SqlitePool, secret: &str) -> anyhow::Result<bool> {
    let hash = link_hash(secret);
    update_links(pool, |links| {
        let before = links.len();
        links.retain(|l| !constant_time_eq(l.hash.as_bytes(), hash.as_bytes()));
        links.len() < before
    })
    .await
}

#[derive(Deserialize)]
struct LinkQuery {
    secret: String,
}

/// Open a sign-in link: signed in and on to the UI, or to the UI's sign-in
/// if the link is used up or expired. Only from this machine: the link is
/// for whoever is at it.
async fn get_sign_in_link(
    State(state): State<Arc<AppState>>,
    ConnectInfo(conn): ConnectInfo<Conn>,
    Query(q): Query<LinkQuery>,
) -> Response {
    let mut resp = Redirect::to("/").into_response();
    if !conn.addr.ip().is_loopback() {
        warn!(from = %conn.addr, "a sign-in link opened from another machine: ignored");
        return resp;
    }
    match take_sign_in_link(&state.db, &q.secret).await {
        Ok(true) => {
            if let Some(token) = state.access_token.get() {
                resp.headers_mut()
                    .insert(header::SET_COOKIE, cookie(&state, conn, Some(&token)));
            }
        }
        Ok(false) => info!("a used or expired sign-in link was opened"),
        Err(e) => warn!(error = %e, "couldn't check a sign-in link"),
    }
    resp
}

/// A controller adding this node: with the right pairing code, the token it
/// sends becomes this node's. The code is used up either way after a few
/// wrong tries, and on success.
async fn post_pair(State(state): State<Arc<AppState>>, Json(req): Json<PairRequest>) -> Response {
    match state.access_token.attempt(&req.code) {
        Attempt::Right => {}
        attempt => {
            if attempt == Attempt::Exhausted {
                warn!("too many wrong pairing codes: made a new one");
                renew_pairing_code(&state).await;
            }
            tokio::time::sleep(WRONG_TOKEN_DELAY).await;
            return (StatusCode::FORBIDDEN, "that isn't this node's pairing code").into_response();
        }
    }
    let token = match validate(&req.token) {
        Ok(token) => token,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = store(&state, token).await {
        return e.into_response();
    }
    renew_pairing_code(&state).await;
    info!("paired with a controller: now using its access token");
    StatusCode::NO_CONTENT.into_response()
}

/// A token to store: trimmed, long enough to resist guessing, and only
/// characters that are safe in a cookie and a header.
fn validate(token: &str) -> ApiResult<String> {
    let token = token.trim();
    if token.is_empty() {
        return Err(ApiError::bad("an access token can be changed, not removed"));
    }
    if !(MIN_LEN..=MAX_LEN).contains(&token.len()) {
        return Err(ApiError::bad(format!(
            "an access token is {MIN_LEN} to {MAX_LEN} characters"
        )));
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
    {
        return Err(ApiError::bad(
            "an access token uses only letters, digits and - _ . ~",
        ));
    }
    Ok(token.to_string())
}

async fn store(state: &AppState, token: String) -> ApiResult<()> {
    db::config_set(&state.db, CONFIG_KEY, &token).await?;
    info!("access token changed");
    state.access_token.set(token);
    Ok(())
}

/// For a browser signed in here: copying the token to sign in elsewhere,
/// and the code to add this node to a controller.
async fn get_access_token(State(state): State<Arc<AppState>>) -> ApiResult<Json<AccessTokenDto>> {
    Ok(Json(AccessTokenDto {
        token: state
            .access_token
            .get()
            .ok_or(ApiError::NotFound("no access token yet"))?,
        pairing_code: state.access_token.pairing_code(),
    }))
}

async fn put_node_access_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AccessTokenRequest>,
) -> ApiResult<StatusCode> {
    store(&state, validate(&req.token)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Set the token on this controller's nodes (with the old one), then here,
/// and sign this browser in with it. Nodes that couldn't be reached are
/// listed: they keep their old token until they're paired again.
async fn put_access_token(
    State(state): State<Arc<AppState>>,
    ConnectInfo(conn): ConnectInfo<Conn>,
    Json(req): Json<AccessTokenRequest>,
) -> ApiResult<Response> {
    let token = validate(&req.token)?;
    let peers: Vec<(String, String, String)> = match state.controller.read().await.as_ref() {
        Some(c) => c
            .registry
            .read()
            .await
            .all()
            .into_iter()
            .map(|n| {
                let name = n.status.as_ref().map_or(&n.name, |s| &s.name).clone();
                (n.id.clone(), name, n.url.clone())
            })
            .collect(),
        None => Vec::new(),
    };
    let body = AccessTokenRequest {
        token: token.clone(),
    };
    let failed = join_all(peers.into_iter().map(|(node_id, name, url)| {
        let state = &state;
        let body = &body;
        async move {
            let result = async {
                let resp = state
                    .peer(Method::PUT, &format!("{url}/api/v1/node/access-token"))
                    .json(body)
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await?;
                if resp.status() == StatusCode::UNAUTHORIZED {
                    return Err(AccessDenied.into());
                }
                resp.error_for_status()?;
                anyhow::Ok(())
            }
            .await;
            result.err().map(|e| AccessTokenFailure {
                node_id,
                name,
                error: format!("{e:#}"),
            })
        }
    }))
    .await
    .into_iter()
    .flatten()
    .collect();

    store(&state, token.clone()).await?;
    let mut resp = Json(AccessTokenResultDto { failed }).into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, cookie(&state, conn, Some(&token)));
    Ok(resp)
}

/// The link Open Capture Room should open for the installed service on
/// `port`, from another process: the app opened from the Finder. Waits a
/// little for a service that's still starting to make its token; without
/// one, the plain address, which shows the sign-in.
pub fn installed_sign_in_url(port: u16) -> String {
    let plain = format!("http://localhost:{port}");
    let Some(path) = crate::service::installed_db() else {
        return plain;
    };
    let link = async {
        for _ in 0..20 {
            if path.exists() {
                if let Ok(pool) = db::open_existing(&path).await {
                    let ready = matches!(db::config_get(&pool, CONFIG_KEY).await, Ok(Some(_)));
                    let link = if ready {
                        sign_in_link(&pool, port).await.ok()
                    } else {
                        None
                    };
                    pool.close().await;
                    if link.is_some() {
                        return link;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        None
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
        .and_then(|rt| rt.block_on(link))
        .unwrap_or(plain)
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum Command {
    /// Print the access token and the pairing code.
    Show {
        /// The node's database [default: the installed service's, else
        /// capture-room.db here].
        #[arg(long)]
        db: Option<std::path::PathBuf>,
    },
    /// Replace the access token and the pairing code with new ones. Browsers
    /// and controllers using the old token need the new one.
    Reset {
        #[arg(long)]
        db: Option<std::path::PathBuf>,
        /// Restart the installed service so it takes effect now. Stops any
        /// recording or playout on it.
        #[arg(long)]
        restart: bool,
    },
}

/// Run a `capture-room access-token …` command. Whoever can read the node's
/// database can already do anything this does.
pub fn run_command(command: Command) -> anyhow::Result<()> {
    let (db, reset, restart) = match command {
        Command::Show { db } => (db, false, false),
        Command::Reset { db, restart } => (db, true, restart),
    };
    let installed = crate::service::installed_db();
    let is_service = db.is_none() && installed.is_some();
    let db = db.or(installed).unwrap_or_else(|| "capture-room.db".into());
    anyhow::ensure!(db.exists(), "no database at {}", db.display());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (token, code) = runtime.block_on(async {
        let pool = db::open_existing(&db).await?;
        if reset {
            db::config_set(&pool, CONFIG_KEY, &new_token()).await?;
            replace_pairing_code(&pool).await?;
        }
        let token = db::config_get(&pool, CONFIG_KEY).await?;
        let code = db::config_get(&pool, PAIRING_KEY).await?;
        pool.close().await;
        anyhow::Ok((token, code))
    })?;

    if !reset {
        match (token, code) {
            (Some(token), Some(code)) => {
                println!("Access token: {token}");
                println!("Pairing code: {}", format_pairing_code(&code));
            }
            _ => println!("None yet: the node makes them the first time it starts."),
        }
        return Ok(());
    }
    println!(
        "Made a new access token and pairing code in {}.",
        db.display()
    );
    // The running service holds the token it started with.
    if is_service && crate::service::is_running() {
        if restart {
            crate::service::restart()?;
            println!("Restarted the service: it uses them now.");
        } else {
            println!(
                "The running service keeps the old ones until it restarts. To restart it now \
                 (this stops any recording on it):\n  capture-room access-token reset --restart"
            );
        }
    }
    println!("See them with: capture-room access-token show");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_gets_in_before_a_token_is_loaded() {
        let t = AccessToken::default();
        assert!(!t.accepts(None));
        assert!(!t.accepts(Some("")));
        assert!(!t.accepts(Some("anything")));
    }

    #[test]
    fn needs_the_exact_token() {
        let t = AccessToken::default();
        t.set("abcdefghijklmnop".into());
        assert!(t.accepts(Some("abcdefghijklmnop")));
        assert!(!t.accepts(Some("abcdefghijklmnoq")));
        assert!(!t.accepts(Some("abcdefghijklmno")));
        assert!(!t.accepts(None));
    }

    #[test]
    fn guards_api_and_sockets() {
        for path in [
            "/api/v1/nodes",
            "/api/v1/node/sources",
            "/api/v1/access-token",
            "/api/v1/auth-anything",
            "/api/v1/auth/other",
            "/ws",
            "/ws/thumbnails",
        ] {
            assert!(guarded(path), "{path} must need the token");
        }
        for path in OPEN_PATHS
            .into_iter()
            .chain(["/", "/index.html", "/whep/program", "/wsx"])
        {
            assert!(!guarded(path), "{path} must stay open");
        }
    }

    #[test]
    fn validates_tokens() {
        assert!(validate("").is_err());
        assert!(validate("  ").is_err());
        assert!(validate("short").is_err());
        assert!(validate("has;a;semicolon;in;it").is_err());
        assert_eq!(validate(" 0123456789abcdef ").unwrap(), "0123456789abcdef");
        assert_eq!(validate(&new_token()).unwrap().len(), TOKEN_LEN);
    }

    #[test]
    fn random_strings_use_only_their_alphabet() {
        let code = new_pairing_code();
        assert_eq!(code.len(), PAIRING_LEN);
        assert!(code.bytes().all(|b| PAIRING_ALPHABET.contains(&b)));
        assert_ne!(new_token(), new_token());
    }

    #[test]
    fn pairing_codes_are_typed_loosely() {
        assert_eq!(format_pairing_code("ABCDEFGH"), "ABCD-EFGH");
        assert_eq!(normalize_pairing_code(" abcd-efgh "), "ABCDEFGH");
        let t = AccessToken::default();
        t.set_pairing_code("ABCDEFGH".into());
        assert_eq!(t.attempt("abcd efgh"), Attempt::Right);
    }

    #[test]
    fn a_pairing_code_has_a_few_tries() {
        let t = AccessToken::default();
        assert_eq!(t.attempt(""), Attempt::Wrong, "none before load");
        t.set_pairing_code("ABCDEFGH".into());
        for _ in 1..PAIRING_ATTEMPTS {
            assert_eq!(t.attempt("ABCDEFGX"), Attempt::Wrong);
        }
        assert_eq!(t.attempt("ABCDEFGX"), Attempt::Exhausted);
        // Used up: even the right code fails until there's a new one.
        assert_eq!(t.attempt("ABCDEFGH"), Attempt::Wrong);
        t.set_pairing_code("JKMNPQRS".into());
        assert_eq!(t.attempt("JKMN-PQRS"), Attempt::Right);
    }

    #[tokio::test]
    async fn a_sign_in_link_works_once() {
        let path =
            std::env::temp_dir().join(format!("capture-room-link-{}.db", std::process::id()));
        let pool = db::init(&path).await.unwrap();
        let link = sign_in_link(&pool, 7700).await.unwrap();
        let secret = link.split("secret=").nth(1).unwrap();
        assert!(link.starts_with("http://localhost:7700/api/v1/auth/link?"));
        assert!(!take_sign_in_link(&pool, "not-it").await.unwrap());
        assert!(take_sign_in_link(&pool, secret).await.unwrap());
        assert!(!take_sign_in_link(&pool, secret).await.unwrap());
        pool.close().await;
        let _ = std::fs::remove_file(&path);
    }
}
