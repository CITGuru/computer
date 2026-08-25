#!/usr/bin/env bash
#
# Start or stop one screen's whole stack: compositor, browser, viewer. Screen N
# is `wayland-N+1` under its own runtime directory — one compositor per screen,
# because a Wayland compositor is the display server and the window manager at
# once.
#
# The verbs, the ports and the token protocol are the X11 image's. Only what is
# under them differs, so the crate drives both through the same commands.
set -uo pipefail

action="${1:?usage: computer-screen start|stop|control|release|open|viewers <screen> [url]}"
screen="${2:?usage: computer-screen start|stop|control|release|open|viewers <screen> [url]}"
url="${3:-}"

number=$((screen + 1))
view_port=$((6080 + screen * 2))
control_port=$((6081 + screen * 2))
view_vnc=$((5900 + screen * 2))
control_vnc=$((5901 + screen * 2))

width="${COMPUTER_SCREEN_WIDTH:-1280}"
height="${COMPUTER_SCREEN_HEIGHT:-800}"

# A runtime directory per screen, not per box: a Wayland socket lives in one,
# and two compositors sharing a directory would each claim `wayland-1`.
runtime="/tmp/computer/run-${number}"
# The same name on every screen. A socket is a file, unique only inside its
# directory, and a compositor takes the first free name there — so the
# directory is what tells screens apart. A number in the name would point
# every screen after the first at nothing.
wayland_display="wayland-1"
sockfile="/tmp/computer/screen-${screen}.sway"
control_token="/tmp/computer/screen-${screen}.control"
profile="${HOME:-/home/computer}/.browser-profiles/screen-${number}"
logs="/tmp/computer/screen-${number}"

export XDG_RUNTIME_DIR="$runtime"
export WAYLAND_DISPLAY="$wayland_display"

await() {
  # Bounded, because a compositor that has not answered in ten seconds is not
  # slow — it is broken, and waiting longer only delays the report.
  local deadline=$((SECONDS + 10))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.1
  done
  return 1
}

listening() {
  bash -c "echo > /dev/tcp/127.0.0.1/$1" 2>/dev/null
}

# Whether this screen's compositor answers now.
#
# Asked of sway rather than read off a socket file: a compositor that died
# leaves the file behind, and every check that reads configuration passes while
# the first screenshot fails.
alive() {
  local sock
  sock=$(cat "$sockfile" 2>/dev/null) || return 1
  [ -n "$sock" ] || return 1
  swaymsg -s "$sock" -t get_version >/dev/null 2>&1
}

# How many browsers are on a viewer, not whether the server is up: websockify
# holds its connection to wayvnc only while a client is attached, so an
# established one is a person looking. From /proc, because `ss` and `netstat`
# are packages this image would otherwise not need.
established() {
  local hex
  hex=$(printf "%04X" "$1")
  awk -v p="$hex" '$4=="01" && $2 ~ ":"p"$" {n++} END {print n+0}' \
    /proc/net/tcp /proc/net/tcp6 2>/dev/null
}

viewers() {
  echo "watching=$(established "$view_vnc") driving=$(established "$control_vnc")"
}

start() {
  if alive; then
    exit 0
  fi

  mkdir -p /tmp/computer "$runtime" "$profile"
  chmod 700 "$runtime"
  rm -f "$sockfile"

  # The geometry and the socket path go into the configuration, because sway
  # reads no environment in it.
  sed -e "s/%WIDTH%/${width}/" \
      -e "s/%HEIGHT%/${height}/" \
      -e "s|%SOCKFILE%|${sockfile}|" \
      /etc/computer/sway.config > "${runtime}/sway.config"

  # Headless, and told there are no input devices: sway on a real backend
  # refuses to start without a seat, and there is no seat in a box.
  WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    sway --config "${runtime}/sway.config" >"${logs}-sway.log" 2>&1 &

  await alive || { echo "no compositor in ${runtime}" >&2; exit 1; }

  # sway picks the name rather than taking it from the environment, and every
  # later command connects through it — caught here instead of as a capture
  # that returns no image.
  await test -S "${runtime}/${wayland_display}" \
    || { echo "the compositor is not on ${wayland_display}" >&2; exit 1; }

  # One profile per screen. A shared profile makes one screen's login every
  # screen's, and the singleton lock stops the second launch outright.
  computer-browser --user-data-dir="$profile" >"${logs}-browser.log" 2>&1 &

  # `-d` is the read-only guarantee: a viewer told to be read-only by its own
  # page stops being read-only when somebody opens the page differently.
  wayvnc -d 127.0.0.1 "$view_vnc" >"${logs}-vnc.log" 2>&1 &
  websockify --web=/usr/share/novnc "0.0.0.0:${view_port}" "127.0.0.1:${view_vnc}" \
    >"${logs}-novnc.log" 2>&1 &

  await listening "${view_port}" \
    || { echo "viewer never came up on ${view_port}" >&2; exit 1; }
}

