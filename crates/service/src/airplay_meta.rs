use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::time::Duration;

use base64::Engine;
use carplay_protocol::TrackInfo;
use tokio::sync::mpsc::UnboundedSender;

pub const PIPE_PATH: &str = "/tmp/shairport-sync-metadata";

// Hex-encoded type / code values from the shairport-sync metadata pipe format.
const T_SSNC: &str = "73736e63"; // "ssnc"
const T_CORE: &str = "636f7265"; // "core"
const C_MDST: &str = "6d647374"; // metadata block start
const C_MDEN: &str = "6d64656e"; // metadata block end → emit track
const C_ENDR: &str = "656e6472"; // AirPlay session ended → clear track
const C_MINM: &str = "6d696e6d"; // track title (minimum item name)
const C_ASAR: &str = "61736172"; // artist
const C_ASAL: &str = "6173616c"; // album
const C_PRGR: &str = "70726772"; // progress "start/current/end" at 44100 Hz RTP clock

pub fn watch_loop(tx: UnboundedSender<Option<TrackInfo>>) {
    loop {
        match read_pipe(&tx) {
            Ok(()) => {}
            Err(e) => eprintln!("[airplay-meta] {}, retrying in 3s", e),
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

fn read_pipe(tx: &UnboundedSender<Option<TrackInfo>>) -> anyhow::Result<()> {
    // Open read-write: prevents blocking on open when no writer exists, and
    // prevents EOF when shairport-sync closes its write end between tracks.
    let file = OpenOptions::new().read(true).write(true).open(PIPE_PATH)?;
    let reader = BufReader::new(file);

    let mut in_meta = false;
    let mut pending = TrackInfo::default(); // assembling during mdst..mden
    let mut current = TrackInfo::default(); // last emitted track (updated on prgr)
    let mut last_sent_pos: Option<u64> = None;

    for line in reader.lines() {
        let line = line?;
        let Some((itype, code, data)) = parse_item(&line) else { continue };

        match (itype, code) {
            (T_SSNC, C_MDST) => {
                pending = TrackInfo::default();
                in_meta = true;
            }
            (T_SSNC, C_MDEN) if in_meta => {
                in_meta = false;
                current = pending.clone();
                last_sent_pos = None;
                pending = TrackInfo::default();
                let _ = tx.send(Some(current.clone()));
            }
            (T_SSNC, C_ENDR) => {
                in_meta = false;
                current = TrackInfo::default();
                last_sent_pos = None;
                let _ = tx.send(None);
            }
            (T_CORE, C_MINM) if in_meta => {
                pending.title = data.and_then(decode_b64);
            }
            (T_CORE, C_ASAR) if in_meta => {
                pending.artist = data.and_then(decode_b64);
            }
            (T_CORE, C_ASAL) if in_meta => {
                pending.album = data.and_then(decode_b64);
            }
            // prgr arrives both inside and outside metadata blocks
            (T_SSNC, C_PRGR) => {
                if let Some((pos_ms, dur_ms)) =
                    data.and_then(decode_b64).as_deref().and_then(parse_progress)
                {
                    // Only re-emit when position changed by >= 1 s to avoid flooding
                    let should_send = last_sent_pos
                        .map(|p| pos_ms.abs_diff(p) >= 1000)
                        .unwrap_or(true);

                    if in_meta {
                        pending.position_ms = Some(pos_ms);
                        pending.duration_ms = Some(dur_ms);
                    } else if !current.is_empty() && should_send {
                        current.position_ms = Some(pos_ms);
                        current.duration_ms = Some(dur_ms);
                        last_sent_pos = Some(pos_ms);
                        let _ = tx.send(Some(current.clone()));
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

// Parse "start/current/end" RTP timestamps (44100 Hz clock) into (position_ms, duration_ms).
fn parse_progress(s: &str) -> Option<(u64, u64)> {
    let mut parts = s.trim().splitn(3, '/');
    let start: u32 = parts.next()?.trim().parse().ok()?;
    let current: u32 = parts.next()?.trim().parse().ok()?;
    let end: u32 = parts.next()?.trim().parse().ok()?;
    // Wrapping subtraction handles RTP timestamp rollover (u32 wraps at ~27 hours)
    let pos_samples = current.wrapping_sub(start) as u64;
    let dur_samples = end.wrapping_sub(start) as u64;
    Some((pos_samples * 1000 / 44100, dur_samples * 1000 / 44100))
}

// Parse a single shairport-sync metadata XML item line.
// Format: <item><type>HEX</type><code>HEX</code><length>N</length>[<data encoding="base64">B64</data>]</item>
fn parse_item(line: &str) -> Option<(&str, &str, Option<&str>)> {
    const TYPE_OPEN: &str = "<type>";
    const TYPE_CLOSE: &str = "</type>";
    const CODE_OPEN: &str = "<code>";
    const CODE_CLOSE: &str = "</code>";
    const DATA_OPEN: &str = "<data encoding=\"base64\">";
    const DATA_CLOSE: &str = "</data>";

    let ts = line.find(TYPE_OPEN)? + TYPE_OPEN.len();
    let te = ts + line[ts..].find(TYPE_CLOSE)?;
    let itype = &line[ts..te];

    let cs = line.find(CODE_OPEN)? + CODE_OPEN.len();
    let ce = cs + line[cs..].find(CODE_CLOSE)?;
    let code = &line[cs..ce];

    let data = if let Some(ds) = line.find(DATA_OPEN) {
        let ds = ds + DATA_OPEN.len();
        let de = ds + line[ds..].find(DATA_CLOSE)?;
        Some(&line[ds..de])
    } else {
        None
    };

    Some((itype, code, data))
}

fn decode_b64(s: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()?;
    String::from_utf8(bytes).ok()
}
