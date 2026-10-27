//! This node's log lines, for the Logs view. A [`tracing`] layer keeps the
//! last [`CAPACITY`] lines in memory, numbered, and broadcasts each new one.
//!
//! `GET /api/v1/node/logs` pages back through the kept lines.
//! `/api/v1/node/ws/logs` streams new ones as JSON [`LogLine`] text messages
//! once the client sends a [`LogSubscription`] (each replaces the last).
//! `/ws/logs` does the same for the nodes the subscription names while this
//! node is a controller, through one socket to each of those peers.
//!
//! Lines from this module are never kept: streaming them would log itself.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::Response;
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::api::types::{LogLevel, LogLine, LogPage, LogSubscription};
use crate::controller::peer_ws::{Peers, Subscription};
use crate::state::AppState;

/// Lines kept in memory; older ones are only in the log file.
pub const CAPACITY: usize = 10_000;
/// The most lines one page request returns.
const MAX_PAGE: usize = 5_000;
/// New lines waiting for a slow socket; more are dropped (and counted).
const LIVE_BUFFER: usize = 1024;
/// Peers' lines waiting for a slow client; more are dropped.
const PEER_BUFFER: usize = 1024;
const OWN_TARGET: &str = module_path!();

struct Store {
    lines: Mutex<VecDeque<LogLine>>,
    next_seq: AtomicU64,
    live: broadcast::Sender<LogLine>,
}

impl Store {
    fn new() -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(CAPACITY)),
            next_seq: AtomicU64::new(1),
            live: broadcast::channel(LIVE_BUFFER).0,
        }
    }

    fn push(&self, mut line: LogLine) {
        line.seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        {
            let mut lines = self.lines.lock().unwrap();
            if lines.len() == CAPACITY {
                lines.pop_front();
            }
            lines.push_back(line.clone());
        }
        // No receivers is fine.
        let _ = self.live.send(line);
    }

    /// Up to `limit` lines at `level` or more severe, older than `before`,
    /// oldest first.
    fn page(&self, before: Option<u64>, limit: usize, level: LogLevel) -> Vec<LogLine> {
        let lines = self.lines.lock().unwrap();
        let mut out: Vec<LogLine> = lines
            .iter()
            .rev()
            .filter(|l| before.is_none_or(|b| l.seq < b) && l.level <= level)
            .take(limit)
            .cloned()
            .collect();
        out.reverse();
        out
    }
}

fn store() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(Store::new)
}

/// The layer that keeps lines; add it next to the formatting layer.
pub fn layer() -> LogLayer {
    LogLayer
}

pub struct LogLayer;

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if meta.target().starts_with(OWN_TARGET) {
            return;
        }
        let mut visitor = Fields::default();
        event.record(&mut visitor);
        store().push(LogLine {
            seq: 0,
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level: LogLevel::from(*meta.level()),
            target: meta.target().to_string(),
            message: visitor.message,
            fields: visitor.fields,
            node_id: None,
        });
    }
}

/// The `message` field, and every other field as `key=value`.
#[derive(Default)]
struct Fields {
    message: String,
    fields: String,
}

impl Fields {
    fn add(&mut self, field: &Field, value: std::fmt::Arguments) {
        if field.name() == "message" {
            let _ = self.message.write_fmt(value);
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            let _ = write!(self.fields, "{}={}", field.name(), value);
        }
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.add(field, format_args!("{value}"));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.add(field, format_args!("{value:?}"));
    }
}

impl From<tracing::Level> for LogLevel {
    fn from(level: tracing::Level) -> Self {
        match level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::TRACE => Self::Trace,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    /// Only lines numbered below this.
    before: Option<u64>,
    limit: Option<usize>,
    /// This level and more severe; all by default.
    level: Option<LogLevel>,
}

/// `GET /api/v1/node/logs`.
pub async fn get_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LogQuery>,
) -> Json<LogPage> {
    let mut lines = store().page(
        q.before,
        q.limit.unwrap_or(1000).min(MAX_PAGE),
        q.level.unwrap_or(LogLevel::Trace),
    );
    for l in &mut lines {
        l.node_id = Some(state.node_id.clone());
    }
    Json(LogPage {
        lines,
        log_file: crate::service::log_file().map(|p| p.display().to_string()),
    })
}

