#!/usr/bin/env bash
# setup-bluetooth.sh -- First-time Bluetooth setup for the Raspberry Pi.
# Run once after deploying the project.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Installing dependencies..."
sudo apt-get update -q
sudo apt-get install -y bluez bluez-alsa-utils bluez-tools

echo "Configuring ALSA loopback (2 substreams)..."
# snd_aloop needs at least 2 substreams: substream 0 for AirPlay, 1 for Bluetooth.
sudo tee /etc/modprobe.d/snd-aloop.conf > /dev/null << 'EOF'
options snd_aloop pcm_substreams=2
EOF
sudo tee /etc/modules-load.d/snd-aloop.conf > /dev/null << 'EOF'
snd_aloop
EOF
sudo modprobe -r snd_aloop 2>/dev/null || true
sudo modprobe snd_aloop pcm_substreams=2

echo "Deploying ALSA config..."
sudo cp "$SCRIPT_DIR/asound.conf" /etc/asound.conf

echo "Deploying BlueZ config..."
sudo cp "$SCRIPT_DIR/bluetooth-main.conf" /etc/bluetooth/main.conf

echo "Configuring bluetooth.service (rfkill unblock before start)..."
sudo mkdir -p /etc/systemd/system/bluetooth.service.d
sudo tee /etc/systemd/system/bluetooth.service.d/rfkill.conf > /dev/null << 'EOF'
[Service]
ExecStartPre=/usr/sbin/rfkill unblock bluetooth
EOF

echo "Configuring bluealsa (A2DP sink + HFP-AG)..."
sudo mkdir -p /etc/systemd/system/bluealsa.service.d
sudo tee /etc/systemd/system/bluealsa.service.d/override.conf > /dev/null << 'EOF'
[Service]
ExecStart=
ExecStart=/usr/bin/bluealsa --profile=a2dp-sink --profile=hfp-ag
Restart=always
RestartSec=3
StartLimitIntervalSec=0
TimeoutStopSec=5
EOF

echo "Enabling systemd services..."
sudo cp "$SCRIPT_DIR/bluealsa-aplay.service"   /etc/systemd/system/
sudo cp "$SCRIPT_DIR/bt-agent.service"         /etc/systemd/system/
sudo cp "$SCRIPT_DIR/bluetooth-init.service"   /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable bluealsa bluealsa-aplay bt-agent bluetooth-init

echo "Starting services..."
sudo systemctl restart bluetooth
sudo systemctl start bluetooth-init
sudo systemctl restart bluealsa bluealsa-aplay bt-agent

echo ""
echo "Bluetooth configured."
echo ""
echo "First pairing:"
echo "  1. Enable Bluetooth on your phone"
echo "  2. Search for 'BMW E46' in the Bluetooth list"
echo "  3. Pairing is automatic (no PIN required)"
echo "  Subsequent connections happen automatically."
echo ""
echo "Diagnostics:"
echo "  bluetoothctl show"
echo "  journalctl -u bluetooth-init -f"
echo "  journalctl -u bluealsa-aplay -f"
echo "  bluetoothctl devices"
echo "  cat /proc/asound/Loopback/pcm1p/sub0/hw_params"
