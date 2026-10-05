//! `/api/v1/nodes/{node_id}/{*path}` → that node's `/api/v1/node/{path}`.
//!
//! This is the only way the UI talks to a node's local API. Requests for this
//! instance are served in-process by the node router; requests for a peer are
//! forwarded over HTTP (controller only). Nothing here knows about individual
//! endpoints — the method, query and body pass through untouched, and peer
//! responses are streamed (recordings can be gigabytes) with range and caching
//! headers passed both ways, so a browser can seek in a peer's file.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use tower::ServiceExt;

use crate::state::AppState;

/// How long a peer may take to answer (with headers), long enough for a
/// recording stop, which waits for the EOS drain. The body after that isn't
/// timed: a file download takes as long as it takes.
const PEER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 16 * 1024 * 1024;

/// Request headers passed to a peer.
const REQUEST_HEADERS: [header::HeaderName; 5] =
    [header::CONTENT_TYPE, header::RANGE, header::IF_RANGE, header::IF_MODIFIED_SINCE, header::IF_NONE_MATCH];

/// Response headers passed back from a peer.
const RESPONSE_HEADERS: [header::HeaderName; 8] = [
    header::CONTENT_TYPE,
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
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();

    if node_id == state.node_id {
        // The node router's paths are relative to `/api/v1/node`.
        return forward_local(&state, req, &format!("/{path}{query}")).await;
    }
    let local_path = format!("/api/v1/node/{path}{query}");

    let url = match state.controller.read().await.as_ref() {
        Some(c) => c.registry.read().await.url_of(&node_id),
        None => None,
    };
    match url {
        Some(url) => forward_peer(&state, req, &format!("{url}{local_path}")).await,
        None => (StatusCode::NOT_FOUND, "unknown node").into_response(),
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

    let mut out = state.http.request(parts.method, url).body(body);
    for name in REQUEST_HEADERS {
        if let Some(v) = parts.headers.get(&name) {
            out = out.header(name, v);
        }
    }

    let resp = match tokio::time::timeout(PEER_TIMEOUT, out.send()).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
        Err(_) => return (StatusCode::GATEWAY_TIMEOUT, "node didn't answer in time").into_response(),
    };

    let mut builder = Response::builder().status(resp.status());
    for name in RESPONSE_HEADERS {
        if let Some(v) = resp.headers().get(&name) {
            builder = builder.header(name, v);
        }
    }
    builder.body(Body::from_stream(resp.bytes_stream())).unwrap()
}
