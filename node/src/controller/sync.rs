//! Preset (and later schedule) sync from an aggregator down to its nodes.

use anyhow::Result;
use serde_json::Value;
use tracing::warn;

use crate::api::types::{PresetCacheDto, PresetDto, PresetOutputDto, PresetSyncRequest};
use crate::db::{self, PresetOutputRow, PresetRow};
use crate::state::AppState;

pub fn preset_to_dto(row: &PresetRow, outputs: &[PresetOutputRow]) -> PresetDto {
    PresetDto {
        id: row.id.clone(),
        name: row.name.clone(),
        outputs: outputs.iter().map(output_row_to_dto).collect(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        version: row.version,
    }
}

pub fn output_row_to_dto(o: &PresetOutputRow) -> PresetOutputDto {
    PresetOutputDto {
        id: o.id.clone(),
        preset_id: o.preset_id.clone(),
        name: o.name.clone(),
        codec: o.codec.clone(),
        container: o.container.clone(),
        resolution: o.resolution.clone(),
        framerate: o.framerate.clone(),
        bitrate_kbps: o.bitrate_kbps,
        quality: o.quality.clone(),
        path_template: o.path_template.clone(),
        sort_order: o.sort_order,
    }
}

/// Re-derive the cache form (full preset JSON in `data`) from the authoritative
/// `presets` + `preset_outputs` tables, write it to our own `presets_cache`,
/// and push it to every healthy peer. Best-effort: a peer that's unreachable
/// just misses this round and will be reconciled the next time presets change.
pub async fn sync_presets_to_nodes(state: &AppState) -> Result<()> {
    let rows = db::presets_list(&state.db).await?;
    let all_outputs = db::preset_outputs_list_all(&state.db).await?;
    let now = chrono::Utc::now().to_rfc3339();

    let cache: Vec<PresetCacheDto> = rows
        .iter()
        .map(|r| {
            let outputs: Vec<&PresetOutputRow> = all_outputs
                .iter()
                .filter(|o| o.preset_id == r.id)
                .collect();
            let outputs_owned: Vec<PresetOutputRow> = outputs.into_iter().cloned().collect();
            PresetCacheDto {
                id: r.id.clone(),
                name: r.name.clone(),
                data: serde_json::to_value(preset_to_dto(r, &outputs_owned))
                    .unwrap_or(Value::Null),
                version: r.version,
                synced_at: now.clone(),
            }
        })
        .collect();

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
