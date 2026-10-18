//! Controller-facing routes: the node list, promotion toggle, presets, request
//! forwarding and the merged event stream. Mounted on every instance — on a
//! plain node `/nodes` simply lists itself, so the UI works the same either way.

use std::sync::Arc;

use axum::{
    extract::{ws::WebSocketUpgrade, Path, State},
    http::StatusCode,
    response::Response,
    routing::{any, delete, get, post, put},
    Json, Router,
};

use super::scheduler::{Rule, Scheduler};
use super::{clock_mode, discovery, forward, Controller, CLOCK_MODE_KEY, CONFIG_KEY};
use crate::api::error::{ApiError, ApiResult};
use crate::api::types::{
    AddNodeRequest, CheckDto, ClockMode, ControllerClockDto, ControllerToggleRequest, FoundNodeDto,
    NodeChecksDto, NodeDto, PresetCreateRequest, PresetDto, PresetOutputInput, ScheduleAction,
    ScheduleDto, ScheduleInput, ScheduleRunDto, WsEvent,
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
        .route("/api/v1/found-nodes", get(get_found_nodes))
        .route("/api/v1/found-nodes/{id}", delete(delete_found_node))
        .route("/api/v1/found-nodes/{id}/approve", post(post_approve_found))
        .route("/api/v1/reachability/{id}", get(get_reachability))
        .route("/api/v1/presets", get(get_presets).post(post_preset))
        .route(
            "/api/v1/presets/{id}",
            put(put_preset).delete(delete_preset),
        )
        .route("/api/v1/schedules", get(get_schedules).post(post_schedule))
        .route(
            "/api/v1/schedules/{id}",
            get(get_schedule).put(put_schedule).delete(delete_schedule),
        )
        .route("/api/v1/schedules/{id}/stop", post(post_schedule_stop))
        .route("/api/v1/schedules/{id}/runs", get(get_schedule_runs))
        .route("/ws", get(ws_handler))
        .route("/ws/thumbnails", get(thumbnails_ws_handler))
        .route("/ws/logs", get(crate::logs::relay_ws))
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
        fingerprint: Some(state.fingerprint.clone()),
        unreachable: None,
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
    discovery::add_by_url(&ctx, &discovery::https_url(&req.url))
        .await
        .map_err(|e| match e.downcast::<discovery::KeyMismatch>() {
            Ok(mismatch) => ApiError::bad(mismatch),
            Err(e) => ApiError::BadGateway(format!("{e:#}")),
        })?
        .ok_or_else(|| ApiError::bad("that address is this node"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Nodes that announced themselves and wait to be approved.
async fn get_found_nodes(State(state): State<Arc<AppState>>) -> Json<Vec<FoundNodeDto>> {
    let mut found: Vec<FoundNodeDto> = match state.controller.read().await.as_ref() {
        Some(c) => c
            .registry
            .read()
            .await
            .found
            .values()
            .map(|f| f.dto.clone())
            .collect(),
        None => Vec::new(),
    };
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Json(found)
}

async fn post_approve_found(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let ctx = with_controller(&state, |c| c.ctx(&state)).await?;
    if !discovery::approve_found(&ctx, &id).await? {
        return Err(ApiError::NotFound("that node isn't waiting to be approved"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Dismiss a found node until this controller restarts.
async fn delete_found_node(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let registry = with_controller(&state, |c| Arc::clone(&c.registry)).await?;
    if registry.write().await.ignore(&id) {
        state.emit_controller(&WsEvent::NodeUpdated);
    }
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

// Schedules live on the controller, which runs them (see `scheduler`).

const SCHEDULE_NOT_FOUND: ApiError = ApiError::NotFound("schedule not found");

async fn scheduler(state: &AppState) -> ApiResult<Arc<Scheduler>> {
    with_controller(state, |c| Arc::clone(&c.scheduler)).await
}

/// `s` with when it next runs and its latest run.
async fn decorate(state: &AppState, mut s: ScheduleDto) -> ApiResult<ScheduleDto> {
    if s.enabled {
        if let Ok(rule) = Rule::of(&s) {
            let next = rule.next_after(&chrono::Local, chrono::Utc::now());
            s.next_start = next.map(|t| t.to_rfc3339());
            s.next_end = next.zip(rule.duration).map(|(t, d)| (t + d).to_rfc3339());
        }
    }
    s.last_run = db::schedule_runs(&state.db, &s.id, 1)
        .await?
        .into_iter()
        .next();
    Ok(s)
}

async fn get_schedules(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<ScheduleDto>>> {
    scheduler(&state).await?;
    let mut out = vec![];
    for s in db::schedules_list(&state.db).await? {
        out.push(decorate(&state, s).await?);
    }
    Ok(Json(out))
}

async fn get_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ScheduleDto>> {
    scheduler(&state).await?;
    let s = db::schedule_get(&state.db, &id)
        .await?
        .ok_or(SCHEDULE_NOT_FOUND)?;
    Ok(Json(decorate(&state, s).await?))
}

async fn post_schedule(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ScheduleInput>,
) -> ApiResult<(StatusCode, Json<ScheduleDto>)> {
    let now = chrono::Utc::now().to_rfc3339();
    let s = save_schedule(&state, uuid::Uuid::new_v4().to_string(), now, req).await?;
    Ok((StatusCode::CREATED, Json(s)))
}

async fn put_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ScheduleInput>,
) -> ApiResult<Json<ScheduleDto>> {
    let old = db::schedule_get(&state.db, &id)
        .await?
        .ok_or(SCHEDULE_NOT_FOUND)?;
    Ok(Json(save_schedule(&state, id, old.created_at, req).await?))
}

async fn save_schedule(
    state: &AppState,
    id: String,
    created_at: String,
    req: ScheduleInput,
) -> ApiResult<ScheduleDto> {
    let scheduler = scheduler(state).await?;
    validate_schedule(state, &req).await?;
    let s = ScheduleDto {
        id,
        name: req.name.trim().to_string(),
        enabled: req.enabled,
        kind: req.kind,
        date: req.date,
        weekdays: req.weekdays,
        start_time: req.start_time,
        duration_s: req.duration_s,
        actions: req.actions,
        created_at,
        updated_at: chrono::Utc::now().to_rfc3339(),
        next_start: None,
        next_end: None,
        last_run: None,
    };
    db::schedule_save(&state.db, &s).await?;
    scheduler.wake();
    state.emit_controller(&WsEvent::ScheduleUpdated {
        schedule_id: s.id.clone(),
        name: s.name.clone(),
        run: None,
    });
    decorate(state, s).await
}

async fn validate_schedule(state: &AppState, req: &ScheduleInput) -> ApiResult<()> {
    if req.name.trim().is_empty() {
        return Err(ApiError::bad("a schedule needs a name"));
    }
    let rule = Rule::parse(
        req.kind,
        req.date.as_deref(),
        req.weekdays,
        &req.start_time,
        req.duration_s,
    )
    .map_err(ApiError::bad)?;
    if req.kind == crate::api::types::ScheduleKind::Once {
        let now = chrono::Utc::now();
        // Still worth saving while its window is open: it starts late.
        let window = rule.duration.unwrap_or(chrono::Duration::minutes(2));
        if rule
            .latest_at_or_before(&chrono::Local, now + chrono::Duration::days(400))
            .is_none_or(|start| start + window <= now)
        {
            return Err(ApiError::bad("that time has passed"));
        }
    }
    if req.actions.is_empty() {
        return Err(ApiError::bad(
            "add a recording or playout for the schedule to start",
        ));
    }

    let known: Vec<String> = match state.controller.read().await.as_ref() {
        Some(c) => c
            .registry
            .read()
            .await
            .all()
            .iter()
            .map(|n| n.id.clone())
            .collect(),
        None => vec![],
    };
    let check_node = |id: &str| {
        if id == state.node_id || known.iter().any(|k| k == id) {
            Ok(())
        } else {
            Err(ApiError::bad(format!(
                "node {id} isn't one of this controller's"
            )))
        }
    };
    let presets = db::presets_list(&state.db).await?;
    for action in &req.actions {
        match action {
            ScheduleAction::Record { targets, preset_id } => {
                if targets.is_empty() {
                    return Err(ApiError::bad("pick the sources to record"));
                }
                if preset_id != super::scheduler::DEFAULT_PRESET_ID
                    && !presets.iter().any(|p| p.id == *preset_id)
                {
                    return Err(ApiError::bad("pick a preset to record with"));
                }
                for t in targets {
                    check_node(&t.node_id)?;
                }
            }
            ScheduleAction::Playout {
                node_id,
                channel_id,
                ..
            } => {
                if channel_id.is_empty() {
                    return Err(ApiError::bad("pick a channel to play"));
                }
                check_node(node_id)?;
            }
        }
    }
    Ok(())
}

/// Delete a schedule, stopping what its running occurrence started first.
async fn delete_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let scheduler = scheduler(&state).await?;
    let s = db::schedule_get(&state.db, &id)
        .await?
        .ok_or(SCHEDULE_NOT_FOUND)?;
    scheduler.stop_now(&state, &id).await?;
    db::schedule_delete(&state.db, &id).await?;
    scheduler.wake();
    state.emit_controller(&WsEvent::ScheduleUpdated {
        schedule_id: id,
        name: s.name,
        run: None,
    });
    Ok(StatusCode::NO_CONTENT)
}

/// End a schedule's running occurrence now.
async fn post_schedule_stop(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ScheduleDto>> {
    let scheduler = scheduler(&state).await?;
    let s = db::schedule_get(&state.db, &id)
        .await?
        .ok_or(SCHEDULE_NOT_FOUND)?;
    if !scheduler.stop_now(&state, &id).await? {
        return Err(ApiError::Conflict("the schedule isn't running"));
    }
    Ok(Json(decorate(&state, s).await?))
}

#[derive(serde::Deserialize)]
struct RunsQuery {
    limit: Option<u32>,
}

async fn get_schedule_runs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RunsQuery>,
) -> ApiResult<Json<Vec<ScheduleRunDto>>> {
    scheduler(&state).await?;
    let limit = q.limit.unwrap_or(20).min(200);
    Ok(Json(db::schedule_runs(&state.db, &id, limit).await?))
}

async fn ws_handler(State(state): State<Arc<AppState>>, upgrade: WebSocketUpgrade) -> Response {
    let rx = state.ws_tx.subscribe();
    let token_changed = state.access_token.subscribe();
    upgrade.on_upgrade(move |socket| ws::handle(socket, rx, token_changed))
}

/// This node's thumbnails and, while a controller, its peers'.
async fn thumbnails_ws_handler(
    State(state): State<Arc<AppState>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| crate::thumbnails::serve(socket, state, true))
}