/// `/api/v1/node/ws/logs`: this node's lines only.
pub async fn local_ws(State(state): State<Arc<AppState>>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| serve(socket, state, false))
}

/// `/ws/logs`: this node's lines and, while a controller, its peers'.
pub async fn relay_ws(State(state): State<Arc<AppState>>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| serve(socket, state, true))
}

/// Drive one client's socket until it closes or the access token changes.
/// Nothing is sent before the first subscription.
async fn serve(socket: WebSocket, state: Arc<AppState>, relay: bool) {
    let (mut sink, mut stream) = socket.split();
    let mut token_changed = state.access_token.subscribe();
    let mut live = store().live.subscribe();

    // `None` until subscribed, or while the subscription leaves this node out.
    let mut local_level: Option<LogLevel> = None;
    let (peer_tx, mut peer_rx) = mpsc::channel::<String>(PEER_BUFFER);
    let mut peers = Peers::new(Arc::clone(&state), peer_tx);

    loop {
        tokio::select! {
            _ = token_changed.changed() => break,
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let Ok(sub) = serde_json::from_str::<LogSubscription>(&text) else {
                        continue;
                    };
                    // A node's own socket serves only it, whatever it's asked.
                    let wants_self = !relay || sub.nodes.iter().any(|n| *n == state.node_id);
                    local_level = wants_self.then_some(sub.level);
                    if relay {
                        let peer_sub = || LogSubscription { nodes: vec![], level: sub.level };
                        peers.set(
                            sub.nodes
                                .iter()
                                .filter(|n| **n != state.node_id)
                                .map(|n| (n.clone(), peer_sub()))
                                .collect(),
                        );
                    }
                }
                Some(Ok(Message::Ping(data))) => {
                    let _ = sink.send(Message::Pong(data)).await;
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            },
            line = live.recv() => {
                let line = match line {
                    Ok(line) => line,
                    Err(broadcast::error::RecvError::Lagged(n)) => skipped(n),
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let Some(level) = local_level else { continue };
                if line.level > level {
                    continue;
                }
                let line = LogLine { node_id: Some(state.node_id.clone()), ..line };
                let Ok(text) = serde_json::to_string(&line) else { continue };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            Some(text) = peer_rx.recv() => {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = sink.close().await;
}

/// Stands in for lines a slow socket missed.
fn skipped(n: u64) -> LogLine {
    LogLine {
        seq: 0,
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        level: LogLevel::Warn,
        target: OWN_TARGET.to_string(),
        message: format!("{n} log lines skipped: the viewer fell behind"),
        fields: String::new(),
        node_id: None,
    }
}

impl Subscription for LogSubscription {
    const PATH: &'static str = "/api/v1/node/ws/logs";
    // They'd be streamed back to the viewer.
    const LOG_ERRORS: bool = false;
    // Lines arrive stamped with the peer's ID, so they're passed on as they are.
    type Item = String;

    fn item(msg: tungstenite::Message) -> Option<String> {
        match msg {
            tungstenite::Message::Text(text) => Some(text.to_string()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(level: LogLevel, message: &str) -> LogLine {
        LogLine {
            seq: 0,
            at: String::new(),
            level,
            target: "t".into(),
            message: message.into(),
            fields: String::new(),
            node_id: None,
        }
    }

    #[test]
    fn keeps_the_newest_lines_up_to_capacity() {
        let s = Store::new();
        for i in 0..CAPACITY + 5 {
            s.push(line(LogLevel::Info, &i.to_string()));
        }
        let all = s.page(None, CAPACITY * 2, LogLevel::Trace);
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].message, "5");
        assert_eq!(all.last().unwrap().seq, (CAPACITY + 5) as u64);
    }

    #[test]
    fn pages_back_oldest_first_by_level() {
        let s = Store::new();
        for (i, level) in [
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Warn,
            LogLevel::Error,
            LogLevel::Info,
        ]
        .into_iter()
        .enumerate()
        {
            s.push(line(level, &i.to_string()));
        }
        let page = s.page(None, 2, LogLevel::Info);
        assert_eq!(
            page.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(),
            ["3", "4"]
        );
        let older = s.page(Some(page[0].seq), 10, LogLevel::Info);
        assert_eq!(
            older.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(),
            ["0", "2"]
        );
        assert!(s
            .page(None, 10, LogLevel::Warn)
            .iter()
            .all(|l| l.level <= LogLevel::Warn));
    }
}
