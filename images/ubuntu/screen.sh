#!/usr/bin/env bash
#
# Start or stop one screen's whole stack: X server, window manager, browser,
# viewer. Screen N is display :N+1 — never :0, which is a real console on a
# real host.
#
# The control viewer is a second server on a second port, started only when
# somebody asks for it: a person already watching the read-only stream cannot
# be handed the input without changing where they are connected.
set -uo pipefail

action="${1:?usage: computer-screen start|stop|control|release|open|viewers <screen> [url]}"
screen="${2:?usage: computer-screen start|stop|control|release|open|viewers <screen> [url]}"
url="${3:-}"

display=":$((screen + 1))"
number=$((screen + 1))
view_port=$((6080 + screen * 2))
control_port=$((6081 + screen * 2))
view_vnc=$((5900 + screen * 2))
control_vnc=$((5901 + screen * 2))

width="${COMPUTER_SCREEN_WIDTH:-1280}"
height="${COMPUTER_SCREEN_HEIGHT:-800}"

control_token="/tmp/computer/screen-${screen}.control"
# One daemon for the box, one sink per screen: PulseAudio is a singleton per
# user, so a second daemon refuses to start and that screen gets no sound card.
pulse_home="/tmp/computer/pulse"
pulse_socket="/tmp/computer/pulse.socket"
wm_home="/tmp/computer-wm-${number}"
profile="${HOME:-/home/computer}/.browser-profiles/screen-${number}"
logs="/tmp/computer/screen-${number}"

# The gate in front of both viewers, as `docs/viewer-auth.md` describes it.
#
# `open` is what a box on loopback has always been; the crate refuses to publish
# an open viewer beyond loopback, so anything reachable arrives here gated.
viewer_auth="${COMPUTER_VIEWER_AUTH:-open}"
gate_dir="/tmp/computer/gate"

# The websockify arguments for one door, left in `gate_args`.
#
# `token` drops the positional target: websockify reads it from the token file
# instead, which is also what keeps the secret out of `ps` in here. BasicHTTPAuth
# has no equivalent and takes its source on the command line.
build_gate() {
  local door="$1" target="$2" secret file
  gate_args=("$target")

  if [ "$viewer_auth" = "open" ]; then return 0; fi

  case "$door" in
    view) secret="${COMPUTER_VIEW_SECRET:-}" ;;
    control) secret="${COMPUTER_CONTROL_SECRET:-}" ;;
  esac

  # An empty secret would start a viewer that accepts everybody while the crate
  # believes it is gated. Refused rather than defaulted: this is the failure the
  # whole design exists to prevent.
  if [ -z "$secret" ]; then
    echo "viewer auth is ${viewer_auth} but the ${door} secret is unset" >&2
    return 1
  fi

  case "$viewer_auth" in
    token)
      mkdir -p "$gate_dir"
      file="${gate_dir}/${door}-${screen}"
      # The file is the credential, so it is unreadable to anybody else before
      # websockify is ever pointed at it.
      (umask 077; printf '%s: %s\n' "$secret" "$target" >"$file")
      gate_args=(--token-plugin TokenFile --token-source "$file")
      ;;
    password)
      gate_args=(--auth-plugin BasicHTTPAuth
        --auth-source "computer:${secret}" --web-auth "$target")
      ;;
    *)
      echo "unknown viewer auth: ${viewer_auth}" >&2
      return 1
      ;;
  esac
}

await() {
  # Bounded, because a display that has not answered in ten seconds is not
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

# How many browsers are on a viewer, not whether the server is up: websockify
# holds its connection to x11vnc only while a client is attached, so an
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
  if xdpyinfo -display "$display" >/dev/null 2>&1; then
    exit 0
  fi

  mkdir -p /tmp/computer "$wm_home/.fluxbox" /tmp/.X11-unix "$profile"
  rm -f "/tmp/.X${number}-lock" "/tmp/.X11-unix/X${number}"

  Xvfb "$display" -screen 0 "${width}x${height}x24" -ac +extension RANDR +render -noreset \
    >"${logs}-xvfb.log" 2>&1 &
  await xdpyinfo -display "$display" || { echo "no X server on $display" >&2; exit 1; }

  # **Copied, not pointed at.** fluxbox rewrites its own apps file when a
  # window is remembered, and /etc is read-only to the user the box runs as —
  # so a config it cannot write is one it complains about on every start.
  cp /etc/computer/fluxbox/init "$wm_home/.fluxbox/init"
  cp /etc/computer/fluxbox/menu "$wm_home/.fluxbox/menu"
  cp /etc/computer/fluxbox/apps "$wm_home/.fluxbox/apps"
  cp /etc/computer/fluxbox/style "$wm_home/.fluxbox/style"
  HOME="$wm_home" DISPLAY="$display" fluxbox -rc "$wm_home/.fluxbox/init" \
    >"${logs}-wm.log" 2>&1 &

  # After the window manager, never before — see `wallpaper.sh`.
  DISPLAY="$display" computer-wallpaper "$wm_home/wallpaper.jpg" \
    >>"${logs}-wm.log" 2>&1 || true

  # Opt-in through `Extras::dock`: a box driven by a program wants the pixels,
  # a box a person looks at wants the desk.
  if command -v tint2 >/dev/null 2>&1; then
    DISPLAY="$display" tint2 -c /etc/computer/tint2rc >"${logs}-dock.log" 2>&1 &
  fi

  # One profile per screen. A shared profile makes one screen's login every
  # screen's, and the singleton lock stops the second launch outright.
  DISPLAY="$display" computer-browser --user-data-dir="$profile" >"${logs}-browser.log" 2>&1 &

  # The sink goes nowhere — nothing plays out of a box, and a recorder can
  # still capture it.
  #
  # The socket is named rather than defaulted: PulseAudio puts it under
  # whichever runtime directory the caller happens to have, so a daemon here
  # and a client from an exec look in two places and the client reports
  # "connection refused" beside a running daemon.
  if command -v pulseaudio >/dev/null 2>&1; then
    mkdir -p "$pulse_home"

    if [ ! -S "$pulse_socket" ]; then
      XDG_RUNTIME_DIR="$pulse_home" HOME="$pulse_home" \
        pulseaudio --daemonize=yes --exit-idle-time=-1 --disallow-exit -n \
          --load="module-native-protocol-unix auth-anonymous=1 socket=${pulse_socket}" \
          >"${logs}-audio.log" 2>&1 || true

      await test -S "$pulse_socket" || echo "no sound card" >>"${logs}-audio.log"
    fi

    # One sink for this screen, and its monitor is what a recorder listens to.
    pactl -s "unix:${pulse_socket}" load-module module-null-sink \
      sink_name="screen${number}" \
      sink_properties="device.description=screen${number}" \
      >>"${logs}-audio.log" 2>&1 || true
  fi

  x11vnc -display "$display" -forever -shared -viewonly -nopw \
    -listen 127.0.0.1 -rfbport "$view_vnc" -xkb -ncache 0 >"${logs}-vnc.log" 2>&1 &
  build_gate view "127.0.0.1:${view_vnc}" || exit 1
  websockify --web=/usr/share/novnc "0.0.0.0:${view_port}" "${gate_args[@]}" \
    >"${logs}-novnc.log" 2>&1 &

  await listening "${view_port}" \
    || { echo "viewer never came up on ${view_port}" >&2; exit 1; }
}

