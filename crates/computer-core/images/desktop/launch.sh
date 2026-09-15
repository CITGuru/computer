#!/usr/bin/env bash
set -uo pipefail

new=0
if [ "${1:-}" = "--new" ]; then
  new=1
  shift
fi

class="${1:?usage: computer-launch [--new] <window-class> <command> [args...]}"
shift

if [ "$new" -eq 0 ]; then
  # The real xdotool: this is the person's own click, not a program's.
  existing=$(/usr/bin/xdotool search --class "$class" 2>/dev/null | tail -1)
  if [ -n "$existing" ]; then
    /usr/bin/xdotool windowactivate "$existing" 2>/dev/null && exit 0
  fi
fi

exec "$@"
