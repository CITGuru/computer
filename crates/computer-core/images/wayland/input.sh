#!/usr/bin/env bash
# Input arrives as devices, a pointer and a keyboard, both made by `computer-pointer`.
# Not sway's `seat cursor`: on a headless backend it exits zero and moves nothing.
# On the path, so a shell or an `exec` meets the takeover gate too.
set -uo pipefail

verb="${1:?usage: computer-input move|click|dblclick|drag|path|sweep|scroll|down|up|type|paced|press|with ...}"
shift

runtime="${XDG_RUNTIME_DIR:-}"
number="${runtime##*/run-}"
case "$number" in
  ''|*[!0-9]*)
    echo "XDG_RUNTIME_DIR is '${runtime}', which is no screen's runtime directory" >&2
    exit 2
    ;;
esac
screen=$(( number - 1 ))
sockfile="/tmp/computer/screen-${screen}.sway"
token_file="/tmp/computer/screen-${screen}.control"

if [ -s "$token_file" ]; then
  held=$(cat "$token_file" 2>/dev/null || true)
  if [ "${COMPUTER_TOKEN:-}" != "$held" ]; then
    echo "a person is driving screen ${screen}; observe, do not act" >&2
    exit 3
  fi
fi

[ -s "$sockfile" ] || { echo "screen ${screen} is not running" >&2; exit 1; }

case "$verb" in
  move|click|dblclick|drag|path|sweep|scroll|down|up|type|paced|press|with)
    computer-pointer "$verb" "$@" || exit $?
    ;;
  *)
    echo "usage: computer-input move|click|dblclick|drag|path|sweep|scroll|down|up|type|paced|press|with ..." >&2
    exit 2
    ;;
esac
