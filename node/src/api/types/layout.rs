//! The Record page's layout: the order sources show in and the tags they
//! carry. The controller keeps it, naming each source by node and id, so one
//! layout spans all its nodes and every operator sees the same grid.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

/// A source on one of the controller's nodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SourceRef {
    pub node_id: String,
    pub source_id: String,
}

/// One source's tags, by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SourceTagsDto {
    pub node_id: String,
    pub source_id: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct RecordLayoutDto {
    /// Sources placed by hand, first to last. A source that isn't here goes
    /// after them.
    pub order: Vec<SourceRef>,
    /// Only sources with at least one tag.
    pub tags: Vec<SourceTagsDto>,
}

/// Replaces the whole order.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct RecordOrderRequest {
    pub order: Vec<SourceRef>,
}

/// Adds and removes tags on several sources at once. A tag that differs from
/// one already in use only by case takes that one's spelling.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SourceTagsRequest {
    pub sources: Vec<SourceRef>,
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}
