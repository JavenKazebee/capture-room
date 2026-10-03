//! Controller-facing routes: the node list, promotion toggle, presets, request
//! forwarding and the merged event stream. Mounted on every instance — on a
//! plain node `/nodes` simply lists itself, so the UI works the same either way.

use std::sync::Arc;

use axum::{
    extract::{ws::WebSocketUpgrade, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{any, delete, get, put},
    Json, Router,
};
use tracing::error;

use super::{discovery, forward, Controller, CONFIG_KEY};
use crate::api::types::{
    AddNodeRequest, ControllerToggleRequest, NodeDto, PresetCreateRequest, PresetDto,
    PresetOutputDto, PresetOutputInput,
};
use crate::db::{self, PresetOutputRow, PresetRow};
use crate::state::AppState;
use crate::ws;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/controller", put(put_controller))
        .route("/api/v1/nodes", get(get_nodes).post(post_node))
        .route("/api/v1/nodes/{id}", delete(delete_node))
        .route("/api/v1/nodes/{id}/{*path}", any(forward::forward))
        .route("/api/v1/presets", get(get_presets).post(post_preset))
        .route("/api/v1/presets/{id}", put(put_preset).delete(delete_preset))
        .route("/ws", get(ws_handler))
}

fn internal(e: impl ToString) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
}

fn not_controller() -> Response {
    (StatusCode::CONFLICT, "this node is not acting as a controller").into_response()
}

// ── /api/v1/controller ───────────────────────────────────────────────────────

async fn put_controller(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ControllerToggleRequest>,
) -> Response {
    if let Err(e) = db::config_set(&state.db, CONFIG_KEY, &req.enabled.to_string()).await {
        return internal(e);
    }
    if req.enabled {
        if let Err(e) = Controller::enable(&state).await {
            return internal(e);
        }
    } else {
        Controller::disable(&state).await;
    }
    Json(req).into_response()
}

// ── /api/v1/nodes ────────────────────────────────────────────────────────────

