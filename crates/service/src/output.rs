use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use libc;
use tokio::sync::mpsc::UnboundedSender;

use carplay_protocol::{DspCommand, Source};

use crate::airplay::{CHANNELS, PERIOD_SIZE};
use crate::dsp::DspPipeline;
use crate::stats::AudioStats;

/// Smoothed RMS below which a source is considered silent.
const ACTIVITY_THRESHOLD: f32 = 0.002; // ~−54 dBFS
/// A source must emit signal for this long before it is considered active.
const ACTIVATE_DURATION: Duration = Duration::from_millis(500);
/// A source must be silent for this long before it is considered inactive.
const DEACTIVATE_DURATION: Duration = Duration::from_secs(3);

struct SourceActivity {
    rms: f32,
    is_active: bool,
    went_active_at: Option<Instant>,
    went_silent_at: Option<Instant>,
}

impl SourceActivity {
    fn new() -> Self {
        Self { rms: 0.0, is_active: false, went_active_at: None, went_silent_at: None }
    }

    fn feed(&mut self, block: &[f32]) {
        let instant_rms: f32 =
            (block.iter().map(|&s| s * s).sum::<f32>() / block.len() as f32).sqrt();
        // Smooth with a slow attack / slow decay IIR
        self.rms = self.rms * 0.95 + instant_rms * 0.05;

        let now = Instant::now();
        if self.rms > ACTIVITY_THRESHOLD {
            self.went_silent_at = None;
            let active_since = *self.went_active_at.get_or_insert(now);
            if !self.is_active && now.duration_since(active_since) >= ACTIVATE_DURATION {
                self.is_active = true;
            }
        } else {
            self.went_active_at = None;
            if self.is_active {
                let silent_since = *self.went_silent_at.get_or_insert(now);
                if now.duration_since(silent_since) >= DEACTIVATE_DURATION {
                    self.is_active = false;
                    self.went_silent_at = None;
                }
            }
        }
    }
}

pub fn open_output(device: &str, sample_rate: u32) -> Result<PCM> {
    let pcm = PCM::new(device, Direction::Playback, false)
        .with_context(|| format!("failed to open ALSA output device: {}", device))?;
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_channels(CHANNELS)?;
        hwp.set_rate(sample_rate, ValueOr::Nearest)?;
        hwp.set_format(Format::FloatLE)?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_period_size(PERIOD_SIZE as alsa::pcm::Frames, ValueOr::Nearest)?;
        hwp.set_buffer_size((PERIOD_SIZE * 8) as alsa::pcm::Frames)?;
        pcm.hw_params(&hwp)?;
    }
    Ok(pcm)
}

pub fn playback_loop(
    device: &str,
    airplay_rx: Receiver<Vec<f32>>,
    bt_rx: Receiver<Vec<f32>>,
    cmd_rx: Receiver<DspCommand>,
    stats: Arc<AudioStats>,
    running: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    source: Arc<AtomicUsize>,
    // Notifies the Hub of auto-switch events so it can broadcast state to clients.
    auto_src_tx: UnboundedSender<Source>,
) -> Result<()> {
    let mut current_source = Source::Airplay;
    let mut pcm = open_output(device, current_source.sample_rate())?;
    let silence = vec![0f32; (PERIOD_SIZE * CHANNELS) as usize];
    let mut pipeline = DspPipeline::new(current_source.sample_rate() as f32);

    let mut airplay_act = SourceActivity::new();
    let mut bt_act = SourceActivity::new();

    while running.load(Ordering::Relaxed) {
        // Drain all pending DSP commands
        while let Ok(cmd) = cmd_rx.try_recv() {
            match &cmd {
                DspCommand::SetSource { value } => {
                    source.store(value.as_usize(), Ordering::Relaxed);
                }
                DspCommand::SetMute { value } => {
                    muted.store(*value, Ordering::Relaxed);
                }
                _ => pipeline.apply(&cmd),
            }
        }

        // Apply source change (manual or auto)
        let new_source = Source::from_usize(source.load(Ordering::Relaxed));
        if new_source != current_source {
            let sr = new_source.sample_rate();
            drop(pcm);
            pcm = open_output(device, sr)?;
            pipeline.rebuild_for_rate(sr as f32);
            current_source = new_source;
        }

        // Drain the inactive channel and feed its frames to the activity detector.
        match current_source {
            Source::Airplay => {
                while let Ok(frames) = bt_rx.try_recv() {
                    bt_act.feed(&frames);
                }
            }
            Source::Bluetooth => {
                while let Ok(frames) = airplay_rx.try_recv() {
                    airplay_act.feed(&frames);
                }
            }
        }

        // Read one block from the active source
        let mut block = match current_source {
            Source::Airplay => match airplay_rx.recv_timeout(Duration::from_millis(20)) {
                Ok(b) => b,
                Err(RecvTimeoutError::Timeout) => silence.clone(),
                Err(_) => break,
            },
            Source::Bluetooth => match bt_rx.recv_timeout(Duration::from_millis(20)) {
                Ok(b) => b,
                Err(RecvTimeoutError::Timeout) => silence.clone(),
                Err(_) => break,
            },
        };

        // Feed active source to its detector
        match current_source {
            Source::Airplay => airplay_act.feed(&block),
            Source::Bluetooth => bt_act.feed(&block),
        }

        // Auto-switch:
        //   - AirPlay takes priority: if it becomes active, always switch to it.
        //   - Switch to BT only when AirPlay is inactive and BT is active.
        //   - If both inactive, keep current source.
        let want = if airplay_act.is_active {
            Source::Airplay
        } else if bt_act.is_active {
            Source::Bluetooth
        } else {
            current_source
        };

        if want != current_source {
            source.store(want.as_usize(), Ordering::Relaxed);
            // Best-effort: notify Hub so it can update its state and broadcast to clients.
            let _ = auto_src_tx.send(want);
        }

        // Compute stats before mute so VU meters stay active in silence
        stats.update(&block);

        let to_write: &[f32] = if muted.load(Ordering::Relaxed) {
            &silence
        } else {
            pipeline.process(&mut block);
            stats.limiter_active.store(pipeline.limiter_active, Ordering::Relaxed);
            &block
        };

        match pcm.io_f32()?.writei(to_write) {
            Ok(_) => {}
            Err(e) => {
                let errno = e.errno();
                if errno == libc::EPIPE || errno == libc::ESTRPIPE {
                    pcm.recover(errno, false)?;
                } else {
                    return Err(anyhow::anyhow!("ALSA playback error: {}", e));
                }
            }
        }
    }
    Ok(())
}
