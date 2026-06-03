#!/usr/bin/env bash
# setup-bluetooth.sh — Configure le Bluetooth sur le Raspberry Pi
# À lancer une seule fois sur le Pi après avoir copié le projet.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== Dépendances ==="
sudo apt-get update -q
sudo apt-get install -y bluez bluez-alsa-utils bluez-tools

echo "=== Loopback ALSA (2 substreams) ==="
# Le module snd_aloop doit exposer au moins 2 substreams pour AirPlay + BT
sudo tee /etc/modprobe.d/snd-aloop.conf > /dev/null << 'EOF'
options snd_aloop pcm_substreams=2
EOF
sudo tee /etc/modules-load.d/snd-aloop.conf > /dev/null << 'EOF'
snd_aloop
EOF
# Recharge le module avec la nouvelle option
sudo modprobe -r snd_aloop 2>/dev/null || true
sudo modprobe snd_aloop pcm_substreams=2

echo "=== Configuration ALSA ==="
sudo cp "$SCRIPT_DIR/asound.conf" /etc/asound.conf

echo "=== Configuration BlueZ ==="
sudo cp "$SCRIPT_DIR/bluetooth-main.conf" /etc/bluetooth/main.conf

# Override bluealsa pour forcer le profil A2DP sink
sudo mkdir -p /etc/systemd/system/bluealsa.service.d
sudo tee /etc/systemd/system/bluealsa.service.d/override.conf > /dev/null << 'EOF'
[Service]
ExecStart=
ExecStart=/usr/bin/bluealsa --profile=a2dp-sink --profile=hfp-ag
EOF

echo "=== Services systemd ==="
sudo cp "$SCRIPT_DIR/bluealsa-aplay.service" /etc/systemd/system/
sudo cp "$SCRIPT_DIR/bt-agent.service"        /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable bluealsa bluealsa-aplay bt-agent
sudo systemctl restart bluetooth bluealsa bluealsa-aplay bt-agent

echo ""
echo "=== OK — Bluetooth configuré ==="
echo ""
echo "Couplage initial :"
echo "  1. Activez le Bluetooth sur votre téléphone"
echo "  2. Cherchez 'BMW E46' dans la liste des appareils"
echo "  3. Le couplage est automatique (aucun code PIN)"
echo "  Les connexions suivantes se feront automatiquement."
echo ""
echo "Diagnostic :"
echo "  journalctl -u bluealsa-aplay -f   # flux audio BT en direct"
echo "  bluetoothctl devices              # appareils couplés"
echo "  cat /proc/asound/Loopback/pcm1p/sub0/hw_params  # format loopback BT"