stop() {
  local sock
  sock=$(cat "$sockfile" 2>/dev/null || true)
  [ -n "$sock" ] && swaymsg -s "$sock" exit >/dev/null 2>&1

  pkill -f -- "--user-data-dir=${profile}" || true
  pkill -f "wayvnc .* ${view_vnc}$" || true
  pkill -f "wayvnc .* ${control_vnc}$" || true
  pkill -f "websockify.*${view_port}" || true
  pkill -f "websockify.*${control_port}" || true
  rm -f "$sockfile" "$control_token"
}

control() {
  # The token makes a takeover endable by whoever started it and nobody else.
  # Kept here rather than in the caller: a caller that exits takes its memory
  # with it, and the next one would have no way to learn somebody is driving.
  token="${3:-}"
  mode="${4:-exclusive}"
  [ -n "$token" ] || { echo "usage: computer-screen control <screen> <token> [shared]" >&2; exit 2; }

  alive || { echo "screen ${screen} is not running" >&2; exit 1; }

  # Already open, so the server stays and only the token changes hands.
  # Recorded even here: skipping the write would leave the replaced holder's
  # token in the file, letting them end the takeover that replaced them.
  if listening "${control_port}"; then
    record_token
    exit 0
  fi

  # No `-d`, so this one accepts input. A second server on a second port, never
  # a mode switch on the one somebody is already watching.
  wayvnc 0.0.0.0 "$control_vnc" >"${logs}-vnc-control.log" 2>&1 &
  websockify --web=/usr/share/novnc "0.0.0.0:${control_port}" "127.0.0.1:${control_vnc}" \
    >"${logs}-novnc-control.log" 2>&1 &

  await listening "${control_port}" \
    || { echo "control viewer never came up on ${control_port}" >&2; exit 1; }

  record_token
}

# **Only an exclusive takeover writes the token.** The file is what the input
# guard refuses on, and a shared session is one where both sides are meant to
# drive: recording a token there would lock out the owner it was sharing with.
record_token() {
  if [ "$mode" = "shared" ]; then
    rm -f "$control_token"
  else
    printf '%s' "$token" > "$control_token"
  fi
}

release() {
  # A stale release is refused rather than obeyed. A takeover that was replaced
  # must not be endable by whoever it replaced: that would take the keyboard
  # from the person driving now, and neither of them would be told why.
  # `--force` is the deliberate way past it.
  want="${3:-}"
  held=$(cat "$control_token" 2>/dev/null || true)

  if [ -n "$held" ] && [ "$want" != "--force" ] && [ "$want" != "$held" ]; then
    echo "the takeover on screen ${screen} was replaced" >&2
    exit 3
  fi

  # Only the control pair. The read-only viewer stays up, so whoever was
  # watching keeps watching.
  pkill -f "wayvnc .* ${control_vnc}$" || true
  pkill -f "websockify.*${control_port}" || true
  rm -f "$control_token"
}

open_url() {
  [ -n "$url" ] || { echo "usage: computer-screen open <screen> <url>" >&2; exit 2; }
  alive || { echo "screen ${screen} is not running" >&2; exit 1; }

  # The same profile as the running browser, so this joins that window rather
  # than fighting it for the singleton lock.
  computer-browser --user-data-dir="$profile" "$url" >>"${logs}-browser.log" 2>&1 &
}

case "$action" in
  start)   start ;;
  viewers) viewers ;;
  stop)    stop ;;
  control) control "$@" ;;
  release) release "$@" ;;
  open)    open_url ;;
  *) echo "usage: computer-screen start|stop|control|release|open|viewers <screen> [url]" >&2; exit 2 ;;
esac
