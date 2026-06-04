#!/usr/bin/env bash
# setup.sh -- Full system setup for carplay-audio on Raspberry Pi.
# Installs and configures AirPlay (shairport-sync), Bluetooth (A2DP),
# ALSA loopback, and the carplay-audio DSP daemon.
# Safe to re-run after config changes.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "========================================"
echo "  carplay-audio full system setup"
echo "========================================"
echo ""

# ---------------------------------------------------------------------------
# 1. System dependencies
# ---------------------------------------------------------------------------
echo "[1/5] Installing dependencies..."
sudo apt-get update -q
sudo apt-get install -y \
    bluez bluez-alsa-utils \
    shairport-sync \
    python3-dbus python3-gi

# ---------------------------------------------------------------------------
# 2. AirPlay — shairport-sync
# ---------------------------------------------------------------------------
echo "[2/5] Configuring AirPlay (shairport-sync)..."
sudo cp "$SCRIPT_DIR/shairport-sync.conf" /etc/shairport-sync.conf
sudo systemctl enable shairport-sync
sudo systemctl restart shairport-sync

# ---------------------------------------------------------------------------
# 3. Bluetooth — delegates to setup-bluetooth.sh
#    (handles ALSA loopback, asound.conf, BlueZ config, all BT services)
# ---------------------------------------------------------------------------
echo "[3/5] Configuring Bluetooth..."
bash "$SCRIPT_DIR/setup-bluetooth.sh"

# ---------------------------------------------------------------------------
# 4. carplay-audio binary
# ---------------------------------------------------------------------------
echo "[4/5] Deploying carplay-audio binary..."
BINARY="$SCRIPT_DIR/carplay-audio"
if [ -f "$BINARY" ]; then
    sudo cp "$BINARY" /usr/local/bin/carplay-audio
    sudo chmod +x /usr/local/bin/carplay-audio
    echo "  Deployed: /usr/local/bin/carplay-audio"
else
    echo "  WARNING: $BINARY not found — build it first with ./build.sh"
    echo "  The service will be configured but won't start until the binary is deployed."
fi

# ---------------------------------------------------------------------------
# 5. carplay-audio service
# ---------------------------------------------------------------------------
echo "[5/5] Enabling carplay-audio service..."
sudo cp "$SCRIPT_DIR/carplay-audio.service" /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable carplay-audio
if [ -f /usr/local/bin/carplay-audio ]; then
    sudo systemctl restart carplay-audio
fi

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
echo ""
echo "========================================"
echo "  Setup complete"
echo "========================================"
echo ""
echo "Services:"
echo "  shairport-sync   — AirPlay receiver (BMW E46)"
echo "  bluealsa         — Bluetooth A2DP sink"
echo "  bluealsa-aplay   — BT audio → ALSA loopback"
echo "  bt-agent         — Auto-pairing agent"
echo "  bluetooth-init   — BT power-on + alias at boot"
echo "  carplay-audio    — DSP daemon (reads loopback, outputs to DAC)"
echo ""
echo "AirPlay  : connect to 'BMW E46' from any AirPlay device"
echo "Bluetooth: pair with 'BMW E46' — automatic, no PIN"
echo ""
echo "Diagnostics:"
echo "  sudo systemctl status shairport-sync bluealsa bluealsa-aplay carplay-audio"
echo "  bluetoothctl show"
echo "  journalctl -u carplay-audio -f"
