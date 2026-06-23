use std::collections::HashMap;
use std::time::Instant;

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
}

#[derive(Default)]
pub struct NodeRegistry {
    pub entries: HashMap<String, NodeEntry>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or update. Returns `true` if the node was newly added.
    /// An update refreshes the URL (e.g. after an IP change) and clears
    /// the failure state.
    pub fn upsert(&mut self, entry: NodeEntry) -> bool {
        let is_new = !self.entries.contains_key(&entry.id);
        self.entries.insert(entry.id.clone(), entry);
        is_new
    }

    pub fn remove(&mut self, id: &str) {
        self.entries.remove(id);
    }

    pub fn all(&self) -> Vec<&NodeEntry> {
        self.entries.values().collect()
    }

    pub fn healthy(&self) -> Vec<&NodeEntry> {
        self.entries.values().filter(|n| n.healthy).collect()
    }

    /// Current URL for a node, or `None` if it's no longer registered.
    pub fn url_of(&self, id: &str) -> Option<String> {
        self.entries.get(id).map(|n| n.url.clone())
    }

    pub fn record_success(&mut self, id: &str, uptime_secs: u64, version: &str) {
        if let Some(e) = self.entries.get_mut(id) {
            e.healthy = true;
            e.fail_count = 0;
            e.last_seen = Instant::now();
            e.uptime_secs = uptime_secs;
            e.version = version.to_string();
        }
    }

    /// Record a failed health check. Returns the new consecutive failure count.
    pub fn record_failure(&mut self, id: &str) -> u32 {
        if let Some(e) = self.entries.get_mut(id) {
            e.healthy = false;
            e.fail_count += 1;
            e.fail_count
        } else {
            0
        }
    }
}
