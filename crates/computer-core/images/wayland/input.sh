#!/usr/bin/env bash
# Input arrives as devices: the pointer via `computer-pointer`, keys via `wtype`.
# Not sway's `seat cursor`: on a headless backend it exits zero and moves nothing.
# On the path, so a shell or an `exec` meets the takeover gate too.
set -uo pipefail

verb="${1:?usage: computer-input move|click|dblclick|drag|path|sweep|scroll|down|up|type|paced|key ...}"
shift

screen=$(( ${WAYLAND_DISPLAY#wayland-} - 1 ))
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
  move|click|dblclick|drag|path|sweep|scroll|down|up)
    computer-pointer "$verb" "$@" || exit $?
    ;;
  # `-s` first: the first key races the keymap, so `KEYBOARD` arrives as `EYBOARD`.
  # `--` next, or text starting with a dash is read as a flag.
  type)
    said=$(wtype -s 120 -- "$@" 2>&1) || true
    ;;
  paced)
    pause="${1:?usage: computer-input paced MS TEXT}"
    shift
    said=$(wtype -s 120 -d "$pause" -- "$@" 2>&1) || true
    ;;
  key)
    said=$(wtype -s 120 "$@" 2>&1) || true
    ;;
  *)
    echo "usage: computer-input move|click|dblclick|drag|path|sweep|scroll|down|up|type|paced|key ..." >&2
    exit 2
    ;;
esac

# `wtype` exits zero whatever happens; its output is the only signal.
if [ -n "${said:-}" ]; then
  echo "$said" >&2
  exit 1
fi
