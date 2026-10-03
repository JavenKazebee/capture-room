use std::collections::HashMap;
use std::time::Instant;

use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct NodeEntry {
    pub id: String,
    pub name: String,
    pub url: String,
    pub version: String,
    pub healthy: bool,
    pub last_seen: Instant,
    pub uptime_secs: u64,
    /// Consecutive failed health checks. Reset to 0 on success.
    pub fail_count: u32,
    /// Added by URL and persisted in the `nodes` table. Manual nodes are never
    /// pruned for being unreachable; mDNS ones are (they re-announce).
    pub manual: bool,
    /// Stops this node's WS relay. Cancelled when the entry is removed, so a
    /// node that is removed and re-added never ends up with two relays.
    pub relay: CancellationToken,
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

    pub fn record_success(&mut self, id: &str, name: &str, uptime_secs: u64, version: &str) {
        if let Some(e) = self.entries.get_mut(id) {
            e.healthy = true;
            e.fail_count = 0;
            e.last_seen = Instant::now();
            e.name = name.to_string();
            e.uptime_secs = uptime_secs;
            e.version = version.to_string();
        }
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
