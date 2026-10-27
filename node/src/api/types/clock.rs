//! Clock sync: what a node's pipelines run on and how a controller keeps nodes
//! in step.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

/// How a controller keeps its nodes' clocks in step. Set on the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ClockMode {
    /// Nodes follow the controller's clock over the network.
    #[default]
    Controller,
    /// The controller and its nodes follow a PTP grandmaster on the network.
    Ptp { domain: u8 },
}

/// What a node's pipelines run on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ClockSourceDto {
    /// This machine's own clock (a controller's, or a node no controller
    /// has claimed).
    Local,
    /// A controller's clock, served at `address:port`.
    Controller {
        controller_id: String,
        controller_name: String,
        address: String,
        port: u16,
    },
    /// A PTP domain's grandmaster.
    Ptp { domain: u8 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ClockStatusDto {
    pub source: ClockSourceDto,
    /// Which shared clock this is: nodes with the same domain agree on the
    /// time. `controller:{id}` (a controller and the nodes following it),
    /// `ptp:{domain}`, or `local:{node id}` for a node on its own.
    pub domain: String,
    /// Whether `source` is in sync (always true for `local`).
    pub synced: bool,
    /// A clock being switched to, until it syncs.
    pub pending: Option<ClockSourceDto>,
    /// Network delay to the master: the round trip for a controller's
    /// clock, the mean path delay for PTP. Accuracy is a fraction of it.
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub delay_us: Option<u64>,
    /// No word from the master lately: the clock is running on its last
    /// known rate and drifting slowly.
    pub lost: bool,
    /// Sources still on the previous clock because they were recording or
    /// playing out when it changed. They switch once idle.
    pub stale_sources: u32,
    /// Why the wanted clock couldn't be set up (e.g. PTP not permitted).
    pub error: Option<String>,
    /// UTC minus the clock's time, in µs: the domain's mapping to wall-clock
    /// time (the controller's wall clock, or TAI − 37 s on PTP). The same on
    /// every node in the domain. `None` until a controller has sent it.
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub utc_offset_us: Option<i64>,
}

/// A controller asking a node to follow its clock. Sent with every health
/// check; the node keeps the first controller that claims it until that one
/// goes quiet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ClockClaim {
    pub controller_id: String,
    pub controller_name: String,
    pub mode: ClockMode,
    /// The UDP port the controller serves its clock on.
    pub port: u16,
    /// UTC minus the controller's clock, in µs: maps times on its clock to
    /// wall-clock time, the same way on every node. Absent from controllers
    /// that predate it, and in PTP mode (PTP time maps to UTC by itself).
    #[serde(default)]
    #[cfg_attr(feature = "export-types", ts(type = "number | null"))]
    pub utc_offset_us: Option<i64>,
}

/// A controller's clock settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ControllerClockDto {
    pub mode: ClockMode,
    /// The UDP port the clock is served on.
    pub port: u16,
    /// Whether it's being served (the port could be bound).
    pub serving: bool,
}

/// A time on a clock domain (see [`ClockStatusDto::domain`]), in
/// microseconds: precise enough to pick a frame, and small enough for a
/// JavaScript number even on a PTP clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct ClockTimeDto {
    pub domain: String,
    #[cfg_attr(feature = "export-types", ts(type = "number"))]
    pub time_us: u64,
}
