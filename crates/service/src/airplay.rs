use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use libc;

pub const SAMPLE_RATE: u32 = 352800;
pub const CHANNELS: u32 = 2;
pub const PERIOD_SIZE: u32 = 2048;

pub fn open_loopback_capture(device: &str) -> Result<PCM> {
    let pcm = PCM::new(device, Direction::Capture, false)
        .with_context(|| format!("failed to open ALSA capture device: {}", device))?;
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_channels(CHANNELS)?;
        hwp.set_rate(SAMPLE_RATE, ValueOr::Nearest)?;
        hwp.set_format(Format::S32LE)?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_period_size(PERIOD_SIZE as alsa::pcm::Frames, ValueOr::Nearest)?;
        hwp.set_buffer_size((PERIOD_SIZE * 4) as alsa::pcm::Frames)?;
        pcm.hw_params(&hwp)?;
    }
    pcm.start()?;
    Ok(pcm)
}

pub fn capture_loop(
    device: &str,
    tx: Sender<Vec<f32>>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let pcm = open_loopback_capture(device)?;
    let io = pcm.io_i32()?;
    let mut buf = vec![0i32; (PERIOD_SIZE * CHANNELS) as usize];

    while running.load(std::sync::atomic::Ordering::Relaxed) {
        match pcm.wait(Some(200)) {
            Ok(false) => continue,
            Err(e) => {
                let errno = e.errno();
                if errno == libc::EPIPE || errno == libc::ESTRPIPE {
                    pcm.recover(errno, false)?;
                }
                continue;
            }
            Ok(true) => {}
        }

        match io.readi(&mut buf) {
            Ok(n) if n > 0 => {
                let floats: Vec<f32> = buf[..n * CHANNELS as usize]
                    .iter()
                    .map(|&s| s as f32 / i32::MAX as f32)
                    .collect();
                let _ = tx.try_send(floats);
            }
            Ok(_) => {}
            Err(e) => {
                let errno = e.errno();
                if errno == libc::EPIPE || errno == libc::ESTRPIPE {
                    pcm.recover(errno, false)?;
                } else {
                    return Err(anyhow::anyhow!("AirPlay capture error: {}", e));
                }
            }
        }
    }
    Ok(())
}
