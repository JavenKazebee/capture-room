use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tracing::warn;

use crate::api::types::WsEvent;

/// Capacity of the broadcast channel.  Old messages are dropped when the
/// channel is full and no receiver is fast enough.
///
/// On a controller this single channel carries the local emitter's output
/// plus every relayed peer event. Peak rate ≈ (total sources × 1/s) +
/// (active recordings × ~11/s). 1024 slots buys ~2s of burst tolerance at a
/// few hundred events/sec; the ring holds one copy of each message regardless
/// of receiver count, so the memory cost is ~1024 × message size (trivial).
const CHANNEL_CAPACITY: usize = 1024;

pub fn channel() -> (broadcast::Sender<String>, broadcast::Receiver<String>) {
    broadcast::channel(CHANNEL_CAPACITY)
}

/// Broadcast `event`, stamped with the `node_id` of the node it describes.
/// Source and session ids inside events are always local to that node.
pub fn send(tx: &broadcast::Sender<String>, node_id: &str, event: &WsEvent) {
    if let Some(json) = encode(node_id, event) {
        let _ = tx.send(json);
    }
}

pub fn encode(node_id: &str, event: &WsEvent) -> Option<String> {
    match serde_json::to_value(event) {
        Ok(mut value) => {
            value["node_id"] = node_id.into();
            Some(value.to_string())
        }
        Err(e) => {
            warn!(error = %e, "failed to serialize WsEvent");
            None
        }
    }
}

/// Drive a single WebSocket connection: forward broadcast events to the client
/// and keep the connection alive until it closes.
pub async fn handle(socket: WebSocket, mut rx: broadcast::Receiver<String>) {
    let (mut sender, mut receiver) = socket.split();

    loop {
        tokio::select! {
            // Broadcast → client
            result = rx.recv() => {
                match result {
                    Ok(msg) => {
                        if sender.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(dropped = n, "ws client lagged, events dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // Client → server (close detection / ping-pong)
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        let _ = sender.send(Message::Pong(data)).await;
                    }
                    _ => {}
                }
            }
        }
    }
}
