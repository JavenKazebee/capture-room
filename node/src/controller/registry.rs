use std::collections::HashMap;

use tokio_util::sync::CancellationToken;

use crate::api::types::{ClockStatusDto, NodeDto, NodeStatus};

#[derive(Clone)]
pub struct NodeEntry {
    pub id: String,
    /// The name it was stored under, shown until it answers.
    pub name: String,
    pub url: String,
    pub healthy: bool,
    /// Consecutive failed health checks. Reset to 0 on success.
    pub fail_count: u32,
    /// The last check was refused: it has a different access token.
    pub access_denied: bool,
    /// Added by URL and persisted in the `nodes` table. Manual nodes are never
    /// pruned for being unreachable; mDNS ones are (they re-announce).
    pub manual: bool,
    /// What it reported at its last successful health check; `None` until
    /// it has answered once.
    pub status: Option<NodeStatus>,
    /// Stops this node's WS relay. Cancelled when the entry is removed, so a
    /// node that is removed and re-added never ends up with two relays.
    pub relay: CancellationToken,
}

impl From<&NodeEntry> for NodeDto {
    fn from(n: &NodeEntry) -> Self {
        NodeDto {
            url: n.url.clone(),
            healthy: n.healthy,
            is_self: false,
            manual: n.manual,
            access_denied: n.access_denied,
            status: n
                .status
                .clone()
                .unwrap_or_else(|| NodeStatus::unanswered(n.id.clone(), n.name.clone())),
        }
    }
}

#[derive(Default)]
pub struct NodeRegistry {
    pub entries: HashMap<String, NodeEntry>,
}

impl NodeRegistry {
    /// Insert or update. Returns `true` if the node was newly added, in which
    /// case the caller starts a relay for `entry.relay`. An update refreshes
    /// the URL (e.g. after an IP change) and clears the failure state, keeps
    /// the running relay, and never downgrades a manual entry to mDNS.
    pub fn upsert(&mut self, mut entry: NodeEntry) -> bool {
        match self.entries.get(&entry.id) {
            Some(existing) => {
                entry.manual |= existing.manual;
                entry.relay = existing.relay.clone();
                self.entries.insert(entry.id.clone(), entry);
                false
            }
            None => {
                self.entries.insert(entry.id.clone(), entry);
                true
            }
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<NodeEntry> {
        let entry = self.entries.remove(id)?;
        entry.relay.cancel();
        Some(entry)
    }

    pub fn all(&self) -> Vec<&NodeEntry> {
        self.entries.values().collect()
    }

    /// Current URL for a node, or `None` if it's no longer registered.
    pub fn url_of(&self, id: &str) -> Option<String> {
        self.entries.get(id).map(|n| n.url.clone())
    }

    /// Record a successful health check. Returns whether the node was
    /// unhealthy until now, and whether its clock status changed.
    pub fn record_success(&mut self, status: &NodeStatus) -> (bool, bool) {
        let Some(e) = self.entries.get_mut(&status.id) else {
            return (false, false);
        };
        let recovered = !e.healthy;
        // The delay and UTC offset wobble with every exchange; only state
        // changes count.
        let state_of = |s: Option<&NodeStatus>| {
            s.and_then(|s| s.clock.clone()).map(|c| ClockStatusDto {
                delay_us: None,
                utc_offset_us: None,
                ..c
            })
        };
        let clock_changed = state_of(e.status.as_ref()) != state_of(Some(status));
        e.status = Some(status.clone());
        e.healthy = true;
        e.fail_count = 0;
        e.access_denied = false;
        (recovered, clock_changed)
    }

    /// Record a failed health check, `denied` if it refused this node's
    /// access token. Returns the new consecutive failure count and whether
    /// the entry is manual.
    pub fn record_failure(&mut self, id: &str, denied: bool) -> (u32, bool) {
        match self.entries.get_mut(id) {
            Some(e) => {
                e.healthy = false;
                e.fail_count += 1;
                e.access_denied = denied;
                (e.fail_count, e.manual)
            }
            None => (0, false),
        }
    }
}
