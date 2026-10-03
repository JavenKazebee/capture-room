//! WS relay: a peer's local event stream → this controller's merged `/ws`.
//!
//! Peers stamp every event with their own `node_id`, so frames are forwarded
//! verbatim. The relay subscribes to `/api/v1/node/ws` (the peer's *own*
//! events only), so a peer that is itself a controller never echoes its peers.
//!
//! The relay owns no URL: it looks the node up in the registry on every
//! connection attempt, so an IP change is picked up on the next reconnect.
//! It stops as soon as its entry's token is cancelled — when the node is
//! removed or the controller is disabled.

use std::time::Duration;

use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
    loop {
        let url = match ctx.registry.read().await.url_of(&node_id) {
            Some(u) => u,
            None => {
                info!(node_id = %node_id, "node removed, stopping WS relay");
                return;
            }
        };
        // http → ws, https → wss.
        let ws_url = url.replacen("http", "ws", 1) + "/api/v1/node/ws";

        match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((stream, _)) => {
                info!(node_id = %node_id, "WS relay connected");
                let (_, mut read) = stream.split();
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            let _ = ctx.state.ws_tx.send(text.to_string());
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
            Err(e) => {
                warn!(node_id = %node_id, error = %e, "WS relay connect failed, retrying in 5s");
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
