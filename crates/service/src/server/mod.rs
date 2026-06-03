pub mod ble;
pub mod ws;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use carplay_protocol::{DspCommand, DspState, ServiceMessage};
use crossbeam_channel::Sender;
use tokio::sync::{broadcast, RwLock};

use crate::stats::AudioStats;

pub const BROADCAST_CAPACITY: usize = 64;

// Shared state passed to every server module (Unix socket, WebSocket, BLE).
// Clone is cheap: Arc under the hood for state and stats.
#[derive(Clone)]
pub struct Hub {
    // Forwards commands to the audio playback thread
    pub cmd_tx: Sender<DspCommand>,
    // Broadcasts ServiceMessage to every connected client
    pub broadcast_tx: broadcast::Sender<ServiceMessage>,
    // Canonical DSP state — source of truth for new client connections
    pub state: Arc<RwLock<DspState>>,
    pub stats: Arc<AudioStats>,
}

impl Hub {
    pub fn new(cmd_tx: Sender<DspCommand>, initial_state: DspState, stats: Arc<AudioStats>) -> Self {
        let (broadcast_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            cmd_tx,
            broadcast_tx,
            state: Arc::new(RwLock::new(initial_state)),
            stats,
        }
    }

    // Process an incoming command: update canonical state, forward to DSP thread, broadcast.
    pub async fn dispatch(&self, cmd: DspCommand) {
        {
            let mut state = self.state.write().await;
            apply_to_state(&mut state, &cmd);
        }
        let _ = self.cmd_tx.try_send(cmd);
        let state = self.state.read().await.clone();
        let _ = self.broadcast_tx.send(ServiceMessage::State(state));
    }

    // Start the stats pusher background task and the Unix socket listener.
    pub async fn run(self, socket_path: &str) -> Result<()> {
        let hub_stats = self.clone();
        tokio::spawn(async move { stats_pusher(hub_stats).await });
        unix_socket_server(self, socket_path).await
    }
}

// Reflect a DSP command onto the canonical state (no ALSA side effects).
fn apply_to_state(state: &mut DspState, cmd: &DspCommand) {
    match cmd {
        DspCommand::SetVolume { value } => state.volume = value.clamp(0.0, 1.0),
        DspCommand::SetEqBand { band, gain_db } => {
            if *band < state.eq_gains.len() {
                state.eq_gains[*band] = gain_db.clamp(-12.0, 12.0);
            }
        }
        DspCommand::SetLoudness { value } => state.loudness = *value,
        DspCommand::SetLimiter { value } => state.limiter = *value,
        DspCommand::SetSource { value } => state.source = *value,
        DspCommand::SetMute { value } => state.muted = *value,
    }
}

// Push an audio stats snapshot to all connected clients at 20 Hz.
async fn stats_pusher(hub: Hub) {
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    let mut last_reset = std::time::Instant::now();
    let mut frames_per_sec: u64 = 0;

    loop {
        interval.tick().await;

        if last_reset.elapsed() >= Duration::from_secs(1) {
            frames_per_sec = hub.stats.reset_per_sec();
            last_reset = std::time::Instant::now();
        }

        let snap = hub.stats.snapshot(frames_per_sec);
        let _ = hub.broadcast_tx.send(ServiceMessage::Stats(snap));
    }
}

async fn unix_socket_server(hub: Hub, path: &str) -> Result<()> {
    let _ = tokio::fs::remove_file(path).await;
    let listener = tokio::net::UnixListener::bind(path)?;
    eprintln!("[unix] listening on {}", path);

    loop {
        let (stream, _) = listener.accept().await?;
        let hub = hub.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_unix_client(hub, stream).await {
                eprintln!("[unix] client disconnected: {}", e);
            }
        });
    }
}

async fn handle_unix_client(hub: Hub, stream: tokio::net::UnixStream) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = stream.into_split();
    let mut rx = hub.broadcast_tx.subscribe();

    // Send current state immediately on connect
    let initial = serde_json::to_string(&ServiceMessage::State(hub.state.read().await.clone()))? + "\n";
    writer.write_all(initial.as_bytes()).await?;

    // Read incoming commands in a background task
    let hub_cmd = hub.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(cmd) = serde_json::from_str::<DspCommand>(&line) {
                hub_cmd.dispatch(cmd).await;
            }
        }
    });

    // Forward broadcast messages to this client
    loop {
        match rx.recv().await {
            Ok(msg) => {
                let line = serde_json::to_string(&msg)? + "\n";
                writer.write_all(line.as_bytes()).await?;
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }

    Ok(())
}
