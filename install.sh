#!/bin/sh
# Build, put `slingr` on PATH, and wire up the two hooks.
#
# Nothing here runs resident: AeroSpace and herdr invoke slingr themselves.
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
  echo "herdr plugin linked (tab.focused -> slingr goto)"
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
EOF
