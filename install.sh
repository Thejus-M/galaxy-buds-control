#!/usr/bin/env bash
# Build and install the daemon, the CLI/TUI, the systemd user service and the
# Omarchy bar widget.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
plugin_src="$(dirname "$repo")/omarchy-plugins/home.buds"
plugin_link="$HOME/.config/omarchy/plugins/home.buds"
unit_dir="$HOME/.config/systemd/user"

echo "==> building release binaries"
cargo install --path "$repo" --root "$HOME/.local" --force

echo "==> installing the user service"
mkdir -p "$unit_dir"
# Copied rather than symlinked: the unit must still resolve when the external
# drive is not mounted, otherwise the service fails to start at login.
install -m 0644 "$repo/systemd/galaxybuds.service" "$unit_dir/galaxybuds.service"
systemctl --user daemon-reload
systemctl --user enable --now galaxybuds.service

if [[ -d $plugin_src ]]; then
  echo "==> installing the bar widget"
  ln -sfn "$plugin_src" "$plugin_link"
  omarchy plugin validate "$plugin_src"
  omarchy-shell shell rescanPlugins || true
  omarchy plugin enable home.buds right || true
fi

echo "==> done"
systemctl --user --no-pager status galaxybuds.service | head -5
