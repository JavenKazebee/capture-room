//! Controller: an optional layer on top of a node that discovers other nodes,
//! forwards commands to them, and merges their event streams.
//!
//! Promotion is live: [`Controller::enable`] / [`Controller::disable`] start and
//! stop the background tasks without touching local capture.

pub mod api;
pub mod discovery;
pub mod forward;
pub mod registry;
pub mod relay;

use std::sync::Arc;

use anyhow::Result;
use mdns_sd::ServiceDaemon;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::api::types::ClockMode;
use crate::db;
use crate::state::AppState;
use registry::{NodeEntry, NodeRegistry};

pub const CONFIG_KEY: &str = "controller_enabled";
/// The [`ClockMode`] this controller gives its nodes, as JSON.
pub const CLOCK_MODE_KEY: &str = "clock_mode";

pub struct Controller {
    pub registry: Arc<RwLock<NodeRegistry>>,
    /// Cancels the health poller, WS relays and discovery handlers.
    cancel: CancellationToken,
    mdns_browser: Option<ServiceDaemon>,
    /// Serves this instance's clock to its nodes. `None` if the port
    /// couldn't be bound.
    clock_server: Option<gstreamer_net::NetTimeProvider>,
}

/// Handle shared by the controller's background tasks.
#[derive(Clone)]
pub struct Ctx {
    pub state: Arc<AppState>,
    pub registry: Arc<RwLock<NodeRegistry>>,
    pub cancel: CancellationToken,
}

impl Controller {
    /// Promote this instance. No-op if already a controller.
    pub async fn enable(state: &Arc<AppState>) -> Result<()> {
        let mut slot = state.controller.write().await;
        if slot.is_some() {
            return Ok(());
        }

        let ctx = Ctx {
            state: Arc::clone(state),
            registry: Arc::new(RwLock::new(NodeRegistry::default())),
            cancel: CancellationToken::new(),
        };

        // Manually-registered nodes come back immediately (marked unhealthy
        // until the first health check succeeds).
        for row in db::nodes_list(&state.db).await? {
            let entry = NodeEntry {
                id: row.id.clone(),
                name: row.name,
                url: row.url,
                version: String::new(),
                healthy: false,
                uptime_secs: 0,
                fail_count: 0,
                manual: true,
                encoders: Vec::new(),
                source_types: Vec::new(),
                clock: None,
                relay: ctx.cancel.child_token(),
            };
            let relay = entry.relay.clone();
            if ctx.registry.write().await.upsert(entry) {
                relay::spawn(ctx.clone(), row.id, relay);
            }
        }

        // Serve the clock before claiming anyone. It's the local clock: a
        // controller follows no one (in PTP mode nobody uses it).
        let clock_server = crate::clock::serve(state.clock.local(), state.clock_port)
            .inspect(|_| info!(port = state.clock_port, "serving clock"))
            .inspect_err(|e| warn!(error = %e, "nodes can't follow this controller's clock"))
            .ok();
        state.clock.set_own_mode(Some(clock_mode(state).await?));

        discovery::start_health_poller(ctx.clone());
        let mdns_browser = match discovery::start_mdns_browser(ctx.clone()) {
            Ok(d) => Some(d),
            Err(e) => {
                warn!(error = %e, "mDNS browse failed; only manually added nodes will be found");
                None
            }
        };

        *slot = Some(Controller {
            registry: ctx.registry,
            cancel: ctx.cancel,
            mdns_browser,
            clock_server,
        });
        info!("controller enabled");
        Ok(())
    }

    /// Demote this instance back to a plain node. Local capture is untouched.
    pub async fn disable(state: &AppState) {
        if let Some(c) = state.controller.write().await.take() {
            c.cancel.cancel();
            if let Some(d) = c.mdns_browser {
                let _ = d.shutdown();
            }
            if let Some(server) = c.clock_server {
                server.set_active(false);
            }
            state.clock.set_own_mode(None);
            info!("controller disabled");
        }
    }

    /// Whether nodes can follow this controller's clock.
    pub fn serves_clock(&self) -> bool {
        self.clock_server.is_some()
    }

    pub fn ctx(&self, state: &Arc<AppState>) -> Ctx {
        Ctx {
            state: Arc::clone(state),
            registry: Arc::clone(&self.registry),
            cancel: self.cancel.clone(),
        }
    }
}

/// The clock mode this instance gives its nodes as a controller.
pub async fn clock_mode(state: &AppState) -> Result<ClockMode> {
    Ok(db::config_get(&state.db, CLOCK_MODE_KEY)
        .await?
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}
