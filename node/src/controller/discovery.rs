//! mDNS advertisement (every instance) and node discovery + health polling
//! (controller only).
//!
//! A controller trusts a node only once it's approved: it asks the node who
//! it is and pins the key from the TLS handshake. Approving a found node or
//! adding one by address also pairs it, unless it already takes this
//! controller's token: the operator types in the node's pairing code, and the
//! controller sends it with its own token to `/api/v1/auth/pair`, over a
//! client that trusts only the key it identified. An mDNS announcement moves
//! an approved node to a new URL only if it holds the pinned key.

use std::time::Duration;

use anyhow::{anyhow, Result};
use futures_util::future::join_all;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tracing::{debug, info, warn};

use super::registry::NodeEntry;
use super::{relay, Ctx};
use crate::api::types::{
    ClockClaim, ClockMode, FoundNodeDto, IdentityDto, NodeStatus, PairRequest, Unreachable, WsEvent,
};
use crate::auth::AccessDenied;
use crate::db;

const SERVICE_TYPE: &str = "_capture-room._tcp.local.";
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);

/// After this many consecutive failed checks (~15s at a 5s interval) a found
/// node is dropped; mDNS offers it again if it comes back. Approved nodes are
/// kept, shown as unreachable.
const PRUNE_AFTER_FAILURES: u32 = 3;

/// This machine's hostname without a trailing `.local` (macOS often reports
/// `name.local`). `$HOSTNAME` is a shell variable and usually isn't exported.
pub fn local_hostname() -> Option<String> {
    let name = sysinfo::System::host_name()?;
    let name = name.trim_end_matches('.');
    let name = name.strip_suffix(".local").unwrap_or(name);
    (!name.is_empty()).then(|| name.to_string())
}

/// Advertise this instance so controllers can find it. Keep the daemon alive.
/// Fails without working mDNS; the node then has to be added by URL.
pub fn register_mdns_service(node_id: &str, node_name: &str, port: u16) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    let hostname = local_hostname().unwrap_or_else(|| "capture-room".to_string());
    let mdns_host = format!("{hostname}.local.");

    // Instance name must be unique on the network; suffix with a short id slice.
    let instance = format!("{} ({})", node_name, &node_id[..node_id.len().min(8)]);

    let service =
        ServiceInfo::new(SERVICE_TYPE, &instance, &mdns_host, (), port, None)?.enable_addr_auto();

    daemon.register(service)?;
    info!(instance = %instance, port = port, "registered mDNS service");
    Ok(daemon)
}

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
                let ip = info
                    .get_addresses()
                    .iter()
                    .find(|a| a.is_ipv4())
                    .map(|a| a.to_string());
                if let Some(ip) = ip {
                    let url = format!("https://{}:{}", ip, info.get_port());
                    let ctx = ctx.clone();
                    handle.spawn(async move {
                        if let Err(e) = announced(&ctx, &url).await {
                            debug!(url = %url, error = %e, "mDNS announcement not followed");
                        }
                    });
                }
            }
        }
    });

    Ok(daemon)
}

/// A node's API URL. Always HTTPS: nodes never talk plain HTTP to each other.
pub fn https_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    format!("https://{rest}")
}

/// An mDNS announcement. Anything on the LAN can make one, so it only
/// moves an approved node that holds its pinned key. A new node is offered
/// for approval, which takes its pairing code.
async fn announced(ctx: &Ctx, url: &str) -> Result<()> {
    let (identity, fingerprint) = ctx.state.peers.identify(url).await?;
    if identity.id == ctx.state.node_id {
        return Ok(());
    }
    let known = ctx
        .registry
        .read()
        .await
        .get(&identity.id)
        .map(|n| (n.fingerprint.clone(), n.url.clone()));
    match known {
        Some((Some(pinned), _)) if pinned != fingerprint => {
            warn!(
                id = %identity.id,
                url,
                "an announcement claims an approved node but holds another key: ignored"
            );
        }
        Some((Some(_), known_url)) if known_url != url => {
            info!(id = %identity.id, from = %known_url, to = url, "node moved");
            approve(ctx, url, &identity, fingerprint).await?;
        }
        // Up to date; or approved before TLS, pinned at its stored URL only.
        Some(_) => {}
        None if ctx.registry.read().await.is_ignored(&fingerprint) => {}
        None => {
            let found = FoundNodeDto {
                id: identity.id,
                name: identity.name,
                url: url.to_string(),
                version: identity.version,
                fingerprint,
            };
            if ctx.registry.write().await.set_found(found) {
                ctx.state.emit_controller(&WsEvent::NodeUpdated);
            }
        }
    }
    Ok(())
}

