use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use carplay_protocol::{DspCommand, ServiceMessage};
use futures_util::{SinkExt, StreamExt};

use super::Hub;

pub async fn serve(hub: Hub, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(hub);

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("[ws] listening on ws://{}/ws", addr);

    axum::serve(listener, app).await?;
    Ok(())
}

async fn ws_handler(ws: WebSocketUpgrade, State(hub): State<Hub>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, hub))
}

async fn handle_socket(socket: WebSocket, hub: Hub) {
    if let Err(e) = run_socket(socket, hub).await {
        eprintln!("[ws] client disconnected: {}", e);
    }
}

async fn run_socket(socket: WebSocket, hub: Hub) -> Result<()> {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = hub.broadcast_tx.subscribe();

    // Send current state on connect
    let initial = serde_json::to_string(&ServiceMessage::State(hub.state.read().await.clone()))?;
    sender.send(Message::Text(initial.into())).await?;

    // Read incoming commands in the background
    let hub_cmd = hub.clone();
    tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(text) = msg {
                if let Ok(cmd) = serde_json::from_str::<DspCommand>(&text) {
                    hub_cmd.dispatch(cmd).await;
                }
            }
        }
    });

    // Forward broadcast messages to this client
    loop {
        match rx.recv().await {
            Ok(msg) => {
                let text = serde_json::to_string(&msg)?;
                sender.send(Message::Text(text.into())).await?;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }

    Ok(())
}
