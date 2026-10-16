//! Access token: one shared secret for a whole cluster. With none set the API
//! is open (as before); once set, every `/api` route and `/ws` needs it.
//!
//! Browsers sign in once and carry it in an HttpOnly cookie, so `<img>`,
//! `<video>`, downloads and the WebSocket work without code of their own.
//! Nodes send it to each other as `Authorization: Bearer`. The UI assets,
//! `/api/v1/auth/…` and `/whep` (the program, like RTSP) stay open.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::future::join_all;
use tracing::{info, warn};

use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    AccessTokenFailure, AccessTokenRequest, AccessTokenResultDto, AuthStatusDto, SignInRequest,
};
use crate::db;
use crate::state::AppState;

const CONFIG_KEY: &str = "access_token";
const MIN_LEN: usize = 16;
const MAX_LEN: usize = 128;
/// Chrome's cap on a cookie's lifetime: an operator's browser stays signed in.
const COOKIE_MAX_AGE: u64 = 400 * 24 * 60 * 60;
/// Slows guessing; a token is long enough that this is a backstop.
const WRONG_TOKEN_DELAY: Duration = Duration::from_secs(1);

/// A peer refused this node's token: it has a different one (or none of ours).
#[derive(Debug)]
pub struct AccessDenied;

impl std::fmt::Display for AccessDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("that node uses a different access token")
    }
}

impl std::error::Error for AccessDenied {}

/// This node's token; `None` leaves the API open.
pub struct AccessToken {
    token: std::sync::RwLock<Option<String>>,
    changed: tokio::sync::watch::Sender<()>,
}

impl Default for AccessToken {
    fn default() -> Self {
        Self {
            token: Default::default(),
            changed: tokio::sync::watch::channel(()).0,
        }
    }
}

impl AccessToken {
    pub fn is_set(&self) -> bool {
        self.token.read().unwrap().is_some()
    }

    fn set(&self, token: Option<String>) {
        *self.token.write().unwrap() = token;
        self.changed.send_replace(());
    }

    /// Fires when the token changes: WebSockets signed in with the old one
    /// close, and their clients reconnect with (or sign in for) the new one.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<()> {
        self.changed.subscribe()
    }

    /// Whether `presented` gets in: anything does while no token is set.
    fn accepts(&self, presented: Option<&str>) -> bool {
        match self.token.read().unwrap().as_deref() {
            None => true,
            Some(token) => {
                presented.is_some_and(|p| constant_time_eq(p.as_bytes(), token.as_bytes()))
            }
        }
    }

    /// The `Authorization` header for requests to peers.
    pub fn bearer(&self) -> Option<HeaderValue> {
        let token = self.token.read().unwrap();
        let mut value = HeaderValue::from_str(&format!("Bearer {}", token.as_deref()?)).ok()?;
        value.set_sensitive(true);
        Some(value)
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Read the stored token into `state`.
pub async fn load(state: &AppState) -> anyhow::Result<()> {
    let token = db::config_get(&state.db, CONFIG_KEY).await?;
    if token.is_none() {
        warn!("no access token set: anyone who can reach this node can control it");
    }
    state.access_token.set(token);
    Ok(())
}

/// Routes anyone can call: what a browser needs to sign in.
pub fn open_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/auth", get(get_auth))
        .route("/api/v1/auth/sign-in", post(post_sign_in))
        .route("/api/v1/auth/sign-out", post(post_sign_out))
}

/// Sets the token here and on every node this controller has.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/access-token", put(put_access_token))
}

/// Part of the node API: sets this node's token only.
pub fn node_router() -> Router<Arc<AppState>> {
    Router::new().route("/access-token", put(put_node_access_token))
}

/// Middleware: refuse `/api` and `/ws` without the token, except the open
/// routes. Page loads stay open so the UI can show its sign-in.
pub async fn require(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let guarded = (path.starts_with("/api/") && !path.starts_with("/api/v1/auth")) || path == "/ws";
    if guarded
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
fn cookie(state: &AppState, token: Option<&str>) -> HeaderValue {
    let name = cookie_name(state);
    let value = match token {
        Some(t) => {
            format!("{name}={t}; Path=/; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE}")
        }
        None => format!("{name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"),
    };
    HeaderValue::from_str(&value).expect("token characters are checked")
}

async fn get_auth(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Json<AuthStatusDto> {
    let required = state.access_token.is_set();
    Json(AuthStatusDto {
        required,
        signed_in: !required
            || state
                .access_token
                .accepts(presented(&state, &headers).as_deref()),
    })
}

async fn post_sign_in(
    State(state): State<Arc<AppState>>,
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
    if state.access_token.is_set() {
        resp.headers_mut()
            .insert(header::SET_COOKIE, cookie(&state, Some(token)));
    }
    resp
}

async fn post_sign_out(State(state): State<Arc<AppState>>) -> Response {
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, cookie(&state, None));
    resp
}

/// A token to store: trimmed, long enough to resist guessing, and only
/// characters that are safe in a cookie and a header. Blank turns it off.
fn validate(token: Option<String>) -> ApiResult<Option<String>> {
    let Some(token) = token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    else {
        return Ok(None);
    };
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
    Ok(Some(token))
}

async fn store(state: &AppState, token: Option<String>) -> ApiResult<()> {
    match &token {
        Some(t) => db::config_set(&state.db, CONFIG_KEY, t).await?,
        None => db::config_delete(&state.db, CONFIG_KEY).await?,
    }
    info!(set = token.is_some(), "access token changed");
    state.access_token.set(token);
    Ok(())
}

async fn put_node_access_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AccessTokenRequest>,
) -> ApiResult<StatusCode> {
    store(&state, validate(req.token)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Set the token on this controller's nodes (with the old one), then here,
/// and sign this browser in with it. Nodes that couldn't be reached are
/// listed: they keep their old token until it's set there.
async fn put_access_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AccessTokenRequest>,
) -> ApiResult<Response> {
    let token = validate(req.token)?;
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
        .insert(header::SET_COOKIE, cookie(&state, token.as_deref()));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_without_a_token() {
        let t = AccessToken::default();
        assert!(t.accepts(None));
        assert!(t.accepts(Some("anything")));
    }

    #[test]
    fn needs_the_exact_token_once_set() {
        let t = AccessToken::default();
        t.set(Some("abcdefghijklmnop".into()));
        assert!(t.accepts(Some("abcdefghijklmnop")));
        assert!(!t.accepts(Some("abcdefghijklmnoq")));
        assert!(!t.accepts(Some("abcdefghijklmno")));
        assert!(!t.accepts(None));
    }

    #[test]
    fn validates_tokens() {
        assert_eq!(validate(None).unwrap(), None);
        assert_eq!(validate(Some("  ".into())).unwrap(), None);
        assert!(validate(Some("short".into())).is_err());
        assert!(validate(Some("has;a;semicolon;in;it".into())).is_err());
        assert_eq!(
            validate(Some(" 0123456789abcdef ".into()))
                .unwrap()
                .as_deref(),
            Some("0123456789abcdef")
        );
    }
}
