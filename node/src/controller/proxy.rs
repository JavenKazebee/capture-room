//! HTTP fan-out and proxy helpers used by an aggregator to reach its peers.
//!
//! Peers already return composite source IDs (`{node_id}:{source}`), so these
//! helpers never rewrite IDs — they merge or forward responses as-is.

use std::time::Duration;

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::future::join_all;
use serde_json::Value;

use crate::api::types::{PatchRecordingRequest, StartRecordingRequest};
use crate::state::AppState;

const TIMEOUT: Duration = Duration::from_secs(3);

async fn peer_url(state: &AppState, node_id: &str) -> Option<String> {
    state.peers.read().await.entries.get(node_id).map(|n| n.url.clone())
}

async fn healthy_urls(state: &AppState) -> Vec<String> {
    state.peers.read().await.healthy().iter().map(|n| n.url.clone()).collect()
}

async fn all_urls(state: &AppState) -> Vec<String> {
    state.peers.read().await.all().iter().map(|n| n.url.clone()).collect()
}

// ── Reads (fan-out across peers) ─────────────────────────────────────────────

pub async fn fan_out_sources(state: &AppState) -> Vec<Value> {
    fan_out_list(state, "/api/v1/sources").await
}

pub async fn fan_out_recordings(state: &AppState) -> Vec<Value> {
    fan_out_list(state, "/api/v1/recordings").await
}

async fn fan_out_list(state: &AppState, path: &str) -> Vec<Value> {
    let urls = healthy_urls(state).await;
    let fetches = urls.into_iter().map(|url| {
        let http = state.http.clone();
        let path = path.to_string();
        async move {
            match http.get(format!("{url}{path}")).timeout(TIMEOUT).send().await {
                Ok(r) if r.status().is_success() => r.json::<Vec<Value>>().await.unwrap_or_default(),
                _ => vec![],
            }
        }
    });
    join_all(fetches).await.into_iter().flatten().collect()
}

/// Find a session by id across all peers (session ids carry no node prefix).
pub async fn find_recording(state: &AppState, session_id: &str) -> Option<Value> {
    for url in all_urls(state).await {
        if let Ok(r) = state
            .http
            .get(format!("{url}/api/v1/recordings/{session_id}"))
            .timeout(TIMEOUT)
            .send()
            .await
        {
            if r.status().is_success() {
                if let Ok(body) = r.json::<Value>().await {
                    return Some(body);
                }
            }
        }
    }
    None
}

// ── Writes (proxy to a specific or unknown peer) ─────────────────────────────

pub async fn start_recording(
    state: &AppState,
    node_id: &str,
    req: &StartRecordingRequest,
) -> Response {
    let url = match peer_url(state, node_id).await {
        Some(u) => u,
        None => return (StatusCode::NOT_FOUND, "unknown node").into_response(),
    };

    match state.http.post(format!("{url}/api/v1/recordings")).json(req).send().await {
        Ok(r) => {
            let status = r.status();
            match r.json::<Value>().await {
                Ok(body) => (status, Json(body)).into_response(),
                Err(_) => status.into_response(),
            }
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

/// Fan a stop out to every peer until one owns the session.
pub async fn stop_recording(
    state: &AppState,
    session_id: &str,
    req: &PatchRecordingRequest,
) -> Option<Value> {
    for url in all_urls(state).await {
        if let Ok(r) = state
            .http
            .patch(format!("{url}/api/v1/recordings/{session_id}"))
            .json(req)
            .send()
            .await
        {
            if r.status().is_success() {
                if let Ok(body) = r.json::<Value>().await {
                    return Some(body);
                }
            }
        }
    }
    None
}

pub async fn thumbnail(state: &AppState, node_id: &str, local_source: &str) -> Response {
    let url = match peer_url(state, node_id).await {
        Some(u) => u,
        None => return (StatusCode::NOT_FOUND, "unknown node").into_response(),
    };

    match state
        .http
        .get(format!("{url}/api/v1/thumbnails/{node_id}:{local_source}"))
        .timeout(TIMEOUT)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.bytes().await {
            Ok(bytes) => Response::builder()
                .header(header::CONTENT_TYPE, "image/jpeg")
                .body(Body::from(bytes))
                .unwrap(),
            Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
        },
        Ok(r) => r.status().into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}