stop() {
  pkill -f "Xvfb ${display} -screen" || true
  # Matched on argv, not on the environment: `HOME=` is a shell assignment
  # consumed before exec, so it never reaches /proc/PID/cmdline and a
  # pattern built from it matches nothing.
  pkill -f "fluxbox -rc ${wm_home}/.fluxbox/init" || true
  pkill -f -- "--user-data-dir=${profile}" || true
  pkill -f "tint2 -c /etc/computer/tint2rc" || true
  pkill -f "^x11vnc .* -rfbport ${view_vnc}" || true
  pkill -f "^x11vnc .* -rfbport ${control_vnc}" || true
  pkill -f "websockify.*${view_port}" || true
  pkill -f "websockify.*${control_port}" || true
  # The sink goes; the daemon stays, because the other screens are using it.
  if [ -S "$pulse_socket" ]; then
    pactl -s "unix:${pulse_socket}" unload-module module-null-sink 2>/dev/null | true
  fi
  rm -f "/tmp/.X${number}-lock" "/tmp/.X11-unix/X${number}"
}

control() {
  # The token makes a takeover endable by whoever started it and nobody else.
  # Kept here rather than in the caller: a caller that exits takes its memory
  # with it, and the next one would have no way to learn somebody is driving.
  token="${3:-}"
  mode="${4:-exclusive}"
  [ -n "$token" ] || { echo "usage: computer-screen control <screen> <token> [shared]" >&2; exit 2; }

  xdpyinfo -display "$display" >/dev/null 2>&1 \
    || { echo "screen ${screen} is not running" >&2; exit 1; }

  # Already open, so the server is shared and only the token changes hands.
  # Recorded even here: skipping the write would leave the replaced holder's
  # token in the file, letting them end the takeover that replaced them.
  if listening "${control_port}"; then
    record_token
    exit 0
  fi

  mkdir -p /tmp/computer
  x11vnc -display "$display" -forever -shared -nopw \
    -listen 0.0.0.0 -rfbport "$control_vnc" -xkb -ncache 0 >"${logs}-vnc-control.log" 2>&1 &
  build_gate control "127.0.0.1:${control_vnc}" || exit 1
  websockify --web=/usr/share/novnc "0.0.0.0:${control_port}" "${gate_args[@]}" \
    >"${logs}-novnc-control.log" 2>&1 &

  await listening "${control_port}" \
    || { echo "control viewer never came up on ${control_port}" >&2; exit 1; }

  record_token
}

# Only an exclusive takeover writes the token. The file is what the input guard
# refuses on, and a shared session means both sides drive — a token there would
# lock out the owner it was sharing with.
record_token() {
  if [ "$mode" = "shared" ]; then
    rm -f "$control_token"
  else
    printf '%s' "$token" > "$control_token"
  fi
}

release() {
  # A replaced takeover must not be endable by whoever it replaced: that takes
  # the keyboard from the person driving now, and tells neither of them why.
  # `--force` is the deliberate way past.
  want="${3:-}"
  held=$(cat "$control_token" 2>/dev/null || true)

  if [ -n "$held" ] && [ "$want" != "--force" ] && [ "$want" != "$held" ]; then
    echo "the takeover on screen ${screen} was replaced" >&2
    exit 3
  fi

  # Only the control pair. The read-only viewer stays up, so whoever was
  # watching keeps watching.
  pkill -f "^x11vnc .* -rfbport ${control_vnc}" || true
  pkill -f "websockify.*${control_port}" || true
  rm -f "$control_token"
}

open_url() {
  [ -n "$url" ] || { echo "usage: computer-screen open <screen> <url>" >&2; exit 2; }
  xdpyinfo -display "$display" >/dev/null 2>&1 \
    || { echo "screen ${screen} is not running" >&2; exit 1; }

  # The same profile as the running browser, so this joins that window rather
  # than fighting it for the singleton lock.
  DISPLAY="$display" computer-browser --user-data-dir="$profile" "$url" \
    >>"${logs}-browser.log" 2>&1 &
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
