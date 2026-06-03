use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use carplay_protocol::{DspState, EQ_BANDS};

#[derive(Serialize, Deserialize)]
pub struct Settings {
    pub volume: f32,
    pub eq_gains: Vec<f32>,
    pub loudness: bool,
    pub limiter: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 0.8,
            eq_gains: vec![0.0; EQ_BANDS],
            loudness: true,
            limiter: true,
        }
    }
}

impl From<&DspState> for Settings {
    fn from(s: &DspState) -> Self {
        Self {
            volume: s.volume,
            eq_gains: s.eq_gains.clone(),
            loudness: s.loudness,
            limiter: s.limiter,
        }
    }
}

impl From<Settings> for DspState {
    fn from(s: Settings) -> Self {
        let mut eq_gains = vec![0.0f32; EQ_BANDS];
        let len = s.eq_gains.len().min(EQ_BANDS);
        eq_gains[..len].copy_from_slice(&s.eq_gains[..len]);
        DspState {
            volume: s.volume,
            eq_gains,
            loudness: s.loudness,
            limiter: s.limiter,
            ..DspState::new()
        }
    }
}

fn path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".carplay-audio.json")
}

pub fn load() -> DspState {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .map(DspState::from)
        .unwrap_or_else(DspState::new)
}

pub fn save(state: &DspState) -> Result<()> {
    let json = serde_json::to_string_pretty(&Settings::from(state))?;
    std::fs::write(path(), json)?;
    Ok(())
}
