#!/bin/sh
#
# Chromium, with the flags a box actually needs.
set -eu

BIN="$(command -v chromium || command -v chromium-browser || true)"
[ -n "$BIN" ] || { echo "chromium is not installed" >&2; exit 1; }

# The screen, derived from the display. The crate passes a profile; a browser
# started from the dock passes nothing and would land in chromium's default —
# different cookies, no DevTools port, and nothing for `computer-screen stop`
# to match. The same derivation `computer-screen` does.
case " $* " in
  *" --user-data-dir="* | *" --user-data-dir "*) ;;
  *)
    set -- "--user-data-dir=${HOME:-/home/computer}/.browser-profiles/screen-${DISPLAY#:}" "$@"
    ;;
esac

# --no-sandbox: the box is the isolation, and chromium's own needs privileges
#   the box does not have.
# --disable-dev-shm-usage, --disable-gpu: both fail without a real display.
# --no-first-run, --no-default-browser-check: a dialogue is an obstacle a
#   caller driving from screenshots cannot recognise.
# --test-type: drops the "unsupported command-line flag" banner --no-sandbox
#   raises, which covers the top of the page and shifts every coordinate under
#   it.
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
