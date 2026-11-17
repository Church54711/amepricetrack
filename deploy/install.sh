#!/usr/bin/env bash
# Installs or updates AmePriceTrack as a background service on Ubuntu.
#
#   Without building anything (downloads the latest release):
#       curl -fsSL https://raw.githubusercontent.com/Church54711/amepricetrack/master/deploy/install.sh | sudo bash
#   From the source, after `cargo build --release`:
#       sudo deploy/install.sh
#
#   Afterwards:
#       sudo amepricetrack-update                 update to the latest release
#       sudo amepricetrack-update --https         give it a secure (HTTPS) address
#
# Data (/var/lib/amepricetrack) and settings (/etc/amepricetrack.env) are kept.
set -euo pipefail

REPO="${APT_REPO:-Church54711/amepricetrack}"
ENV_FILE=/etc/amepricetrack.env
DATA_DIR=/var/lib/amepricetrack
STATUS_FILE=/run/amepricetrack-update.status

usage() {
    sed -n '2,13p' "${BASH_SOURCE[0]:-/dev/null}" 2>/dev/null | sed 's/^# \{0,1\}//'
    cat <<'EOF'

Options:
  --https[=tailscale|caddy]   set up a secure address (asks which when not given)
  --domain=NAME               the domain name to use with --https=caddy
  --local                     only use the program next to this script; never download
EOF
}

https=""
domain=""
requested=0
local_only=0
for arg in "$@"; do
    case "$arg" in
        --https) https=ask ;;
        --https=tailscale | --https=caddy) https="${arg#--https=}" ;;
        --domain=*) domain="${arg#--domain=}" ;;
        --requested) requested=1 ;;
        --local) local_only=1 ;;
        -h | --help) usage; exit 0 ;;
        *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
    esac
done

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this with sudo." >&2
    exit 1
fi

# The website's "Update" button asks for an update by creating a file; this is
# the root half of that. Nothing in the file is read.
note_status() {
    [ "$requested" -eq 1 ] || return 0
    printf 'state=%s\nat=%s\nmessage=%s\n' "$1" "$(date +%s)" "$2" >"$STATUS_FILE.tmp" && mv -f "$STATUS_FILE.tmp" "$STATUS_FILE"
    chmod 644 "$STATUS_FILE" 2>/dev/null || true
}
fail() {
    echo "$1" >&2
    note_status failed "$1"
    exit 1
}
if [ "$requested" -eq 1 ]; then
    rm -f "$DATA_DIR/update-requested"
    note_status running "Downloading the new version"
    trap 'note_status failed "The update stopped unexpectedly. See: journalctl -u amepricetrack-update"' ERR
fi

# ------------------------------------------------------------ what to install

here=""
if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
    here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
fi
binary=""
if [ -n "$here" ]; then
    for candidate in "$here/../amepricetrack" "$here/../target/release/amepricetrack"; do
        if [ -x "$candidate" ] && [ -f "$here/amepricetrack.service" ]; then
            binary=$candidate
            break
        fi
    done
fi

if [ -z "$binary" ]; then
    if [ "$local_only" -eq 1 ] || [ -n "${APT_FETCHED:-}" ]; then
        fail "The program was not found next to this script. Build it first: cargo build --release"
    fi
    case "$(uname -m)" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) fail "There is no ready-made download for this kind of machine ($(uname -m)). Build from the source instead." ;;
    esac
    for tool in curl tar sha256sum; do
        command -v "$tool" >/dev/null || { apt-get update -qq && apt-get install -y -qq curl tar coreutils ca-certificates; break; }
    done
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    name="amepricetrack-linux-$arch.tar.gz"
    # APT_RELEASE_BASE points somewhere else, for trying the installer out.
    base="${APT_RELEASE_BASE:-https://github.com/$REPO/releases/latest/download}"
    echo "Downloading the latest release..."
    curl -fsSL --retry 3 -o "$work/$name" "$base/$name" || fail "Could not download $base/$name"
    curl -fsSL --retry 3 -o "$work/SHA256SUMS" "$base/SHA256SUMS" || fail "Could not download $base/SHA256SUMS"
    (cd "$work" && grep " $name\$" SHA256SUMS | sha256sum -c --status) || fail "The download does not match its checksum; nothing was installed."
    tar -xzf "$work/$name" -C "$work"
    next="$work/amepricetrack-linux-$arch/deploy/install.sh"
    [ -f "$next" ] || fail "The download does not look like an AmePriceTrack release."
    # The release brings its own installer, so an update can change how installing works.
    args=(--local)
    if [ "$requested" -eq 1 ]; then args+=(--requested); fi
    case "$https" in
        "") ;;
        ask) args+=(--https) ;;
        *) args+=("--https=$https") ;;
    esac
    if [ -n "$domain" ]; then args+=("--domain=$domain"); fi
    # From here the release's own installer reports how it went.
    trap - ERR
    set +e
    APT_FETCHED=1 bash "$next" "${args[@]}"
    code=$?
    exit $code
