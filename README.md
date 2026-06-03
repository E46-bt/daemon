# carplay-audio

Rust audio daemon for Raspberry Pi car stereo. Bridges AirPlay and Bluetooth A2DP sources through a DSP pipeline to a PCM5122 DAC (HiFiBerry).

Controlled from three places simultaneously: a ratatui TUI on the Pi, any WebSocket client over Wi-Fi, and any BLE central (iOS/Android) — without interrupting the music stream.

## Architecture

```
iPhone (AirPlay)
  shairport-sync -> pcm.airplay -> hw:Loopback,0,0
                                         | snd_aloop substream 0
                                   hw:Loopback,1,0  <- airplay-capture thread
                                         | Vec<f32> 352800 Hz

Phone (Bluetooth A2DP)
  bluez-alsa -> bluealsa-aplay -> hw:Loopback,0,1
                                         | snd_aloop substream 1
                                   hw:Loopback,1,1  <- bt-capture thread
                                         | Vec<f32> 48000 Hz

                         source switcher (Tab / SetSource command)

                         audio-playback thread
                           DspPipeline: volume ramp, 10-band EQ, loudness, limiter
                         plughw:sndrpihifiberry  (PCM5122 HiFiBerry DAC)
```

The DSP pipeline is owned by the audio playback thread with no locks in the hot path.
Commands arrive via a crossbeam channel from the control servers.

## Crates

- `crates/protocol` — shared types: `DspCommand`, `DspState`, `StatsSnapshot`, `Source`
- `crates/service` — audio daemon: capture threads, DSP pipeline, all three control servers
- `crates/tui` — ratatui TUI client, connects to the service via Unix socket

## Service modules

| File | Role |
|---|---|
| `service/src/main.rs` | Spawns audio threads, starts tokio runtime for servers |
| `service/src/airplay.rs` | AirPlay ALSA loopback capture |
| `service/src/bluetooth.rs` | Bluetooth A2DP ALSA loopback capture |
| `service/src/output.rs` | DAC output, source switching, playback loop |
| `service/src/dsp.rs` | Biquad EQ, volume ramp, loudness, brick-wall limiter |
| `service/src/stats.rs` | Lock-free audio stats via atomics |
| `service/src/settings.rs` | Persist DSP state to `~/.carplay-audio.json` |
| `service/src/server/mod.rs` | Hub: broadcast channel + Unix socket listener |
| `service/src/server/ws.rs` | WebSocket server on `:9000` (axum) |
| `service/src/server/ble.rs` | BLE GATT server (bluer / BlueZ) |

## Control

Three transports share the same newline-delimited JSON protocol:

| Transport | Address | Use case |
|---|---|---|
| Unix socket | `/run/carplay-audio.sock` | TUI on the Pi |
| WebSocket | `ws://<pi-ip>:9000/ws` | Phone or browser over Wi-Fi |
| BLE GATT | advertised as "carplay-audio" | Phone over Bluetooth |

BLE is completely separate from A2DP music. Classic BT handles audio (A2DP profile),
BLE handles control (GATT profile). No BT PAN required.

Commands sent on any transport are applied immediately and broadcast to all connected clients.

### Protocol

Commands (client to service):
```json
{"cmd": "set_volume", "value": 0.8}
{"cmd": "set_eq_band", "band": 3, "gain_db": 4.0}
{"cmd": "set_loudness", "value": true}
{"cmd": "set_limiter", "value": true}
{"cmd": "set_source", "value": "bluetooth"}
{"cmd": "set_mute", "value": false}
```

Messages pushed by the service (~2 Hz for stats):
```json
{"type": "state", "volume": 0.8, "eq_gains": [...], "loudness": true, "limiter": true, "muted": false, "source": "airplay"}
{"type": "stats", "rms_l": 0.12, "rms_r": 0.11, "peak_l": 0.3, "peak_r": 0.28, "clipping": false, "limiter_active": false, "signal_active": true, "frames_per_sec": 705600}
```

### BLE GATT UUIDs

Service: `cafecafe-cafe-cafe-cafe-cafecafe0001`
Command characteristic (write-without-response): `cafecafe-cafe-cafe-cafe-cafecafe0002`
Stats characteristic (notify): `cafecafe-cafe-cafe-cafe-cafecafe0003`

## Audio constants

```
AirPlay sample rate:  352800 Hz  (shairport-sync configured without soxr resampler)
Bluetooth sample rate: 48000 Hz  (bluez-alsa default)
Channels:                     2  (stereo interleaved L/R)
Period size:               2048  frames per ALSA period
```

shairport-sync outputs S32_LE. The service converts to f32 at capture, not in the playback loop.
To verify the actual loopback format at runtime: `cat /proc/asound/Loopback/pcm0p/sub0/hw_params`

## Setup on Raspberry Pi

### AirPlay (shairport-sync)

```bash
sudo apt install shairport-sync
sudo cp shairport-sync.conf /etc/shairport-sync.conf
sudo systemctl enable --now shairport-sync
```

### Bluetooth

```bash
./setup-bluetooth.sh
```

Installs bluez, bluez-alsa-utils and bluez-tools, configures the ALSA loopback with two
substreams, and starts the required services.

| Service | Role |
|---|---|
| `bluetooth` | BlueZ daemon |
| `bluealsa` | A2DP sink bridge (BlueZ to ALSA) |
| `bluealsa-aplay` | Routes BT audio to `hw:Loopback,0,1` |
| `bt-agent` | Auto-accepts pairing (no PIN) |

### Config files

| File in repo | Deploy to |
|---|---|
| `asound.conf` | `/etc/asound.conf` |
| `shairport-sync.conf` | `/etc/shairport-sync.conf` |
| `bluetooth-main.conf` | `/etc/bluetooth/main.conf` |
| `bluealsa-aplay.service` | `/etc/systemd/system/` |
| `bt-agent.service` | `/etc/systemd/system/` |

## Building

Produces two binaries: `carplay-audio` (service daemon) and `carplay-tui` (TUI client).

```bash
./build.sh all              # build Docker image + compile (first run)
./build.sh build            # compile only
./build.sh deploy pi@<ip>   # copy both binaries to the Pi via scp
```

On Apple Silicon the Docker image runs `linux/arm64` natively (same ISA as the Pi).
On x86 Linux it cross-compiles to `aarch64-unknown-linux-gnu`.

To build directly on the Pi:
```bash
sudo apt install libdbus-1-dev libasound2-dev
cargo build --release --workspace
```

## Diagnostics

```bash
cat /proc/asound/Loopback/pcm0p/sub0/hw_params   # AirPlay loopback format
cat /proc/asound/Loopback/pcm1p/sub0/hw_params   # Bluetooth loopback format
journalctl -u bluealsa-aplay -f                   # BT audio stream live
bluetoothctl devices                              # paired BT devices
```

## TUI keybinds

| Key | Action |
|---|---|
| Tab | Switch source (AirPlay / Bluetooth) |
| M | Mute / unmute |
| + / - | Volume |
| L | Loudness |
| R | Limiter |
| E | EQ edit mode (arrow keys: select band, adjust gain) |
| Q / Esc | Quit |

## Coding conventions

- All comments and user-visible strings in English
- No `unwrap()` in audio threads -- use `?` and propagate via the join handle
- No `Mutex` in the audio hot path -- atomics or channels only
- Stats are computed before mute so VU meters stay active in silence
- Internal audio format: always `f32` interleaved L/R
- Format conversions (S32LE or S16LE to f32) happen at capture, not in the playback loop

## License

GPLv3 -- see [LICENSE](LICENSE).
