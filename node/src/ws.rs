use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tracing::warn;

use crate::api::types::WsEvent;

/// Capacity of the broadcast channel.  Old messages are dropped when the
/// channel is full and no receiver is fast enough.
///
/// On an aggregator this single channel carries the local emitter's output
/// plus every relayed peer event. Peak rate ≈ (total sources × 1/s) +
/// (active recordings × ~11/s). 1024 slots buys ~2s of burst tolerance at a
/// few hundred events/sec; the ring holds one copy of each message regardless
/// of receiver count, so the memory cost is ~1024 × message size (trivial).
const CHANNEL_CAPACITY: usize = 1024;

pub fn channel() -> (broadcast::Sender<String>, broadcast::Receiver<String>) {
    broadcast::channel(CHANNEL_CAPACITY)
}

pub fn send(tx: &broadcast::Sender<String>, event: &WsEvent) {
    match serde_json::to_string(event) {
        Ok(json) => {
            let _ = tx.send(json);
        }
        Err(e) => warn!(error = %e, "failed to serialize WsEvent"),
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
