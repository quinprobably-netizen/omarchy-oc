#!/bin/bash
# Stops the daemon (every setting goes back to stock) and removes system files.
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "must run as root" >&2; exit 1; }
systemctl disable --now omarchy-ocd.service 2>/dev/null || true
/usr/local/bin/omarchy-ocd restore 2>/dev/null || true
rm -f /usr/local/bin/omarchy-ocd /usr/local/bin/omarchy-oc /usr/local/bin/omarchy-oc-gpustress \
  /etc/systemd/system/omarchy-ocd.service
rm -rf /usr/local/share/doc/omarchy-oc
systemctl daemon-reload
echo "Removed. Profiles and test history remain in /var/lib/omarchy-oc (delete by hand if wanted)."
