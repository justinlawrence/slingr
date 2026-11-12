#!/bin/sh
# Build, put `slingr` on PATH, and wire up the two hooks.
#
# This sets up AeroSpace, where nothing runs resident: AeroSpace and herdr
# invoke slingr themselves. For macOS Spaces, run ./spaces.sh on instead.
set -e
cd "$(dirname "$0")"
root="$(pwd -P)"

./build.sh

mkdir -p "$HOME/.local/bin"
ln -sf "$root/target/release/slingr" "$HOME/.local/bin/slingr"
echo "slingr -> $HOME/.local/bin/slingr"

# herdr wants an absolute path to the binary, so the manifest is generated.
if command -v herdr >/dev/null 2>&1; then
  sed "s|SLINGR_BINARY|$root/target/release/slingr|" \
    plugin/herdr-plugin.toml.example > plugin/herdr-plugin.toml
  herdr plugin unlink slingr >/dev/null 2>&1 || true
  herdr plugin link "$root/plugin" >/dev/null
  echo "herdr plugin linked (tab.focused -> slingr goto, pane.agent_status_changed -> slingr notify)"
else
  echo "herdr not found — skipping the plugin"
fi

cat <<EOF

Add to ~/.aerospace.toml, and note that persistent-workspaces needs
config-version = 2:

  config-version = 2
  exec-on-workspace-change = ['$HOME/.local/bin/slingr', 'follow']

  [mode.main.binding]
  ctrl-alt-cmd-s = 'exec-and-forget $HOME/.local/bin/slingr'
  ctrl-alt-cmd-i = 'exec-and-forget $HOME/.local/bin/slingr jump'

Then: aerospace reload-config && slingr sync

Or use macOS Spaces in place of AeroSpace: ./spaces.sh on (docs/SPACES.md).

slingr now says when an agent wants you, in a notification that goes to it
when clicked (needs terminal-notifier: brew install terminal-notifier). Turn
herdr's own off in ~/.config/herdr/config.toml, or each comes twice:

  [ui.toast]
  delivery = "off"
EOF
