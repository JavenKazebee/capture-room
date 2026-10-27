//! WebSockets from this controller to its nodes. [`connect`] opens one to a
//! peer's path with this node's token and the pinned-key TLS config.
//!
//! [`Peers`] keeps one subscribed socket to each peer a client watches: it
//! sends the peer the client's [`Subscription`] (again on every reconnect,
//! and whenever it changes) and passes the peer's messages on as they are.
//! The peer's URL is looked up on every reconnect, so a changed address is
//! picked up. Nothing is passed on from a peer running another version: its
//! messages may not mean what the UI takes them to.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, RwLock};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::debug;

use super::registry::NodeRegistry;
use crate::state::AppState;

pub type PeerSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const RETRY: Duration = Duration::from_secs(2);

/// Open a socket to `path` on the node at `url` (`http` → `ws`, `https` → `wss`).
pub async fn connect(state: &AppState, url: &str, path: &str) -> anyhow::Result<PeerSocket> {
    let mut request = (url.replacen("http", "ws", 1) + path).into_client_request()?;
    if let Some(bearer) = state.access_token.bearer() {
        request.headers_mut().insert("authorization", bearer);
    }
    let connector = Some(state.peers.ws_connector());
    let (socket, _) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector).await?;
    Ok(socket)
}

/// What a client sends a peer's socket to say what it wants from it.
pub trait Subscription: Serialize + PartialEq + Send + Sync + 'static {
    /// The peer's socket for this subscription.
    const PATH: &'static str;
    /// Whether connection errors are logged.
    const LOG_ERRORS: bool = true;
    /// What's passed on from the peer.
    type Item: Send + 'static;

    /// The item a message from the peer carries, if any.
    fn item(msg: Message) -> Option<Self::Item>;
}

/// One client's subscribed sockets to peers, by node ID. Each is closed when
/// its peer leaves the subscription or this is dropped.
pub struct Peers<S: Subscription> {
    state: Arc<AppState>,
    out: mpsc::Sender<S::Item>,
    peers: HashMap<String, Peer<S>>,
}

impl<S: Subscription> Peers<S> {
    /// Peers' items go to `out`; a full channel drops them.
    pub fn new(state: Arc<AppState>, out: mpsc::Sender<S::Item>) -> Self {
        Self {
            state,
            out,
            peers: HashMap::new(),
        }
    }

    /// Subscribe to exactly these peers.
    pub fn set(&mut self, subs: HashMap<String, S>) {
        self.peers.retain(|id, _| subs.contains_key(id));
        for (node_id, sub) in subs {
            match self.peers.get(&node_id) {
                Some(peer) => {
                    peer.sub.send_if_modified(|s| {
                        let modified = *s != sub;
                        *s = sub;
                        modified
                    });
                }
                None => {
                    let peer = Peer::spawn(
                        Arc::clone(&self.state),
                        node_id.clone(),
                        sub,
                        self.out.clone(),
                    );
                    self.peers.insert(node_id, peer);
                }
            }
        }
    }
}

struct Peer<S> {
    sub: watch::Sender<S>,
    task: JoinHandle<()>,
}

impl<S: Subscription> Peer<S> {
    fn spawn(state: Arc<AppState>, node_id: String, sub: S, out: mpsc::Sender<S::Item>) -> Self {
        let (tx, rx) = watch::channel(sub);
        let task = tokio::spawn(run(state, node_id, rx, out));
        Self { sub: tx, task }
    }
}

impl<S> Drop for Peer<S> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Connect and pass items on, reconnecting until the peer is removed or this
/// node stops being a controller.
async fn run<S: Subscription>(
    state: Arc<AppState>,
    node_id: String,
    mut sub: watch::Receiver<S>,
    out: mpsc::Sender<S::Item>,
) {
    loop {
        let Some(registry) = state
            .controller
            .read()
            .await
            .as_ref()
            .map(|c| Arc::clone(&c.registry))
        else {
            return;
        };
        let Some(url) = registry.read().await.url_of(&node_id) else {
            return;
        };
        let result = relay(&state, &registry, &node_id, &url, &mut sub, &out).await;
        if let Err(e) = result {
            if S::LOG_ERRORS {
                debug!(node_id = %node_id, path = S::PATH, error = %e, "peer socket");
            }
        }
        tokio::time::sleep(RETRY).await;
    }
}

async fn relay<S: Subscription>(
    state: &AppState,
    registry: &RwLock<NodeRegistry>,
    node_id: &str,
    url: &str,
    sub: &mut watch::Receiver<S>,
    out: &mpsc::Sender<S::Item>,
) -> anyhow::Result<()> {
    let (mut write, mut read) = connect(state, url, S::PATH).await?.split();
    // A new connection needs the subscription, whether or not it changed.
    sub.mark_changed();
    loop {
        tokio::select! {
            changed = sub.changed() => {
                changed?;
                let text = serde_json::to_string(&*sub.borrow_and_update())?;
                write.send(Message::Text(text.into())).await?;
            }
            msg = read.next() => match msg {
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Ok(msg)) => {
                    // Checked per message: a peer reconnects after an update
                    // before the health check sees its new version.
                    if let Some(item) = S::item(msg) {
                        if registry.read().await.other_version(node_id).is_none() {
                            let _ = out.try_send(item);
                        }
                    }
                }
                Some(Err(e)) => return Err(e.into()),
            },
        }
    }
}
