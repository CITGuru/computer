#!/usr/bin/env bash
#
# Every input this box accepts, and the only path that carries it.
#
# **Wayland has no `xdotool`.** Synthetic input is a compositor privilege and
# has to arrive as a *device*: the pointer through `computer-pointer` on the
# virtual-pointer protocol, the keyboard through `wtype` on the
# virtual-keyboard one. Neither needs `/dev/uinput`, so the box keeps the
# isolation it was started with.
#
# **Not sway's `seat cursor` commands.** They move the seat's own pointer, and
# a headless backend has no input devices for the seat to own — so sway accepts
# every one of them, exits zero, and the screen does not move.
#
# **This is also the part that confines rather than coordinates.** The gate
# inside the SDK is a promise: the owner stops sending input because it agreed
# to, and a shell, an `exec`, or another program in the box is not stopped by
# an agreement it never made. This is the only way in, so every caller meets
# it. A takeover records its token beside the screen, and a caller holding that
# token passes it in COMPUTER_TOKEN; anything else is refused with status 3.
#
# Reads are not here at all. Where the pointer is and how big the screen is
# tell a run what a person is doing to the screen, which is the whole reason
# the gate withholds input and not observation.
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
  # **`-s` before anything else.** `wtype` makes a virtual keyboard, uploads a
  # keymap and starts typing; the first keystroke goes out before the
  # compositor has applied the keymap and is dropped, so `KEYBOARD` arrives as
  # `EYBOARD`. The pause is what the new device needs to become real.
  #
  # `--` next, or text starting with a dash is read as a flag and the failure
  # is a refusal the caller cannot diagnose.
  type)
    said=$(wtype -s 120 -- "$@" 2>&1) || true
    ;;
  # The modifiers and the key are already worked out; this only carries them.
  key)
    said=$(wtype -s 120 "$@" 2>&1) || true
    ;;
  *)
    echo "usage: computer-input move|click|dblclick|drag|scroll|type|key ..." >&2
    exit 2
    ;;
esac

# **`wtype` exits zero whatever happens** — a bad flag, no compositor, a
# keystroke that never left. What it says is the only signal there is, so
# anything on its output is turned into the failure it already was.
if [ -n "${said:-}" ]; then
  echo "$said" >&2
  exit 1
fi
