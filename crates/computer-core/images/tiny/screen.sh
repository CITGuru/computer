#!/usr/bin/env bash
# Screen N is display :N+1, never :0, which is a real console on a real host.
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
# PulseAudio is a singleton per user: one daemon for the box, one sink per screen.
pulse_home="/tmp/computer/pulse"
pulse_socket="/tmp/computer/pulse.socket"
wm_home="/tmp/computer-wm-${number}"
profile="${HOME:-/home/computer}/.browser-profiles/screen-${number}"
running="$(ps -eo args= | grep -oE -- "--user-data-dir=[^ ]*/\.browser-profiles/screen-${number}( |$)" | head -n 1)"
if [ -n "$running" ]; then
  profile="${running#--user-data-dir=}"
  profile="${profile% }"
fi
logs="/tmp/computer/screen-${number}"

viewer_auth="${COMPUTER_VIEWER_AUTH:-open}"
gate_dir="/tmp/computer/gate"

# `token` reads its target from the file, which keeps the secret out of `ps`.
build_gate() {
  local door="$1" target="$2" secret file
  gate_args=("$target")

  if [ "$viewer_auth" = "open" ]; then return 0; fi

  case "$door" in
    view) secret="${COMPUTER_VIEW_SECRET:-}" ;;
    control) secret="${COMPUTER_CONTROL_SECRET:-}" ;;
  esac

  # An empty secret would serve an open viewer the crate believes is gated.
  if [ -z "$secret" ]; then
    echo "viewer auth is ${viewer_auth} but the ${door} secret is unset" >&2
    return 1
  fi

  case "$viewer_auth" in
    token)
      mkdir -p "$gate_dir"
      file="${gate_dir}/${door}-${screen}"
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

closed() {
  ! listening "$1"
}

# websockify holds its x11vnc connection only while a client is attached.
established() {
  local hex
  hex=$(printf "%04X" "$1")
  awk -v p="$hex" '$4=="01" && $2 ~ ":"p"$" {n++} END {print n+0}' \
    /proc/net/tcp /proc/net/tcp6 2>/dev/null
}

viewers() {
  echo "watching=$(established "$view_vnc") driving=$(established "$control_vnc")"
}

reopen_view() {
  local door pid running
  build_gate view "127.0.0.1:${view_vnc}" || return 1

  door="websockify --web=/usr/share/novnc 0.0.0.0:${view_port} "
  pid=$(pgrep -o -f "$door")
  if [ -n "$pid" ]; then
    running=$(tr '\0' ' ' <"/proc/${pid}/cmdline" 2>/dev/null)
    case "$running" in
      *" 0.0.0.0:${view_port} ${gate_args[*]} ") return 0 ;;
    esac
    pkill -f "$door" || true
    await closed "${view_port}" || { echo "the view door on ${view_port} would not close" >&2; return 1; }
  fi

  websockify --web=/usr/share/novnc "0.0.0.0:${view_port}" "${gate_args[@]}" \
    >"${logs}-novnc.log" 2>&1 &
  await listening "${view_port}" \
    || { echo "viewer never came up on ${view_port}" >&2; return 1; }
}

start() {
  if xdpyinfo -display "$display" >/dev/null 2>&1; then
    reopen_view || exit 1
    exit 0
  fi

  mkdir -p /tmp/computer "$wm_home/.fluxbox" /tmp/.X11-unix "$profile"
  rm -f "/tmp/.X${number}-lock" "/tmp/.X11-unix/X${number}"

  Xvfb "$display" -screen 0 "${width}x${height}x24" -ac +extension RANDR +render -noreset \
    >"${logs}-xvfb.log" 2>&1 &
  await xdpyinfo -display "$display" || { echo "no X server on $display" >&2; exit 1; }

  # Copied: fluxbox rewrites its apps file, and /etc is read-only to the box user.
  cp /etc/computer/fluxbox/init "$wm_home/.fluxbox/init"
  cp /etc/computer/fluxbox/menu "$wm_home/.fluxbox/menu"
  cp /etc/computer/fluxbox/apps "$wm_home/.fluxbox/apps"
  cp /etc/computer/fluxbox/style "$wm_home/.fluxbox/style"
  HOME="$wm_home" DISPLAY="$display" fluxbox -rc "$wm_home/.fluxbox/init" \
    >"${logs}-wm.log" 2>&1 &

  # After the window manager, never before — see `wallpaper.sh`.
  DISPLAY="$display" computer-wallpaper "$wm_home/wallpaper.jpg" \
    >>"${logs}-wm.log" 2>&1 || true

  if command -v tint2 >/dev/null 2>&1; then
    apps=""
    for entry in /usr/share/applications/computer-app-*.desktop; do
      [ -e "$entry" ] || continue
      apps="${apps}launcher_item_app = ${entry}
"
    done

    count=$(printf '%s' "$apps" | grep -c launcher_item_app || true)
    width=$(( (count + 2) * 52 + 28 ))

    dock="${wm_home}/tint2rc"
    awk -v apps="$apps" -v width="$width" \
      '{ sub(/^%APPS%$/, apps); sub(/%PANELWIDTH%/, width); print }' \
      /etc/computer/tint2rc > "$dock"

    DISPLAY="$display" tint2 -c "$dock" >"${logs}-dock.log" 2>&1 &
  fi

  # A profile kept in a volume brings back a lock naming a gone container; clear only foreign ones.
  lock="$profile/SingletonLock"
  if [ -L "$lock" ]; then
    case "$(readlink "$lock")" in
      "$(hostname)-"*) ;;
      *) rm -f "$lock" "$profile/SingletonSocket" "$profile/SingletonCookie" ;;
    esac
  fi

  DISPLAY="$display" computer-browser --user-data-dir="$profile" >"${logs}-browser.log" 2>&1 &

  # The socket is named: PulseAudio otherwise puts it under the caller's runtime directory.
  if command -v pulseaudio >/dev/null 2>&1; then
    mkdir -p "$pulse_home"

    if [ ! -S "$pulse_socket" ]; then
      XDG_RUNTIME_DIR="$pulse_home" HOME="$pulse_home" \
        pulseaudio --daemonize=yes --exit-idle-time=-1 --disallow-exit -n \
          --load="module-native-protocol-unix auth-anonymous=1 socket=${pulse_socket}" \
          >"${logs}-audio.log" 2>&1 || true

      await test -S "$pulse_socket" || echo "no sound card" >>"${logs}-audio.log"
    fi

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
  # Matched on argv: `HOME=` is consumed by the shell and never reaches cmdline.
  pkill -f "fluxbox -rc ${wm_home}/.fluxbox/init" || true
  pkill -f -- "--user-data-dir=${profile}" || true
  pkill -f "tint2 -c ${wm_home}/tint2rc" || true
  pkill -f "^x11vnc .* -rfbport ${view_vnc}" || true
  pkill -f "^x11vnc .* -rfbport ${control_vnc}" || true
  pkill -f "websockify.*${view_port}" || true
  pkill -f "websockify.*${control_port}" || true
  # The daemon stays: the other screens use it.
  if [ -S "$pulse_socket" ]; then
    pactl -s "unix:${pulse_socket}" unload-module module-null-sink 2>/dev/null | true
  fi
  rm -f "/tmp/.X${number}-lock" "/tmp/.X11-unix/X${number}"
}