/// What answers at an address claims to be a node that's approved with
/// another key: a different machine, or that node with a new database.
#[derive(Debug)]
pub struct KeyMismatch(pub String);

impl std::fmt::Display for KeyMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "that address claims to be {}, which is approved with a different TLS key; \
             remove {} first if it was reinstalled",
            self.0, self.0
        )
    }
}

impl std::error::Error for KeyMismatch {}

/// A node to add doesn't use this controller's token, and no pairing code
/// came with it.
#[derive(Debug)]
pub struct NeedsPairingCode;

impl std::fmt::Display for NeedsPairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("enter the pairing code from the node's menu bar")
    }
}

impl std::error::Error for NeedsPairingCode {}

#[derive(Debug)]
pub struct WrongPairingCode;

impl std::fmt::Display for WrongPairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "that isn't the node's pairing code: check its menu bar (it makes a new one after \
             a few wrong tries)",
        )
    }
}

impl std::error::Error for WrongPairingCode {}

/// Make sure the node at `url`, holding the key `fingerprint`, uses this
/// controller's token: it does already, or `code` (its pairing code) gets it
/// to. Only that key is trusted, so the token goes to no other machine.
async fn pair(ctx: &Ctx, url: &str, fingerprint: &str, code: Option<&str>) -> Result<()> {
    let token = ctx
        .state
        .access_token
        .get()
        .ok_or_else(|| anyhow!("this controller has no access token yet"))?;
    let client = ctx.state.peers.trusting(fingerprint)?;
    let resp = client
        .get(format!("{url}/api/v1/node/status"))
        .bearer_auth(&token)
        .timeout(STATUS_TIMEOUT)
        .send()
        .await?;
    if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
        resp.error_for_status()?;
        return Ok(());
    }
    let code = code
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .ok_or(NeedsPairingCode)?;
    let resp = client
        .post(format!("{url}/api/v1/auth/pair"))
        .json(&PairRequest {
            code: code.to_string(),
            token,
        })
        .timeout(STATUS_TIMEOUT)
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        return Err(WrongPairingCode.into());
    }
    resp.error_for_status()?;
    info!(url, "paired a node");
    Ok(())
}

/// Add the node at `url`, typed in by the operator, pairing it with `code`
/// unless it uses this controller's token already. Returns who it is;
/// `None` if `url` is this node.
pub async fn add_by_url(ctx: &Ctx, url: &str, code: Option<&str>) -> Result<Option<IdentityDto>> {
    let (identity, fingerprint) = ctx.state.peers.identify(url).await?;
    if identity.id == ctx.state.node_id {
        return Ok(None);
    }
    let known = ctx
        .registry
        .read()
        .await
        .get(&identity.id)
        .and_then(|n| Some((n.fingerprint.clone()?, n.name.clone())));
    if let Some((pinned, name)) = known.filter(|(pinned, _)| *pinned != fingerprint) {
        warn!(id = %identity.id, url, %pinned, presented = %fingerprint, "refused a node with another key");
        return Err(KeyMismatch(name).into());
    }
    pair(ctx, url, &fingerprint, code).await?;
    approve(ctx, url, &identity, fingerprint).await?;
    Ok(Some(identity))
}

/// Approve a found node, pairing it with `code` unless it uses this
/// controller's token already. `false` if it isn't (or is no longer) found.
pub async fn approve_found(ctx: &Ctx, id: &str, code: Option<&str>) -> Result<bool> {
    let Some(found) = ctx
        .registry
        .read()
        .await
        .found
        .get(id)
        .map(|f| f.dto.clone())
    else {
        return Ok(false);
    };
    let identity = IdentityDto {
        id: found.id,
        name: found.name,
        version: found.version,
        fingerprint: found.fingerprint.clone(),
    };
    pair(ctx, &found.url, &found.fingerprint, code).await?;
    approve(ctx, &found.url, &identity, found.fingerprint).await?;
    Ok(true)
}

