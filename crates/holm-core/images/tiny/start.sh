#!/usr/bin/env bash
set -uo pipefail

# Before anything draws a window: a program joins the tree only if the bus was up first.
# A fixed address, because `dbus-launch` picks a path no later `exec` can learn.
a11y_bus() {
  [ -x /usr/libexec/at-spi-bus-launcher ] || return 0

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

  if [ -n "${COMPUTER_DEVTOOLS_SECRET:-}" ] && command -v computer-devtools-bridge >/dev/null; then
    computer-devtools-bridge >/tmp/computer/devtools-bridge.log 2>&1 &
  else
    socat TCP-LISTEN:9223,fork,reuseaddr TCP:127.0.0.1:9222 \
      >/tmp/computer/devtools-bridge.log 2>&1 &
  fi
}

# `--once` for a microVM, which outlives the call that started it.
if [ "${1:-}" = "--once" ]; then
  boot || exit 1
  exit 0
fi

boot || exit 1

# Exiting when screen 0 dies is what makes a healthy-looking box one with a screen.
while xdpyinfo -display :1 >/dev/null 2>&1; do
  sleep 5
done

echo "screen 0 went away" >&2
exit 1
