//! Schedules and their runs.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

/// How a schedule repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum ScheduleKind {
    /// On `date`.
    Once,
    /// On each of `weekdays`.
    Weekly,
    /// Once a month: on `month_day`, or on the `month_week`th of the one
    /// day in `weekdays`.
    Monthly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ScheduleTarget {
    pub node_id: String,
    pub source_id: String,
}

/// What a schedule does at its start, and undoes at its end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum ScheduleAction {
    /// Record these sources with a preset, all starting on the same frame
    /// of the shared clock.
    Record {
        targets: Vec<ScheduleTarget>,
        preset_id: String,
    },
    /// Play a channel's playlist, from `item_id` or where it's cued.
    Playout {
        node_id: String,
        channel_id: String,
        #[serde(default)]
        item_id: Option<String>,
        /// Stop the channel when the schedule ends.
        #[serde(default)]
        stop_at_end: bool,
    },
}

/// A schedule as created or changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ScheduleInput {
    pub name: String,
    pub enabled: bool,
    pub kind: ScheduleKind,
    /// `YYYY-MM-DD`, for `once`. For `monthly`, optional: the first run is
    /// on or after it.
    #[serde(default)]
    pub date: Option<String>,
    /// For `weekly`: bit 0 is Monday … bit 6 Sunday. For `monthly` by
    /// weekday, the one day.
    #[serde(default)]
    pub weekdays: u8,
    /// For `monthly` by date: 1–31. Months without that day are skipped.
    #[serde(default)]
    pub month_day: Option<u8>,
    /// For `monthly` by weekday: 1–4, or -1 for the last in the month.
    #[serde(default)]
    pub month_week: Option<i8>,
    /// `HH:MM` in the controller's local time.
    pub start_time: String,
    /// `None`: nothing is stopped.
    #[serde(default)]
    pub duration_s: Option<u32>,
    pub actions: Vec<ScheduleAction>,
}

/// A stored schedule, with when it next runs and its latest run.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ScheduleDto {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub kind: ScheduleKind,
    pub date: Option<String>,
    pub weekdays: u8,
    pub month_day: Option<u8>,
    pub month_week: Option<i8>,
    pub start_time: String,
    pub duration_s: Option<u32>,
    #[sqlx(json)]
    pub actions: Vec<ScheduleAction>,
    pub created_at: String,
    pub updated_at: String,
    /// RFC 3339; `None` when disabled or nothing is left to run.
    #[sqlx(skip)]
    #[serde(default)]
    pub next_start: Option<String>,
    #[sqlx(skip)]
    #[serde(default)]
    pub next_end: Option<String>,
    /// The latest run, running or finished.
    #[sqlx(skip)]
    #[serde(default)]
    pub last_run: Option<ScheduleRunDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum ScheduleRunStatus {
    /// Started; stops at `planned_end`.
    Running,
    /// Everything started (and stopped) as planned.
    Done,
    /// Some targets failed.
    Partial,
    /// Every target failed.
    Failed,
    /// Its whole window passed while the controller was off.
    Skipped,
}

/// One occurrence of a schedule.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ScheduleRunDto {
    pub id: String,
    pub schedule_id: String,
    pub planned_start: String,
    pub planned_end: Option<String>,
    pub started_at: Option<String>,
    pub stopped_at: Option<String>,
    pub status: ScheduleRunStatus,
    #[sqlx(json)]
    pub targets: Vec<ScheduleRunTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub enum ScheduleTargetKind {
    Recording,
    Channel,
}

/// What a run did with one source or channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ScheduleRunTarget {
    pub kind: ScheduleTargetKind,
    pub node_id: String,
    /// The source or channel.
    pub id: String,
    /// The recording it started.
    pub session_id: Option<String>,
    /// Why it didn't start.
    pub error: Option<String>,
    /// Whether the run stops it at its end.
    pub stop_at_end: bool,
}
