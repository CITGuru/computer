#!/usr/bin/env bash
# On the PATH, so a shell or an `exec` meets the gate the SDK only promises.
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
