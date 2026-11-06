#!/bin/bash
# Root half of the installer. Run via install.sh (sudo or pkexec).
set -euo pipefail
src="$(cd "$(dirname "$0")/.." && pwd)"
bin="${CARGO_TARGET_DIR:-$src/target}/release"
gbin="$src/gpustress/target/release"

[[ $EUID -eq 0 ]] || { echo "install-system.sh must run as root" >&2; exit 1; }

install -Dm755 "$bin/omarchy-ocd" /usr/local/bin/omarchy-ocd
install -Dm755 "$bin/omarchy-oc" /usr/local/bin/omarchy-oc
[[ -x $gbin/omarchy-oc-gpustress ]] && install -Dm755 "$gbin/omarchy-oc-gpustress" /usr/local/bin/omarchy-oc-gpustress
install -Dm644 "$src/system/omarchy-ocd.service" /etc/systemd/system/omarchy-ocd.service
install -Dm644 "$src/README.md" /usr/local/share/doc/omarchy-oc/README.md 2>/dev/null || true

# DDR5 SPD hub driver: gives DIMM temperatures where the board exposes the
# SMBus (harmless where it does not).
modprobe spd5118 2>/dev/null || true

systemctl daemon-reload
systemctl enable omarchy-ocd.service
systemctl restart omarchy-ocd.service   # picks up a new binary on reinstall
sleep 2
systemctl --no-pager --lines=8 status omarchy-ocd.service || true
