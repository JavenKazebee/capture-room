//! The event WebSocket. Events are JSON with a `type` ([`WsEvent`]) and the
//! `node_id` they describe. `/api/v1/node/ws` sends this node's own; `/ws`
//! adds those relayed from peers while it's a controller.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tracing::warn;

use crate::api::types::{TransportState, WsEvent};
use crate::state::AppState;

/// Slow receivers lose the oldest messages. On a controller this also carries
/// every relayed peer event: ~2s of burst at a few hundred events/s.
const CHANNEL_CAPACITY: usize = 1024;

pub fn channel() -> (broadcast::Sender<String>, broadcast::Receiver<String>) {
    broadcast::channel(CHANNEL_CAPACITY)
}

/// Serialize `event`, stamped with the `node_id` of the node it describes.
/// Source and session ids inside events are always local to that node.
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
/// and keep the connection alive until it closes or the access token changes.
pub async fn handle(
    socket: WebSocket,
    mut rx: broadcast::Receiver<String>,
    mut token_changed: tokio::sync::watch::Receiver<()>,
) {
    let (mut sender, mut receiver) = socket.split();

    loop {
        tokio::select! {
            _ = token_changed.changed() => break,
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

/// Emit this node's periodic events: audio levels every tick (~10 fps),
/// timecode and recording stats at 1 Hz. Thumbnails have their own
/// sockets (see [`crate::thumbnails`]).
pub fn spawn_emitter(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        let mut tick: u32 = 0;
        loop {
            interval.tick().await;
            tick = tick.wrapping_add(1);

            let mgr = state.source_manager.read().await;

            for (source_id, channels) in mgr.all_audio_levels() {
                state.emit(&WsEvent::AudioLevels {
                    source_id,
                    channels,
                });
            }

            if tick.is_multiple_of(10) {
                for session in mgr.active_sessions() {
                    state.emit(&WsEvent::RecordingStats {
                        session_id: session.id,
                        source_id: session.source_id,
                        dropped_frames: session.dropped_frames,
                        files: session.files,
                    });
                }
                for source in mgr.sources() {
                    state.emit(&WsEvent::FeedStatus {
                        source_id: source.id().to_string(),
                        timecode: source.timecode(),
                        error: mgr.monitor_error(source.id()),
                        link: mgr.link(source.as_ref()),
                    });
                }
            }

            // Channels: every tick while playing (the position moves), else
            // once a second.
            for source in mgr.sources() {
                let Some(playout) = source.playout() else {
                    continue;
                };
                let status = playout.status();
                // Display outputs that hide while idle follow this.
                mgr.set_on_air(source.id(), status.state != TransportState::Idle);
                if status.state == TransportState::Playing || tick.is_multiple_of(10) {
                    state.emit(&WsEvent::ChannelState {
                        source_id: source.id().to_string(),
                        status: Box::new(crate::api::playout::status_dto(
                            status,
                            mgr.output_status(source.id()),
                        )),
                    });
                }
            }
        }
    });
}
