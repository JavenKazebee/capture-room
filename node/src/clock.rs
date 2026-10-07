//! The node clock: what every pipeline on this node runs on.
//!
//! On its own a node runs on its local system clock. A controller serves that
//! clock on the network (a `NetTimeProvider`) and claims its nodes with every
//! health check; a claimed node follows it (a `NetClientClock`), so running
//! time means the same thing on every machine. In PTP mode the controller and
//! its nodes all follow a PTP domain's grandmaster instead.
//!
//! A node keeps the first controller that claims it until that controller
//! goes quiet ([`MASTER_TIMEOUT`]). A controller follows no one: it's a
//! master itself. A new clock is used once it has synced; until then the old
//! one stays. Pipelines can't change clocks while running, so the source
//! manager rebuilds idle monitors on the new clock and leaves busy ones
//! (recording, playing out) until they're idle.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use gstreamer::{self as gst, prelude::*};
use gstreamer_net as gst_net;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::api::types::{ClockClaim, ClockMode, ClockSourceDto, ClockStatusDto, ClockTimeDto};

/// A claiming controller is dropped after this long without a claim, so
/// another can take the node.
pub const MASTER_TIMEOUT: Duration = Duration::from_secs(20);
/// No update from the master for this long: the clock is free-running.
const LOST_AFTER: Duration = Duration::from_secs(10);
/// How often a switch checks whether the new clock has synced.
const SYNC_POLL: gst::ClockTime = gst::ClockTime::SECOND;

pub struct NodeClock {
    node_id: String,
    local: gst::Clock,
    inner: Mutex<Inner>,
    /// Bumped whenever the clock in use changes.
    changed: watch::Sender<u64>,
}

struct Inner {
    /// While this node is a controller: its own mode. It takes no claims.
    own_mode: Option<ClockMode>,
    master: Option<Master>,
    active: Follow,
    /// A clock being switched to, until it syncs.
    pending: Option<Follow>,
    error: Option<String>,
    serial: u64,
}

struct Master {
    id: String,
    last_claim: Instant,
}

#[derive(Clone)]
struct Follow {
    source: ClockSourceDto,
    clock: gst::Clock,
    stats: Arc<Mutex<Stats>>,
    serial: u64,
    since: Instant,
}

#[derive(Default)]
struct Stats {
    delay: Option<gst::ClockTime>,
    last_update: Option<Instant>,
}

impl NodeClock {
    pub fn new(node_id: &str) -> Self {
        let local = gst::SystemClock::obtain();
        let active = Follow {
            source: ClockSourceDto::Local,
            clock: local.clone(),
            stats: Arc::default(),
            serial: 0,
            since: Instant::now(),
        };
        Self {
            node_id: node_id.to_string(),
            local,
            inner: Mutex::new(Inner {
                own_mode: None,
                master: None,
                active,
                pending: None,
                error: None,
                serial: 0,
            }),
            changed: watch::channel(0).0,
        }
    }

    /// This machine's own clock: what a controller serves.
    pub fn local(&self) -> &gst::Clock {
        &self.local
    }

    /// The clock new pipelines should run on.
    pub fn clock(&self) -> gst::Clock {
        self.inner.lock().unwrap().active.clock.clone()
    }

    /// The clock new pipelines should run on, and its domain.
    pub fn current(&self) -> (gst::Clock, String) {
        let inner = self.inner.lock().unwrap();
        (inner.active.clock.clone(), self.domain_of(&inner))
    }

    /// The time now on the clock in use.
    pub fn now(&self) -> ClockTimeDto {
        let (clock, domain) = self.current();
        ClockTimeDto {
            domain,
            time_us: clock.time().map_or(0, |t| t.useconds()),
        }
    }

