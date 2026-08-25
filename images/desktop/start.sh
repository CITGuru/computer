#!/usr/bin/env bash
#
# Screen 0, then idle. Extra screens are started on demand by `computer-screen`,
# because eight X servers nobody asked for is eight X servers' worth of memory.
set -uo pipefail

# The bridge is how DevTools is reachable at all: chromium binds the debugging
# port to loopback whatever --remote-debugging-address says, so a published
# 9222 forwards to nothing and reads as a browser without DevTools. Screen 0's
# browser only — the second screen's chromium cannot have 9222.
boot() {
  mkdir -p /tmp/computer "${HOME:-/home/computer}"
  computer-screen start 0 || return 1

  socat TCP-LISTEN:9223,fork,reuseaddr TCP:127.0.0.1:9222 \
    >/tmp/computer/devtools-bridge.log 2>&1 &
}

# `--once` for a microVM, which outlives the call that started it. A container
# does not, and needs the loop below to hold it open.
if [ "${1:-}" = "--once" ]; then
  boot || exit 1
  exit 0
fi

boot || exit 1

# The container is a place, not a command: work arrives through exec. Exiting
# when screen 0 dies is what makes a healthy-looking box one with a screen.
while xdpyinfo -display :1 >/dev/null 2>&1; do
  sleep 5
done

echo "screen 0 went away" >&2
exit 1
