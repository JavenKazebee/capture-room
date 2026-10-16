//! Controller-facing routes: the node list, promotion toggle, presets, request
//! forwarding and the merged event stream. Mounted on every instance — on a
//! plain node `/nodes` simply lists itself, so the UI works the same either way.

use std::sync::Arc;

use axum::{
    extract::{ws::WebSocketUpgrade, Path, State},
    http::StatusCode,
    response::Response,
    routing::{any, delete, get, put},
    Json, Router,
};

use super::{clock_mode, discovery, forward, Controller, CLOCK_MODE_KEY, CONFIG_KEY};
use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    AddNodeRequest, CheckDto, ClockMode, ControllerClockDto, ControllerToggleRequest,
    NodeChecksDto, NodeDto, PresetCreateRequest, PresetDto, PresetOutputInput, WsEvent,
};
use crate::db;
use crate::pipeline::profile::{plan_legs, unknown_token};
use crate::pipeline::recording;
use crate::state::AppState;
use crate::ws;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/controller", put(put_controller))
        .route(
            "/api/v1/controller/clock",
            get(get_controller_clock).put(put_controller_clock),
        )
        .route("/api/v1/nodes", get(get_nodes).post(post_node))
        .route("/api/v1/nodes/{id}", delete(delete_node))
        .route("/api/v1/nodes/{id}/{*path}", any(forward::forward))
        .route("/api/v1/reachability/{id}", get(get_reachability))
        .route("/api/v1/presets", get(get_presets).post(post_preset))
        .route(
            "/api/v1/presets/{id}",
            put(put_preset).delete(delete_preset),
        )
        .route("/ws", get(ws_handler))
}

const NOT_CONTROLLER: ApiError = ApiError::Conflict("this node is not acting as a controller");

/// `f` of this node's controller, or a conflict if it isn't acting as one.
async fn with_controller<T>(state: &AppState, f: impl FnOnce(&Controller) -> T) -> ApiResult<T> {
    state
        .controller
        .read()
        .await
        .as_ref()
        .map(f)
        .ok_or(NOT_CONTROLLER)
}

async fn put_controller(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ControllerToggleRequest>,
) -> ApiResult<Json<ControllerToggleRequest>> {
    db::config_set(&state.db, CONFIG_KEY, &req.enabled.to_string()).await?;
    if req.enabled {
        Controller::enable(&state).await?;
    } else {
        Controller::disable(&state).await;
    }
    Ok(Json(req))
}

async fn controller_clock(state: &AppState) -> ApiResult<ControllerClockDto> {
    let serving = with_controller(state, Controller::serves_clock).await?;
    Ok(ControllerClockDto {
        mode: clock_mode(state).await?,
        port: state.clock_port,
        serving,
    })
}

