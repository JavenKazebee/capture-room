use anyhow::{Context, Result};
use sqlx::{sqlite::SqliteConnectOptions, types::Json, FromRow, Sqlite, SqlitePool, Transaction};
use std::str::FromStr;
use tracing::info;

use crate::api::types::{
    BenchmarkRunDto, BenchmarkStatus, ConfiguredSourceDto, MonitorSettingsDto, PresetDto, PresetOutputDto, RecordingSessionDto, RecordingStatus,
};

pub async fn init(db_path: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(db_path)
        .context("parse db path")?
        .create_if_missing(true)
        // Preset outputs cascade-delete with their preset.
        .foreign_keys(true);

    let pool = SqlitePool::connect_with(opts)
        .await
        .context("open sqlite")?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("run migrations")?;

    info!(path = db_path, "database ready");
    Ok(pool)
}

// ── node_config ───────────────────────────────────────────────────────────────

pub async fn config_get(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let row = sqlx::query_scalar::<_, String>(
        "SELECT value FROM node_config WHERE key = ?",
    )
    .bind(key)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn config_set(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO node_config (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

// ── monitor settings (node_config keys) ──────────────────────────────────────

/// Stored settings, with defaults for anything missing or unparsable.
pub async fn monitor_settings_get(pool: &SqlitePool) -> Result<MonitorSettingsDto> {
    async fn get<T: FromStr>(pool: &SqlitePool, key: &str) -> Result<Option<T>> {
        Ok(config_get(pool, key).await?.and_then(|v| v.parse().ok()))
    }
    let def = MonitorSettingsDto::default();
    // Clamped so values stored under older, looser limits are brought in range.
    Ok(MonitorSettingsDto {
        thumb_fps: get(pool, "monitor_thumb_fps").await?.unwrap_or(def.thumb_fps),
        thumb_width: get(pool, "monitor_thumb_width").await?.unwrap_or(def.thumb_width),
        thumb_height: get(pool, "monitor_thumb_height").await?.unwrap_or(def.thumb_height),
        level_interval_ms: get(pool, "monitor_level_ms").await?.unwrap_or(def.level_interval_ms),
    }
    .clamped())
}

pub async fn monitor_settings_set(pool: &SqlitePool, m: &MonitorSettingsDto) -> Result<()> {
    config_set(pool, "monitor_thumb_fps", &m.thumb_fps.to_string()).await?;
    config_set(pool, "monitor_thumb_width", &m.thumb_width.to_string()).await?;
    config_set(pool, "monitor_thumb_height", &m.thumb_height.to_string()).await?;
    config_set(pool, "monitor_level_ms", &m.level_interval_ms.to_string()).await?;
    Ok(())
}

// ── recording_sessions ────────────────────────────────────────────────────────

const SESSION_SELECT: &str = "SELECT id, source_id, preset_id, started_at, stopped_at,
                                     output_paths, dropped_frames, files, status, error_message
                              FROM recording_sessions";

pub async fn sessions_mark_crashed(pool: &SqlitePool) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE recording_sessions
         SET status = 'error', stopped_at = ?, error_message = 'node restarted'
         WHERE status = 'active'",
    )
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn session_insert(pool: &SqlitePool, s: &RecordingSessionDto) -> Result<()> {
    sqlx::query(
        "INSERT INTO recording_sessions
         (id, source_id, preset_id, started_at, stopped_at, output_paths, dropped_frames, files, status, error_message)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&s.id)
    .bind(&s.source_id)
    .bind(&s.preset_id)
    .bind(&s.started_at)
    .bind(&s.stopped_at)
    .bind(Json(&s.output_paths))
    .bind(Json(&s.dropped_frames))
    .bind(Json(&s.files))
    .bind(s.status)
    .bind(&s.error_message)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn session_update_stop(
    pool: &SqlitePool,
    id: &str,
    stopped_at: &str,
    status: RecordingStatus,
    error_message: Option<&str>,
    dropped_frames: Option<&[u64]>,
    files: Option<&[Vec<String>]>,
) -> Result<()> {
    sqlx::query(
        "UPDATE recording_sessions
         SET stopped_at = ?, status = ?, error_message = ?, dropped_frames = COALESCE(?, dropped_frames),
             files = COALESCE(?, files)
         WHERE id = ?",
    )
    .bind(stopped_at)
    .bind(status)
    .bind(error_message)
    .bind(dropped_frames.map(Json))
    .bind(files.map(Json))
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record the files a running session's legs have written so far.
pub async fn session_update_files(pool: &SqlitePool, id: &str, files: &[Vec<String>]) -> Result<()> {
    sqlx::query("UPDATE recording_sessions SET files = ? WHERE id = ?")
        .bind(Json(files))
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn sessions_list(pool: &SqlitePool) -> Result<Vec<RecordingSessionDto>> {
    let rows = sqlx::query_as::<_, RecordingSessionDto>(&format!(
        "{SESSION_SELECT} ORDER BY started_at DESC LIMIT 100"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn session_get(pool: &SqlitePool, id: &str) -> Result<Option<RecordingSessionDto>> {
    let row = sqlx::query_as::<_, RecordingSessionDto>(&format!("{SESSION_SELECT} WHERE id = ?"))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// ── presets ───────────────────────────────────────────────────────────────────

/// Every preset with its output legs, ordered by name.
pub async fn presets_list(pool: &SqlitePool) -> Result<Vec<PresetDto>> {
    let mut presets = sqlx::query_as::<_, PresetDto>(
        "SELECT id, name, created_at, updated_at, version FROM presets ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    let outputs = sqlx::query_as::<_, PresetOutputDto>(
        "SELECT id, preset_id, name, codec, container, resolution, framerate,
                bitrate_kbps, chroma, path_template, advanced, sort_order
         FROM preset_outputs ORDER BY preset_id, sort_order",
    )
    .fetch_all(pool)
    .await?;
    for output in outputs {
        if let Some(p) = presets.iter_mut().find(|p| p.id == output.preset_id) {
            p.outputs.push(output);
        }
    }
    Ok(presets)
}

/// Insert a preset and its output legs atomically.
pub async fn preset_insert(pool: &SqlitePool, p: &PresetDto) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO presets (id, name, created_at, updated_at, version)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&p.id)
    .bind(&p.name)
    .bind(&p.created_at)
    .bind(&p.updated_at)
    .bind(p.version)
    .execute(&mut *tx)
    .await?;
    preset_outputs_replace(&mut tx, &p.id, &p.outputs).await?;
    tx.commit().await?;
    Ok(())
}

/// Rename a preset and replace its output legs atomically. Returns the
/// updated preset, or `None` if it doesn't exist.
pub async fn preset_update(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    updated_at: &str,
    outputs: Vec<PresetOutputDto>,
) -> Result<Option<PresetDto>> {
    let mut tx = pool.begin().await?;
    let preset = sqlx::query_as::<_, PresetDto>(
        "UPDATE presets SET name = ?, updated_at = ?, version = version + 1 WHERE id = ?
         RETURNING id, name, created_at, updated_at, version",
    )
    .bind(name)
    .bind(updated_at)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(mut preset) = preset else {
        return Ok(None);
    };
    preset_outputs_replace(&mut tx, id, &outputs).await?;
    tx.commit().await?;
    preset.outputs = outputs;
    Ok(Some(preset))
}

/// Delete a preset; its outputs go with it (`ON DELETE CASCADE`).
pub async fn preset_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM presets WHERE id = ?").bind(id).execute(pool).await?;
    Ok(res.rows_affected() > 0)
}

/// Replace all output legs for a preset, inside the caller's transaction.
async fn preset_outputs_replace(
    tx: &mut Transaction<'_, Sqlite>,
    preset_id: &str,
    outputs: &[PresetOutputDto],
) -> Result<()> {
    sqlx::query("DELETE FROM preset_outputs WHERE preset_id = ?")
        .bind(preset_id)
        .execute(&mut **tx)
        .await?;
    for PresetOutputDto { id, preset_id, output: o, sort_order } in outputs {
        sqlx::query(
            "INSERT INTO preset_outputs
             (id, preset_id, name, codec, container, resolution, framerate,
              bitrate_kbps, chroma, path_template, advanced, sort_order)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(preset_id)
        .bind(&o.name)
        .bind(o.codec)
        .bind(o.container)
        .bind(&o.resolution)
        .bind(&o.framerate)
        .bind(o.bitrate_kbps)
        .bind(o.chroma)
        .bind(&o.path_template)
        .bind(Json(&o.advanced))
        .bind(sort_order)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

// ── nodes (manual registrations on a controller) ─────────────────────────────

#[derive(Debug, Clone, FromRow)]
pub struct NodeRow {
    pub id: String,
    pub name: String,
    pub url: String,
    pub added_at: String,
}

pub async fn nodes_list(pool: &SqlitePool) -> Result<Vec<NodeRow>> {
    let rows = sqlx::query_as::<_, NodeRow>("SELECT id, name, url, added_at FROM nodes")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn node_upsert(pool: &SqlitePool, row: &NodeRow) -> Result<()> {
    sqlx::query(
        "INSERT INTO nodes (id, name, url, added_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, url = excluded.url",
    )
    .bind(&row.id)
    .bind(&row.name)
    .bind(&row.url)
    .bind(&row.added_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn node_delete(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM nodes WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── configured_sources ────────────────────────────────────────────────────────

const CONFIGURED_SOURCE_SELECT: &str = "SELECT id, name, config, created_at FROM configured_sources";

pub async fn configured_sources_list(pool: &SqlitePool) -> Result<Vec<ConfiguredSourceDto>> {
    let rows = sqlx::query_as::<_, ConfiguredSourceDto>(&format!("{CONFIGURED_SOURCE_SELECT} ORDER BY created_at"))
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn configured_source_get(pool: &SqlitePool, id: &str) -> Result<Option<ConfiguredSourceDto>> {
    let row = sqlx::query_as::<_, ConfiguredSourceDto>(&format!("{CONFIGURED_SOURCE_SELECT} WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn configured_source_insert(pool: &SqlitePool, row: &ConfiguredSourceDto) -> Result<()> {
    sqlx::query("INSERT INTO configured_sources (id, name, config, created_at) VALUES (?, ?, ?, ?)")
        .bind(&row.id)
        .bind(&row.name)
        .bind(Json(&row.config))
        .bind(&row.created_at)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn configured_source_update(pool: &SqlitePool, row: &ConfiguredSourceDto) -> Result<bool> {
    let res = sqlx::query("UPDATE configured_sources SET name = ?, config = ? WHERE id = ?")
        .bind(&row.name)
        .bind(Json(&row.config))
        .bind(&row.id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn configured_source_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM configured_sources WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

// ── benchmark_results ─────────────────────────────────────────────────────────

/// Insert or replace a benchmark run.
pub async fn benchmark_save(pool: &SqlitePool, run: &BenchmarkRunDto) -> Result<()> {
    sqlx::query(
        "INSERT INTO benchmark_results (id, started_at, status, run) VALUES (?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET status = excluded.status, run = excluded.run",
    )
    .bind(&run.id)
    .bind(&run.started_at)
    .bind(run.status)
    .bind(Json(run))
    .execute(pool)
    .await?;
    Ok(())
}

/// The latest runs, newest first.
pub async fn benchmarks_list(pool: &SqlitePool) -> Result<Vec<BenchmarkRunDto>> {
    let rows = sqlx::query_scalar::<_, Json<BenchmarkRunDto>>(
        "SELECT run FROM benchmark_results ORDER BY started_at DESC LIMIT 200",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn benchmark_get(pool: &SqlitePool, id: &str) -> Result<Option<BenchmarkRunDto>> {
    let row = sqlx::query_scalar::<_, Json<BenchmarkRunDto>>("SELECT run FROM benchmark_results WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn benchmark_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM benchmark_results WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Runs still marked running: on startup, ones a crash or restart cut short.
pub async fn benchmarks_running(pool: &SqlitePool) -> Result<Vec<BenchmarkRunDto>> {
    let rows = sqlx::query_scalar::<_, Json<BenchmarkRunDto>>("SELECT run FROM benchmark_results WHERE status = ?")
        .bind(BenchmarkStatus::Running)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}