    /// Which shared clock `inner.active` is (see `ClockStatusDto::domain`).
    /// A controller's own clock is the one its nodes follow.
    fn domain_of(&self, inner: &Inner) -> String {
        match &inner.active.source {
            ClockSourceDto::Local if inner.own_mode == Some(ClockMode::Controller) => {
                format!("controller:{}", self.node_id)
            }
            ClockSourceDto::Local => format!("local:{}", self.node_id),
            ClockSourceDto::Controller { controller_id, .. } => {
                format!("controller:{controller_id}")
            }
            ClockSourceDto::Ptp { domain } => format!("ptp:{domain}"),
        }
    }

    /// Notified whenever [`Self::clock`] changes.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// This node became a controller (`Some`), changed its mode, or stopped
    /// being one (`None`). A controller follows its own mode; a node that
    /// stops being one keeps its clock until a controller claims it.
    pub fn set_own_mode(self: &Arc<Self>, mode: Option<ClockMode>) {
        let mut inner = self.inner.lock().unwrap();
        inner.own_mode = mode;
        inner.master = None;
        if let Some(mode) = mode {
            let source = match mode {
                ClockMode::Controller => ClockSourceDto::Local,
                ClockMode::Ptp { domain } => ClockSourceDto::Ptp { domain },
            };
            self.follow(&mut inner, source);
        }
        // The domain can change with the clock unchanged (becoming the
        // controller of the clock this node already runs on).
        self.changed.send_modify(|n| *n += 1);
    }

