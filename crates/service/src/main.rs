mod airplay;
mod bluetooth;
mod dsp;
mod output;
mod server;
mod settings;
mod stats;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::Result;
use carplay_protocol::{DspCommand, EQ_BANDS, SOCKET_PATH, WS_PORT};
use crossbeam_channel::bounded;

use server::Hub;
use stats::AudioStats;

const AIRPLAY_LOOPBACK: &str = "hw:Loopback,1,0";
const BT_LOOPBACK: &str = "hw:Loopback,1,1";
const OUTPUT_DEVICE: &str = "plughw:sndrpihifiberry";

fn main() -> Result<()> {
    let initial_state = settings::load();

    let running = Arc::new(AtomicBool::new(true));
    let muted = Arc::new(AtomicBool::new(initial_state.muted));
    let source = Arc::new(AtomicUsize::new(initial_state.source.as_usize()));
    let audio_stats = AudioStats::new();

    let (airplay_tx, airplay_rx) = bounded::<Vec<f32>>(8);
    let (bt_tx, bt_rx) = bounded::<Vec<f32>>(8);
    let (cmd_tx, cmd_rx) = bounded::<DspCommand>(64);

    // Apply saved settings to the DSP pipeline at startup
    let _ = cmd_tx.try_send(DspCommand::SetVolume { value: initial_state.volume });
    let _ = cmd_tx.try_send(DspCommand::SetLoudness { value: initial_state.loudness });
    let _ = cmd_tx.try_send(DspCommand::SetLimiter { value: initial_state.limiter });
    for (i, &gain) in initial_state.eq_gains.iter().enumerate().take(EQ_BANDS) {
        let _ = cmd_tx.try_send(DspCommand::SetEqBand { band: i, gain_db: gain });
    }

    let hub = Hub::new(cmd_tx, initial_state, audio_stats.clone());
    // Keep a reference to the canonical state so we can save it on shutdown
    let final_state_ref = hub.state.clone();

    let airplay_handle = {
        let running = running.clone();
        thread::Builder::new()
            .name("airplay-capture".into())
            .spawn(move || {
                if let Err(e) = airplay::capture_loop(AIRPLAY_LOOPBACK, airplay_tx, running) {
                    eprintln!("[airplay] {}", e);
                }
            })?
    };

    let bt_handle = {
        let running = running.clone();
        thread::Builder::new()
            .name("bt-capture".into())
            .spawn(move || {
                if let Err(e) = bluetooth::capture_loop(BT_LOOPBACK, bt_tx, running) {
                    eprintln!("[bluetooth] {}", e);
                }
            })?
    };

    let playback_handle = {
        let running = running.clone();
        let muted = muted.clone();
        let stats = audio_stats.clone();
        let source = source.clone();
        thread::Builder::new()
            .name("audio-playback".into())
            .spawn(move || {
                if let Err(e) = output::playback_loop(
                    OUTPUT_DEVICE,
                    airplay_rx,
                    bt_rx,
                    cmd_rx,
                    stats,
                    running,
                    muted,
                    source,
                ) {
                    eprintln!("[playback] {}", e);
                }
            })?
    };

    // Run async servers on a separate tokio runtime.
    // Audio threads stay as std::thread for real-time priority.
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let hub_ws = hub.clone();
        let hub_ble = hub.clone();

        tokio::spawn(async move {
            if let Err(e) = server::ws::serve(hub_ws, WS_PORT).await {
                eprintln!("[ws] {}", e);
            }
        });

        tokio::spawn(async move {
            if let Err(e) = server::ble::serve(hub_ble).await {
                eprintln!("[ble] {}", e);
            }
        });

        if let Err(e) = hub.run(SOCKET_PATH).await {
            eprintln!("[server] {}", e);
        }
    });

    running.store(false, Ordering::Relaxed);
    let _ = airplay_handle.join();
    let _ = bt_handle.join();
    let _ = playback_handle.join();

    // Persist the final state (synchronously — runtime is already dropped)
    let state = final_state_ref.blocking_read().clone();
    if let Err(e) = settings::save(&state) {
        eprintln!("[settings] save failed: {}", e);
    }

    Ok(())
}
