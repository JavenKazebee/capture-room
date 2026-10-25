use anyhow::{Context, Result};
use sqlx::{sqlite::SqliteConnectOptions, types::Json, FromRow, Sqlite, SqlitePool, Transaction};
use std::collections::HashMap;
use std::str::FromStr;
use tracing::info;

use crate::api::types::{
    BenchmarkRunDto, BenchmarkStatus, ClipEnd, ConfiguredSourceDto, DecklinkProfile, MediaItemDto,
    MonitorSettingsDto, NdiConnect, NdiSubscriptionsInput, PresetDto, PresetOutputInput,
    RecordingSessionDto, ReplayCodec, ReplayConfigInput, ScheduleDto, ScheduleRunDto,
    ScheduleRunStatus,
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

    let migrator = sqlx::migrate!("./migrations");
    let fresh = is_fresh(&pool).await;
    adopt_squashed_schema(&pool, &migrator).await?;
    migrator.run(&pool).await.context("run migrations")?;
    if fresh {
        seed_fresh(&pool).await.context("seed a new database")?;
    }

    info!(path = db_path, "database ready");
    Ok(pool)
}

/// Nothing has been migrated yet: no migrations table, or an empty one.
async fn is_fresh(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
        .map_or(true, |n| n == 0)
}

/// `0001_schema.sql` seeds two test sources, and can't be changed without
/// breaking its checksum. A new install starts with one instead; databases
/// that already have the two keep them.
async fn seed_fresh(pool: &SqlitePool) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM configured_sources WHERE id IN ('test-1', 'test-2')")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO configured_sources (id, name, config, created_at) VALUES \
         ('test-1', 'Test Source', ?, datetime('now'))",
    )
    .bind(
        r#"{"type":"test","pattern":"smpte","width":1920,"height":1080,"fps_num":30,"fps_den":1,"audio_signal":"pink-noise","frequency":0.0,"channels":2}"#,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The last of the development migrations squashed into `0001_schema.sql`.
const SQUASHED_LAST: (i64, &str) = (20, "replay clip groups");

/// A database from before the squash has all 20 development migrations
/// applied, and the same schema `0001_schema.sql` creates: record it as that
/// one migration instead, so sqlx doesn't refuse it. Remove once no node
/// predates the squash.
async fn adopt_squashed_schema(
    pool: &SqlitePool,
    migrator: &sqlx::migrate::Migrator,
) -> Result<()> {
    let applied: Vec<(i64, String)> = match sqlx::query_as(
        "SELECT version, description FROM _sqlx_migrations WHERE success ORDER BY version",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        // A new database: no migrations table yet.
        Err(_) => return Ok(()),
    };
    if !matches!(applied.first(), Some((1, d)) if d == "baseline") {
        return Ok(());
    }
    if applied.last().map(|(v, d)| (*v, d.as_str())) != Some(SQUASHED_LAST)
        || applied.len() as i64 != SQUASHED_LAST.0
    {
        anyhow::bail!(
            "this database is from a development build older than its last schema change; \
             start that build once to bring it up to date, or delete the database"
        );
    }
    let schema = migrator
        .iter()
        .find(|m| m.version == 1)
        .context("no 0001 migration")?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version > 1")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE _sqlx_migrations SET description = ?, checksum = ? WHERE version = 1")
        .bind(schema.description.as_ref())
        .bind(schema.checksum.as_ref())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    info!("recorded the development migrations as the squashed schema");
    Ok(())
}

/// Run a `DELETE ... WHERE <key> = ?` for `id`: whether it removed a row.
async fn delete_one(pool: &SqlitePool, sql: &str, id: &str) -> Result<bool> {
    let res = sqlx::query(sql).bind(id).execute(pool).await?;
    Ok(res.rows_affected() > 0)
}

