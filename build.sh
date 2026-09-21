#!/bin/sh
# Both halves. The panel has to sit next to the binary that launches it.
set -e
cd "$(dirname "$0")"
cargo build --release
swiftc -O -o target/release/slingr-panel panel/main.swift
echo "built: target/release/slingr  +  target/release/slingr-panel"