fi

# ------------------------------------------------------------------- settings

# Sets KEY=value in the settings file, replacing a line that already sets it.
set_env() {
    local key=$1 value=$2
    if grep -q "^$key=" "$ENV_FILE"; then
        sed -i "s|^$key=.*|$key=$value|" "$ENV_FILE"
    else
        printf '%s=%s\n' "$key" "$value" >>"$ENV_FILE"
    fi
}
get_env() {
    { grep "^$1=" "$ENV_FILE" 2>/dev/null || true; } | tail -1 | cut -d= -f2-
}

https_tailscale() {
    if ! command -v tailscale >/dev/null; then
        echo "Installing Tailscale..."
        curl -fsSL https://tailscale.com/install.sh | sh
    fi
    if ! tailscale status >/dev/null 2>&1; then
        echo
        echo "Sign this machine in to your Tailscale account (a free one is enough)."
        echo "Open the link below in any browser:"
        tailscale up
    fi
    echo
    echo "Asking Tailscale to serve the website over HTTPS ..."
    echo "(If Tailscale prints a link here, open it to switch HTTPS on for your account, once.)"
    tailscale serve --bg 8787 || fail "Tailscale could not start serving. Its message is above."
    # This machine's name on your Tailscale network, from Tailscale's own description
    # of the machine, or failing that from the address it says it serves.
    local name
    name=$(tailscale status --json --peers=false 2>/dev/null | sed -n 's/.*"DNSName": *"\([^"]*[^".]\)\.\{0,1\}".*/\1/p' | head -1)
    if [ -z "$name" ]; then
        name=$(tailscale serve status 2>/dev/null | sed -n 's|.*https://\([A-Za-z0-9.-]*\).*|\1|p' | head -1)
    fi
    case "$name" in
        "" | *[!A-Za-z0-9.-]*)
            fail "Tailscale is serving, but did not say at what address. Run 'tailscale serve status', then set APT_PUBLIC_URL in $ENV_FILE to that address."
            ;;
    esac
    set_env APT_PUBLIC_URL "https://$name"
    set_env APT_SECURE_COOKIES 1
    # Only Tailscale reaches the program now; the plain address on the home network is closed.
    set_env APT_BIND 127.0.0.1:8787
    secure_address="https://$name"
    secure_note="Reachable from any device signed in to your Tailscale account (install the Tailscale app on the phone)."
}

https_caddy() {
    if [ -z "$domain" ] && [ -r /dev/tty ]; then
        read -r -p "Domain name that points at this machine (for example prices.example.com): " domain </dev/tty || true
    fi
    case "$domain" in
        "" | *[!A-Za-z0-9.-]* | .* | *. ) fail "Give the domain name: --https=caddy --domain=prices.example.com" ;;
    esac
    if ! command -v caddy >/dev/null; then
        echo "Installing Caddy..."
        apt-get update -qq
        apt-get install -y -qq caddy
    fi
    local file=/etc/caddy/Caddyfile
    local block
    block=$(printf '%s {\n\treverse_proxy 127.0.0.1:8787\n}\n' "$domain")
    mkdir -p /etc/caddy
    if [ -f "$file" ] && grep -q "^$domain " "$file"; then
        :
    elif [ ! -f "$file" ] || grep -q '^:80 {' "$file"; then
        # Nothing of the owner's is in the file yet (missing, or the placeholder page Caddy ships with).
        if [ -f "$file" ] && [ ! -e "$file.before-amepricetrack" ]; then cp "$file" "$file.before-amepricetrack"; fi
        printf '%s\n' "$block" >"$file"
    else
        printf '\n%s\n' "$block" >>"$file"
    fi
    systemctl enable caddy >/dev/null 2>&1 || true
    systemctl reload caddy 2>/dev/null || systemctl restart caddy || fail "Caddy did not start. See: journalctl -u caddy"
    set_env APT_PUBLIC_URL "https://$domain"
    set_env APT_SECURE_COOKIES 1
    set_env APT_BIND 127.0.0.1:8787
    secure_address="https://$domain"
    secure_note="Caddy gets the certificate by itself once $domain points at this machine and ports 80 and 443 reach it."
}

