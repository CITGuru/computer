#!/usr/bin/env bash
#
# Screen 0, then idle. Extra screens are started on demand by `computer-screen`,
# because eight X servers nobody asked for is eight X servers' worth of memory.
set -uo pipefail

# The bridge is how DevTools is reachable at all: chromium binds the debugging
# port to loopback whatever --remote-debugging-address says, so a published
# 9222 forwards to nothing and reads as a browser without DevTools. Screen 0's
# browser only — the second screen's chromium cannot have 9222.

# The accessibility bus, before anything draws a window.
#
# A program joins the tree only if the bridge was there when it came up, so a
# box that starts its browser first and its bus second publishes nothing for
# the life of the box.
#
# The address is fixed and set in the image rather than chosen here, because
# `dbus-launch` picks a random path and every later `exec` arrives in its own
# environment with no way to learn it. Nothing answers on it unless the image
# was built with Feature::Accessibility, which is what installs the launcher;
# a GTK program given an address nothing answers on carries on in silence.
a11y_bus() {
  [ -x /usr/libexec/at-spi-bus-launcher ] || return 0

  # Defaulted rather than read bare: `set -u` turns an unset variable into an
  # exit, and this runs before screen 0 does.
  local address="${DBUS_SESSION_BUS_ADDRESS:-}"
  [ -n "$address" ] || {
    echo "no bus address in the environment, so nothing could find the tree" >&2
    return 0
  }

  rm -f "${address#unix:path=}"

  dbus-daemon --session --address="$address" --fork --nopidfile || {
    echo "the accessibility bus would not start" >&2
    return 0
  }

  /usr/libexec/at-spi-bus-launcher --launch-immediately \
    >/tmp/computer/a11y.log 2>&1 &
}

boot() {
  mkdir -p /tmp/computer "${HOME:-/home/computer}"
  a11y_bus
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