control() {
  # The token lives in the box, so it outlives a caller that exits.
  token="${3:-}"
  mode="${4:-exclusive}"
  [ -n "$token" ] || { echo "usage: computer-screen control <screen> <token> [shared]" >&2; exit 2; }

  xdpyinfo -display "$display" >/dev/null 2>&1 \
    || { echo "screen ${screen} is not running" >&2; exit 1; }

  # Already open: record anyway, or the replaced holder could end this takeover.
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

# A shared session writes no token, or the guard would lock out the owner.
record_token() {
  if [ "$mode" = "shared" ]; then
    rm -f "$control_token"
  else
    printf '%s' "$token" > "$control_token"
  fi
}

release() {
  # A replaced takeover is not endable by whoever it replaced; `--force` is the way past.
  want="${3:-}"
  held=$(cat "$control_token" 2>/dev/null || true)

  if [ -n "$held" ] && [ "$want" != "--force" ] && [ "$want" != "$held" ]; then
    echo "the takeover on screen ${screen} was replaced" >&2
    exit 3
  fi

  pkill -f "^x11vnc .* -rfbport ${control_vnc}" || true
  pkill -f "websockify.*${control_port}" || true
  rm -f "$control_token"
}

open_url() {
  [ -n "$url" ] || { echo "usage: computer-screen open <screen> <url>" >&2; exit 2; }
  xdpyinfo -display "$display" >/dev/null 2>&1 \
    || { echo "screen ${screen} is not running" >&2; exit 1; }

  # The running browser's profile, so this joins it instead of fighting for the lock.
  DISPLAY="$display" computer-browser --user-data-dir="$profile" "$url" \
    >>"${logs}-browser.log" 2>&1 &
}

recording_file="/tmp/computer/recording-${screen}.mp4"
recording_pid="/tmp/computer/recording-${screen}.pid"

# Ended with SIGINT and waited on: killed outright, ffmpeg writes no index.
record() {
  what="${3:-}"
  fps="${4:-12}"

  running() {
    pid=$(cat "$recording_pid" 2>/dev/null || true)
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
  }

  case "$what" in
    start)
      command -v ffmpeg >/dev/null 2>&1 \
        || { echo "this box has no ffmpeg; open it with the video feature" >&2; exit 4; }
      xdpyinfo -display "$display" >/dev/null 2>&1 \
        || { echo "screen ${screen} is not running" >&2; exit 1; }
      running && { echo "screen ${screen} is already recording" >&2; exit 3; }

      rm -f "$recording_file"

      # ffmpeg fails on a missing audio input and loses the video with it.
      sound=()
      if command -v pactl >/dev/null 2>&1 && [ -S "$pulse_socket" ]; then
        sound=(-f pulse -i "screen${number}.monitor")
      fi

      PULSE_SERVER="unix:${pulse_socket}" ffmpeg -nostdin -loglevel error -y \
        -f x11grab -draw_mouse 1 -framerate "$fps" -i "$display" \
        "${sound[@]}" \
        -c:v libx264 -preset ultrafast -pix_fmt yuv420p \
        -movflags frag_keyframe+empty_moov \
        "$recording_file" >>"${logs}-record.log" 2>&1 &
      echo $! > "$recording_pid"
      echo "$recording_file"
      ;;
    stop)
      running || { echo "screen ${screen} is not recording" >&2; exit 3; }
      pid=$(cat "$recording_pid")
      kill -INT "$pid" 2>/dev/null || true
      for _ in $(seq 1 100); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
      done
      kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null
      rm -f "$recording_pid"
      echo "$recording_file"
      ;;
    status)
      running && echo "recording ${recording_file}" || echo "idle"
      ;;
    *)
      echo "usage: computer-screen record <screen> start|stop|status [fps]" >&2
      exit 2
      ;;
  esac
}

case "$action" in
  start)   start ;;
  viewers) viewers ;;
  stop)    stop ;;
  control) control "$@" ;;
  release) release "$@" ;;
  open)    open_url ;;
  record)  record "$@" ;;
  *) echo "usage: computer-screen start|stop|control|release|open|record|viewers <screen> [arg]" >&2; exit 2 ;;
esac
