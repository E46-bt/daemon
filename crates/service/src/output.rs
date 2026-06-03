use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use libc;

use carplay_protocol::{DspCommand, Source};

use crate::airplay::{CHANNELS, PERIOD_SIZE};
use crate::dsp::DspPipeline;
use crate::stats::AudioStats;

pub fn open_output(device: &str, sample_rate: u32) -> Result<PCM> {
    let pcm = PCM::new(device, Direction::Playback, false)
        .with_context(|| format!("Impossible d'ouvrir le device ALSA output: {}", device))?;
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
) -> Result<()> {
    let mut current_source = Source::Airplay;
    let mut pcm = open_output(device, current_source.sample_rate())?;
    let silence = vec![0f32; (PERIOD_SIZE * CHANNELS) as usize];
    let mut pipeline = DspPipeline::new(current_source.sample_rate() as f32);

    while running.load(Ordering::Relaxed) {
        // Dépile toutes les commandes DSP disponibles
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

        // Changement de source
        let new_source = Source::from_usize(source.load(Ordering::Relaxed));
        if new_source != current_source {
            let sr = new_source.sample_rate();
            drop(pcm);
            pcm = open_output(device, sr)?;
            pipeline.rebuild_for_rate(sr as f32);
            current_source = new_source;
        }

        // Vide le canal inactif
        match current_source {
            Source::Airplay => { while bt_rx.try_recv().is_ok() {} }
            Source::Bluetooth => { while airplay_rx.try_recv().is_ok() {} }
        }

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

        // Stats avant mute — VU-mètres actifs même en silence
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
                    return Err(anyhow::anyhow!("Erreur playback ALSA: {}", e));
                }
            }
        }
    }
    Ok(())
}
