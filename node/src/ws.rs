//! The event WebSocket. Events are JSON with a `type` ([`WsEvent`]) and the
//! `node_id` they describe. `/api/v1/node/ws` sends this node's own; `/ws`
//! adds those relayed from peers while it's a controller.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use tokio::sync::broadcast;
use tracing::{debug, warn};

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

/// How often a client is pinged. One that has sent nothing, not even the
/// pong, by the next ping is gone: a closed laptop lid or dropped Wi-Fi
/// leaves the TCP connection half open for many minutes otherwise.
const PING_INTERVAL: Duration = Duration::from_secs(15);
/// A send that hasn't gone out in this long means the client stopped
/// reading: its socket buffer is full, and waiting longer blocks the ping.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// Drive a single WebSocket connection: forward broadcast events to the client
/// and keep the connection alive until it closes, stops answering pings, or
/// the access token changes.
pub async fn handle(
    socket: WebSocket,
    rx: broadcast::Receiver<String>,
    token_changed: tokio::sync::watch::Receiver<()>,
) {
    let (sender, receiver) = socket.split();
    drive(sender, receiver, rx, token_changed, PING_INTERVAL).await;
}

/// Whether `msg` went out in time.
async fn send<S: Sink<Message> + Unpin>(sender: &mut S, msg: Message) -> bool {
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, sender.send(msg)).await,
        Ok(Ok(()))
    )
}

async fn drive<S, R, E>(
    mut sender: S,
    mut receiver: R,
    mut rx: broadcast::Receiver<String>,
    mut token_changed: tokio::sync::watch::Receiver<()>,
    ping_every: Duration,
) where
    S: Sink<Message> + Unpin,
    R: Stream<Item = Result<Message, E>> + Unpin,
{
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + ping_every, ping_every);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heard = true;

    loop {
        tokio::select! {
            _ = token_changed.changed() => break,
            _ = ping.tick() => {
                if !heard {
                    debug!("ws client missed a pong, dropping it");
                    break;
                }
                heard = false;
                if !send(&mut sender, Message::Ping(Default::default())).await {
                    break;
                }
            }
            // Broadcast → client
            result = rx.recv() => {
                match result {
                    Ok(msg) => {
                        if !send(&mut sender, Message::Text(msg.into())).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(dropped = n, "ws client lagged, events dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // Client → server
            msg = receiver.next() => {
                heard = true;
                match msg {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        if !send(&mut sender, Message::Pong(data)).await {
                            break;
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use tokio::sync::mpsc;
    use tokio_util::sync::PollSender;

    const EVERY: Duration = Duration::from_millis(50);

    /// A client: what the server sent it, and a way to send the server frames.
    struct Client {
        got: mpsc::Receiver<Message>,
        say: mpsc::Sender<Result<Message, Infallible>>,
        done: tokio::task::JoinHandle<()>,
        _events: broadcast::Sender<String>,
        _token: tokio::sync::watch::Sender<()>,
    }

    fn connect() -> Client {
        let (to_client, got) = mpsc::channel(16);
        let (say, from_client) = mpsc::channel(16);
        let (events, rx) = channel();
        let (token, token_changed) = tokio::sync::watch::channel(());
        let receiver = futures_util::stream::unfold(from_client, |mut r| async move {
            r.recv().await.map(|m| (m, r))
        });
        let done = tokio::spawn(drive(
            PollSender::new(to_client),
            Box::pin(receiver),
            rx,
            token_changed,
            EVERY,
        ));
        Client {
            got,
            say,
            done,
            _events: events,
            _token: token,
        }
    }

    #[tokio::test]
    async fn a_client_that_answers_pings_stays() {
        let mut client = connect();
        for _ in 0..4 {
            let msg = client.got.recv().await.unwrap();
            let Message::Ping(data) = msg else {
                panic!("expected a ping, got {msg:?}");
            };
            client.say.send(Ok(Message::Pong(data))).await.unwrap();
        }
        assert!(!client.done.is_finished());
    }

    #[tokio::test]
    async fn a_client_that_misses_a_pong_is_dropped() {
        let mut client = connect();
        assert!(matches!(client.got.recv().await, Some(Message::Ping(_))));
        tokio::time::timeout(EVERY * 4, &mut client.done)
            .await
            .expect("the silent client was dropped")
            .unwrap();
    }
}
