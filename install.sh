#!/bin/bash
# omarchy-oc installer: builds the Rust binaries, installs the daemon
# (needs root once), and adds the Overclock widget to the Omarchy bar.
#
#   ./install.sh            build + install everything (retires the qqq.fans widget)
#   ./install.sh --plugin   only (re)install the shell plugin
#   ./install.sh --uninstall
set -euo pipefail
src="$(cd "$(dirname "$0")" && pwd)"
plugin_dir="$HOME/.config/omarchy/plugins/qqq.oc"

as_root() {
  if [[ $EUID -eq 0 ]]; then "$@"
  elif [[ -t 0 ]]; then sudo "$@"
  else pkexec "$@"
  fi
}

install_plugin() {
  [[ -L $plugin_dir ]] && rm "$plugin_dir"
  mkdir -p "$plugin_dir"
  # Copy then rename: the shell's hot-reload watcher reacts to new files
  # landing in the directory, not to in-place overwrites.
  while IFS= read -r -d '' f; do
    rel="${f#"$src/plugin/"}"
    mkdir -p "$plugin_dir/$(dirname "$rel")"
    cp "$f" "$plugin_dir/$(dirname "$rel")/.$(basename "$rel").new"
    mv -f "$plugin_dir/$(dirname "$rel")/.$(basename "$rel").new" "$plugin_dir/$rel"
  done < <(find "$src/plugin" -type f ! -path "*/tests/*" -print0)
  omarchy plugin validate "$plugin_dir"
  # A first install is a new directory the running shell has not scanned yet.
  omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true
  if ! omarchy plugin list --json | jq -e '.[] | select(.id == "qqq.oc" and .enabled)' >/dev/null; then
    omarchy plugin enable qqq.oc --section right
  fi
  echo "Overclock widget installed in the bar (run 'omarchy restart shell' to load new QML)."
}

case "${1:-}" in
  "") ;;
  -h|--help) sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  --plugin) install_plugin; exit 0 ;;
  --uninstall)
    as_root "$src/system/uninstall-system.sh"
    omarchy plugin disable qqq.oc 2>/dev/null || true
    rm -rf "$plugin_dir"
    exit 0 ;;
  *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
esac

cargo build --release --manifest-path "$src/Cargo.toml"
cargo build --release --manifest-path "$src/gpustress/Cargo.toml"
as_root "$src/system/install-system.sh"
# The Overclock panel absorbs the Fans widget's job; only the full install
# retires it, so a plugin-only reinstall keeps it until the Fans page exists.
omarchy plugin disable qqq.fans 2>/dev/null || true
install_plugin
cat <<MSG

Done. Open the Overclock widget in the bar, or run: omarchy-oc probe
MSG
