#!/usr/bin/env bash
# One compositor per screen, each in its own runtime directory.
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

# Per screen: two compositors sharing a directory would each claim `wayland-1`.
runtime="/tmp/computer/run-${number}"
# The same name on every screen: the directory, not the name, tells screens apart.
wayland_display="wayland-1"
sockfile="/tmp/computer/screen-${screen}.sway"
control_token="/tmp/computer/screen-${screen}.control"
pointer_door="${runtime}/computer-pointer"
profile="${HOME:-/home/computer}/.browser-profiles/screen-${number}"
logs="/tmp/computer/screen-${number}"

export XDG_RUNTIME_DIR="$runtime"
export WAYLAND_DISPLAY="$wayland_display"

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

# Asked of sway: a dead compositor leaves its socket file behind.
alive() {
  local sock
  sock=$(cat "$sockfile" 2>/dev/null) || return 1
  [ -n "$sock" ] || return 1
  swaymsg -s "$sock" -t get_version >/dev/null 2>&1
}

# websockify holds its wayvnc connection only while a client is attached.
established() {
  local hex
  hex=$(printf "%04X" "$1")
  awk -v p="$hex" '$4=="01" && $2 ~ ":"p"$" {n++} END {print n+0}' \
    /proc/net/tcp /proc/net/tcp6 2>/dev/null
}

viewers() {
  echo "watching=$(established "$view_vnc") driving=$(established "$control_vnc")"
}

resident_pointer() {
  if [ -S "$pointer_door" ] && pgrep -f "computer-pointer serve ${pointer_door}$" >/dev/null; then
    return 0
  fi

  rm -f "$pointer_door"
  computer-pointer serve "$pointer_door" >>"${logs}-pointer.log" 2>&1 &
  await test -S "$pointer_door"
}

start() {
  if alive; then
    resident_pointer
    exit 0
  fi

  mkdir -p /tmp/computer "$runtime" "$profile"
  chmod 700 "$runtime"
  rm -f "$sockfile"

  # Enabled by presence: an image without Xwayland must not be told to start it.
  xwayland=disable
  command -v Xwayland >/dev/null 2>&1 && xwayland=enable

  sed -e "s/%WIDTH%/${width}/" \
      -e "s/%HEIGHT%/${height}/" \
      -e "s|%SOCKFILE%|${sockfile}|" \
      -e "s/%XWAYLAND%/${xwayland}/" \
      /etc/computer/sway.config > "${runtime}/sway.config"

  # sway on a real backend refuses to start without a seat, and a box has none.
  WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    sway --config "${runtime}/sway.config" >"${logs}-sway.log" 2>&1 &

  await alive || { echo "no compositor in ${runtime}" >&2; exit 1; }

  await test -S "${runtime}/${wayland_display}" \
    || { echo "the compositor is not on ${wayland_display}" >&2; exit 1; }

  resident_pointer \
    || { echo "no resident pointer on ${pointer_door}" >&2; exit 1; }

  # A profile kept in a volume brings back a lock naming a gone container; clear only foreign ones.
  lock="$profile/SingletonLock"
  if [ -L "$lock" ]; then
    case "$(readlink "$lock")" in
      "$(hostname)-"*) ;;
      *) rm -f "$lock" "$profile/SingletonSocket" "$profile/SingletonCookie" ;;
    esac
  fi

  computer-browser --user-data-dir="$profile" >"${logs}-browser.log" 2>&1 &

  # `-d` is the read-only guarantee; the page's own setting is a courtesy.
  wayvnc -d 127.0.0.1 "$view_vnc" >"${logs}-vnc.log" 2>&1 &
  build_gate view "127.0.0.1:${view_vnc}" || exit 1
  websockify --web=/usr/share/novnc "0.0.0.0:${view_port}" "${gate_args[@]}" \
    >"${logs}-novnc.log" 2>&1 &

  await listening "${view_port}" \
    || { echo "viewer never came up on ${view_port}" >&2; exit 1; }
}

stop() {
  local sock
  sock=$(cat "$sockfile" 2>/dev/null || true)
  [ -n "$sock" ] && swaymsg -s "$sock" exit >/dev/null 2>&1

  pkill -f -- "--user-data-dir=${profile}" || true
  pkill -f "computer-pointer serve ${pointer_door}$" || true
  pkill -f "wayvnc .* ${view_vnc}$" || true
  pkill -f "wayvnc .* ${control_vnc}$" || true
  pkill -f "websockify.*${view_port}" || true
  pkill -f "websockify.*${control_port}" || true
  rm -f "$sockfile" "$control_token" "$pointer_door"
}

control() {
  # The token lives in the box, so it outlives a caller that exits.
  token="${3:-}"
  mode="${4:-exclusive}"
  [ -n "$token" ] || { echo "usage: computer-screen control <screen> <token> [shared]" >&2; exit 2; }

  alive || { echo "screen ${screen} is not running" >&2; exit 1; }

  # Already open: record anyway, or the replaced holder could end this takeover.
  if listening "${control_port}"; then
    record_token
    exit 0
  fi

  # No `-d`: this one accepts input.
  wayvnc 127.0.0.1 "$control_vnc" >"${logs}-vnc-control.log" 2>&1 &
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

  pkill -f "wayvnc .* ${control_vnc}$" || true
  pkill -f "websockify.*${control_port}" || true
  rm -f "$control_token"
}

open_url() {
  [ -n "$url" ] || { echo "usage: computer-screen open <screen> <url>" >&2; exit 2; }
  alive || { echo "screen ${screen} is not running" >&2; exit 1; }

  # The running browser's profile, so this joins it instead of fighting for the lock.
  computer-browser --user-data-dir="$profile" "$url" >>"${logs}-browser.log" 2>&1 &
}

recording_file="/tmp/computer/recording-${screen}.mp4"
recording_pid="/tmp/computer/recording-${screen}.pid"
recording_flag="/tmp/computer/recording-${screen}.on"

# wlroots publishes no input ffmpeg can read, so the frames come from `grim`.
# No pointer: headless sway draws no cursor for `grim` to capture.
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
      grim -t png - >/dev/null 2>&1 \
        || { echo "screen ${screen} is not running" >&2; exit 1; }
      running && { echo "screen ${screen} is already recording" >&2; exit 3; }

      rm -f "$recording_file"
      : > "$recording_flag"

      pause=$(awk "BEGIN { printf \"%.4f\", 1 / $fps }")

      # Wall-clock timestamps, because the loop keeps no exact cadence.
      # $! after a pipeline is ffmpeg, the half to wait on.
      (
        while [ -e "$recording_flag" ]; do
          grim -t ppm - || break
          sleep "$pause"
        done
      ) | ffmpeg -nostdin -loglevel error -y \
            -f image2pipe -use_wallclock_as_timestamps 1 -i - \
            -c:v libx264 -preset ultrafast -pix_fmt yuv420p -vsync vfr \
            -movflags frag_keyframe+empty_moov \
            "$recording_file" >>"${logs}-record.log" 2>&1 &
      echo $! > "$recording_pid"
      echo "$recording_file"
      ;;
    stop)
      running || { echo "screen ${screen} is not recording" >&2; exit 3; }
      pid=$(cat "$recording_pid")

      # Not a signal: an mp4 cut off mid-write has no index.
      rm -f "$recording_flag"
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
