#!/bin/sh
set -eu

BIN="$(command -v chromium || command -v chromium-browser || true)"
[ -n "$BIN" ] || { echo "chromium is not installed" >&2; exit 1; }

# Derived from the display, so a browser started from the dock gets the screen's profile.
case " $* " in
  *" --user-data-dir="* | *" --user-data-dir "*) ;;
  *)
    set -- "--user-data-dir=${HOME:-/home/computer}/.browser-profiles/screen-${DISPLAY#:}" "$@"
    ;;
esac

# --no-sandbox: the box is the isolation.
# --disable-dev-shm-usage, --disable-gpu: both fail without a real display.
# --no-first-run, --no-default-browser-check: a dialogue blocks a screenshot-driven caller.
# --test-type: hides the --no-sandbox banner, which shifts every coordinate.
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