/// Pair an approved node that refuses this controller's token: one paired
/// with another controller, or that made a new token. `false` if there's no
/// such node.
pub async fn pair_approved(ctx: &Ctx, id: &str, code: &str) -> Result<bool> {
    let Some((url, fingerprint)) = ctx
        .registry
        .read()
        .await
        .get(id)
        .map(|n| (n.url.clone(), n.fingerprint.clone()))
    else {
        return Ok(false);
    };
    let fingerprint =
        fingerprint.ok_or_else(|| anyhow!("its key isn't pinned yet: try again shortly"))?;
    pair(ctx, &url, &fingerprint, Some(code)).await?;
    Ok(true)
}

/// Store the node at `url` with its key pinned, and from now on check it,
/// relay its events and send it the token. Approving a known node updates
/// its URL and key.
async fn approve(ctx: &Ctx, url: &str, identity: &IdentityDto, fingerprint: String) -> Result<()> {
    if ctx.cancel.is_cancelled() {
        return Err(anyhow!("controller disabled"));
    }
    db::node_upsert(
        &ctx.state.db,
        &db::NodeRow {
            id: identity.id.clone(),
            name: identity.name.clone(),
            url: url.to_string(),
            fingerprint: Some(fingerprint.clone()),
            added_at: chrono::Utc::now().to_rfc3339(),
        },
    )
    .await?;

    let mut registry = ctx.registry.write().await;
    let entry = match registry.get(&identity.id) {
        Some(known) => NodeEntry {
            url: url.to_string(),
            fingerprint: Some(fingerprint),
            ..known.clone()
        },
        None => NodeEntry {
            id: identity.id.clone(),
            name: identity.name.clone(),
            url: url.to_string(),
            fingerprint: Some(fingerprint),
            healthy: false,
            fail_count: 0,
            unreachable: None,
            status: None,
            relay: ctx.cancel.child_token(),
        },
    };
    let relay = entry.relay.clone();
    if registry.upsert(entry) {
        drop(registry);
        info!(id = %identity.id, url, "node approved");
        relay::spawn(ctx.clone(), identity.id.clone(), relay);
    }
    // It shows as approved now, and online after its first health check.
    ctx.state.emit_controller(&WsEvent::NodeUpdated);
    Ok(())
}

async fn send_claim(ctx: &Ctx, url: &str, claim: &ClockClaim) -> Result<()> {
    ctx.state
        .peer(
            reqwest::Method::POST,
            &format!("{url}/api/v1/node/clock/claim"),
        )
        .json(claim)
        .timeout(STATUS_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn fetch_status(ctx: &Ctx, url: &str) -> Result<NodeStatus> {
    let resp = ctx
        .state
        .peer(reqwest::Method::GET, &format!("{url}/api/v1/node/status"))
        .timeout(STATUS_TIMEOUT)
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AccessDenied.into());
    }
    let resp = resp.error_for_status()?;
    Ok(resp.json::<NodeStatus>().await?)
}

/// Why the check of node `id` at `url` failed with `error`. Unless the node
/// refused the token, it's asked who it is without pinning, which tells a
/// changed key or another node at that address from nothing answering.
async fn diagnose(
    ctx: &Ctx,
    id: &str,
    url: &str,
    pinned: Option<&str>,
    error: anyhow::Error,
) -> Unreachable {
    if error.is::<AccessDenied>() {
        return Unreachable::AccessDenied;
    }
    match ctx.state.peers.identify(url).await {
        Err(_) => Unreachable::Offline,
        Ok((identity, _)) if identity.id != id => Unreachable::OtherNode {
            name: identity.name,
        },
        Ok((_, fingerprint)) if pinned.is_some_and(|p| p != fingerprint) => {
            Unreachable::KeyChanged { fingerprint }
        }
        Ok(_) => Unreachable::Failing {
            error: format!("{error:#}"),
        },
    }
}

/// Pin the key of a node approved before TLS, at the URL it was approved
/// at: trusted once, as it was then.
async fn pin(ctx: &Ctx, id: &str, url: &str) -> Result<()> {
    let (identity, fingerprint) = ctx.state.peers.identify(url).await?;
    if identity.id != id {
        return Err(anyhow!("another node ({}) answers there", identity.name));
    }
    info!(id, url, %fingerprint, "pinned the key of a node approved before TLS");
    approve(ctx, url, &identity, fingerprint).await
}

