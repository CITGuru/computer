#!/usr/bin/env bash
#
# `xdotool`, refused while a person is driving the screen.
#
# **This is the part that confines rather than coordinates.** The gate inside
# the SDK is a promise: the owner stops sending input because it agreed to, and
# anything that reaches past the API — a shell, an `exec`, another program in
# the box — is not stopped by an agreement it never made. This is on the path
# instead, so every caller meets it.
#
# It shadows the real binary at /usr/bin/xdotool. A takeover records its token
# beside the screen, and a caller that holds the token passes it in
# COMPUTER_TOKEN; anything else is refused with status 3 while the file exists.
#
# Reads are always allowed. Asking where the pointer is, or how big the screen
# is, tells a run what a person is doing to it — which is the whole reason the
# gate withholds input and not observation.
set -uo pipefail

real=/usr/bin/xdotool
screen=$(( ${DISPLAY#:} - 1 ))
token_file="/tmp/computer/screen-${screen}.control"

case "${1:-}" in
  # Input. Everything that moves a pointer or presses a key.
  mousemove|mousemove_relative|click|mousedown|mouseup|key|keydown|keyup|type|windowactivate|windowfocus)
    if [ -s "$token_file" ]; then
      held=$(cat "$token_file" 2>/dev/null || true)
      if [ "${COMPUTER_TOKEN:-}" != "$held" ]; then
        echo "a person is driving screen ${screen}; observe, do not act" >&2
        exit 3
      fi
    fi
    ;;
esac

exec "$real" "$@"