    /// A controller at `address` claims this node. Returns whether it's
    /// (still) this node's master.
    pub fn claim(self: &Arc<Self>, claim: &ClockClaim, address: IpAddr) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.own_mode.is_some() {
            return false;
        }
        let taken = inner.master.as_ref().is_some_and(|m| {
            m.id != claim.controller_id && m.last_claim.elapsed() < MASTER_TIMEOUT
        });
        if taken {
            return false;
        }
        if inner
            .master
            .as_ref()
            .is_none_or(|m| m.id != claim.controller_id)
        {
            info!(controller = %claim.controller_name, "clock claimed");
        }
        inner.master = Some(Master {
            id: claim.controller_id.clone(),
            last_claim: Instant::now(),
        });
        let source = match claim.mode {
            ClockMode::Controller => ClockSourceDto::Controller {
                controller_id: claim.controller_id.clone(),
                controller_name: claim.controller_name.clone(),
                address: address.to_canonical().to_string(),
                port: claim.port,
            },
            ClockMode::Ptp { domain } => ClockSourceDto::Ptp { domain },
        };
        self.follow(&mut inner, source);
        true
    }

    /// Switch to `source`: now for the local clock, otherwise once it syncs.
    fn follow(self: &Arc<Self>, inner: &mut Inner, source: ClockSourceDto) {
        if same_source(&inner.active.source, &source) {
            inner.pending = None;
            inner.error = None;
            return;
        }
        if inner
            .pending
            .as_ref()
            .is_some_and(|p| same_source(&p.source, &source))
        {
            return;
        }
        inner.serial += 1;
        let serial = inner.serial;
        let stats = Arc::<Mutex<Stats>>::default();
        let clock = match self.make_clock(&source, &stats) {
            Ok(clock) => clock,
            Err(e) => {
                let error = format!("{e:#}");
                warn!(?source, error = %error, "can't follow clock");
                inner.pending = None;
                inner.error = Some(error);
                return;
            }
        };
        inner.error = None;
        let follow = Follow {
            source,
            clock,
            stats,
            serial,
            since: Instant::now(),
        };
        if follow.source == ClockSourceDto::Local {
            inner.pending = None;
            self.activate(inner, follow);
            return;
        }
        info!(source = ?follow.source, "waiting for clock to sync");
        inner.pending = Some(follow.clone());
        let this = Arc::clone(self);
        std::thread::spawn(move || this.await_sync(follow));
    }

    fn make_clock(&self, source: &ClockSourceDto, stats: &Arc<Mutex<Stats>>) -> Result<gst::Clock> {
        match source {
            ClockSourceDto::Local => Ok(self.local.clone()),
            ClockSourceDto::Controller { address, port, .. } => {
                let clock = gst_net::NetClientClock::new(
                    Some("controller-clock"),
                    address,
                    i32::from(*port),
                    self.local.time(),
                );
                let bus = gst::Bus::new();
                let stats = Arc::clone(stats);
                bus.set_sync_handler(move |_, msg| {
                    if let Some(s) = msg.structure() {
                        if s.name() == "gst-netclock-statistics" {
                            let mut stats = stats.lock().unwrap();
                            stats.delay = s.get::<gst::ClockTime>("rtt-average").ok();
                            stats.last_update = Some(Instant::now());
                        }
                    }
                    gst::BusSyncReply::Drop
                });
                clock.set_bus(Some(&bus));
                Ok(clock.upcast())
            }
            ClockSourceDto::Ptp { domain } => {
                init_ptp()?;
                ptp_stats(*domain, stats);
                let clock = gst_net::PtpClock::new(Some("ptp-clock"), u32::from(*domain))
                    .map_err(|e| anyhow!("can't make a PTP clock: {e}"))?;
                Ok(clock.upcast())
            }
        }
    }

    /// Wait for `follow` to sync, then use it, unless it has been replaced.
    fn await_sync(&self, follow: Follow) {
        loop {
            let synced = follow.clock.wait_for_sync(SYNC_POLL).is_ok();
            let mut inner = self.inner.lock().unwrap();
            if inner.pending.as_ref().map(|p| p.serial) != Some(follow.serial) {
                return;
            }
            if synced {
                inner.pending = None;
                self.activate(&mut inner, follow);
                return;
            }
        }
    }

    fn activate(&self, inner: &mut Inner, follow: Follow) {
        info!(source = ?follow.source, "clock in use");
        inner.active = follow;
        self.changed.send_modify(|n| *n += 1);
    }

    /// Where the clock stands. `stale_sources` is the source manager's.
    pub fn status(&self, stale_sources: u32) -> ClockStatusDto {
        let inner = self.inner.lock().unwrap();
        let active = &inner.active;
        let stats = active.stats.lock().unwrap();
        let remote = active.source != ClockSourceDto::Local;
        let heard = stats.last_update.unwrap_or(active.since);
        ClockStatusDto {
            source: active.source.clone(),
            domain: self.domain_of(&inner),
            synced: !remote || active.clock.is_synced(),
            pending: inner.pending.as_ref().map(|p| p.source.clone()),
            delay_us: stats.delay.map(|d| d.useconds()),
            lost: remote && heard.elapsed() > LOST_AFTER,
            stale_sources,
            error: inner.error.clone(),
        }
    }
}

/// Whether two sources are the same clock. A controller's name is only a
/// label: renaming it doesn't change its clock.
fn same_source(a: &ClockSourceDto, b: &ClockSourceDto) -> bool {
    match (a, b) {
        (
            ClockSourceDto::Controller {
                controller_id: a_id,
                address: a_addr,
                port: a_port,
                ..
            },
            ClockSourceDto::Controller {
                controller_id: b_id,
                address: b_addr,
                port: b_port,
                ..
            },
        ) => a_id == b_id && a_addr == b_addr && a_port == b_port,
        _ => a == b,
    }
}

// ── Serving (controllers) ─────────────────────────────────────────────────────

/// Serve `clock` on UDP `port` (all interfaces) for nodes to follow.
pub fn serve(clock: &gst::Clock, port: u16) -> Result<gst_net::NetTimeProvider> {
    gst_net::NetTimeProvider::new(clock, None, i32::from(port))
        .map_err(|e| anyhow!("can't serve the clock on UDP port {port}: {e}"))
}

// ── PTP ───────────────────────────────────────────────────────────────────────

