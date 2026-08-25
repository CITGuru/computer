#!/usr/bin/env bash
#
# `xdotool`, refused while a person is driving the screen.
#
# On the PATH rather than in the SDK, because the in-process gate is a promise
# and a shell or an `exec` never made it. This is the part that confines.
#
# Shadows the real binary at /usr/bin/xdotool. A takeover records its token
# beside the screen; a caller holding it passes COMPUTER_TOKEN, and anything
# else is refused with status 3 while the file exists.
#
# Reads stay allowed: withholding input and not observation is the point.
set -uo pipefail

real=/usr/bin/xdotool
screen=$(( ${DISPLAY#:} - 1 ))
token_file="/tmp/computer/screen-${screen}.control"

case "${1:-}" in
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
