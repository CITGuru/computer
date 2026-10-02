#!/usr/bin/env bash
# Run after the window manager: fluxbox's `fbsetbg` covers anything set earlier.
set -uo pipefail

width="${COMPUTER_SCREEN_WIDTH:-1280}"
height="${COMPUTER_SCREEN_HEIGHT:-800}"
out="${1:-/tmp/computer/wallpaper.jpg}"

mkdir -p "$(dirname "$out")"

command -v convert >/dev/null 2>&1 || exit 0

# Muted mid-tones: the dock samples what is behind it.
# JPEG, because the noise that breaks 8-bit banding makes a PNG 5 MB a screen.
convert -size "${width}x${height}" \
  gradient:'#41506b-#7d6f78' \
  \( -size "${width}x${height}" plasma:fractal -blur 0x30 -modulate 100,28 \) \
  -compose overlay -composite \
  -modulate 100,62 \
  -attenuate 0.15 +noise Gaussian \
  -quality 92 "$out" 2>/dev/null || exit 0

# `hsetroot` publishes `_XROOTPMAP_ID`, which the dock needs to draw its corners.
if command -v hsetroot >/dev/null 2>&1; then
  hsetroot -fill "$out" >/dev/null 2>&1 || true
else
  display -window root "$out" >/dev/null 2>&1 || true
fi
