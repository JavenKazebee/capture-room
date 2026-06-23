//! Preset (and later schedule) sync from an aggregator down to its nodes.

use anyhow::Result;
use serde_json::Value;
use tracing::warn;

use crate::api::types::{PresetCacheDto, PresetDto, PresetSyncRequest};
use crate::db::{self, PresetRow};
use crate::state::AppState;

pub fn preset_row_to_dto(r: &PresetRow) -> PresetDto {
    PresetDto {
        id: r.id.clone(),
        name: r.name.clone(),
        codec: r.codec.clone(),
        container: r.container.clone(),
        resolution: r.resolution.clone(),
        framerate: r.framerate.clone(),
        bitrate_kbps: r.bitrate_kbps,
        quality: r.quality.clone(),
        output_template: r.output_template.clone(),
        secondary_output_template: r.secondary_output_template.clone(),
        redundant_output_template: r.redundant_output_template.clone(),
        created_at: r.created_at.clone(),
        updated_at: r.updated_at.clone(),
        version: r.version,
    }
}

/// Re-derive the cache form (full preset JSON in `data`) from the authoritative
/// `presets` table, write it to our own `presets_cache`, and push it to every
/// healthy peer. Best-effort: a peer that's unreachable just misses this round
/// and will be reconciled the next time presets change.
pub async fn sync_presets_to_nodes(state: &AppState) -> Result<()> {
    let rows = db::presets_full_list(&state.db).await?;
    let now = chrono::Utc::now().to_rfc3339();

    let cache: Vec<PresetCacheDto> = rows
        .iter()
        .map(|r| PresetCacheDto {
            id: r.id.clone(),
            name: r.name.clone(),
            data: serde_json::to_value(preset_row_to_dto(r)).unwrap_or(Value::Null),
            version: r.version,
            synced_at: now.clone(),
        })
        .collect();

    // Our own cache, so recordings executed locally resolve presets too.
    let local_rows: Vec<db::PresetCacheRow> = cache
        .iter()
        .map(|c| db::PresetCacheRow {
            id: c.id.clone(),
            name: c.name.clone(),
            data: c.data.to_string(),
            version: c.version,
            synced_at: c.synced_at.clone(),
        })
        .collect();
    db::presets_replace(&state.db, &local_rows).await?;

    // Push to peers.
    let payload = PresetSyncRequest { presets: cache };
    let urls: Vec<String> = state
        .peers
        .read()
        .await
        .healthy()
        .iter()
        .map(|n| n.url.clone())
        .collect();

    for url in urls {
        if let Err(e) = state
            .http
            .post(format!("{url}/api/v1/presets/sync"))
            .json(&payload)
            .send()
            .await
        {
            warn!(url = %url, error = %e, "preset sync to peer failed");
        }
    }

    Ok(())
}
