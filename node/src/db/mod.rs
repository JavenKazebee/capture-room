use anyhow::{Context, Result};
use sqlx::{sqlite::SqliteConnectOptions, FromRow, SqlitePool};
use std::str::FromStr;
use tracing::info;

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

// ── recording_sessions ────────────────────────────────────────────────────────

#[derive(Debug, FromRow)]
pub struct SessionRow {
    pub id: String,
    pub source_id: String,
    pub preset_id: String,
    pub started_at: String,
    pub stopped_at: Option<String>,
    pub primary_path: String,
    pub secondary_path: Option<String>,
    pub redundant_path: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
}

pub async fn session_insert(pool: &SqlitePool, s: &SessionRow) -> Result<()> {
    sqlx::query(
        "INSERT INTO recording_sessions
         (id, source_id, preset_id, started_at, stopped_at, primary_path,
          secondary_path, redundant_path, status, error_message)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&s.id)
    .bind(&s.source_id)
    .bind(&s.preset_id)
    .bind(&s.started_at)
    .bind(&s.stopped_at)
    .bind(&s.primary_path)
    .bind(&s.secondary_path)
    .bind(&s.redundant_path)
    .bind(&s.status)
    .bind(&s.error_message)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn session_update_stop(
    pool: &SqlitePool,
    id: &str,
    stopped_at: &str,
    status: &str,
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

pub async fn sessions_list(pool: &SqlitePool) -> Result<Vec<SessionRow>> {
    let rows = sqlx::query_as::<_, SessionRow>(
        "SELECT id, source_id, preset_id, started_at, stopped_at,
                primary_path, secondary_path, redundant_path,
                status, error_message
         FROM recording_sessions
         ORDER BY started_at DESC
         LIMIT 100",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn session_get(pool: &SqlitePool, id: &str) -> Result<Option<SessionRow>> {
    let row = sqlx::query_as::<_, SessionRow>(
        "SELECT id, source_id, preset_id, started_at, stopped_at,
                primary_path, secondary_path, redundant_path,
                status, error_message
         FROM recording_sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// ── presets_cache ─────────────────────────────────────────────────────────────

#[derive(Debug, FromRow)]
pub struct PresetCacheRow {
    pub id: String,
    pub name: String,
    pub data: String,
    pub version: i64,
    pub synced_at: String,
}

pub async fn presets_list(pool: &SqlitePool) -> Result<Vec<PresetCacheRow>> {
    let rows = sqlx::query_as::<_, PresetCacheRow>(
        "SELECT id, name, data, version, synced_at FROM presets_cache ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn presets_replace(pool: &SqlitePool, presets: &[PresetCacheRow]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM presets_cache")
        .execute(&mut *tx)
        .await?;
    for p in presets {
        sqlx::query(
            "INSERT INTO presets_cache (id, name, data, version, synced_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&p.id)
        .bind(&p.name)
        .bind(&p.data)
        .bind(p.version)
        .bind(&p.synced_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
