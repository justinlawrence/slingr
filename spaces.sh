#!/bin/sh
# Try macOS Spaces in place of AeroSpace, and go back. See docs/SPACES.md.
#
#   ./spaces.sh on    build here; stop AeroSpace; point slingr, herdr's hook and
#                     ~/.local/bin/slingr at this checkout; start the agent
#   ./spaces.sh off   every one of those undone, back to the main checkout
#   ./spaces.sh shortcuts
#                     only give Desktops 4-16 the Dock shortcuts the agent
#                     presses, which `on` does too; `off` leaves them
set -e
cd "$(dirname "$0")"
root="$(pwd -P)"
# The main checkout is the one `off` hands everything back to; git always
# lists it first.
main="$(git worktree list --porcelain | sed -n '1s/^worktree //p')"
label=dev.slingr.agent
plist="$HOME/Library/LaunchAgents/$label.plist"
config="$HOME/.config/slingr/workspaces.toml"
domain="gui/$(id -u)"

# A top-level key has to come before the first [table], so it goes on the
# first line, and any earlier one is taken out.
use_backend() {
  tmp="$(mktemp)"
  { echo "backend = \"$1\""; grep -v '^backend *=' "$config" || true; } > "$tmp"
  mv "$tmp" "$config"
}

# The agent switches desktops by pressing the Dock's own "Switch to Desktop N"
# (docs/SPACES.md), so every desktop a task can have needs one. Symbolic
# hotkeys 118-133 are Desktops 1-16. Any already switched on is left as it is;
# the rest become ctrl-F4 to ctrl-F16, after the ctrl-F1 to F3 macOS starts
# with here. activateSettings applies them without logging out.
dock_shortcuts() {
  off="$(defaults export com.apple.symbolichotkeys - | python3 -I -c '
import plistlib, sys
keys = plistlib.loads(sys.stdin.buffer.read()).get("AppleSymbolicHotKeys", {})
print(" ".join(str(i) for i in range(121, 134) if not keys.get(str(i), {}).get("enabled")))')"
  for id in $off; do
    # F4 to F16, as virtual key codes.
    case "$id" in
      121) key=118 ;; 122) key=96 ;; 123) key=97 ;; 124) key=98 ;; 125) key=100 ;;
      126) key=101 ;; 127) key=109 ;; 128) key=103 ;; 129) key=111 ;; 130) key=105 ;;
      131) key=107 ;; 132) key=113 ;; 133) key=106 ;;
    esac
    # 8650752 is control plus the function-key flag, as F1 to F3 are stored.
    defaults write com.apple.symbolichotkeys AppleSymbolicHotKeys -dict-add "$id" \
      "<dict><key>enabled</key><true/><key>value</key><dict><key>parameters</key><array><integer>65535</integer><integer>$key</integer><integer>8650752</integer></array><key>type</key><string>standard</string></dict></dict>"
  done
  /System/Library/PrivateFrameworks/SystemAdministration.framework/Resources/activateSettings -u
  echo "Switch to Desktop 4-16: ctrl-F4 to ctrl-F16, wherever not already set (${off:-none needed})."
}

# herdr wants an absolute path, so the manifest is generated, as install.sh does.
link_herdr() {
  sed "s|SLINGR_BINARY|$1/target/release/slingr|" \
    "$1/plugin/herdr-plugin.toml.example" > "$1/plugin/herdr-plugin.toml"
  herdr plugin unlink slingr >/dev/null 2>&1 || true
  herdr plugin link "$1/plugin" >/dev/null
}

case "$1" in
on)
  ./build.sh
  osascript -e 'quit app "AeroSpace"' >/dev/null 2>&1 || true
  use_backend spaces
  link_herdr "$root"
  ln -sf "$root/target/release/slingr" "$HOME/.local/bin/slingr"
  mkdir -p "$HOME/.local/state/slingr" "$HOME/Library/LaunchAgents"
  sed -e "s|SLINGR_AGENT|$root/target/release/SlingrAgent.app/Contents/MacOS/slingr-agent|" \
      -e "s|SLINGR_LOG|$HOME/.local/state/slingr/agent.log|" \
      agent/dev.slingr.agent.plist.example > "$plist"
  launchctl bootout "$domain/$label" 2>/dev/null || true
  launchctl bootstrap "$domain" "$plist"
  dock_shortcuts
  cat <<EOF

Spaces on. AeroSpace is stopped, and the agent owns ctrl-alt-cmd-s/i/b.
  - Add desktops in Mission Control: a task claims an empty one on its first
    visit, so make as many as you have tasks on the go.
  - Allow Screen Recording for "slingr agent" when asked, or the board shows
    application names where titles should be.
  - slingr spaces     every desktop and the task it is for
  - slingr bind NAME  make the desktop you are on a task
  - ./spaces.sh off   back to AeroSpace
EOF
  ;;
off)
  launchctl bootout "$domain/$label" 2>/dev/null || true
  rm -f "$plist"
  use_backend aerospace
  link_herdr "$main"
  ln -sf "$main/target/release/slingr" "$HOME/.local/bin/slingr"
  open -a AeroSpace
  echo "AeroSpace back, and slingr pointed at $main again."
  ;;
shortcuts)
  dock_shortcuts
  ;;
*)
  echo "usage: $0 on|off|shortcuts" >&2
  exit 2
  ;;
esac
