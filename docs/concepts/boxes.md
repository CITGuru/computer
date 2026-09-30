# Boxes

A box is one isolated Linux desktop. It has one or more screens, a browser, a filesystem, and a shell. An agent drives it, and a person can watch it or take control.

In the Rust library, a box is a `Computer`. In the CLI, REST API, and MCP tools, a box has an ID such as `box_9cf78792…`, and each command or tool takes that ID.

## Spec and placement

The server describes a box with two parts:

| Part | Describes | Examples |
| --- | --- | --- |
| Spec | The desktop | Screen size, number of screens, applications, packages, features, network policy, viewer access |
| Placement | Where the box runs, and for how long | Runtime, memory, CPUs, lifetime, idle timeout, browser profile |

They are separate because two desktops that differ only in a memory limit are the same desktop. Each spec has a `spec_digest`, and `computer box` shows it.

```json
{
  "spec": {
    "desktop": { "width": 1280, "height": 800, "packages": ["jq"] },
    "policy": { "network": true }
  },
  "placement": { "runtime": "docker", "memory": "2g", "expires_after_secs": 3600 }
}
```

The CLI flags of `computer new` and the parameters of `launch_box` fill in the same two parts. `computer new --spec box.json` reads a spec from a file. The server refuses unknown keys in a spec.

## Screens

A box has one screen unless the spec asks for more, up to 8. Each screen has:

- Its own display, at the size of the spec.
- Its own Chromium, with a separate browser profile.
- Its own viewer URL.

Coordinates are device pixels on one screen. `(0, 0)` is the top-left corner.

## States

| State | Meaning |
| --- | --- |
| `Ready` | The box accepts actions. |
| `Paused` | The box is frozen. It keeps its memory, ports, and open windows, and uses no CPU. Calls to it wait until it resumes. They do not fail. |
| `Stopped` | All processes are stopped. The files stay. |
| `Unreachable` | The runtime of the box is missing or unavailable. The server keeps the record, and the box comes back when the runtime does. The `reason` field says why. |
| `Gone` | The box was removed, expired, or is no longer on its runtime. |

```text
          pause              stop
 Ready ──────────► Paused   Ready ──────────► Stopped
   ▲                 │        ▲                  │
   └──── resume ─────┘        └───── resume ─────┘
                                (new desktop, new viewer URL)
```

| Operation | CLI | MCP | Rust |
| --- | --- | --- | --- |
| Pause | `computer pause` | `pause_box` | `pause()` |
| Stop | `computer stop` | `stop_box` | `stop()` |
| Resume | `computer resume` | `resume_box` | `resume()`, or `start()` after a stop |
| Remove | `computer rm` | `remove_box` | `shutdown()` |

Use pause when you will come back soon: the desktop comes back as it was. Use stop to release the memory: a resumed box starts a new desktop with nothing open. Remove deletes the files.

Not all runtimes support pause and stop. See [Runtimes](runtimes.md#lifecycle-support).

## Lifetime

Each box has a deadline. A box lives one hour unless it asks for a different time or its runtime has a different default.

| Limit | CLI | REST placement | MCP |
| --- | --- | --- | --- |
| Fixed lifetime | `--ttl MINUTES` | `expires_after_secs` | `ttl_minutes` |
| Idle timeout | `--idle MINUTES` | `idle_timeout_secs` | `idle_minutes` |

The server refuses values under 60 seconds. The clock starts when the server creates the box, not when the box is ready.

`computerd` removes expired boxes every 30 seconds (`COMPUTER_SERVER_REAP_SECS`). It also forgets boxes that the runtime no longer has. The trace records each removal as `gone`, with the reason.

In the Rust library with no server, a box is removed when its `Computer` is dropped, unless you set `keep_on_drop(true)`.

## History

`computerd` records a trace for each box: actions, frames, commands, lifecycle changes, file transfers, and changes of control between the agent and a person.

```bash
computer trace "$BOX"
curl -s "localhost:8080/v1/boxes/$BOX/trace?after=12&limit=100"
```

A trace records that a file write or clipboard change occurred, but not the bytes.

With the default memory store, traces stop when the server stops. Use a durable storage backend to keep them across a restart.

## Forks

A fork makes a new box from the same spec, on the same runtime, and does the recorded actions again.

```bash
NEW_BOX=$(computer fork "$BOX")
computer fork "$BOX" --up-to 40
```

A fork does not copy memory or disk. The result can be different from the original, because:

- The page can change between the two runs.
- Timing and network speed can change.
- File writes and clipboard changes are skipped, because the trace does not keep their bytes. The result lists them under `skipped`.

Actions that the original box refused are also skipped. A fork stops at the first failure and names the step that failed. It stops after three minutes.

## Restart

A box runs in its runtime, not in the server process. When `computerd` starts, it takes back each box that it recorded. Then it scans each runtime for boxes labeled by an earlier server, so it can also find a box that the store lost. A box that it takes back after a restart starts a new trace, marked `adopted`, unless the store is durable.

## What is inside a box

The default X11 image is based on `debian:bookworm-slim`:

| Component | Purpose |
| --- | --- |
| Xvfb, fluxbox | The virtual display and window manager |
| Chromium | The browser, with a separate profile for each screen |
| x11vnc, websockify, noVNC | The viewer in a browser |
| xdotool, wmctrl | Pointer, keyboard, and window control |
| ImageMagick | Screenshots |
| xclip | Clipboard and primary selection |
| socat, `computer-devtools-bridge` | The Chrome DevTools bridge on port 9223. Host runtimes use socat. On E2B and Vercel, a bridge that accepts only requests with the box's secret. |
| xterm | A terminal |
| Input guard | Blocks agent input while a person has control |

The Wayland image uses headless sway, wayvnc, and grim instead. See [Runtimes](runtimes.md#display-servers).

Features and applications add more packages. A box gets the default features unless its spec lists its own:

| Feature | Adds | Default |
| --- | --- | --- |
| `wide_fonts` | Noto CJK and color emoji fonts | Yes |
| `video` | ffmpeg | Yes |
| `dock` | tint2 | Yes |
| `accessibility` | AT-SPI, for reading native widgets | Yes |
| `x11_apps` | XWayland | On Wayland |
| `audio` | PulseAudio | No |

A spec that lists features gets exactly that list, and `"features": []` is the bare desktop. The default set and the same features listed by name are one spec, so they use one image.

The image tag contains a hash of the image source and the added packages, and the CPU architecture. A change to the source or the packages makes a new image, so a box never uses an old image by mistake.
