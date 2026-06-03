# carplay-audio

Rust audio daemon for Raspberry Pi car stereo. Bridges AirPlay and Bluetooth A2DP sources through a DSP pipeline to a PCM5122 DAC (HiFiBerry).

Controlled from three places at once: the TUI on the Pi, any WebSocket client over Wi-Fi, and any BLE central (iOS/Android) without touching the music stream.

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

The DSP pipeline is owned by the audio playback thread (no locks in the hot path). Commands arrive via a crossbeam channel.

## Control

Three control transports share the same protocol (newline-delimited JSON):

| Transport | Address | Use case |
|---|---|---|
| Unix socket | `/run/carplay-audio.sock` | TUI on the Pi |
| WebSocket | `ws://<pi-ip>:9000/ws` | phone or browser over Wi-Fi |
| BLE GATT | advertised as "carplay-audio" | phone over Bluetooth |

The BLE transport is separate from A2DP music — Classic BT handles audio, BLE handles control. No BT PAN required.

Commands sent to any transport are applied immediately and broadcast to all other connected clients.

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

Messages pushed by the service:
```json
{"type": "state", "volume": 0.8, "eq_gains": [...], "loudness": true, ...}
{"type": "stats", "rms_l": 0.12, "rms_r": 0.11, "clipping": false, ...}
```

### BLE GATT UUIDs

These UUIDs are used by the mobile app to discover and communicate with the service.

Service: `cafecafe-cafe-cafe-cafe-cafecafe0001`
Command characteristic (write-without-response): `cafecafe-cafe-cafe-cafe-cafecafe0002`
Stats characteristic (notify): `cafecafe-cafe-cafe-cafe-cafecafe0003`

## Crates

- `crates/protocol` — shared types (DspCommand, ServiceMessage, DspState, Source)
- `crates/service` — audio daemon: capture threads, DSP pipeline, all three control servers
- `crates/tui` — ratatui client that connects to the service via Unix socket

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

Installs bluez, bluez-alsa-utils, bluez-tools, configures the ALSA loopback with two substreams, sets the device name and starts the required services.

| Service | Role |
|---|---|
| `bluetooth` | BlueZ daemon |
| `bluealsa` | A2DP sink bridge (BlueZ to ALSA) |
| `bluealsa-aplay` | Routes BT audio to `hw:Loopback,0,1` |
| `bt-agent` | Auto-accepts pairing |

First pairing: scan for the device name on your phone. Subsequent connections are automatic.

### Config files

| File in repo | Deploy to |
|---|---|
| `asound.conf` | `/etc/asound.conf` |
| `shairport-sync.conf` | `/etc/shairport-sync.conf` |
| `bluetooth-main.conf` | `/etc/bluetooth/main.conf` |
| `bluealsa-aplay.service` | `/etc/systemd/system/` |
| `bt-agent.service` | `/etc/systemd/system/` |

## Building

Binaries: `carplay-audio` (service daemon) and `carplay-tui` (TUI client).

```bash
./build.sh all              # build Docker image + compile (first run)
./build.sh build            # compile only
./build.sh deploy pi@<ip>   # copy both binaries to the Pi via scp
```

On an Apple Silicon Mac, the Docker image runs `linux/arm64` natively, same ISA as the Pi. On x86 Linux, it cross-compiles to `aarch64-unknown-linux-gnu`.

To build directly on the Pi:
```bash
sudo apt install libdbus-1-dev libasound2-dev
cargo build --release --workspace
```

## Diagnostics

```bash
# Check AirPlay loopback format
cat /proc/asound/Loopback/pcm0p/sub0/hw_params

# Check Bluetooth loopback format
cat /proc/asound/Loopback/pcm1p/sub0/hw_params

# Watch BT audio in real time
journalctl -u bluealsa-aplay -f

# List paired BT devices
bluetoothctl devices
```

## TUI keybinds

| Key | Action |
|---|---|
| Tab | Switch source (AirPlay / Bluetooth) |
| M | Mute / unmute |
| + / - | Volume |
| L | Loudness |
| R | Limiter |
| E | EQ edit mode (arrow keys: band and gain) |
| Q / Esc | Quit |

Settings are saved to `~/.carplay-audio.json` when the service exits.
