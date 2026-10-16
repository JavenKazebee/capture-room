//! `/whep/<path>`: proxies to each WHEP output's signaller on localhost.
//! Offers POST to `/whep/<path>`; the session lives at
//! `/whep/<path>/resource/<id>` (PATCH for ICE, DELETE to leave). Any origin
//! is allowed: these only ever give out the program, like RTSP.

use std::sync::LazyLock;
use std::time::Duration;

use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request},
    http::{header, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};

use crate::pipeline::whep;

/// Longer than the signaller takes to answer an offer.
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 64 * 1024;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/whep/{*path}", any(proxy))
}

async fn proxy(Path(path): Path<String>, req: Request) -> Response {
    let method = req.method().clone();
    if method == Method::OPTIONS
        && req
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
    {
        return cors(StatusCode::NO_CONTENT.into_response());
    }
    // The endpoint is the mount; a resource is under it.
    let (mount, upstream) = match path.rsplit_once("/resource/") {
        Some((mount, id)) if matches!(method, Method::PATCH | Method::DELETE) => {
            (format!("/{mount}"), format!("/whep/resource/{id}"))
        }
        _ if matches!(method, Method::POST | Method::OPTIONS) => {
            (format!("/{path}"), "/whep/endpoint".to_string())
        }
        _ => return cors(StatusCode::METHOD_NOT_ALLOWED.into_response()),
    };
    let Some(port) = whep::port(&mount) else {
        return cors((StatusCode::NOT_FOUND, "no WHEP output here").into_response());
    };

    let (parts, body) = req.into_parts();
    let body = match to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(e) => return cors((StatusCode::BAD_REQUEST, e.to_string()).into_response()),
    };
    let mut out = HTTP
        .request(method, format!("http://127.0.0.1:{port}{upstream}"))
        .body(body);
    for name in [header::CONTENT_TYPE, header::IF_MATCH] {
        if let Some(v) = parts.headers.get(&name) {
            out = out.header(name, v);
        }
    }
    let resp = match tokio::time::timeout(TIMEOUT, out.send()).await {
        Ok(Ok(r)) => r,
        // The signaller starts once the program reaches the output.
        Ok(Err(e)) if e.is_connect() => {
            let mut resp =
                (StatusCode::SERVICE_UNAVAILABLE, "the output is starting").into_response();
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            return cors(resp);
        }
        Ok(Err(e)) => return cors((StatusCode::BAD_GATEWAY, e.to_string()).into_response()),
        Err(_) => return cors(StatusCode::GATEWAY_TIMEOUT.into_response()),
    };

    let mut builder = Response::builder().status(resp.status());
    for name in [header::CONTENT_TYPE, header::LINK, header::ETAG] {
        for v in resp.headers().get_all(&name) {
            builder = builder.header(&name, v);
        }
    }
    // The signaller's resource URL, as it is through here.
    if let Some(id) = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|l| l.rsplit_once("/resource/"))
        .map(|(_, id)| id)
    {
        builder = builder.header(header::LOCATION, format!("/whep{mount}/resource/{id}"));
    }
    if let Some(v) = resp.headers().get(HeaderName::from_static("access-post")) {
        builder = builder.header("accept-post", v);
    }
    let body = resp.bytes().await.unwrap_or_default();
    cors(builder.body(Body::from(body)).unwrap())
}

fn cors(mut resp: Response) -> Response {
    let h = resp.headers_mut();
    for (name, value) in [
        (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        (
            header::ACCESS_CONTROL_ALLOW_METHODS,
            "POST, PATCH, DELETE, OPTIONS",
        ),
        (
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            "content-type, if-match",
        ),
        (
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "location, link, etag",
        ),
    ] {
        h.insert(name, HeaderValue::from_static(value));
    }
    resp
}
