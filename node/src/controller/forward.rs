//! `/api/v1/nodes/{node_id}/{*path}` → that node's `/api/v1/node/{path}`: the
//! only way the UI reaches a node's API. This instance is served in-process,
//! peers over HTTP. Bodies stream through untouched with range and caching
//! headers, so a browser can seek in a peer's file. Peers get this node's
//! access token, never the browser's credentials.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Path, Request, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use tower::ServiceExt;

use crate::state::AppState;

/// Time to response headers: enough for a recording stop's EOS drain. The
/// body isn't timed (downloads take as long as they take).
const PEER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 16 * 1024 * 1024;

/// Request headers passed to a peer.
const REQUEST_HEADERS: [header::HeaderName; 6] = [
    header::CONTENT_TYPE,
    header::IF_MATCH,
    header::RANGE,
    header::IF_RANGE,
    header::IF_MODIFIED_SINCE,
    header::IF_NONE_MATCH,
];

/// Response headers passed back from a peer.
const RESPONSE_HEADERS: [header::HeaderName; 10] = [
    header::CONTENT_TYPE,
    // WHEP's resource URL and ICE servers.
    header::LOCATION,
    header::LINK,
    header::CONTENT_LENGTH,
    header::CACHE_CONTROL,
    header::ACCEPT_RANGES,
    header::CONTENT_RANGE,
    header::CONTENT_DISPOSITION,
    header::LAST_MODIFIED,
    header::ETAG,
];

pub async fn forward(
    State(state): State<Arc<AppState>>,
    Path((node_id, path)): Path<(String, String)>,
    req: Request,
) -> Response {
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();

    if node_id == state.node_id {
        // The node router's paths are relative to `/api/v1/node`.
        return forward_local(&state, req, &format!("/{path}{query}")).await;
    }
    match peer_url(&state, &node_id, &path).await {
        Ok(url) => forward_peer(&state, req, &format!("{url}/api/v1/node/{path}{query}")).await,
        Err(refused) => refused.into_response(),
    }
}

/// A peer's URL, unless it's unknown, or runs another version than this
/// controller and `path` (relative to its `/api/v1/node`) isn't its update:
/// a request from this version could mean something else to it.
async fn peer_url(
    state: &AppState,
    node_id: &str,
    path: &str,
) -> Result<String, (StatusCode, String)> {
    let unknown = || (StatusCode::NOT_FOUND, "unknown node".to_string());
    let controller = state.controller.read().await;
    let registry = controller
        .as_ref()
        .ok_or_else(unknown)?
        .registry
        .read()
        .await;
    let node = registry.get(node_id).ok_or_else(unknown)?;
    if let Some(version) = registry.other_version(node_id) {
        if !is_update(path) {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "{} runs {version} and this controller {}: update them to the same version",
                    node.name,
                    env!("CARGO_PKG_VERSION")
                ),
            ));
        }
    }
    Ok(node.url.clone())
}

/// Whether `path` is a node's update, which it answers on any version.
fn is_update(path: &str) -> bool {
    let path = path.trim_start_matches('/');
    matches!(
        path.split('?').next().unwrap_or_default(),
        "update" | "update/check"
    )
}

/// Call a node's API (`path` relative to its `/api/v1/node`) from this
/// instance, the way [`forward`] does for the UI. The response body on
/// success, its text as the error otherwise.
pub async fn call_node(
    state: &AppState,
    node_id: &str,
    method: axum::http::Method,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<Bytes, String> {
    let mut req = Request::builder().method(method).uri("/");
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let req = req.body(body).map_err(|e| e.to_string())?;

    let resp = if node_id == state.node_id {
        forward_local(state, req, path).await
    } else {
        let url = peer_url(state, node_id, path).await.map_err(|(_, e)| e)?;
        forward_peer(state, req, &format!("{url}/api/v1/node{path}")).await
    };
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), MAX_BODY)
        .await
        .map_err(|e| e.to_string())?;
    if status.is_success() {
        Ok(bytes)
    } else {
        let text = String::from_utf8_lossy(&bytes).trim().to_string();
        Err(if text.is_empty() {
            status.to_string()
        } else {
            text
        })
    }
}

async fn forward_local(state: &AppState, req: Request, path: &str) -> Response {
    let Some(router) = state.node_router.get() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let uri = match path.parse::<Uri>() {
        Ok(u) => u,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    // Rebuild rather than mutate: the original request's extensions carry
    // this route's path params, which the node router's extractors would see.
    let (parts, body) = req.into_parts();
    let mut req = Request::new(body);
    *req.method_mut() = parts.method;
    *req.uri_mut() = uri;
    *req.headers_mut() = parts.headers;
    match router.clone().oneshot(req).await {
        Ok(resp) => resp,
        Err(never) => match never {},
    }
}

async fn forward_peer(state: &AppState, req: Request, url: &str) -> Response {
    let (parts, body) = req.into_parts();
    let body = match to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let mut out = state.peer(parts.method, url).body(body);
    for name in REQUEST_HEADERS {
        if let Some(v) = parts.headers.get(&name) {
            out = out.header(name, v);
        }
    }

    let resp = match tokio::time::timeout(PEER_TIMEOUT, out.send()).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
        Err(_) => {
            return (StatusCode::GATEWAY_TIMEOUT, "node didn't answer in time").into_response()
        }
    };

    // The browser is signed in here; a peer refusing this controller's
    // token mustn't look like the browser being signed out.
    if resp.status() == StatusCode::UNAUTHORIZED {
        return (
            StatusCode::BAD_GATEWAY,
            crate::auth::AccessDenied.to_string(),
        )
            .into_response();
    }

    let mut builder = Response::builder().status(resp.status());
    for name in RESPONSE_HEADERS {
        // WHEP sends a `Link` per ICE server.
        for v in resp.headers().get_all(&name) {
            builder = builder.header(&name, v);
        }
    }
    builder
        .body(Body::from_stream(resp.bytes_stream()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::is_update;

    #[test]
    fn only_the_update_reaches_another_version() {
        for path in ["update", "/update", "update/check", "/update?x=1"] {
            assert!(is_update(path), "{path}");
        }
        for path in ["status", "/recordings", "update/other", "updates"] {
            assert!(!is_update(path), "{path}");
        }
    }
}
