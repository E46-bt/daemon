use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use anyhow::Result;
use crossbeam_channel::Sender;
use libc;

use crate::airplay::{CHANNELS, PERIOD_SIZE};

/// bluez-alsa sort en 48 kHz — doit correspondre à la config bluez-alsa.
pub const BT_SAMPLE_RATE: u32 = 48000;

fn open_bt_capture(device: &str) -> Result<PCM> {
    let pcm = PCM::new(device, Direction::Capture, false)?;
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_channels(CHANNELS)?;
        hwp.set_rate(BT_SAMPLE_RATE, ValueOr::Nearest)?;
        hwp.set_format(Format::S16LE)?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_period_size(PERIOD_SIZE as alsa::pcm::Frames, ValueOr::Nearest)?;
        hwp.set_buffer_size((PERIOD_SIZE * 4) as alsa::pcm::Frames)?;
        pcm.hw_params(&hwp)?;
    }
    pcm.start()?;
    Ok(pcm)
}

/// Thread de capture Bluetooth (A2DP via bluez-alsa → loopback ALSA).
/// Réessaie toutes les 2 s si bluez-alsa n'est pas encore démarré.
pub fn capture_loop(device: &str, tx: Sender<Vec<f32>>, running: Arc<AtomicBool>) -> Result<()> {
    'outer: while running.load(Ordering::Relaxed) {
        let pcm = match open_bt_capture(device) {
            Ok(p) => p,
            Err(_) => {
                for _ in 0..20 {
                    if !running.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
        };

        let io = pcm.io_i16()?;
        let mut buf = vec![0i16; (PERIOD_SIZE * CHANNELS) as usize];

        while running.load(Ordering::Relaxed) {
            match pcm.wait(Some(200)) {
                Ok(false) => continue,
                Err(e) => {
                    let errno = e.errno();
                    if errno == libc::EPIPE || errno == libc::ESTRPIPE {
                        pcm.recover(errno, false)?;
                    } else {
                        continue 'outer;
                    }
                    continue;
                }
                Ok(true) => {}
            }

            match io.readi(&mut buf) {
                Ok(n) if n > 0 => {
                    let floats: Vec<f32> = buf[..n * CHANNELS as usize]
                        .iter()
                        .map(|&s| s as f32 / i16::MAX as f32)
                        .collect();
                    let _ = tx.try_send(floats);
                }
                Ok(_) => {}
                Err(e) => {
                    let errno = e.errno();
                    if errno == libc::EPIPE || errno == libc::ESTRPIPE {
                        pcm.recover(errno, false)?;
                    } else {
                        continue 'outer;
                    }
                }
            }
        }
    }
    Ok(())
}