async fn get_nodes(State(state): State<Arc<AppState>>) -> Json<Vec<NodeDto>> {
    let mut dtos = vec![NodeDto {
        id: state.node_id.clone(),
        name: state.node_name(),
        url: String::new(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        healthy: true,
        uptime_secs: state.started_at.elapsed().as_secs(),
        is_self: true,
        manual: false,
    }];

    if let Some(c) = state.controller.read().await.as_ref() {
        let reg = c.registry.read().await;
        let mut peers: Vec<NodeDto> = reg
            .all()
            .into_iter()
            .map(|n| NodeDto {
                id: n.id.clone(),
                name: n.name.clone(),
                url: n.url.clone(),
                version: n.version.clone(),
                healthy: n.healthy,
                uptime_secs: n.uptime_secs,
                is_self: false,
                manual: n.manual,
            })
            .collect();
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        dtos.extend(peers);
    }

    Json(dtos)
}

async fn post_node(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddNodeRequest>,
) -> Response {
    let ctx = match state.controller.read().await.as_ref() {
        Some(c) => c.ctx(&state),
        None => return not_controller(),
    };
    let mut url = req.url.trim().trim_end_matches('/').to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("http://{url}");
    }

    let status = match discovery::add_node(&ctx, url.clone(), true).await {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    let row = db::NodeRow {
        id: status.id,
        name: status.name,
        url,
        added_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = db::node_upsert(&state.db, &row).await {
        return internal(e);
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn delete_node(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let registry = match state.controller.read().await.as_ref() {
        Some(c) => Arc::clone(&c.registry),
        None => return not_controller(),
    };
    // Removing the entry also stops its WS relay.
    registry.write().await.remove(&id);
    if let Err(e) = db::node_delete(&state.db, &id).await {
        return internal(e);
    }
    StatusCode::NO_CONTENT.into_response()
}

// ── /api/v1/presets ──────────────────────────────────────────────────────────
//
// Presets live on whichever instance the UI is talking to. Nodes never store
// them: starting a recording sends the preset's outputs inline.

async fn get_presets(State(state): State<Arc<AppState>>) -> Response {
    let rows = match db::presets_list(&state.db).await {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    let all_outputs = match db::preset_outputs_list_all(&state.db).await {
        Ok(o) => o,
        Err(e) => return internal(e),
    };
    let dtos: Vec<PresetDto> = rows
        .iter()
        .map(|r| {
            let outputs: Vec<PresetOutputRow> =
                all_outputs.iter().filter(|o| o.preset_id == r.id).cloned().collect();
            preset_to_dto(r, &outputs)
        })
        .collect();
    Json(dtos).into_response()
}

async fn post_preset(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PresetCreateRequest>,
) -> Response {
    let now = chrono::Utc::now().to_rfc3339();
    let preset_id = uuid::Uuid::new_v4().to_string();
    let row = PresetRow {
        id: preset_id.clone(),
        name: req.name,
        created_at: now.clone(),
        updated_at: now,
        version: 1,
    };
    if let Err(e) = db::preset_insert(&state.db, &row).await {
        return internal(e);
    }
    let output_rows = build_output_rows(&preset_id, &req.outputs);
    if let Err(e) = db::preset_outputs_replace(&state.db, &preset_id, &output_rows).await {
        return internal(e);
    }
    (StatusCode::CREATED, Json(preset_to_dto(&row, &output_rows))).into_response()
}

async fn put_preset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<PresetCreateRequest>,
) -> Response {
    let row = PresetRow {
        id: id.clone(),
        name: req.name,
        created_at: String::new(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        version: 0,
    };
    match db::preset_update(&state.db, &row).await {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "preset not found").into_response(),
        Err(e) => return internal(e),
    }
    let output_rows = build_output_rows(&id, &req.outputs);
    if let Err(e) = db::preset_outputs_replace(&state.db, &id, &output_rows).await {
        return internal(e);
    }
    match db::preset_get(&state.db, &id).await {
        Ok(Some(updated)) => Json(preset_to_dto(&updated, &output_rows)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "preset not found").into_response(),
        Err(e) => {
            error!(error = %e, "reload preset after update");
            internal(e)
        }
    }
}

async fn delete_preset(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match db::preset_delete(&state.db, &id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "preset not found").into_response(),
        Err(e) => internal(e),
    }
}

fn preset_to_dto(row: &PresetRow, outputs: &[PresetOutputRow]) -> PresetDto {
    PresetDto {
        id: row.id.clone(),
        name: row.name.clone(),
        outputs: outputs.iter().map(output_row_to_dto).collect(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        version: row.version,
    }
}

fn output_row_to_dto(o: &PresetOutputRow) -> PresetOutputDto {
    PresetOutputDto {
        id: o.id.clone(),
        preset_id: o.preset_id.clone(),
        name: o.name.clone(),
        codec: o.codec.clone(),
        container: o.container.clone(),
        resolution: o.resolution.clone(),
        framerate: o.framerate.clone(),
        bitrate_kbps: o.bitrate_kbps,
        path_template: o.path_template.clone(),
        sort_order: o.sort_order,
    }
}

fn build_output_rows(preset_id: &str, inputs: &[PresetOutputInput]) -> Vec<PresetOutputRow> {
    inputs
        .iter()
        .enumerate()
        .map(|(i, o)| PresetOutputRow {
            id: uuid::Uuid::new_v4().to_string(),
            preset_id: preset_id.to_string(),
            name: o.name.clone(),
            codec: o.codec.clone(),
            container: o.container.clone(),
            resolution: o.resolution.clone(),
            framerate: o.framerate.clone(),
            bitrate_kbps: o.bitrate_kbps,
            path_template: o.path_template.clone(),
            sort_order: i as i64,
        })
        .collect()
}

// ── /ws (merged: this node + relayed peers) ──────────────────────────────────

async fn ws_handler(State(state): State<Arc<AppState>>, upgrade: WebSocketUpgrade) -> Response {
    let rx = state.ws_tx.subscribe();
    upgrade.on_upgrade(move |socket| ws::handle(socket, rx))
}
