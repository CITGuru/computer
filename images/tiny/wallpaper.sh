#!/usr/bin/env bash
#
# ImageMagick rather than a wallpaper setter: `import` is already here for
# screenshots, and `xsetroot` can only do a flat colour — which is what a
# failed capture looks like.
#
# Run after the window manager. fluxbox paints the root through `fbsetbg` when
# it starts and covers anything set earlier.
set -uo pipefail

width="${COMPUTER_SCREEN_WIDTH:-1280}"
height="${COMPUTER_SCREEN_HEIGHT:-800}"
out="${1:-/tmp/computer/wallpaper.jpg}"

mkdir -p "$(dirname "$out")"

command -v convert >/dev/null 2>&1 || exit 0

# Muted mid-tones, because the dock is a pale panel that samples what is behind
# it: near-black leaves it a shapeless grey slab, saturated stains it.
#
# The noise breaks the banding a slow dark gradient shows at 8 bits. JPEG
# because that noise makes PNG incompressible — 5 MB a screen.
convert -size "${width}x${height}" \
  gradient:'#41506b-#7d6f78' \
  \( -size "${width}x${height}" plasma:fractal -blur 0x30 -modulate 100,28 \) \
  -compose overlay -composite \
  -modulate 100,62 \
  -attenuate 0.15 +noise Gaussian \
  -quality 92 "$out" 2>/dev/null || exit 0

# `hsetroot` publishes `_XROOTPMAP_ID` and ImageMagick's `display -window root`
# does not. Without that atom the dock has nothing to sample behind it and its
# rounded corners fall back to black.
if command -v hsetroot >/dev/null 2>&1; then
  hsetroot -fill "$out" >/dev/null 2>&1 || true
else
  display -window root "$out" >/dev/null 2>&1 || true
fi
