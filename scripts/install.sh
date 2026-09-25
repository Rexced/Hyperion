#!/usr/bin/env bash
# Builds Hyperion from source and installs it for the current user:
#   ~/.local/bin/hyperion
#   ~/.local/share/applications/hyperion.desktop
# Then offers the two optional sensor permissions that need root, once each.
#
# Run it as your normal user (not with sudo): ./scripts/install.sh
# Set PREFIX to install elsewhere, e.g. PREFIX=/opt/hyperion ./scripts/install.sh
set -euo pipefail

cd "$(dirname "$0")/.."

PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/hyperion"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Default is "no": anything but y/yes declines.
ask() {
    local reply
    read -r -p "$1 [y/N] " reply || return 1
    [[ "$reply" =~ ^[Yy]([Ee][Ss])?$ ]]
}

# --- Checks -----------------------------------------------------------------

[[ "$(uname -s)" == Linux ]] || die "this installer is for Linux"
[[ "$EUID" -ne 0 ]] || die "run this as your normal user; it asks for sudo itself when needed"
command -v cargo >/dev/null || die "Rust is not installed; get it from https://rustup.rs"

# Distro Rust can be too old (Debian 12 ships 1.63); compare against Cargo.toml.
need=$(sed -n 's/^rust-version *= *"\([0-9.]*\)"/\1/p' Cargo.toml)
have=$(cargo --version | awk '{print $2}')
if [[ -n "$need" ]] && [[ "$(printf '%s\n%s\n' "$need" "$have" | sort -V | head -n1)" != "$need" ]]; then
    die "Rust $have is too old, Hyperion needs $need or newer. Install it from https://rustup.rs"
fi

# --- Build and install ------------------------------------------------------

say "Building Hyperion (release)..."
cargo build --release --locked

say "Installing to $BIN"
install -Dm755 target/release/hyperion "$BIN"

mkdir -p "$APPS"
cat >"$APPS/hyperion.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Hyperion
GenericName=System Monitor
Comment=Live CPU, GPU, memory, storage and network usage
Exec=$BIN
Icon=utilities-system-monitor
Terminal=false
Categories=System;Monitor;
Keywords=cpu;gpu;ram;disk;network;monitor;
StartupWMClass=hyperion
EOF
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APPS" || true

# --- Optional root-only sensors ---------------------------------------------

grant=()

rapl=/sys/class/powercap/intel-rapl:0/energy_uj
if [[ -e "$rapl" && ! -r "$rapl" ]]; then
    echo
    say "CPU power draw (optional)"
    cat <<'EOF'
Hyperion can show how many watts your CPU is using. Linux keeps that counter
root-only since 2020: very precise power readings were used in an attack
(PLATYPUS) to guess secret keys. Saying yes adds a udev rule that lets only
your user's group read it (never everyone), and it stays after reboots.
You can also turn this on later from Settings.
EOF
    if ask "Allow Hyperion to read CPU power?"; then grant+=(cpu-power); fi
fi

# SATA drives (their sysfs path goes through an ata port; USB drives don't).
has_sata=false
for dev in /sys/block/sd*; do
    if [[ "$(readlink -f "$dev")" == */ata[0-9]* ]]; then has_sata=true; fi
done
if $has_sata && [[ ! -d /sys/module/drivetemp ]]; then
    echo
    say "SATA drive temperatures (optional)"
    cat <<'EOF'
Linux's "drivetemp" module reports SATA drive temperatures live. Saying yes
loads it now and at every boot. Without it Hyperion falls back to SMART data
from UDisks, which updates about once a minute. NVMe drives don't need this.
EOF
    if ask "Enable live SATA temperatures?"; then grant+=(drivetemp); fi
fi

if ((${#grant[@]})); then
    echo
    say "Asking for sudo once to apply: ${grant[*]}"
    if ! sudo "$BIN" --grant-sensors "${grant[@]}"; then
        echo "Could not apply the sensor permissions; Hyperion works without them." >&2
    fi
fi

# --- Done -------------------------------------------------------------------

echo
say "Hyperion is installed. Start it from your app launcher or run: hyperion"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "Note: $PREFIX/bin is not on your PATH, so run it as $BIN or add that folder to PATH." ;;
esac