/// Ask each found node who it is again, dropping those gone for a while.
async fn check_found(ctx: &Ctx, found: Vec<(String, String)>) {
    let results = join_all(found.into_iter().map(|(id, url)| async move {
        let result = ctx.state.peers.identify(&url).await;
        (id, url, result)
    }))
    .await;
    for (id, url, result) in results {
        match result {
            Ok((identity, fingerprint)) if identity.id == id => {
                let found = FoundNodeDto {
                    id: identity.id,
                    name: identity.name,
                    url,
                    version: identity.version,
                    fingerprint,
                };
                if ctx.registry.write().await.set_found(found) {
                    ctx.state.emit_controller(&WsEvent::NodeUpdated);
                }
            }
            _ => {
                let mut reg = ctx.registry.write().await;
                if reg.found_failure(&id) >= PRUNE_AFTER_FAILURES {
                    reg.found.remove(&id);
                    ctx.state.emit_controller(&WsEvent::NodeUpdated);
                }
            }
        }
    }
}

pub fn start_health_poller(ctx: Ctx) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => break,
                _ = interval.tick() => {}
            }

            let (entries, found): (Vec<_>, Vec<_>) = {
                let reg = ctx.registry.read().await;
                (
                    reg.all()
                        .iter()
                        .map(|n| (n.id.clone(), n.url.clone(), n.fingerprint.clone()))
                        .collect(),
                    reg.found
                        .values()
                        .map(|f| (f.dto.id.clone(), f.dto.url.clone()))
                        .collect(),
                )
            };
            check_found(&ctx, found).await;

            // Claimed with every check, so nodes know their master is alive.
            let claim = match super::clock_mode(&ctx.state).await {
                Ok(mode) => Some(ClockClaim {
                    controller_id: ctx.state.node_id.clone(),
                    controller_name: ctx.state.node_name(),
                    // The clock served is this machine's own.
                    utc_offset_us: (mode == ClockMode::Controller)
                        .then(|| crate::clock::local_utc_offset_us(ctx.state.clock.local()))
                        .flatten(),
                    mode,
                    port: ctx.state.clock_port,
                }),
                Err(e) => {
                    warn!(error = %e, "can't read the clock mode; not claiming nodes");
                    None
                }
            };

            // Check concurrently so one unreachable node doesn't delay the rest.
            let results = join_all(entries.into_iter().map(|(id, url, pinned)| {
                let ctx = &ctx;
                let claim = claim.as_ref();
                async move {
                    let result = async {
                        if pinned.is_none() {
                            pin(ctx, &id, &url).await?;
                        }
                        if let Some(claim) = claim {
                            // Nodes that predate clock sync refuse it; that's fine.
                            if let Err(e) = send_claim(ctx, &url, claim).await {
                                debug!(id = %id, error = %e, "clock claim failed");
                            }
                        }
                        let status = fetch_status(ctx, &url).await?;
                        if status.id != id {
                            return Err(anyhow!("answered as {} ({})", status.name, status.id));
                        }
                        Ok(status)
                    }
                    .await;
                    let result = match result {
                        Ok(status) => Ok(status),
                        Err(e) => Err(diagnose(ctx, &id, &url, pinned.as_deref(), e).await),
                    };
                    (id, result)
                }
            }))
            .await;

            let mut reg = ctx.registry.write().await;
            for (id, result) in results {
                match result {
                    Ok(status) => {
                        let (recovered, clock_changed) = reg.record_success(&status);
                        if recovered {
                            info!(id = %id, "node reachable again");
                            ctx.state.emit_controller(&WsEvent::NodeOnline {
                                peer_id: id.clone(),
                            });
                        } else if clock_changed {
                            ctx.state.emit_controller(&WsEvent::NodeUpdated);
                        }
                    }
                    Err(reason) => {
                        let (fails, changed) = reg.record_failure(&id, reason.clone());
                        if changed {
                            warn!(id = %id, reason = ?reason, "node unreachable");
                        }
                        if fails == 1 {
                            ctx.state.emit_controller(&WsEvent::NodeOffline {
                                peer_id: id.clone(),
                            });
                        } else if changed {
                            ctx.state.emit_controller(&WsEvent::NodeUpdated);
                        }
                    }
                }
            }
        }
    });
}
