#!/usr/bin/env bash
#
# Screen 0, then idle. Extra screens are started on demand by `computer-screen`,
# because eight X servers nobody asked for is eight X servers' worth of memory.
set -uo pipefail

# Screen 0 and the DevTools bridge, and nothing else.
#
# DevTools reachable from outside: chromium binds the debugging port to
# loopback whatever --remote-debugging-address says, so a published 9222
# forwards to nothing and the caller gets an empty reply that reads as a
# browser with no DevTools. This listens where the runtime can reach it and
# hands the connection on. It is screen 0's browser: the second screen's
# chromium cannot have 9222 because the first one holds it.
boot() {
  mkdir -p /tmp/computer "${HOME:-/home/computer}"
  computer-screen start 0 || return 1

  socat TCP-LISTEN:9223,fork,reuseaddr TCP:127.0.0.1:9222 \
    >/tmp/computer/devtools-bridge.log 2>&1 &
}

# `--once` brings the screen up and returns.
#
# **A microVM outlives the call that started it; a container does not.** So a
# container needs the supervisor below to hold it open, and a microVM needs
# nothing after the screen is up — running the idle loop there would hold an
# exec open for the life of the machine.
if [ "${1:-}" = "--once" ]; then
  boot || exit 1
  exit 0
fi

boot || exit 1

# The container is a place, not a command. Work arrives through exec, and this
# holds the box open until something disposes of it. If screen 0 dies the
# script exits, so a box that looks healthy is one with a screen in it.
while xdpyinfo -display :1 >/dev/null 2>&1; do
  sleep 5
done

echo "screen 0 went away" >&2
exit 1
