use serde::{Deserialize, Serialize};

pub const EQ_BANDS: usize = 10;
pub const SOCKET_PATH: &str = "/run/carplay-audio/control.sock";
pub const WS_PORT: u16 = 9000;

// ─── Source ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Airplay,
    Bluetooth,
}

impl Source {
    pub fn sample_rate(self) -> u32 {
        match self {
            Source::Airplay => 352800,
            Source::Bluetooth => 48000,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Source::Airplay => "AirPlay",
            Source::Bluetooth => "Bluetooth",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Source::Airplay => Source::Bluetooth,
            Source::Bluetooth => Source::Airplay,
        }
    }

    pub fn as_usize(self) -> usize {
        match self {
            Source::Airplay => 0,
            Source::Bluetooth => 1,
        }
    }

    pub fn from_usize(v: usize) -> Self {
        match v {
            1 => Source::Bluetooth,
            _ => Source::Airplay,
        }
    }
}

// ─── Commandes DSP (client → service) ────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum DspCommand {
    SetVolume { value: f32 },
    SetEqBand { band: usize, gain_db: f32 },
    SetLoudness { value: bool },
    SetLimiter { value: bool },
    SetSource { value: Source },
    SetMute { value: bool },
}

// ─── Messages service → clients ───────────────────────────────────────────────

/// Stats audio poussées ~2 Hz à tous les clients connectés.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StatsSnapshot {
    pub rms_l: f32,
    pub rms_r: f32,
    pub peak_l: f32,
    pub peak_r: f32,
    pub clipping: bool,
    pub limiter_active: bool,
    pub signal_active: bool,
    pub frames_per_sec: u64,
}

/// État complet du DSP — envoyé à la connexion et après chaque commande.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DspState {
    pub volume: f32,
    pub eq_gains: Vec<f32>,
    pub loudness: bool,
    pub limiter: bool,
    pub muted: bool,
    pub source: Source,
}

impl DspState {
    pub fn new() -> Self {
        Self {
            volume: 0.8,
            eq_gains: vec![0.0; EQ_BANDS],
            loudness: true,
            limiter: true,
            muted: false,
            source: Source::Airplay,
        }
    }
}

/// Enveloppe pour les messages poussés aux clients (newline-delimited JSON).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServiceMessage {
    Stats(StatsSnapshot),
    State(DspState),
}
