use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tracing::warn;

use crate::api::types::WsEvent;

/// Capacity of the broadcast channel.  Old messages are dropped when the
/// channel is full and no receiver is fast enough.
const CHANNEL_CAPACITY: usize = 128;

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