# -------------------------------------------------------------------- install

version=$(timeout 10 "$binary" version 2>/dev/null || echo "(unknown version)")
fresh=0
[ -f /usr/local/bin/amepricetrack ] || fresh=1

if ! command -v Xvfb >/dev/null; then
    echo "Installing the virtual screen (Xvfb)..."
    apt-get update -qq
    apt-get install -y -qq xvfb ca-certificates wget
fi

# Staples, Best Buy, Kohl's and Harbor Freight only answer a real browser.
if ! command -v google-chrome-stable >/dev/null && ! command -v chromium >/dev/null && ! command -v chromium-browser >/dev/null; then
    if [ "$(dpkg --print-architecture)" = "amd64" ]; then
        echo "Installing Google Chrome..."
        command -v wget >/dev/null || apt-get install -y -qq wget
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
    useradd --system --home-dir "$DATA_DIR" --create-home --shell /usr/sbin/nologin amepricetrack
fi
install -d -o amepricetrack -g amepricetrack -m 750 "$DATA_DIR"

if [ ! -f "$ENV_FILE" ]; then
    install -m 640 -o root -g amepricetrack "$here/amepricetrack.env.example" "$ENV_FILE"
fi

if [ "$fresh" -eq 0 ] && cmp -s "$binary" /usr/local/bin/amepricetrack; then
    echo "AmePriceTrack $version is already installed."
    changed=0
else
    # Stop the old version (if any) so its program file can be replaced.
    systemctl stop amepricetrack 2>/dev/null || true
    install -m 755 "$binary" /usr/local/bin/amepricetrack
    changed=1
fi
install -m 644 "$here/amepricetrack.service" /etc/systemd/system/amepricetrack.service
install -m 644 "$here/amepricetrack-update.service" /etc/systemd/system/amepricetrack-update.service
install -m 644 "$here/amepricetrack-update.path" /etc/systemd/system/amepricetrack-update.path
install -m 755 "$here/install.sh" /usr/local/sbin/amepricetrack-update

secure_address=""
secure_note=""
if [ "$https" = ask ] || { [ -z "$https" ] && [ "$fresh" -eq 1 ] && [ "$requested" -eq 0 ] && [ -t 1 ] && [ -r /dev/tty ]; }; then
    echo
    echo "A secure (HTTPS) address keeps passwords private and lets the phone app reach"
    echo "this server from anywhere."
    echo "  1) Tailscale: private to your own devices, nothing to open on the router (recommended)"
    echo "  2) Caddy: public, with a domain name you own"
    echo "  3) Not now"
    choice=3
    read -r -p "Choose 1, 2 or 3 [3]: " choice </dev/tty || true
    case "$choice" in
        1) https=tailscale ;;
        2) https=caddy ;;
        *) https="" ;;
    esac
fi
case "$https" in
    tailscale) https_tailscale ;;
    caddy) https_caddy ;;
esac

systemctl daemon-reload
systemctl enable --now amepricetrack-update.path >/dev/null 2>&1 || true
systemctl enable amepricetrack >/dev/null 2>&1
if [ "$changed" -eq 1 ] || [ -n "$https" ]; then
    systemctl restart amepricetrack
else
    systemctl start amepricetrack
fi

note_status ok "Version $version is installed"
trap - ERR

echo
if [ "$changed" -eq 1 ] && [ "$fresh" -eq 0 ]; then
    echo "AmePriceTrack is updated to $version."
elif [ "$fresh" -eq 1 ]; then
    echo "AmePriceTrack $version is installed and running."
fi
if [ -n "$secure_address" ]; then
    echo "Secure address: $secure_address"
    echo "$secure_note"
    echo "Use this address in the phone app. The old http:// address no longer works."
elif [ "$fresh" -eq 1 ]; then
    address=$(hostname -I 2>/dev/null | awk '{print $1}')
    echo "Open http://${address:-<this machine>}:8787 and create the first account."
    echo "For a secure address later: sudo amepricetrack-update --https"
else
    public=$(get_env APT_PUBLIC_URL)
    if [ -n "$public" ]; then echo "Address: $public"; fi
fi
echo "Settings: $ENV_FILE    Data: $DATA_DIR    Log: journalctl -u amepricetrack -f"
