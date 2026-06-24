pub mod proxy;
pub mod registry;
pub mod scheduler;
pub mod sync;

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tracing::{info, warn};

use crate::api::types::{NodeStatus, WsEvent};
use crate::state::AppState;
use crate::ws;
use registry::NodeEntry;

const SERVICE_TYPE: &str = "_capture-room._tcp.local.";

// ── mDNS registration (every instance advertises itself) ─────────────────────

/// Register this instance on the local network so aggregators can find it.
/// The returned daemon must be kept alive for the registration to persist.
pub fn register_mdns_service(node_id: &str, node_name: &str, port: u16) -> ServiceDaemon {
    let daemon = ServiceDaemon::new().expect("mDNS daemon");
    let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "capture-room".to_string());
    let mdns_host = format!("{}.local.", hostname.trim_end_matches('.'));

    // Instance name must be unique on the network; suffix with a short id slice.
    let instance = format!("{} ({})", node_name, &node_id[..node_id.len().min(8)]);

    let service = ServiceInfo::new(SERVICE_TYPE, &instance, &mdns_host, (), port, None)
        .expect("mDNS ServiceInfo")
        .enable_addr_auto();

    daemon.register(service).expect("mDNS register");
    info!(instance = %instance, port = port, "registered mDNS service");
    daemon
}

// ── mDNS browser (aggregator only) ───────────────────────────────────────────

pub fn start_mdns_browser(state: Arc<AppState>) {
    let daemon = ServiceDaemon::new().expect("mDNS daemon");
    let receiver = daemon.browse(SERVICE_TYPE).expect("mDNS browse");
    let handle = tokio::runtime::Handle::current();

    std::thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            if let ServiceEvent::ServiceResolved(info) = event {
                let port = info.get_port();
                let ip = info
                    .get_addresses()
                    .iter()
                    .find(|a| a.is_ipv4())
                    .map(|a| a.to_string());

                if let Some(ip) = ip {
                    let url = format!("http://{}:{}", ip, port);
                    let state = Arc::clone(&state);
                    handle.spawn(async move {
                        on_node_discovered(state, url).await;
                    });
                }
            }
        }
        drop(daemon);
    });
}

pub async fn on_node_discovered(state: Arc<AppState>, url: String) {
    let status = match state
        .http
        .get(format!("{}/api/v1/status", url))
        .timeout(Duration::from_secs(3))
        .send()
        .await
    {
        Ok(resp) => match resp.json::<NodeStatus>().await {
            Ok(s) => s,
            Err(e) => {
                warn!(url = %url, error = %e, "failed to identify mDNS service");
                return;
            }
        },
        Err(e) => {
            warn!(url = %url, error = %e, "mDNS service unreachable");
            return;
        }
    };

    // Never treat ourselves as a peer.
    if status.id == state.node_id {
        return;
    }

    let entry = NodeEntry {
        id: status.id.clone(),
        name: status.name,
        url: url.clone(),
        version: status.version,
        healthy: true,
        last_seen: Instant::now(),
        uptime_secs: status.uptime_secs,
        fail_count: 0,
    };

    let is_new = state.peers.write().await.upsert(entry);
    if is_new {
        info!(id = %status.id, url = %url, "peer added to registry");
        ws::send(&state.ws_tx, &WsEvent::NodeOnline { node_id: status.id.clone() });
        // One relay task per node; it re-reads the URL from the registry on
        // every reconnect, so an IP change is picked up automatically, and it
        // exits once the node is pruned.
        spawn_node_ws_relay(Arc::clone(&state), status.id);
    } else {
        info!(id = %status.id, url = %url, "peer URL refreshed");
    }
}

// ── WS relay: peer events → local broadcast ──────────────────────────────────
//
// Peers already emit composite source IDs (`{their_node_id}:{source}`), so the
// relay forwards frames verbatim — no rewriting needed.
//
// The relay owns no URL of its own: it looks the node's current URL up in the
// registry on every connection attempt. That way an IP change is picked up on
// the next reconnect, and once the health poller prunes the node the lookup
// returns `None` and the task exits.

pub fn spawn_node_ws_relay(state: Arc<AppState>, node_id: String) {
    use tokio_tungstenite::tungstenite::Message;

    tokio::spawn(async move {
        loop {
            let url = match state.peers.read().await.url_of(&node_id) {
                Some(u) => u,
                None => {
                    info!(node_id = %node_id, "peer pruned, stopping WS relay");
                    break;
                }
            };
            let ws_url = url.replacen("http://", "ws://", 1) + "/ws";

            match tokio_tungstenite::connect_async(&ws_url).await {
                Ok((stream, _)) => {
                    info!(node_id = %node_id, "WS relay connected");
                    let (_, mut read) = stream.split();

                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                let _ = state.ws_tx.send(text.to_string());
                            }
                            Ok(Message::Close(_)) => break,
                            Err(e) => {
                                warn!(node_id = %node_id, error = %e, "WS relay error");
                                break;
                            }
                            _ => {}
                        }
                    }
                    info!(node_id = %node_id, "WS relay disconnected, retrying in 5s");
                }
                Err(e) => {
                    warn!(node_id = %node_id, error = %e, "WS relay connect failed, retrying in 5s");
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

// ── Health poller (aggregator only) ──────────────────────────────────────────

/// After this many consecutive failed checks (~15s at a 5s interval) a peer is
/// dropped from the registry. mDNS will re-add it if it comes back.
const PRUNE_AFTER_FAILURES: u32 = 3;

pub fn start_health_poller(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;

            let entries: Vec<(String, String)> = {
                let peers = state.peers.read().await;
                peers.all().iter().map(|n| (n.id.clone(), n.url.clone())).collect()
            };

            for (id, url) in entries {
                let result = state
                    .http
                    .get(format!("{}/api/v1/status", url))
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await;

                let mut peers = state.peers.write().await;
                match result {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(status) = resp.json::<NodeStatus>().await {
                            peers.record_success(&id, status.uptime_secs, &status.version);
                        }
                    }
                    _ => {
                        let failures = peers.record_failure(&id);
                        if failures >= PRUNE_AFTER_FAILURES {
                            peers.remove(&id);
                            info!(id = %id, "peer pruned after {failures} failed checks");
                            drop(peers);
                            ws::send(&state.ws_tx, &WsEvent::NodeOffline { node_id: id.clone() });
                        } else {
                            warn!(id = %id, failures, "peer health check failed");
                        }
                    }
                }
            }
        }
    });
}