pub async fn config_get(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let row = sqlx::query_scalar::<_, String>("SELECT value FROM node_config WHERE key = ?")
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

/// Stored settings, with defaults for anything missing or unparsable.
pub async fn monitor_settings_get(pool: &SqlitePool) -> Result<MonitorSettingsDto> {
    async fn get<T: FromStr>(pool: &SqlitePool, key: &str) -> Result<Option<T>> {
        Ok(config_get(pool, key).await?.and_then(|v| v.parse().ok()))
    }
    let def = MonitorSettingsDto::default();
    // Clamped so values stored under older, looser limits are brought in range.
    Ok(MonitorSettingsDto {
        thumb_fps: get(pool, "monitor_thumb_fps")
            .await?
            .unwrap_or(def.thumb_fps),
        thumb_width: get(pool, "monitor_thumb_width")
            .await?
            .unwrap_or(def.thumb_width),
        thumb_height: get(pool, "monitor_thumb_height")
            .await?
            .unwrap_or(def.thumb_height),
        level_interval_ms: get(pool, "monitor_level_ms")
            .await?
            .unwrap_or(def.level_interval_ms),
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

const SESSION_SELECT: &str = "SELECT id, source_id, preset_id, source_name, preset_name, started_at, stopped_at,
                                     outputs, output_paths, dropped_frames, files, status, error_message,
                                     clock
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
         (id, source_id, preset_id, source_name, preset_name, started_at, stopped_at, outputs,
          output_paths, dropped_frames, files, status, error_message, clock)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&s.id)
    .bind(&s.source_id)
    .bind(&s.preset_id)
    .bind(&s.source_name)
    .bind(&s.preset_name)
    .bind(&s.started_at)
    .bind(&s.stopped_at)
    .bind(Json(&s.outputs))
    .bind(Json(&s.output_paths))
    .bind(Json(&s.dropped_frames))
    .bind(Json(&s.files))
    .bind(s.status)
    .bind(&s.error_message)
    .bind(s.clock.as_ref().map(Json))
    .execute(pool)
    .await?;
    Ok(())
}

/// Save how a session finished: stop time, status, error, dropped frames,
/// files and clock.
pub async fn session_update_stop(pool: &SqlitePool, s: &RecordingSessionDto) -> Result<()> {
    sqlx::query(
        "UPDATE recording_sessions
         SET stopped_at = ?, status = ?, error_message = ?, dropped_frames = ?, files = ?,
             clock = COALESCE(?, clock)
         WHERE id = ?",
    )
    .bind(&s.stopped_at)
    .bind(s.status)
    .bind(&s.error_message)
    .bind(Json(&s.dropped_frames))
    .bind(Json(&s.files))
    .bind(s.clock.as_ref().map(Json))
    .bind(&s.id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn session_update_files(
    pool: &SqlitePool,
    id: &str,
    files: &[Vec<String>],
) -> Result<()> {
    sqlx::query("UPDATE recording_sessions SET files = ? WHERE id = ?")
        .bind(Json(files))
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Sessions newest first, `limit` at a time: those that started before
/// `before` (RFC 3339, compared as text like every stored `started_at`), or
/// the newest without it.
pub async fn sessions_list(
    pool: &SqlitePool,
    before: Option<&str>,
    limit: u32,
) -> Result<Vec<RecordingSessionDto>> {
    let rows = sqlx::query_as::<_, RecordingSessionDto>(&format!(
        "{SESSION_SELECT} WHERE (?1 IS NULL OR started_at < ?1) ORDER BY started_at DESC LIMIT ?2"
    ))
    .bind(before)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Remove a session from history. Its files are left alone; the API deletes
/// them first when asked to.
pub async fn session_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    delete_one(pool, "DELETE FROM recording_sessions WHERE id = ?", id).await
}

pub async fn session_get(pool: &SqlitePool, id: &str) -> Result<Option<RecordingSessionDto>> {
    let row = sqlx::query_as::<_, RecordingSessionDto>(&format!("{SESSION_SELECT} WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn presets_list(pool: &SqlitePool) -> Result<Vec<PresetDto>> {
    let mut presets = sqlx::query_as::<_, PresetDto>(
        "SELECT id, name, created_at, updated_at, version FROM presets ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    /// A row of `preset_outputs`; its id and order stay in the database.
    #[derive(FromRow)]
    struct Row {
        preset_id: String,
        #[sqlx(flatten)]
        output: PresetOutputInput,
    }
    let outputs = sqlx::query_as::<_, Row>(
        "SELECT preset_id, name, codec, container, resolution, framerate,
                bitrate_kbps, chroma, path_template, preview, advanced
         FROM preset_outputs ORDER BY preset_id, sort_order",
    )
    .fetch_all(pool)
    .await?;
    for row in outputs {
        if let Some(p) = presets.iter_mut().find(|p| p.id == row.preset_id) {
            p.outputs.push(row.output);
        }
    }
    Ok(presets)
}

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
    outputs: Vec<PresetOutputInput>,
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
    delete_one(pool, "DELETE FROM presets WHERE id = ?", id).await
}

async fn preset_outputs_replace(
    tx: &mut Transaction<'_, Sqlite>,
    preset_id: &str,
    outputs: &[PresetOutputInput],
) -> Result<()> {
    sqlx::query("DELETE FROM preset_outputs WHERE preset_id = ?")
        .bind(preset_id)
        .execute(&mut **tx)
        .await?;
    for (o, sort_order) in outputs.iter().zip(0u32..) {
        sqlx::query(
            "INSERT INTO preset_outputs
             (id, preset_id, name, codec, container, resolution, framerate,
              bitrate_kbps, chroma, path_template, preview, advanced, sort_order)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(preset_id)
        .bind(&o.name)
        .bind(o.codec)
        .bind(o.container)
        .bind(&o.resolution)
        .bind(&o.framerate)
        .bind(o.bitrate_kbps)
        .bind(o.chroma)
        .bind(&o.path_template)
        .bind(o.preview)
        .bind(Json(&o.advanced))
        .bind(sort_order)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, FromRow)]
pub struct NodeRow {
    pub id: String,
    pub name: String,
    pub url: String,
    pub fingerprint: Option<String>,
    pub added_at: String,
}

pub async fn nodes_list(pool: &SqlitePool) -> Result<Vec<NodeRow>> {
    let rows =
        sqlx::query_as::<_, NodeRow>("SELECT id, name, url, fingerprint, added_at FROM nodes")
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

pub async fn node_upsert(pool: &SqlitePool, row: &NodeRow) -> Result<()> {
    sqlx::query(
        "INSERT INTO nodes (id, name, url, fingerprint, added_at) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             name = excluded.name, url = excluded.url, fingerprint = excluded.fingerprint",
    )
    .bind(&row.id)
    .bind(&row.name)
    .bind(&row.url)
    .bind(&row.fingerprint)
    .bind(&row.added_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn node_delete(pool: &SqlitePool, id: &str) -> Result<()> {
    delete_one(pool, "DELETE FROM nodes WHERE id = ?", id).await?;
    Ok(())
}

const CONFIGURED_SOURCE_SELECT: &str =
    "SELECT id, name, config, created_at FROM configured_sources";

pub async fn configured_sources_list(pool: &SqlitePool) -> Result<Vec<ConfiguredSourceDto>> {
    let rows = sqlx::query_as::<_, ConfiguredSourceDto>(&format!(
        "{CONFIGURED_SOURCE_SELECT} ORDER BY created_at"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn configured_source_get(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<ConfiguredSourceDto>> {
    let row = sqlx::query_as::<_, ConfiguredSourceDto>(&format!(
        "{CONFIGURED_SOURCE_SELECT} WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn configured_source_insert(pool: &SqlitePool, row: &ConfiguredSourceDto) -> Result<()> {
    sqlx::query(
        "INSERT INTO configured_sources (id, name, config, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&row.id)
    .bind(&row.name)
    .bind(Json(&row.config))
    .bind(&row.created_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn configured_source_update(
    pool: &SqlitePool,
    row: &ConfiguredSourceDto,
) -> Result<bool> {
    let res = sqlx::query("UPDATE configured_sources SET name = ?, config = ? WHERE id = ?")
        .bind(&row.name)
        .bind(Json(&row.config))
        .bind(&row.id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Delete a source with its playlist and replay buffer, atomically: false
/// (and nothing deleted) if there's no such source.
pub async fn configured_source_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query("DELETE FROM configured_sources WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if res.rows_affected() == 0 {
        return Ok(false);
    }
    for sql in [
        "DELETE FROM playlist_items WHERE channel_id = ?",
        "DELETE FROM playlists WHERE channel_id = ?",
        "DELETE FROM replay_buffers WHERE source_id = ?",
    ] {
        sqlx::query(sql).bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(true)
}

const MEDIA_SELECT: &str = "SELECT id, path, name, info, origin, session_id, added_at FROM media";

pub async fn media_list(pool: &SqlitePool) -> Result<Vec<MediaItemDto>> {
    let rows = sqlx::query_as::<_, MediaItemDto>(&format!("{MEDIA_SELECT} ORDER BY name"))
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn media_get(pool: &SqlitePool, id: &str) -> Result<Option<MediaItemDto>> {
    Ok(
        sqlx::query_as::<_, MediaItemDto>(&format!("{MEDIA_SELECT} WHERE id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// Add `item`; false if its path is already in the library.
pub async fn media_insert(pool: &SqlitePool, item: &MediaItemDto) -> Result<bool> {
    let res = sqlx::query(
        "INSERT INTO media (id, path, name, info, origin, session_id, added_at)
         VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(path) DO NOTHING",
    )
    .bind(&item.id)
    .bind(&item.path)
    .bind(&item.name)
    .bind(Json(&item.info))
    .bind(item.origin)
    .bind(&item.session_id)
    .bind(&item.added_at)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn media_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    delete_one(pool, "DELETE FROM media WHERE id = ?", id).await
}

const NDI_CONNECT: &str = "ndi_connect";

pub async fn ndi_subscriptions_get(pool: &SqlitePool) -> Result<NdiSubscriptionsInput> {
    let connect = match config_get(pool, NDI_CONNECT).await?.as_deref() {
        Some("chosen") => NdiConnect::Chosen,
        _ => NdiConnect::All,
    };
    let chosen =
        sqlx::query_scalar::<_, String>("SELECT ndi_name FROM ndi_subscriptions ORDER BY ndi_name")
            .fetch_all(pool)
            .await?;
    Ok(NdiSubscriptionsInput { connect, chosen })
}

pub async fn ndi_subscriptions_set(pool: &SqlitePool, subs: &NdiSubscriptionsInput) -> Result<()> {
    let connect = match subs.connect {
        NdiConnect::All => "all",
        NdiConnect::Chosen => "chosen",
    };
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO node_config (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(NDI_CONNECT)
    .bind(connect)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM ndi_subscriptions")
        .execute(&mut *tx)
        .await?;
    for name in &subs.chosen {
        sqlx::query("INSERT OR IGNORE INTO ndi_subscriptions (ndi_name) VALUES (?)")
            .bind(name)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

const DECKLINK_PROFILES: &str = "decklink_profiles";

/// Each Decklink card's profile, by card key (cards left at the default
/// aren't stored).
pub async fn decklink_profiles_get(pool: &SqlitePool) -> Result<HashMap<String, DecklinkProfile>> {
    Ok(config_get(pool, DECKLINK_PROFILES)
        .await?
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}

pub async fn decklink_profiles_set(
    pool: &SqlitePool,
    profiles: &HashMap<String, DecklinkProfile>,
) -> Result<()> {
    config_set(pool, DECKLINK_PROFILES, &serde_json::to_string(profiles)?).await
}

/// Each source with a replay buffer, and its config.
pub async fn replay_buffers_list(pool: &SqlitePool) -> Result<Vec<(String, ReplayConfigInput)>> {
    let rows = sqlx::query_as::<_, (String, ReplayCodec, i64)>(
        "SELECT source_id, codec, minutes FROM replay_buffers",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, codec, minutes)| {
            let minutes = minutes.clamp(1, u32::MAX as i64) as u32;
            (id, ReplayConfigInput { codec, minutes })
        })
        .collect())
}

pub async fn replay_buffer_set(
    pool: &SqlitePool,
    source_id: &str,
    config: &ReplayConfigInput,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO replay_buffers (source_id, codec, minutes) VALUES (?, ?, ?)
         ON CONFLICT(source_id) DO UPDATE SET codec = excluded.codec, minutes = excluded.minutes",
    )
    .bind(source_id)
    .bind(config.codec)
    .bind(config.minutes as i64)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn replay_buffer_delete(pool: &SqlitePool, source_id: &str) -> Result<bool> {
    delete_one(
        pool,
        "DELETE FROM replay_buffers WHERE source_id = ?",
        source_id,
    )
    .await
}

/// A saved replay clip as stored. Positions (µs) count from its first
/// file's start, which was at `base_us` on the node clock.
#[derive(Debug, Clone, FromRow)]
pub struct ReplayClipRow {
    pub id: String,
    pub source_id: String,
    pub source_name: String,
    pub name: String,
    pub codec: ReplayCodec,
    pub files: Json<Vec<String>>,
    pub base_us: i64,
    pub from_us: i64,
    pub to_us: i64,
    pub utc_offset_us: Option<i64>,
    pub bytes: i64,
    pub created_at: String,
    /// The group it was saved in, with the other angles' clips.
    pub group_id: Option<String>,
    /// Read only: the group's name.
    pub group_name: Option<String>,
}

const REPLAY_CLIP_SELECT: &str = "SELECT c.id, c.source_id, c.source_name, c.name, c.codec,
    c.files, c.base_us, c.from_us, c.to_us, c.utc_offset_us, c.bytes, c.created_at,
    c.group_id, g.name AS group_name
    FROM replay_clips c LEFT JOIN replay_clip_groups g ON g.id = c.group_id";

/// Newest first; a group's clips in the order they were saved.
pub async fn replay_clips_list(pool: &SqlitePool) -> Result<Vec<ReplayClipRow>> {
    Ok(sqlx::query_as::<_, ReplayClipRow>(&format!(
        "{REPLAY_CLIP_SELECT} ORDER BY COALESCE(g.created_at, c.created_at) DESC, c.rowid"
    ))
    .fetch_all(pool)
    .await?)
}

pub async fn replay_clip_get(pool: &SqlitePool, id: &str) -> Result<Option<ReplayClipRow>> {
    Ok(
        sqlx::query_as::<_, ReplayClipRow>(&format!("{REPLAY_CLIP_SELECT} WHERE c.id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// A group's clips, in the order they were saved.
pub async fn replay_clip_group_clips(
    pool: &SqlitePool,
    group_id: &str,
) -> Result<Vec<ReplayClipRow>> {
    Ok(sqlx::query_as::<_, ReplayClipRow>(&format!(
        "{REPLAY_CLIP_SELECT} WHERE c.group_id = ? ORDER BY c.rowid"
    ))
    .bind(group_id)
    .fetch_all(pool)
    .await?)
}

pub async fn replay_clip_insert(pool: &SqlitePool, clip: &ReplayClipRow) -> Result<()> {
    insert_clip(pool, clip).await
}

/// A group and its clips, all or none.
pub async fn replay_clip_group_insert(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    created_at: &str,
    clips: &[ReplayClipRow],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO replay_clip_groups (id, name, created_at) VALUES (?, ?, ?)")
        .bind(id)
        .bind(name)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
    for clip in clips {
        insert_clip(&mut *tx, clip).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn replay_clip_group_rename(pool: &SqlitePool, id: &str, name: &str) -> Result<bool> {
    let res = sqlx::query("UPDATE replay_clip_groups SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Delete a group and its clips' rows; their files are the caller's.
pub async fn replay_clip_group_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    delete_one(pool, "DELETE FROM replay_clip_groups WHERE id = ?", id).await
}

async fn insert_clip<'e>(
    db: impl sqlx::Executor<'e, Database = sqlx::Sqlite>,
    clip: &ReplayClipRow,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO replay_clips (id, source_id, source_name, name, codec, files, base_us,
         from_us, to_us, utc_offset_us, bytes, created_at, group_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&clip.id)
    .bind(&clip.source_id)
    .bind(&clip.source_name)
    .bind(&clip.name)
    .bind(clip.codec)
    .bind(&clip.files)
    .bind(clip.base_us)
    .bind(clip.from_us)
    .bind(clip.to_us)
    .bind(clip.utc_offset_us)
    .bind(clip.bytes)
    .bind(&clip.created_at)
    .bind(&clip.group_id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn replay_clip_rename(pool: &SqlitePool, id: &str, name: &str) -> Result<bool> {
    let res = sqlx::query("UPDATE replay_clips SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Delete a clip, and its group if it was the group's last.
pub async fn replay_clip_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let group: Option<Option<String>> =
        sqlx::query_scalar("DELETE FROM replay_clips WHERE id = ? RETURNING group_id")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(group) = group else {
        return Ok(false);
    };
    if let Some(group) = group {
        sqlx::query(
            "DELETE FROM replay_clip_groups WHERE id = ?1
             AND NOT EXISTS (SELECT 1 FROM replay_clips WHERE group_id = ?1)",
        )
        .bind(group)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// A playlist item with its media entry.
#[derive(Debug, Clone, FromRow)]
pub struct PlaylistRow {
    pub id: String,
    pub media_id: String,
    pub in_ms: i64,
    pub out_ms: Option<i64>,
    pub end_action: ClipEnd,
    pub name: String,
    pub path: String,
}

/// A channel's items in order, and whether the playlist loops.
pub async fn playlist_get(pool: &SqlitePool, channel_id: &str) -> Result<(Vec<PlaylistRow>, bool)> {
    let rows = sqlx::query_as::<_, PlaylistRow>(
        "SELECT i.id, i.media_id, i.in_ms, i.out_ms, i.end_action, m.name, m.path
         FROM playlist_items i JOIN media m ON m.id = i.media_id
         WHERE i.channel_id = ? ORDER BY i.position",
    )
    .bind(channel_id)
    .fetch_all(pool)
    .await?;
    let looped =
        sqlx::query_scalar::<_, bool>("SELECT loop_playlist FROM playlists WHERE channel_id = ?")
            .bind(channel_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(false);
    Ok((rows, looped))
}

/// Replace a channel's playlist. `items` are (id, media id, in, out, end).
pub async fn playlist_set(
    pool: &SqlitePool,
    channel_id: &str,
    items: &[(String, String, u64, Option<u64>, ClipEnd)],
    loop_playlist: bool,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM playlist_items WHERE channel_id = ?")
        .bind(channel_id)
        .execute(&mut *tx)
        .await?;
    for (position, (id, media_id, in_ms, out_ms, end)) in items.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_items (id, channel_id, position, media_id, in_ms, out_ms, end_action)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(channel_id)
        .bind(position as i64)
        .bind(media_id)
        .bind(*in_ms as i64)
        .bind(out_ms.map(|v| v as i64))
        .bind(end)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO playlists (channel_id, loop_playlist) VALUES (?, ?)
         ON CONFLICT(channel_id) DO UPDATE SET loop_playlist = excluded.loop_playlist",
    )
    .bind(channel_id)
    .bind(loop_playlist)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Channels whose playlists use `media_id`.
pub async fn playlists_using(pool: &SqlitePool, media_id: &str) -> Result<Vec<String>> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT channel_id FROM playlist_items WHERE media_id = ?",
    )
    .bind(media_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

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

pub async fn benchmarks_list(pool: &SqlitePool) -> Result<Vec<BenchmarkRunDto>> {
    let rows = sqlx::query_scalar::<_, Json<BenchmarkRunDto>>(
        "SELECT run FROM benchmark_results ORDER BY started_at DESC LIMIT 200",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn benchmark_get(pool: &SqlitePool, id: &str) -> Result<Option<BenchmarkRunDto>> {
    let row = sqlx::query_scalar::<_, Json<BenchmarkRunDto>>(
        "SELECT run FROM benchmark_results WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

pub async fn benchmark_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    delete_one(pool, "DELETE FROM benchmark_results WHERE id = ?", id).await
}

/// Runs still marked running: on startup, ones a crash or restart cut short.
pub async fn benchmarks_running(pool: &SqlitePool) -> Result<Vec<BenchmarkRunDto>> {
    let rows = sqlx::query_scalar::<_, Json<BenchmarkRunDto>>(
        "SELECT run FROM benchmark_results WHERE status = ?",
    )
    .bind(BenchmarkStatus::Running)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

const SCHEDULE_SELECT: &str = "SELECT id, name, enabled, kind, date, weekdays, month_day, \
     month_week, start_time, duration_s, actions, created_at, updated_at FROM schedules";

pub async fn schedules_list(pool: &SqlitePool) -> Result<Vec<ScheduleDto>> {
    Ok(
        sqlx::query_as::<_, ScheduleDto>(&format!("{SCHEDULE_SELECT} ORDER BY name"))
            .fetch_all(pool)
            .await?,
    )
}

pub async fn schedule_get(pool: &SqlitePool, id: &str) -> Result<Option<ScheduleDto>> {
    Ok(
        sqlx::query_as::<_, ScheduleDto>(&format!("{SCHEDULE_SELECT} WHERE id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// Insert or replace a schedule (its runs stay).
pub async fn schedule_save(pool: &SqlitePool, s: &ScheduleDto) -> Result<()> {
    sqlx::query(
        "INSERT INTO schedules (id, name, enabled, kind, date, weekdays, month_day, month_week,
                                start_time, duration_s, actions, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET
             name = excluded.name, enabled = excluded.enabled, kind = excluded.kind,
             date = excluded.date, weekdays = excluded.weekdays,
             month_day = excluded.month_day, month_week = excluded.month_week,
             start_time = excluded.start_time, duration_s = excluded.duration_s,
             actions = excluded.actions, updated_at = excluded.updated_at",
    )
    .bind(&s.id)
    .bind(&s.name)
    .bind(s.enabled)
    .bind(s.kind)
    .bind(&s.date)
    .bind(s.weekdays)
    .bind(s.month_day)
    .bind(s.month_week)
    .bind(&s.start_time)
    .bind(s.duration_s)
    .bind(Json(&s.actions))
    .bind(&s.created_at)
    .bind(&s.updated_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a schedule; its runs go with it (`ON DELETE CASCADE`).
pub async fn schedule_delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    delete_one(pool, "DELETE FROM schedules WHERE id = ?", id).await
}

const RUN_SELECT: &str = "SELECT id, schedule_id, planned_start, planned_end, started_at, \
     stopped_at, status, targets FROM schedule_runs";

/// Record a run that came due. `false` if that occurrence already has one.
pub async fn schedule_run_insert(pool: &SqlitePool, r: &ScheduleRunDto) -> Result<bool> {
    let res = sqlx::query(
        "INSERT OR IGNORE INTO schedule_runs (id, schedule_id, planned_start, planned_end,
                                              started_at, stopped_at, status, targets)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&r.id)
    .bind(&r.schedule_id)
    .bind(&r.planned_start)
    .bind(&r.planned_end)
    .bind(&r.started_at)
    .bind(&r.stopped_at)
    .bind(r.status)
    .bind(Json(&r.targets))
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn schedule_run_update(pool: &SqlitePool, r: &ScheduleRunDto) -> Result<()> {
    sqlx::query(
        "UPDATE schedule_runs SET started_at = ?, stopped_at = ?, status = ?, targets = ?
         WHERE id = ?",
    )
    .bind(&r.started_at)
    .bind(&r.stopped_at)
    .bind(r.status)
    .bind(Json(&r.targets))
    .bind(&r.id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether an occurrence already came due.
pub async fn schedule_run_exists(
    pool: &SqlitePool,
    schedule_id: &str,
    planned_start: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM schedule_runs WHERE schedule_id = ? AND planned_start = ?",
    )
    .bind(schedule_id)
    .bind(planned_start)
    .fetch_one(pool)
    .await?
        > 0)
}

pub async fn schedule_runs_running(pool: &SqlitePool) -> Result<Vec<ScheduleRunDto>> {
    Ok(
        sqlx::query_as::<_, ScheduleRunDto>(&format!("{RUN_SELECT} WHERE status = ?"))
            .bind(ScheduleRunStatus::Running)
            .fetch_all(pool)
            .await?,
    )
}

/// A schedule's runs, newest first.
pub async fn schedule_runs(
    pool: &SqlitePool,
    schedule_id: &str,
    limit: u32,
) -> Result<Vec<ScheduleRunDto>> {
    Ok(sqlx::query_as::<_, ScheduleRunDto>(&format!(
        "{RUN_SELECT} WHERE schedule_id = ? ORDER BY planned_start DESC LIMIT ?"
    ))
    .bind(schedule_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{AudioTestSignal, SourceConfig, VideoTestPattern};

    async fn count(pool: &SqlitePool, table: &str, column: &str) -> i64 {
        sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {table} WHERE {column} = 'c'"
        ))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Make `pool` look like a database from before the squash: the squashed
    /// schema (without later migrations' changes), with `upto` development
    /// migrations recorded.
    async fn record_old_migrations(pool: &SqlitePool, upto: i64) {
        sqlx::query("DELETE FROM _sqlx_migrations")
            .execute(pool)
            .await
            .unwrap();
        // Undo 0002, if it's there.
        let _ = sqlx::query("ALTER TABLE nodes DROP COLUMN fingerprint")
            .execute(pool)
            .await;
        // And 0003.
        for table in ["schedule_runs", "schedules"] {
            sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
                .execute(pool)
                .await
                .unwrap();
        }
        for v in 1..=upto {
            let description = match v {
                1 => "baseline",
                20 => "replay clip groups",
                _ => "old",
            };
            sqlx::query(
                "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
                 VALUES (?, ?, 1, x'00', 0)",
            )
            .bind(v)
            .bind(description)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn adopts_a_database_from_before_the_squash() {
        let path =
            std::env::temp_dir().join(format!("capture-room-squash-{}.db", std::process::id()));
        let url = format!("sqlite://{}", path.display());
        let pool = init(&url).await.unwrap();
        record_old_migrations(&pool, 20).await;
        pool.close().await;

        let pool = init(&url).await.unwrap();
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT version, description FROM _sqlx_migrations")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            [
                (1, "schema".to_string()),
                (2, "node fingerprints".to_string()),
                (3, "schedules".to_string()),
                (4, "schedule monthly".to_string())
            ]
        );

        // Short of the last development migration it can't be adopted.
        record_old_migrations(&pool, 19).await;
        pool.close().await;
        assert!(init(&url).await.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn a_new_database_starts_with_one_test_source() {
        let path =
            std::env::temp_dir().join(format!("capture-room-fresh-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let pool = init(&format!("sqlite://{}", path.display())).await.unwrap();
        let sources = configured_sources_list(&pool).await.unwrap();
        assert_eq!(sources.len(), 1);
        let s = &sources[0];
        assert_eq!((s.id.as_str(), s.name.as_str()), ("test-1", "Test Source"));
        let SourceConfig::Test(t) = &s.config else {
            panic!("not a test source: {:?}", s.config);
        };
        assert_eq!(
            (t.pattern, t.width, t.height, t.fps_num, t.fps_den, t.channels),
            (VideoTestPattern::Smpte, 1920, 1080, 30, 1, 2)
        );
        assert_eq!(t.audio_signal, AudioTestSignal::PinkNoise);
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn an_existing_database_keeps_its_test_sources() {
        let path =
            std::env::temp_dir().join(format!("capture-room-seed-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}", path.display());
        // Migrated by an earlier build, which kept the schema's two seeds.
        let opts = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool.close().await;

        let pool = init(&url).await.unwrap();
        let mut ids: Vec<String> = configured_sources_list(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["test-1", "test-2"]);
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn source_delete_takes_its_playlist_and_buffer() {
        let path = std::env::temp_dir().join(format!("capture-room-db-{}.db", std::process::id()));
        let pool = init(&format!("sqlite://{}", path.display())).await.unwrap();
        for sql in [
            "INSERT INTO configured_sources (id, name, config, created_at) VALUES ('c', 'c', '{}', '')",
            "INSERT INTO playlists (channel_id, loop_playlist) VALUES ('c', 0)",
            "INSERT INTO replay_buffers (source_id, codec, minutes) VALUES ('c', 'mjpeg', 5)",
        ] {
            sqlx::query(sql).execute(&pool).await.unwrap();
        }
        assert!(configured_source_delete(&pool, "c").await.unwrap());
        assert!(!configured_source_delete(&pool, "c").await.unwrap());
        for (table, column) in [
            ("configured_sources", "id"),
            ("playlists", "channel_id"),
            ("replay_buffers", "source_id"),
        ] {
            assert_eq!(count(&pool, table, column).await, 0, "{table} left behind");
        }
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn preset_outputs_keep_their_order() {
        let path =
            std::env::temp_dir().join(format!("capture-room-presets-{}.db", std::process::id()));
        let pool = init(&format!("sqlite://{}", path.display())).await.unwrap();
        let leg = |name: &str| -> PresetOutputInput {
            serde_json::from_value(serde_json::json!({
                "name": name, "codec": "h264", "container": "mov", "resolution": null,
                "framerate": null, "bitrate_kbps": null, "chroma": "420",
                "path_template": "~/x.{ext}",
            }))
            .unwrap()
        };
        let names = |p: &PresetDto| p.outputs.iter().map(|o| o.name.clone()).collect::<Vec<_>>();
        let preset = PresetDto {
            id: "p".into(),
            name: "P".into(),
            outputs: vec![leg("b"), leg("a")],
            created_at: String::new(),
            updated_at: String::new(),
            version: 1,
        };
        preset_insert(&pool, &preset).await.unwrap();
        assert_eq!(names(&presets_list(&pool).await.unwrap()[0]), ["b", "a"]);
        let updated = preset_update(&pool, "p", "P", "", vec![leg("c"), leg("b"), leg("a")])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(names(&updated), ["c", "b", "a"]);
        assert_eq!(
            names(&presets_list(&pool).await.unwrap()[0]),
            ["c", "b", "a"]
        );
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn ndi_subscriptions_round_trip() {
        let path = std::env::temp_dir().join(format!("capture-room-ndi-{}.db", std::process::id()));
        let pool = init(&format!("sqlite://{}", path.display())).await.unwrap();
        assert_eq!(
            ndi_subscriptions_get(&pool).await.unwrap(),
            NdiSubscriptionsInput::default()
        );
        let subs = |connect, chosen: &[&str]| NdiSubscriptionsInput {
            connect,
            chosen: chosen.iter().map(|n| n.to_string()).collect(),
        };
        for set in [
            subs(NdiConnect::Chosen, &["A (1)", "B (2)"]),
            subs(NdiConnect::All, &["B (2)"]),
        ] {
            ndi_subscriptions_set(&pool, &set).await.unwrap();
            assert_eq!(ndi_subscriptions_get(&pool).await.unwrap(), set);
        }
        pool.close().await;
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn clip_groups_list_together_and_go_with_their_last_clip() {
        let path =
            std::env::temp_dir().join(format!("capture-room-clips-{}.db", std::process::id()));
        let pool = init(&format!("sqlite://{}", path.display())).await.unwrap();
        let clip = |id: &str, group: Option<&str>, created_at: &str| ReplayClipRow {
            id: id.into(),
            source_id: id.into(),
            source_name: id.into(),
            name: id.into(),
            codec: ReplayCodec::Mjpeg,
            files: Json(vec![]),
            base_us: 0,
            from_us: 0,
            to_us: 1,
            utc_offset_us: None,
            bytes: 0,
            created_at: created_at.into(),
            group_id: group.map(Into::into),
            group_name: None,
        };
        replay_clip_insert(&pool, &clip("old", None, "1"))
            .await
            .unwrap();
        let group = [clip("b", Some("g"), "2"), clip("a", Some("g"), "2")];
        replay_clip_group_insert(&pool, "g", "Goal", "2", &group)
            .await
            .unwrap();
        replay_clip_insert(&pool, &clip("new", None, "3"))
            .await
            .unwrap();

        let list = replay_clips_list(&pool).await.unwrap();
        let ids: Vec<_> = list.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            ["new", "b", "a", "old"],
            "newest first, a group in save order"
        );
        assert_eq!(list[1].group_name.as_deref(), Some("Goal"));

        assert!(replay_clip_group_rename(&pool, "g", "Save").await.unwrap());
        let a = replay_clip_get(&pool, "a").await.unwrap().unwrap();
        assert_eq!(a.group_name.as_deref(), Some("Save"));

        assert!(replay_clip_delete(&pool, "b").await.unwrap());
        assert_eq!(replay_clip_group_clips(&pool, "g").await.unwrap().len(), 1);
        assert!(replay_clip_delete(&pool, "a").await.unwrap());
        assert!(
            !replay_clip_group_rename(&pool, "g", "x").await.unwrap(),
            "group left behind"
        );

        replay_clip_group_insert(&pool, "h", "Miss", "4", &[clip("c", Some("h"), "4")])
            .await
            .unwrap();
        assert!(replay_clip_group_delete(&pool, "h").await.unwrap());
        assert!(
            replay_clip_get(&pool, "c").await.unwrap().is_none(),
            "clip left behind"
        );

        pool.close().await;
        let _ = std::fs::remove_file(path);
    }
}
