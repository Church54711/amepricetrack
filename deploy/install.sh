#!/usr/bin/env bash
# Installs or updates AmePriceTrack as a background service on Ubuntu.
#
# Run from the repository root after building:
#     cargo build --release
#     sudo deploy/install.sh
#
# Running it again after a new build updates the program and keeps all data
# (/var/lib/amepricetrack) and settings (/etc/amepricetrack.env).
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this with sudo: sudo deploy/install.sh" >&2
    exit 1
fi

binary=target/release/amepricetrack
if [ ! -x "$binary" ]; then
    echo "The program has not been built yet. Run 'cargo build --release' first (as your normal user)." >&2
    exit 1
fi

echo "Installing the virtual screen (Xvfb)..."
apt-get update -qq
apt-get install -y -qq xvfb ca-certificates wget

# Staples, Best Buy, Kohl's and Harbor Freight only answer a real browser.
if ! command -v google-chrome-stable >/dev/null && ! command -v chromium >/dev/null && ! command -v chromium-browser >/dev/null; then
    if [ "$(dpkg --print-architecture)" = "amd64" ]; then
        echo "Installing Google Chrome..."
        wget -q -O /tmp/google-chrome.deb https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb
        apt-get install -y -qq /tmp/google-chrome.deb
        rm -f /tmp/google-chrome.deb
    else
        echo "Google Chrome is only published for 64-bit Intel/AMD machines."
        echo "Install Chromium yourself (sudo apt install chromium-browser) and restart the service;"
        echo "until then Walmart, Target and Amazon work and the other stores do not."
    fi
fi

if ! id amepricetrack >/dev/null 2>&1; then
    useradd --system --home-dir /var/lib/amepricetrack --create-home --shell /usr/sbin/nologin amepricetrack
fi
install -d -o amepricetrack -g amepricetrack -m 750 /var/lib/amepricetrack

# Stop the old version (if any) so its program file can be replaced.
systemctl stop amepricetrack 2>/dev/null || true
install -m 755 "$binary" /usr/local/bin/amepricetrack
if [ ! -f /etc/amepricetrack.env ]; then
    install -m 640 -o root -g amepricetrack deploy/amepricetrack.env.example /etc/amepricetrack.env
fi
install -m 644 deploy/amepricetrack.service /etc/systemd/system/amepricetrack.service

systemctl daemon-reload
systemctl enable --now amepricetrack

address=$(hostname -I 2>/dev/null | awk '{print $1}')
echo
echo "AmePriceTrack is running. Open http://${address:-<this machine>}:8787 and create the first account."
echo "Settings: /etc/amepricetrack.env    Data: /var/lib/amepricetrack    Log: journalctl -u amepricetrack -f"