/// Start GStreamer's PTP support (once). It runs a helper that needs to bind
/// UDP ports 319 and 320, which on Linux takes capabilities on the helper.
fn init_ptp() -> Result<()> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        if !gst_net::PtpClock::is_supported() {
            return Err("PTP isn't supported on this platform".to_string());
        }
        gst_net::PtpClock::init(None, &[]).map_err(|_| {
            "PTP couldn't start: its helper (gst-ptp-helper) may lack permission to \
             bind UDP ports 319/320"
                .to_string()
        })?;
        // Stats for every domain go to whichever clock follows it. Never
        // removed: it lives as long as PTP support does.
        gst_net::PtpClock::add_statistics_callback(|domain, s| {
            if s.name() == "GstPtpStatisticsTimeUpdated" {
                if let Some(stats) = PTP_STATS
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|m| m.get(&domain))
                {
                    let mut stats = stats.lock().unwrap();
                    stats.delay = s.get::<gst::ClockTime>("mean-path-delay-avg").ok();
                    stats.last_update = Some(Instant::now());
                }
            }
            gst::glib::ControlFlow::Continue
        });
        Ok(())
    })
    .clone()
    .map_err(|e| anyhow!(e))
}

static PTP_STATS: Mutex<Option<HashMap<u8, Arc<Mutex<Stats>>>>> = Mutex::new(None);

/// Send `domain`'s statistics to `stats` from now on.
fn ptp_stats(domain: u8, stats: &Arc<Mutex<Stats>>) {
    PTP_STATS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(domain, Arc::clone(stats));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(id: &str, port: u16) -> ClockClaim {
        ClockClaim {
            controller_id: id.to_string(),
            controller_name: id.to_string(),
            mode: ClockMode::Controller,
            port,
        }
    }

    /// A node follows a served clock once it syncs, and agrees with it.
    #[test]
    fn follows_a_served_clock() {
        gst::init().unwrap();
        let master = gst::SystemClock::obtain();
        let provider = serve(&master, 0).unwrap();
        let port = provider.port() as u16;

        let node = Arc::new(NodeClock::new("node"));
        let mut changed = node.subscribe();
        assert!(node.claim(&claim("a", port), "127.0.0.1".parse().unwrap()));
        assert!(matches!(
            node.status(0).pending,
            Some(ClockSourceDto::Controller { .. })
        ));

        let deadline = Instant::now() + Duration::from_secs(10);
        while !changed.has_changed().unwrap() {
            assert!(Instant::now() < deadline, "clock never synced");
            std::thread::sleep(Duration::from_millis(50));
        }
        changed.mark_unchanged();
        let status = node.status(0);
        assert!(status.synced);
        assert!(status.pending.is_none());
        assert!(matches!(status.source, ClockSourceDto::Controller { port: p, .. } if p == port));

        // Same machine, same clock: well within a millisecond.
        let clock = node.clock();
        let diff =
            clock.time().unwrap().nseconds() as i64 - master.time().unwrap().nseconds() as i64;
        assert!(diff.abs() < 1_000_000, "off by {diff} ns");

        // A second controller can't take the node while the first is claiming.
        assert!(!node.claim(&claim("b", port), "127.0.0.1".parse().unwrap()));
        // The first one's repeat claim changes nothing.
        assert!(node.claim(&claim("a", port), "127.0.0.1".parse().unwrap()));
        assert!(!changed.has_changed().unwrap());
    }

    /// A controller follows its own mode and takes no claims.
    #[test]
    fn controller_takes_no_claims() {
        gst::init().unwrap();
        let node = Arc::new(NodeClock::new("node"));
        node.set_own_mode(Some(ClockMode::Controller));
        assert!(!node.claim(&claim("a", 9), "127.0.0.1".parse().unwrap()));
        assert_eq!(node.status(0).source, ClockSourceDto::Local);
        assert!(node.status(0).pending.is_none());
    }
}
