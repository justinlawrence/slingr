#!/bin/sh
# Every part. The panel and the Spaces helper have to sit next to the binary
# that launches them, and the agent next to the binary it launches.
set -e
cd "$(dirname "$0")"
cargo build --release
swiftc -O -o target/release/slingr-panel panel/main.swift
swiftc -O -o target/release/slingr-spaces spaces/main.swift

# The agent is an app bundle so macOS has something stable to grant Screen
# Recording to. A permission is tied to the signature, so an ad-hoc signature
# loses it on every rebuild; a real identity keeps it. SLINGR_SIGN overrides.
app=target/release/SlingrAgent.app
mkdir -p "$app/Contents/MacOS"
swiftc -O -o "$app/Contents/MacOS/slingr-agent" agent/main.swift
cp agent/Info.plist "$app/Contents/Info.plist"
identity="${SLINGR_SIGN:-$(security find-identity -v -p codesigning 2>/dev/null \
  | sed -n 's/.*"\(Apple Development[^"]*\)".*/\1/p' | head -1)}"
codesign --force --sign "${identity:--}" "$app" 2>/dev/null
echo "built: target/release/slingr  +  slingr-panel  +  slingr-spaces  +  SlingrAgent.app (signed: ${identity:-ad hoc})"
