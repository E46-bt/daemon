use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::time::Duration;

use base64::Engine;
use carplay_protocol::TrackInfo;
use tokio::sync::mpsc::UnboundedSender;

pub const PIPE_PATH: &str = "/tmp/shairport-sync-metadata";

// Hex-encoded type and code values from the shairport-sync metadata pipe format.
const T_SSNC: &str = "73736e63"; // "ssnc"
const T_CORE: &str = "636f7265"; // "core"
const C_MDST: &str = "6d647374"; // metadata block start
const C_MDEN: &str = "6d64656e"; // metadata block end (emit track)
const C_ENDR: &str = "656e6472"; // AirPlay session ended (clear track)
const C_MINM: &str = "6d696e6d"; // track title (minimum item name)
const C_ASAR: &str = "61736172"; // artist
const C_ASAL: &str = "6173616c"; // album

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

    let mut pending = TrackInfo::default();
    let mut in_meta = false;

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
                let _ = tx.send(Some(pending.clone()));
                pending = TrackInfo::default();
            }
            (T_SSNC, C_ENDR) => {
                in_meta = false;
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
            _ => {}
        }
    }

    Ok(())
}

// Parse a single shairport-sync metadata XML item line.
// Format: <item><type>HEX</type><code>HEX</code><length>N</length>[<data encoding="base64">B64</data>]</item>
fn parse_item<'a>(line: &'a str) -> Option<(&'a str, &'a str, Option<&'a str>)> {
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
