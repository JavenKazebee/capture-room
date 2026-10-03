//! mDNS advertisement (every instance) and node discovery + health polling
//! (controller only).

use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::future::join_all;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tracing::{info, warn};

use super::registry::NodeEntry;
use super::{relay, Ctx};
use crate::api::types::{NodeStatus, WsEvent};

const SERVICE_TYPE: &str = "_capture-room._tcp.local.";
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);

/// After this many consecutive failed checks (~15s at a 5s interval) an
/// mDNS-discovered node is dropped. mDNS will re-add it if it comes back.
/// Manual nodes are kept and just shown as unhealthy.
const PRUNE_AFTER_FAILURES: u32 = 3;

// ── mDNS registration (every instance advertises itself) ─────────────────────

/// This machine's hostname without a trailing `.local` (macOS often reports
/// `name.local`). `$HOSTNAME` is a shell variable and usually isn't exported.
pub fn local_hostname() -> Option<String> {
    let name = sysinfo::System::host_name()?;
    let name = name.trim_end_matches('.');
    let name = name.strip_suffix(".local").unwrap_or(name);
    (!name.is_empty()).then(|| name.to_string())
}

/// Register this instance on the local network so controllers can find it.
/// The returned daemon must be kept alive for the registration to persist.
/// Fails on machines without working mDNS; the node still runs, but has to be
/// added to a controller by URL.
pub fn register_mdns_service(node_id: &str, node_name: &str, port: u16) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    let hostname = local_hostname().unwrap_or_else(|| "capture-room".to_string());
    let mdns_host = format!("{hostname}.local.");

    // Instance name must be unique on the network; suffix with a short id slice.
    let instance = format!("{} ({})", node_name, &node_id[..node_id.len().min(8)]);

    let service = ServiceInfo::new(SERVICE_TYPE, &instance, &mdns_host, (), port, None)?.enable_addr_auto();

    daemon.register(service)?;
    info!(instance = %instance, port = port, "registered mDNS service");
    Ok(daemon)
}

// ── mDNS browser ─────────────────────────────────────────────────────────────

/// Browse for other nodes. Shutting down the returned daemon ends the browse.
pub fn start_mdns_browser(ctx: Ctx) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    let receiver = daemon.browse(SERVICE_TYPE)?;
    let handle = tokio::runtime::Handle::current();

    std::thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            if ctx.cancel.is_cancelled() {
                break;
            }
            if let ServiceEvent::ServiceResolved(info) = event {
                let ip = info.get_addresses().iter().find(|a| a.is_ipv4()).map(|a| a.to_string());
                if let Some(ip) = ip {
                    let url = format!("http://{}:{}", ip, info.get_port());
                    let ctx = ctx.clone();
                    handle.spawn(async move {
                        // Ok(None) is this node's own announcement: skipped quietly.
                        if let Err(e) = add_node(&ctx, url.clone(), false).await {
                            warn!(url = %url, error = %e, "mDNS service not added");
                        }
                    });
                }
            }
        }
    });

    Ok(daemon)
}

// ── Adding a node ────────────────────────────────────────────────────────────

/// Identify the node at `url` and add it to the registry. Returns its status,
/// or `None` if `url` is this node (every controller hears its own mDNS
/// announcement).
pub async fn add_node(ctx: &Ctx, url: String, manual: bool) -> Result<Option<NodeStatus>> {
    let status = fetch_status(ctx, &url).await?;

    if status.id == ctx.state.node_id {
        return Ok(None);
    }
    if ctx.cancel.is_cancelled() {
        return Err(anyhow!("controller disabled"));
    }

    let entry = NodeEntry {
        id: status.id.clone(),
        name: status.name.clone(),
        url: url.clone(),
        version: status.version.clone(),
        healthy: true,
        uptime_secs: status.uptime_secs,
        fail_count: 0,
        manual,
        relay: ctx.cancel.child_token(),
    };

    let relay = entry.relay.clone();
    let is_new = ctx.registry.write().await.upsert(entry);
    if is_new {
        info!(id = %status.id, url = %url, "node added");
        ctx.state.emit_controller(&WsEvent::NodeOnline { peer_id: status.id.clone() });
        relay::spawn(ctx.clone(), status.id.clone(), relay);
    }
    Ok(Some(status))
}

async fn fetch_status(ctx: &Ctx, url: &str) -> Result<NodeStatus> {
    let resp = ctx
        .state
        .http
        .get(format!("{url}/api/v1/node/status"))
        .timeout(STATUS_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.json::<NodeStatus>().await?)
}

// ── Health poller ────────────────────────────────────────────────────────────

pub fn start_health_poller(ctx: Ctx) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => break,
                _ = interval.tick() => {}
            }

            let entries: Vec<(String, String)> = {
                let reg = ctx.registry.read().await;
                reg.all().iter().map(|n| (n.id.clone(), n.url.clone())).collect()
            };

            // Check concurrently so one unreachable node doesn't delay the rest.
            let results = join_all(entries.into_iter().map(|(id, url)| {
                let ctx = &ctx;
                async move {
                    let result = fetch_status(ctx, &url).await;
                    (id, result)
                }
            }))
            .await;

            let mut reg = ctx.registry.write().await;
            for (id, result) in results {
                match result {
                    Ok(status) if status.id == id => {
                        reg.record_success(&id, &status.name, status.uptime_secs, &status.version);
                    }
                    _ => {
                        let (failures, manual) = reg.record_failure(&id);
                        if !manual && failures >= PRUNE_AFTER_FAILURES {
                            reg.remove(&id);
                            info!(id = %id, "node pruned after {failures} failed checks");
                            ctx.state.emit_controller(&WsEvent::NodeOffline { peer_id: id.clone() });
                        } else if failures == 1 {
                            warn!(id = %id, "node health check failed");
                        }
                    }
                }
            }
        }
    });
}
