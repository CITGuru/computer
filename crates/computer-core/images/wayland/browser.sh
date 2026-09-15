#!/bin/sh
set -eu

BIN="$(command -v chromium || command -v chromium-browser || true)"
[ -n "$BIN" ] || { echo "chromium is not installed" >&2; exit 1; }

# --ozone-platform=wayland: without it chromium finds no display and exits.
# --no-sandbox: the box is the isolation.
# --test-type: hides the --no-sandbox banner, which shifts every coordinate.
exec "$BIN" \
  --ozone-platform=wayland \
  --enable-features=UseOzonePlatform \
  --no-sandbox \
  --disable-dev-shm-usage \
  --disable-gpu \
  --disable-software-rasterizer \
  --no-first-run \
  --no-default-browser-check \
  --disable-session-crashed-bubble \
  --test-type \
  --disable-infobars \
  --password-store=basic \
  --remote-debugging-port=9222 \
  --remote-debugging-address=0.0.0.0 \
  --start-maximized \
  "$@"
