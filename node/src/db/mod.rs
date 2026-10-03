use anyhow::{Context, Result};
use sqlx::{sqlite::SqliteConnectOptions, types::Json, FromRow, SqlitePool};
use std::str::FromStr;
use tracing::info;

use crate::api::types::{MonitorSettingsDto, RecordingSessionDto, RecordingStatus, TestSourceConfigDto};

pub async fn init(db_path: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(db_path)
        .context("parse db path")?
        .create_if_missing(true);

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
    Ok(MonitorSettingsDto {
        thumb_fps: get(pool, "monitor_thumb_fps").await?.unwrap_or(def.thumb_fps),
        thumb_width: get(pool, "monitor_thumb_width").await?.unwrap_or(def.thumb_width),
        thumb_height: get(pool, "monitor_thumb_height").await?.unwrap_or(def.thumb_height),
        level_interval_ms: get(pool, "monitor_level_ms").await?.unwrap_or(def.level_interval_ms),
    })
}

pub async fn monitor_settings_set(pool: &SqlitePool, m: &MonitorSettingsDto) -> Result<()> {
    config_set(pool, "monitor_thumb_fps", &m.thumb_fps.to_string()).await?;
    config_set(pool, "monitor_thumb_width", &m.thumb_width.to_string()).await?;
    config_set(pool, "monitor_thumb_height", &m.thumb_height.to_string()).await?;
    config_set(pool, "monitor_level_ms", &m.level_interval_ms.to_string()).await?;
    Ok(())
}

// ── recording_sessions ────────────────────────────────────────────────────────

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
         (id, source_id, preset_id, started_at, stopped_at, output_paths, status, error_message)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&s.id)
    .bind(&s.source_id)
    .bind(&s.preset_id)
    .bind(&s.started_at)
    .bind(&s.stopped_at)
    .bind(Json(&s.output_paths))
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
) -> Result<()> {
    sqlx::query(
        "UPDATE recording_sessions
         SET stopped_at = ?, status = ?, error_message = ?
         WHERE id = ?",
    )
    .bind(stopped_at)
    .bind(status)
    .bind(error_message)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn sessions_list(pool: &SqlitePool) -> Result<Vec<RecordingSessionDto>> {
    let rows = sqlx::query_as::<_, RecordingSessionDto>(
        "SELECT id, source_id, preset_id, started_at, stopped_at,
                output_paths, status, error_message
         FROM recording_sessions
         ORDER BY started_at DESC
         LIMIT 100",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn session_get(pool: &SqlitePool, id: &str) -> Result<Option<RecordingSessionDto>> {
    let row = sqlx::query_as::<_, RecordingSessionDto>(
        "SELECT id, source_id, preset_id, started_at, stopped_at,
                output_paths, status, error_message
         FROM recording_sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// ── presets ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, FromRow)]
pub struct PresetRow {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct PresetOutputRow {
    pub id: String,
    pub preset_id: String,
    pub name: String,
    pub codec: String,
    pub container: String,
    pub resolution: Option<String>,
    pub framerate: Option<String>,
    pub bitrate_kbps: Option<i64>,
    pub chroma: String,
    pub path_template: String,
    pub sort_order: i64,
}

pub async fn presets_list(pool: &SqlitePool) -> Result<Vec<PresetRow>> {
    let rows = sqlx::query_as::<_, PresetRow>(
        "SELECT id, name, created_at, updated_at, version FROM presets ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn preset_get(pool: &SqlitePool, id: &str) -> Result<Option<PresetRow>> {
    let row = sqlx::query_as::<_, PresetRow>(
        "SELECT id, name, created_at, updated_at, version FROM presets WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn preset_insert(pool: &SqlitePool, p: &PresetRow) -> Result<()> {
    sqlx::query(
        "INSERT INTO presets (id, name, created_at, updated_at, version)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&p.id)
    .bind(&p.name)
    .bind(&p.created_at)
    .bind(&p.updated_at)
    .bind(p.version)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn preset_update(pool: &SqlitePool, p: &PresetRow) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE presets SET name = ?, updated_at = ?, version = version + 1 WHERE id = ?",
    )
    .bind(&p.name)
    .bind(&p.updated_at)
    .bind(&p.id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn preset_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    // Delete outputs first (no FK cascade enforcement in SQLite without PRAGMA).
    sqlx::query("DELETE FROM preset_outputs WHERE preset_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let res = sqlx::query("DELETE FROM presets WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

// ── preset_outputs ────────────────────────────────────────────────────────────

pub async fn preset_outputs_list_all(pool: &SqlitePool) -> Result<Vec<PresetOutputRow>> {
    let rows = sqlx::query_as::<_, PresetOutputRow>(
        "SELECT id, preset_id, name, codec, container, resolution, framerate,
                bitrate_kbps, chroma, path_template, sort_order
         FROM preset_outputs ORDER BY preset_id, sort_order",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Replace all output legs for a preset atomically.
pub async fn preset_outputs_replace(
    pool: &SqlitePool,
    preset_id: &str,
    outputs: &[PresetOutputRow],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM preset_outputs WHERE preset_id = ?")
        .bind(preset_id)
        .execute(&mut *tx)
        .await?;
    for o in outputs {
        sqlx::query(
            "INSERT INTO preset_outputs
             (id, preset_id, name, codec, container, resolution, framerate,
              bitrate_kbps, chroma, path_template, sort_order)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&o.id)
        .bind(&o.preset_id)
        .bind(&o.name)
        .bind(&o.codec)
        .bind(&o.container)
        .bind(&o.resolution)
        .bind(&o.framerate)
        .bind(o.bitrate_kbps)
        .bind(&o.chroma)
        .bind(&o.path_template)
        .bind(o.sort_order)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
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

// ── test_sources ──────────────────────────────────────────────────────────────

pub async fn test_sources_list(pool: &SqlitePool) -> Result<Vec<TestSourceConfigDto>> {
    let rows = sqlx::query_as::<_, TestSourceConfigDto>(
        "SELECT id, name, pattern, width, height, fps_num, fps_den,
                audio_signal, frequency, channels, created_at
         FROM test_sources ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn test_source_get(pool: &SqlitePool, id: &str) -> Result<Option<TestSourceConfigDto>> {
    let row = sqlx::query_as::<_, TestSourceConfigDto>(
        "SELECT id, name, pattern, width, height, fps_num, fps_den,
                audio_signal, frequency, channels, created_at
         FROM test_sources WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn test_source_insert(pool: &SqlitePool, row: &TestSourceConfigDto) -> Result<()> {
    sqlx::query(
        "INSERT INTO test_sources
         (id, name, pattern, width, height, fps_num, fps_den,
          audio_signal, frequency, channels, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.id)
    .bind(&row.name)
    .bind(row.pattern)
    .bind(row.width)
    .bind(row.height)
    .bind(row.fps_num)
    .bind(row.fps_den)
    .bind(row.audio_signal)
    .bind(row.frequency)
    .bind(row.channels)
    .bind(&row.created_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn test_source_update(pool: &SqlitePool, row: &TestSourceConfigDto) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE test_sources
         SET name = ?, pattern = ?, width = ?, height = ?,
             fps_num = ?, fps_den = ?, audio_signal = ?,
             frequency = ?, channels = ?
         WHERE id = ?",
    )
    .bind(&row.name)
    .bind(row.pattern)
    .bind(row.width)
    .bind(row.height)
    .bind(row.fps_num)
    .bind(row.fps_den)
    .bind(row.audio_signal)
    .bind(row.frequency)
    .bind(row.channels)
    .bind(&row.id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn test_source_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM test_sources WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}