async fn get_controller_clock(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ControllerClockDto>> {
    Ok(Json(controller_clock(&state).await?))
}

/// Change how this controller's nodes keep time. Nodes get the new mode with
/// their next health check.
async fn put_controller_clock(
    State(state): State<Arc<AppState>>,
    Json(mode): Json<ClockMode>,
) -> ApiResult<Json<ControllerClockDto>> {
    if !state.is_controller().await {
        return Err(NOT_CONTROLLER);
    }
    let json = serde_json::to_string(&mode).map_err(anyhow::Error::from)?;
    db::config_set(&state.db, CLOCK_MODE_KEY, &json).await?;
    state.clock.set_own_mode(Some(mode));
    state.emit(&WsEvent::NodeUpdated);
    Ok(Json(controller_clock(&state).await?))
}

async fn get_nodes(State(state): State<Arc<AppState>>) -> Json<Vec<NodeDto>> {
    let mut dtos = vec![NodeDto {
        url: String::new(),
        healthy: true,
        is_self: true,
        manual: false,
        access_denied: false,
        status: crate::api::node::local_status(&state).await,
    }];

    if let Some(c) = state.controller.read().await.as_ref() {
        let reg = c.registry.read().await;
        let mut peers: Vec<NodeDto> = reg.all().into_iter().map(NodeDto::from).collect();
        peers.sort_by(|a, b| a.status.name.cmp(&b.status.name));
        dtos.extend(peers);
    }

    Json(dtos)
}

/// Probe a node's ports from this controller: a node can't see its own
/// firewall.
async fn get_reachability(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<CheckDto>>> {
    if id == state.node_id {
        return Ok(Json(vec![crate::checks::own_firewall()]));
    }
    let registry = with_controller(&state, |c| Arc::clone(&c.registry)).await?;
    let url = registry
        .read()
        .await
        .url_of(&id)
        .ok_or(ApiError::NotFound("unknown node"))?;
    let checks: NodeChecksDto = async {
        state
            .peer(reqwest::Method::GET, &format!("{url}/api/v1/node/checks"))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
    .await
    .map_err(|e| ApiError::BadGateway(e.to_string()))?;
    let host = reqwest::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .ok_or(ApiError::BadGateway(format!("no host in {url}")))?;
    Ok(Json(
        crate::checks::probe_ports(&host, &checks.os, &checks.ports).await,
    ))
}

async fn post_node(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddNodeRequest>,
) -> ApiResult<StatusCode> {
    let ctx = with_controller(&state, |c| c.ctx(&state)).await?;
    let mut url = req.url.trim().trim_end_matches('/').to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("http://{url}");
    }

    let status = discovery::add_node(&ctx, url.clone(), true)
        .await
        .map_err(|e| ApiError::BadGateway(e.to_string()))?
        .ok_or_else(|| ApiError::bad("that address is this node"))?;
    let row = db::NodeRow {
        id: status.id,
        name: status.name,
        url,
        added_at: chrono::Utc::now().to_rfc3339(),
    };
    db::node_upsert(&state.db, &row).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_node(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let registry = with_controller(&state, |c| Arc::clone(&c.registry)).await?;
    // Removing the entry also stops its WS relay.
    let removed = registry.write().await.remove(&id).is_some();
    db::node_delete(&state.db, &id).await?;
    if removed {
        state.emit_controller(&WsEvent::NodeRemoved { peer_id: id });
    }
    Ok(StatusCode::NO_CONTENT)
}

// Presets live on whichever instance the UI talks to. Nodes never store them:
// starting a recording sends the preset's outputs inline.

const PRESET_NOT_FOUND: ApiError = ApiError::NotFound("preset not found");

async fn get_presets(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<PresetDto>>> {
    Ok(Json(db::presets_list(&state.db).await?))
}

async fn post_preset(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PresetCreateRequest>,
) -> ApiResult<(StatusCode, Json<PresetDto>)> {
    let outputs = validate_outputs(req.outputs)?;
    let now = chrono::Utc::now().to_rfc3339();
    let id = uuid::Uuid::new_v4().to_string();
    let preset = PresetDto {
        outputs,
        id,
        name: req.name,
        created_at: now.clone(),
        updated_at: now,
        version: 1,
    };
    db::preset_insert(&state.db, &preset).await?;
    Ok((StatusCode::CREATED, Json(preset)))
}

async fn put_preset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<PresetCreateRequest>,
) -> ApiResult<Json<PresetDto>> {
    let outputs = validate_outputs(req.outputs)?;
    let now = chrono::Utc::now().to_rfc3339();
    let preset = db::preset_update(&state.db, &id, &req.name, &now, outputs)
        .await?
        .ok_or(PRESET_NOT_FOUND)?;
    Ok(Json(preset))
}

async fn delete_preset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if !db::preset_delete(&state.db, &id).await? {
        return Err(PRESET_NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Store blank resolution/framerate as `None` ("match the source") and reject
/// outputs a recording couldn't start with. Each leg's pipeline is built (not
/// started), which catches a codec its container can't carry.
fn validate_outputs(mut outputs: Vec<PresetOutputInput>) -> ApiResult<Vec<PresetOutputInput>> {
    fn blank_to_none(v: Option<String>) -> Option<String> {
        v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
    for output in &mut outputs {
        output.resolution = blank_to_none(output.resolution.take());
        output.framerate = blank_to_none(output.framerate.take());
        if let Some(token) = unknown_token(&output.path_template) {
            return Err(ApiError::bad(format!(
                "{token} isn't a path template token"
            )));
        }
    }
    let legs = plan_legs(&outputs, None).map_err(ApiError::bad)?;
    recording::check_legs(&legs).map_err(ApiError::bad)?;
    Ok(outputs)
}

async fn ws_handler(State(state): State<Arc<AppState>>, upgrade: WebSocketUpgrade) -> Response {
    let rx = state.ws_tx.subscribe();
    let token_changed = state.access_token.subscribe();
    upgrade.on_upgrade(move |socket| ws::handle(socket, rx, token_changed))
}
