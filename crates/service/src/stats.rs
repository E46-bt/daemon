use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use carplay_protocol::StatsSnapshot;

pub struct AudioStats {
    pub rms_left: AtomicU64,
    pub rms_right: AtomicU64,
    pub peak_left: AtomicU64,
    pub peak_right: AtomicU64,
    pub frames_total: AtomicU64,
    pub frames_last_sec: AtomicU64,
    pub signal_active: AtomicBool,
    pub clipping: AtomicBool,
    pub limiter_active: AtomicBool,
}

impl AudioStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            rms_left: AtomicU64::new(0),
            rms_right: AtomicU64::new(0),
            peak_left: AtomicU64::new(0),
            peak_right: AtomicU64::new(0),
            frames_total: AtomicU64::new(0),
            frames_last_sec: AtomicU64::new(0),
            signal_active: AtomicBool::new(false),
            clipping: AtomicBool::new(false),
            limiter_active: AtomicBool::new(false),
        })
    }

    fn f32_to_u64(v: f32) -> u64 { v.to_bits() as u64 }
    fn u64_to_f32(v: u64) -> f32 { f32::from_bits(v as u32) }

    pub fn update(&self, block: &[f32]) {
        if block.is_empty() { return; }
        let mut sum_sq_l = 0f64;
        let mut sum_sq_r = 0f64;
        let mut peak_l = 0f32;
        let mut peak_r = 0f32;
        let mut clip = false;
        let mut n = 0usize;

        for chunk in block.chunks_exact(2) {
            let l = chunk[0];
            let r = chunk[1];
            sum_sq_l += (l as f64) * (l as f64);
            sum_sq_r += (r as f64) * (r as f64);
            if l.abs() > peak_l { peak_l = l.abs(); }
            if r.abs() > peak_r { peak_r = r.abs(); }
            if l.abs() >= 1.0 || r.abs() >= 1.0 { clip = true; }
            n += 1;
        }
        if n == 0 { return; }

        let rms_l = ((sum_sq_l / n as f64).sqrt()) as f32;
        let rms_r = ((sum_sq_r / n as f64).sqrt()) as f32;

        self.rms_left.store(Self::f32_to_u64(rms_l), Ordering::Relaxed);
        self.rms_right.store(Self::f32_to_u64(rms_r), Ordering::Relaxed);
        self.peak_left.store(Self::f32_to_u64(peak_l), Ordering::Relaxed);
        self.peak_right.store(Self::f32_to_u64(peak_r), Ordering::Relaxed);
        self.signal_active.store(rms_l > 0.001 || rms_r > 0.001, Ordering::Relaxed);
        self.clipping.store(clip, Ordering::Relaxed);

        let frames = (n * 2) as u64;
        self.frames_total.fetch_add(frames, Ordering::Relaxed);
        self.frames_last_sec.fetch_add(frames, Ordering::Relaxed);
    }

    pub fn rms_left(&self) -> f32 { Self::u64_to_f32(self.rms_left.load(Ordering::Relaxed)) }
    pub fn rms_right(&self) -> f32 { Self::u64_to_f32(self.rms_right.load(Ordering::Relaxed)) }
    pub fn peak_left(&self) -> f32 { Self::u64_to_f32(self.peak_left.load(Ordering::Relaxed)) }
    pub fn peak_right(&self) -> f32 { Self::u64_to_f32(self.peak_right.load(Ordering::Relaxed)) }

    pub fn reset_per_sec(&self) -> u64 {
        self.frames_last_sec.swap(0, Ordering::Relaxed)
    }

    pub fn snapshot(&self, frames_per_sec: u64) -> StatsSnapshot {
        StatsSnapshot {
            rms_l: self.rms_left(),
            rms_r: self.rms_right(),
            peak_l: self.peak_left(),
            peak_r: self.peak_right(),
            clipping: self.clipping.load(Ordering::Relaxed),
            limiter_active: self.limiter_active.load(Ordering::Relaxed),
            signal_active: self.signal_active.load(Ordering::Relaxed),
            frames_per_sec,
        }
    }
}
