#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(pwd)"
BIN="$ROOT/bin/linux-x86_64/awe-node"
URL="https://github.com/ARARAT33/AWEP2P/releases/download/latest/AWEP2P-Linux-x64-Portable.tar.gz"
if [[ ! -x "$BIN" ]]; then
  echo "[AWEp2P] Downloading latest ready-to-run Linux package..."
  mkdir -p "$ROOT/bin/linux-x86_64"
  tmp="$(mktemp)"
  trap 'rm -f "$tmp"' EXIT
  curl -fL "$URL" -o "$tmp"
  tar -xzf "$tmp" -C "$ROOT"
  chmod +x "$BIN"
fi
exec "$BIN" "$@"
