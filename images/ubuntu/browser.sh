#!/bin/sh
#
# Chromium, with the flags a box actually needs.
set -eu

BIN="$(command -v chromium || command -v chromium-browser || true)"
[ -n "$BIN" ] || { echo "chromium is not installed" >&2; exit 1; }

# --no-sandbox: the box is the isolation, and chromium's own sandbox needs
#   privileges the box does not have.
# --disable-dev-shm-usage, --disable-gpu: both fail without a real display.
# --no-first-run, --no-default-browser-check: a dialogue on screen is an
#   obstacle a caller driving from screenshots has no way to recognise.
# --test-type: removes the "unsupported command-line flag" banner that
#   --no-sandbox raises. The banner covers the top of the page, so every
#   coordinate below it is one a caller worked out from a shifted screenshot.
exec "$BIN" \
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
