#!/usr/bin/env bash
#
# Bring an application forward, or start it.
#
# tint2 launchers only ever execute, so the focusing is done here: a dock icon
# clicked for something already open should return to it rather than hand back
# a second copy. `--new` skips the lookup, which is what a "new window" wants.
set -uo pipefail

new=0
if [ "${1:-}" = "--new" ]; then
  new=1
  shift
fi

class="${1:?usage: computer-launch [--new] <window-class> <command> [args...]}"
shift

if [ "$new" -eq 0 ]; then
  # The real xdotool, not the guard: the guard stops a *program* driving a
  # screen someone has taken over, and this is that person's own click.
  existing=$(/usr/bin/xdotool search --class "$class" 2>/dev/null | tail -1)
  if [ -n "$existing" ]; then
    /usr/bin/xdotool windowactivate "$existing" 2>/dev/null && exit 0
  fi
fi

exec "$@"
