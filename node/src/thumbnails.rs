//! Thumbnail sockets. A client says which sources it shows and how often (a
//! [`ThumbnailSubscription`] text message, each replacing the last), and gets
//! each new JPEG as a binary message:
//!
//! ```text
//! key length (u16, big-endian) | key "{node_id}/{source_id}" | JPEG
//! ```
//!
//! `/api/v1/node/ws/thumbnails` serves this node's sources. `/ws/thumbnails`
//! serves its peers' too while it's a controller, through one socket to each
//! peer with a watched source, open only while it has one. Peers' frames are
//! passed on as they come; a browser too slow for them misses some.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite;
use tracing::warn;

use crate::api::types::{ThumbnailSource, ThumbnailSubscription};
use crate::controller::peer_ws::{Peers, Subscription};
use crate::state::AppState;

/// The most frames per second a client gets of a source, and a monitor makes.
pub const MAX_FPS: u32 = 30;
/// Peers' frames waiting for a slow client; more are dropped.
const PEER_BUFFER: usize = 8;

/// Drive one client's socket until it closes or the access token changes.
/// With `relay`, sources on peers are fetched from them.
pub async fn serve(socket: WebSocket, state: Arc<AppState>, relay: bool) {
    let (mut sink, mut stream) = socket.split();
    let mut token_changed = state.access_token.subscribe();
    let mut changed = crate::pipeline::thumbnails_changed();

    // This node's watched sources, with the last frame each was sent.
    let mut local: HashMap<String, Option<u64>> = HashMap::new();
    // Between local sends; zero when the client asks for every frame, since
    // monitors make them no faster than `MAX_FPS` anyway.
    let mut period = Duration::ZERO;
    // A local frame may be new, to send once `next` comes.
    let mut pending = false;
    let mut next = Instant::now();

    let (peer_tx, mut peer_rx) = mpsc::channel::<Bytes>(PEER_BUFFER);
    let mut peers = Peers::new(Arc::clone(&state), peer_tx);

    loop {
        tokio::select! {
            _ = token_changed.changed() => break,
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let sub = match serde_json::from_str::<ThumbnailSubscription>(&text) {
                        Ok(sub) => sub,
                        Err(e) => {
                            warn!(error = %e, "bad thumbnail subscription");
                            continue;
                        }
                    };
                    let fps = sub.fps.map(|f| f.clamp(1, MAX_FPS));
                    period = fps.map_or(Duration::ZERO, |f| Duration::from_secs(1) / f);
                    let mut by_node: HashMap<String, Vec<String>> = HashMap::new();
                    for s in sub.sources {
                        by_node.entry(s.node_id).or_default().push(s.source_id);
                    }
                    // A source still watched isn't sent again until it changes.
                    let ids = by_node.remove(&state.node_id).unwrap_or_default();
                    local.retain(|id, _| ids.contains(id));
                    for id in ids {
                        local.entry(id).or_default();
                    }
                    pending = !local.is_empty();
                    if relay {
                        peers.set(
                            by_node
                                .into_iter()
                                .map(|(node_id, ids)| {
                                    let sources = ids
                                        .into_iter()
                                        .map(|source_id| ThumbnailSource {
                                            node_id: node_id.clone(),
                                            source_id,
                                        })
                                        .collect();
                                    (node_id, ThumbnailSubscription { sources, fps })
                                })
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
            _ = changed.changed(), if !pending && !local.is_empty() => pending = true,
            _ = tokio::time::sleep_until(next), if pending => {
                pending = false;
                // A steady cadence, so a late frame doesn't push back the
                // rest; after a pause, at least half a period to the next.
                next = (next + period).max(Instant::now() + period / 2);
                let frames: Vec<(String, crate::pipeline::Thumbnail)> = {
                    let mgr = state.source_manager.read().await;
                    local
                        .iter()
                        .filter_map(|(id, sent)| {
                            let t = mgr.thumbnail(id)?;
                            (*sent != Some(t.seq)).then(|| (id.clone(), t))
                        })
                        .collect()
                };
                for (id, t) in frames {
                    local.insert(id.clone(), Some(t.seq));
                    if sink.send(Message::Binary(frame(&state.node_id, &id, &t.jpeg))).await.is_err() {
                        // Gone: nothing to close.
                        return;
                    }
                }
            }
            Some(frame) = peer_rx.recv() => {
                if sink.send(Message::Binary(frame)).await.is_err() {
                    break;
                }
            }
        }
    }
    // Sends the reply to a client's close.
    let _ = sink.close().await;
}

/// One binary message: see the module docs.
fn frame(node_id: &str, source_id: &str, jpeg: &[u8]) -> Bytes {
    let key = format!("{node_id}/{source_id}");
    let mut out = Vec::with_capacity(2 + key.len() + jpeg.len());
    out.extend_from_slice(&(key.len() as u16).to_be_bytes());
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(jpeg);
    out.into()
}

impl Subscription for ThumbnailSubscription {
    const PATH: &'static str = "/api/v1/node/ws/thumbnails";
    type Item = Bytes;

    fn item(msg: tungstenite::Message) -> Option<Bytes> {
        match msg {
            tungstenite::Message::Binary(frame) => Some(frame),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_has_key_then_jpeg() {
        let f = frame("n1", "cam/a", b"\xff\xd8jpeg");
        assert_eq!(&f[..2], &8u16.to_be_bytes());
        assert_eq!(&f[2..10], b"n1/cam/a");
        assert_eq!(&f[10..], b"\xff\xd8jpeg");
    }
}
