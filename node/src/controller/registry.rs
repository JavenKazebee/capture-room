use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::api::types::{ClockStatusDto, FoundNodeDto, NodeDto, NodeStatus};
use crate::tls::Pins;

/// An approved node: stored in the `nodes` table, its key pinned, sent the
/// access token.
#[derive(Clone)]
pub struct NodeEntry {
    pub id: String,
    /// The name it was stored under, shown until it answers.
    pub name: String,
    pub url: String,
    /// SHA-256 of its TLS key. `None` only for a node approved before TLS,
    /// until the key it presents next is pinned.
    pub fingerprint: Option<String>,
    pub healthy: bool,
    /// Consecutive failed health checks. Reset to 0 on success.
    pub fail_count: u32,
    /// The last check was refused: it has a different access token.
    pub access_denied: bool,
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
            fingerprint: n.fingerprint.clone(),
            access_denied: n.access_denied,
            status: n
                .status
                .clone()
                .unwrap_or_else(|| NodeStatus::unanswered(n.id.clone(), n.name.clone())),
        }
    }
}

/// A node that announced itself over mDNS and waits to be approved. It's
/// asked only who it is, and never sent the token.
#[derive(Clone)]
pub struct FoundNode {
    pub dto: FoundNodeDto,
    pub fail_count: u32,
}

pub struct NodeRegistry {
    pub entries: HashMap<String, NodeEntry>,
    pub found: HashMap<String, FoundNode>,
    /// Keys of found nodes the operator dismissed (or removed), not offered
    /// again until the controller restarts.
    ignored: HashSet<String>,
    /// The keys of `entries`: what the peer TLS client trusts.
    pins: Arc<Pins>,
}

impl NodeRegistry {
    pub fn new(pins: Arc<Pins>) -> Self {
        Self {
            entries: HashMap::new(),
            found: HashMap::new(),
            ignored: HashSet::new(),
            pins,
        }
    }

    /// Insert or update an approved node, pinning its key. Returns `true` if
    /// it's new, in which case the caller starts a relay for `entry.relay`.
    /// An update keeps the running relay.
    pub fn upsert(&mut self, mut entry: NodeEntry) -> bool {
        self.found.remove(&entry.id);
        if let Some(fp) = &entry.fingerprint {
            self.pins.insert(fp);
        }
        match self.entries.get(&entry.id) {
            Some(existing) => {
                if let Some(old) = existing
                    .fingerprint
                    .as_ref()
                    .filter(|old| entry.fingerprint.as_ref() != Some(*old))
                {
                    self.pins.remove(old);
                }
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

    /// Remove an approved node and stop trusting its key. It isn't offered
    /// again as found until the controller restarts.
    pub fn remove(&mut self, id: &str) -> Option<NodeEntry> {
        let entry = self.entries.remove(id)?;
        entry.relay.cancel();
        if let Some(fp) = &entry.fingerprint {
            self.pins.remove(fp);
            self.ignored.insert(fp.clone());
        }
        Some(entry)
    }

    pub fn all(&self) -> Vec<&NodeEntry> {
        self.entries.values().collect()
    }

    pub fn get(&self, id: &str) -> Option<&NodeEntry> {
        self.entries.get(id)
    }

    /// Current URL for a node, or `None` if it's no longer registered.
    pub fn url_of(&self, id: &str) -> Option<String> {
        self.entries.get(id).map(|n| n.url.clone())
    }

    /// Note a node waiting for approval. Returns whether the list changed
    /// in a way the UI shows.
    pub fn set_found(&mut self, dto: FoundNodeDto) -> bool {
        if self.ignored.contains(&dto.fingerprint) || self.entries.contains_key(&dto.id) {
            return false;
        }
        let changed = self.found.get(&dto.id).is_none_or(|f| f.dto != dto);
        self.found.insert(
            dto.id.clone(),
            FoundNode {
                dto,
                fail_count: 0,
            },
        );
        changed
    }

    /// Whether the operator dismissed or removed the node with this key.
    pub fn is_ignored(&self, fingerprint: &str) -> bool {
        self.ignored.contains(fingerprint)
    }

    /// Dismiss a found node until the controller restarts.
    pub fn ignore(&mut self, id: &str) -> bool {
        match self.found.remove(id) {
            Some(f) => {
                self.ignored.insert(f.dto.fingerprint);
                true
            }
            None => false,
        }
    }

    /// A found node didn't answer. Returns its consecutive failure count.
    pub fn found_failure(&mut self, id: &str) -> u32 {
        self.found.get_mut(id).map_or(0, |f| {
            f.fail_count += 1;
            f.fail_count
        })
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
    /// access token. Returns the new consecutive failure count.
    pub fn record_failure(&mut self, id: &str, denied: bool) -> u32 {
        match self.entries.get_mut(id) {
            Some(e) => {
                e.healthy = false;
                e.fail_count += 1;
                e.access_denied = denied;
                e.fail_count
            }
            None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(fingerprint: Option<&str>) -> NodeEntry {
        NodeEntry {
            id: "node".into(),
            name: "Node".into(),
            url: "https://a".into(),
            fingerprint: fingerprint.map(String::from),
            healthy: false,
            fail_count: 0,
            access_denied: false,
            status: None,
            relay: CancellationToken::new(),
        }
    }

    fn found(id: &str, fingerprint: &str) -> FoundNodeDto {
        FoundNodeDto {
            id: id.into(),
            name: "Found".into(),
            url: "https://b".into(),
            version: "0.1.0".into(),
            fingerprint: fingerprint.into(),
        }
    }

    #[test]
    fn approved_keys_are_pinned_until_removed() {
        let pins = Arc::new(Pins::default());
        let mut r = NodeRegistry::new(Arc::clone(&pins));
        assert!(r.upsert(entry(Some("k1"))));
        assert!(pins.contains("k1"));
        // A key pinned on upgrade replaces none; a changed one replaces the old.
        assert!(!r.upsert(entry(Some("k2"))));
        assert!(!pins.contains("k1") && pins.contains("k2"));
        r.remove("node");
        assert!(!pins.contains("k2"));
    }

    #[test]
    fn found_nodes_wait_unless_approved_or_ignored() {
        let mut r = NodeRegistry::new(Arc::default());
        assert!(r.set_found(found("n1", "k1")));
        assert!(!r.set_found(found("n1", "k1")), "unchanged");
        assert!(r.ignore("n1"));
        assert!(!r.set_found(found("n1", "k1")), "ignored by key");
        assert!(r.found.is_empty());

        assert!(r.set_found(found("node", "k2")));
        r.upsert(entry(Some("k2")));
        assert!(r.found.is_empty(), "approving takes it off the list");
        assert!(!r.set_found(found("node", "k2")), "already approved");

        r.remove("node");
        assert!(!r.set_found(found("node", "k2")), "removed: not offered again");
    }
}
