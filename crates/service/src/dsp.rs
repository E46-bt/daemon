use std::f32::consts::PI;

use carplay_protocol::{DspCommand, EQ_BANDS};

pub const EQ_FREQS: [f32; EQ_BANDS] = [
    31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
const EQ_Q: f32 = 1.414;

// ─── Coefficients biquad ─────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Coeffs {
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
}

impl Coeffs {
    fn bypass() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }
    }

    fn peaking(freq: f32, gain_db: f32, q: f32, sr: f32) -> Self {
        if gain_db.abs() < 0.01 { return Self::bypass(); }
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sr;
        let alpha = w0.sin() / (2.0 * q);
        let cw = w0.cos();
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cw) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cw) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }

    fn low_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        if gain_db.abs() < 0.01 { return Self::bypass(); }
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sr;
        let cw = w0.cos();
        let sa = 2.0 * a.sqrt() * w0.sin() / 2.0 * 2.0f32.sqrt();
        let a0 = (a + 1.0) + (a - 1.0) * cw + sa;
        Self {
            b0: a * ((a + 1.0) - (a - 1.0) * cw + sa) / a0,
            b1: 2.0 * a * ((a - 1.0) - (a + 1.0) * cw) / a0,
            b2: a * ((a + 1.0) - (a - 1.0) * cw - sa) / a0,
            a1: -2.0 * ((a - 1.0) + (a + 1.0) * cw) / a0,
            a2: ((a + 1.0) + (a - 1.0) * cw - sa) / a0,
        }
    }

    fn high_shelf(freq: f32, gain_db: f32, sr: f32) -> Self {
        if gain_db.abs() < 0.01 { return Self::bypass(); }
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sr;
        let cw = w0.cos();
        let sa = 2.0 * a.sqrt() * w0.sin() / 2.0 * 2.0f32.sqrt();
        let a0 = (a + 1.0) - (a - 1.0) * cw + sa;
        Self {
            b0: a * ((a + 1.0) + (a - 1.0) * cw + sa) / a0,
            b1: -2.0 * a * ((a - 1.0) + (a + 1.0) * cw) / a0,
            b2: a * ((a + 1.0) + (a - 1.0) * cw - sa) / a0,
            a1: 2.0 * ((a - 1.0) - (a + 1.0) * cw) / a0,
            a2: ((a + 1.0) - (a - 1.0) * cw - sa) / a0,
        }
    }
}

// ─── Filtre biquad stéréo ─────────────────────────────────────────────────────

struct BiquadStereo {
    c: Coeffs,
    s1l: f32, s2l: f32,
    s1r: f32, s2r: f32,
}

impl BiquadStereo {
    fn new(c: Coeffs) -> Self {
        Self { c, s1l: 0.0, s2l: 0.0, s1r: 0.0, s2r: 0.0 }
    }

    #[inline]
    fn process(&mut self, buf: &mut [f32]) {
        let mut i = 0;
        while i + 1 < buf.len() {
            let xl = buf[i];
            let yl = self.c.b0 * xl + self.s1l;
            self.s1l = self.c.b1 * xl - self.c.a1 * yl + self.s2l;
            self.s2l = self.c.b2 * xl - self.c.a2 * yl;
            buf[i] = yl;

            let xr = buf[i + 1];
            let yr = self.c.b0 * xr + self.s1r;
            self.s1r = self.c.b1 * xr - self.c.a1 * yr + self.s2r;
            self.s2r = self.c.b2 * xr - self.c.a2 * yr;
            buf[i + 1] = yr;

            i += 2;
        }
    }
}

// ─── Pipeline DSP ─────────────────────────────────────────────────────────────

pub struct DspPipeline {
    sr: f32,
    pub volume: f32,
    vol_current: f32,
    vol_step: f32,
    pub eq_gains: [f32; EQ_BANDS],
    eq_filters: Vec<BiquadStereo>,
    pub loudness: bool,
    loud_low: BiquadStereo,
    loud_high: BiquadStereo,
    pub limiter: bool,
    lim_gain: f32,
    lim_release: f32,
    pub limiter_active: bool,
}

