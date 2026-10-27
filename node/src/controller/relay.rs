//! WS relay: a peer's `/api/v1/node/ws` (its own events only, so a peer
//! controller never echoes its peers) → this controller's merged `/ws`.
//! Frames are forwarded verbatim, unless the peer runs another version. The URL is looked up on every reconnect so
//! IP changes are picked up; it stops when the entry's token is cancelled.

use std::time::Duration;

use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::Ctx;

pub fn spawn(ctx: Ctx, node_id: String, cancel: CancellationToken) {
    tokio::spawn(async move {
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = run(ctx, node_id) => {}
        }
    });
}

async fn run(ctx: Ctx, node_id: String) {
    // Whether connecting failed last time: a node that's down warns once,
    // not every 5 s, and says when it's back.
    let mut failing = false;
    loop {
        let url = match ctx.registry.read().await.url_of(&node_id) {
            Some(u) => u,
            None => {
                info!(node_id = %node_id, "node removed, stopping WS relay");
                return;
            }
        };
        match super::peer_ws::connect(&ctx.state, &url, "/api/v1/node/ws").await {
            Ok(stream) => {
                if failing {
                    info!(node_id = %node_id, "WS relay connected again");
                    failing = false;
                } else {
                    info!(node_id = %node_id, "WS relay connected");
                }
                let (_, mut read) = stream.split();
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            // Another version's events may not mean what the UI
                            // takes them to. Checked per event: the relay
                            // reconnects to an updated node before the health
                            // check sees its version.
                            if ctx.registry.read().await.other_version(&node_id).is_none() {
                                let _ = ctx.state.ws_tx.send(text.to_string());
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Err(e) => {
                            warn!(node_id = %node_id, error = %e, "WS relay error");
                            break;
                        }
                        _ => {}
                    }
                }
                info!(node_id = %node_id, "WS relay disconnected, retrying in 5s");
            }
            Err(e) if failing => {
                debug!(node_id = %node_id, error = %e, "WS relay connect failed again");
            }
            Err(e) => {
                warn!(node_id = %node_id, error = %e, "WS relay connect failed, retrying every 5s");
                failing = true;
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
