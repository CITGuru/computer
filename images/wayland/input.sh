#!/usr/bin/env bash
#
# Every input this box accepts, and the only path that carries it.
#
# Wayland has no `xdotool`: synthetic input is a compositor privilege and has
# to arrive as a *device* — the pointer through `computer-pointer` on the
# virtual-pointer protocol, the keyboard through `wtype` on the
# virtual-keyboard one. Neither needs `/dev/uinput`.
#
# Not sway's `seat cursor` commands. They move the seat's own pointer, and a
# headless backend has no devices for the seat to own, so sway accepts every
# one of them, exits zero, and the screen does not move.
#
# This is also the part that confines: the in-process gate is a promise, and a
# shell or an `exec` never made it. A takeover records its token beside the
# screen; a caller holding it passes COMPUTER_TOKEN, anything else gets 3.
#
# Reads are not here at all — withholding input and not observation is
# the point.
set -uo pipefail

verb="${1:?usage: computer-input move|click|dblclick|drag|scroll|type|key ...}"
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
  move|click|dblclick|drag|scroll)
    computer-pointer "$verb" "$@"
    ;;
  # `-s` first: `wtype` makes a virtual keyboard, uploads a keymap and starts
  # typing, and the first keystroke goes out before the compositor has applied
  # the keymap — `KEYBOARD` arrives as `EYBOARD`.
  #
  # `--` next, or text starting with a dash is read as a flag and the failure
  # is a refusal the caller cannot diagnose.
  type)
    said=$(wtype -s 120 -- "$@" 2>&1) || true
    ;;
  key)
    said=$(wtype -s 120 "$@" 2>&1) || true
    ;;
  *)
    echo "usage: computer-input move|click|dblclick|drag|scroll|type|key ..." >&2
    exit 2
    ;;
esac

# `wtype` exits zero whatever happens — a bad flag, no compositor, a keystroke
# that never left. Its output is the only signal there is.
if [ -n "${said:-}" ]; then
  echo "$said" >&2
  exit 1
fi