impl DspPipeline {
    pub fn new(sr: f32) -> Self {
        let eq_filters = EQ_FREQS
            .iter()
            .map(|&f| BiquadStereo::new(Coeffs::peaking(f, 0.0, EQ_Q, sr)))
            .collect();

        let mut p = Self {
            sr,
            volume: 0.8,
            vol_current: 0.8,
            vol_step: 1.0 / (sr * 0.01),
            eq_gains: [0.0; EQ_BANDS],
            eq_filters,
            loudness: true,
            loud_low: BiquadStereo::new(Coeffs::bypass()),
            loud_high: BiquadStereo::new(Coeffs::bypass()),
            limiter: true,
            lim_gain: 1.0,
            lim_release: 1.0 / (sr * 0.1),
            limiter_active: false,
        };
        p.rebuild_loudness();
        p
    }

    pub fn rebuild_for_rate(&mut self, sr: f32) {
        self.sr = sr;
        self.vol_step = 1.0 / (sr * 0.01);
        self.lim_release = 1.0 / (sr * 0.1);
        self.lim_gain = 1.0;
        self.limiter_active = false;
        for (i, filter) in self.eq_filters.iter_mut().enumerate() {
            *filter = BiquadStereo::new(Coeffs::peaking(EQ_FREQS[i], self.eq_gains[i], EQ_Q, sr));
        }
        self.rebuild_loudness();
    }

    pub fn apply(&mut self, cmd: &DspCommand) {
        match cmd {
            DspCommand::SetVolume { value } => {
                self.volume = value.clamp(0.0, 1.0);
                self.rebuild_loudness();
            }
            DspCommand::SetEqBand { band, gain_db } if *band < EQ_BANDS => {
                self.eq_gains[*band] = gain_db.clamp(-12.0, 12.0);
                self.eq_filters[*band].c =
                    Coeffs::peaking(EQ_FREQS[*band], self.eq_gains[*band], EQ_Q, self.sr);
            }
            DspCommand::SetLoudness { value } => self.loudness = *value,
            DspCommand::SetLimiter { value } => self.limiter = *value,
            _ => {}
        }
    }

    fn rebuild_loudness(&mut self) {
        let s = (1.0 - self.volume).powi(2);
        self.loud_low = BiquadStereo::new(Coeffs::low_shelf(60.0, 4.0 * s, self.sr));
        self.loud_high = BiquadStereo::new(Coeffs::high_shelf(12000.0, 2.0 * s, self.sr));
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        let n_frames = buf.len() / 2;

        for i in 0..n_frames {
            let diff = self.volume - self.vol_current;
            if diff.abs() > self.vol_step {
                self.vol_current += diff.signum() * self.vol_step;
            } else {
                self.vol_current = self.volume;
            }
            buf[i * 2] *= self.vol_current;
            buf[i * 2 + 1] *= self.vol_current;
        }

        for f in &mut self.eq_filters { f.process(buf); }

        if self.loudness {
            self.loud_low.process(buf);
            self.loud_high.process(buf);
        }

        if self.limiter {
            const THRESHOLD: f32 = 0.98;
            self.limiter_active = false;
            let mut i = 0;
            while i + 1 < buf.len() {
                let peak = buf[i].abs().max(buf[i + 1].abs());
                if peak * self.lim_gain > THRESHOLD {
                    self.lim_gain = THRESHOLD / peak;
                    self.limiter_active = true;
                } else {
                    self.lim_gain = (self.lim_gain + self.lim_release).min(1.0);
                }
                buf[i] *= self.lim_gain;
                buf[i + 1] *= self.lim_gain;
                i += 2;
            }
        } else {
            self.limiter_active = false;
        }
    }
}
