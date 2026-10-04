use std::collections::HashMap;

use tokio_util::sync::CancellationToken;

use crate::api::types::{NodeDto, NodeStatus};

#[derive(Clone)]
pub struct NodeEntry {
    pub id: String,
    pub name: String,
    pub url: String,
    pub version: String,
    pub healthy: bool,
    pub uptime_secs: u64,
    /// Consecutive failed health checks. Reset to 0 on success.
    pub fail_count: u32,
    /// Added by URL and persisted in the `nodes` table. Manual nodes are never
    /// pruned for being unreachable; mDNS ones are (they re-announce).
    pub manual: bool,
    /// Video encoders the node reported; see `NodeStatus::encoders`.
    pub encoders: Vec<String>,
    /// Stops this node's WS relay. Cancelled when the entry is removed, so a
    /// node that is removed and re-added never ends up with two relays.
    pub relay: CancellationToken,
}

impl From<&NodeEntry> for NodeDto {
    fn from(n: &NodeEntry) -> Self {
        NodeDto {
            id: n.id.clone(),
            name: n.name.clone(),
            url: n.url.clone(),
            version: n.version.clone(),
            healthy: n.healthy,
            uptime_secs: n.uptime_secs,
            is_self: false,
            manual: n.manual,
            encoders: n.encoders.clone(),
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

    /// Record a successful health check. Returns `true` if the node was
    /// unhealthy until now.
    pub fn record_success(&mut self, status: &NodeStatus) -> bool {
        let Some(e) = self.entries.get_mut(&status.id) else { return false };
        let recovered = !e.healthy;
        e.healthy = true;
        e.fail_count = 0;
        e.name = status.name.clone();
        e.uptime_secs = status.uptime_secs;
        e.version = status.version.clone();
        e.encoders = status.encoders.clone();
        recovered
    }

    /// Record a failed health check. Returns the new consecutive failure count
    /// and whether the entry is manual.
    pub fn record_failure(&mut self, id: &str) -> (u32, bool) {
        match self.entries.get_mut(id) {
            Some(e) => {
                e.healthy = false;
                e.fail_count += 1;
                (e.fail_count, e.manual)
            }
            None => (0, false),
        }
    }
}
