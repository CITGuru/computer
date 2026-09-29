# Configure a box

This guide sets up a box before it starts: its screens, applications, packages, fonts, network, and browser data. For what each setting means, see [Boxes](../concepts/boxes.md). For a table of each setting in each interface, see [Configuration](../reference/configuration.md#box-settings).

## Image settings and runtime settings

Some settings go into the image. The first box with a new combination of them builds a new image, which takes a few minutes. Later boxes with the same combination start in seconds.

| Goes into the image | Does not |
| --- | --- |
| Applications | Screen size and number of screens |
| Packages | Network |
| Features: `wide_fonts`, `audio`, `video`, `dock`, `x11_apps`, `accessibility` | Memory, CPUs, lifetime |
| X11 or Wayland | Browser profile |

You cannot add a feature to a running box. To add an application to a running box, see [Add an application to a running box](#add-an-application-to-a-running-box).

To build an image before a box needs it:

```bash
computer image build docker --app vscode --package jq
```

## Screens

```bash
BOX=$(computer new --size 1920x1080 --screens 2)
```

Each screen has its own display, browser, and viewer. Screen 0 is the primary screen.

For Wayland instead of X11:

```bash
BOX=$(computer new --wayland)
BOX=$(computer new --wayland --x11-apps)
```

`--x11-apps` adds XWayland, so X11 applications also run on a Wayland box.

## Applications

The catalog has these applications:

| Name | Application |
| --- | --- |
| `xterm` | xterm |
| `gimp` | GIMP |
| `files` | Thunar |
| `text-editor` | Mousepad |
| `vscode` | Visual Studio Code |

List them with `computer apps`, `GET /v1/catalog`, or the MCP tool `list_apps`.

Install applications when you create the box, then open one by name:

```bash
BOX=$(computer new --app gimp,vscode)
computer app "$BOX" gimp
```

`computer app` returns when the application's window has appeared and stopped changing. MCP: `launch_box` with `apps`, then `open_app`.

### Your own applications

Define an application in the spec under `apps`. Give it a name, the packages to install, the command that opens it, and the window that shows it is ready:

```json
{
  "spec": {
    "apps": {
      "inkscape": {
        "packages": ["inkscape"],
        "command": ["inkscape"],
        "window": { "class": "Inkscape" },
        "settle_ms": 1000
      }
    }
  }
}
```

| Field | Effect |
| --- | --- |
| `packages` | Apt packages to install. |
| `command` | The command that opens the application. |
| `window` | `{"class": "…"}` or `{"title": "…"}`. `open_app` waits for this window. |
| `settle_ms` | Time to wait after the window appears, for applications that draw late. |
| `source` | An extra apt repository: `{"key_url": "…", "list": "…"}`. |

```bash
BOX=$(computer new --spec app.json)
computer app "$BOX" inkscape
```

An application with its own `source` makes the image build download and trust that repository's key. The server refuses it unless the spec also sets `"policy": {"custom_sources": true}`. The catalog's own sources, such as the one for `vscode`, need no policy.

### Add an application to a running box

The server can install a catalog application, or an application from the box's spec, into a running box:

```bash
computer app "$BOX" install gimp
computer app "$BOX" gimp
```

MCP: `install_app` with `apps: ["gimp"]`, then `open_app`. REST:

```bash
curl -X POST "$BASE/v1/boxes/$BOX/apps" \
  -H 'content-type: application/json' \
  -d '{"apps": ["gimp"]}'
```

The install runs as root. Where the box's commands run as another user, as on E2B, it runs through `sudo`.

The box installs it with apt, so the box needs network access, and each install takes time. A fork does not keep it, because a fork builds from the spec. When you know the application in advance, put it in the spec.

## Packages

Add apt packages for commands that the agent runs with `run_command` or `computer exec`:

```bash
BOX=$(computer new --package jq --package ripgrep)
```

```rust
let computer = Computer::builder().packages(["jq", "ripgrep"]).launch().await?;
```

## Fonts, sound, and recording

| Flag | Adds | Use it when |
| --- | --- | --- |
| `--wide-fonts` | Noto CJK and color emoji fonts | Pages in Chinese, Japanese, or Korean, or with emoji. Without these fonts, those characters show as empty boxes, and the screenshot still looks correct at a glance. |
| `--audio` | PulseAudio | A page or application does not work with no sound server. |
| `--video` | ffmpeg | You want to record the screen with `computer record` or the MCP `record` tool. |
| `--dock` | A tint2 dock | A person uses the desktop and wants a launcher. |
| `--accessibility` | AT-SPI | You want to drive native windows by widget name. See [Control modes](../concepts/control-modes.md#accessibility). |

## Network

```bash
BOX=$(computer new --no-network)
```

The box has no network access. This does not affect the viewer, which the host serves.

## Browser data

There are two ways to keep a login.

| | Profile | Saved state |
| --- | --- | --- |
| Keeps | The full Chromium profile: logins, history, extensions, storage | Cookies and storage for the origins you name |
| Stored in | A volume on the host | The server's memory, or a file |
| Moves between hosts | No | Yes |
| Runtimes | Container runtimes only | All runtimes with DevTools |
| Boxes at a time | One | Any number |

### Profile

```bash
BOX=$(computer new --profile work)
```

The volume is `computer-profile-work`. The next box with `--profile work` starts with the same browser data. The server refuses a second box that asks for a profile that another box holds, running or stopped, and names that box.

Before the server stops or removes a box with a profile, it closes the browser cleanly, so recent cookies are written. Session cookies end when the browser closes, as on any computer. A site's "remember me" cookie is the one that keeps a login. A fork does not take the profile.

### Saved state

```bash
computer browser "$BOX" state save login.json --origin https://mail.example.com
NEW=$(computer new)
computer browser "$NEW" state load login.json
```

`--name NAME` keeps the state on the server instead of in a file, until the server restarts. A state file contains live logins, and the CLI writes it with mode `0600`.

MCP: `save_state` and `load_state`.

## Start from a spec file

Put a full spec and placement in a file, and pass it to `computer new`:

```json
{
  "spec": {
    "desktop": {
      "server": "x11",
      "width": 1280,
      "height": 800,
      "features": ["wide_fonts"],
      "packages": ["jq"]
    },
    "apps": { "gimp": {} },
    "policy": { "network": true }
  },
  "placement": {
    "runtime": "docker",
    "memory": "2g",
    "expires_after_secs": 3600
  }
}
```

```bash
BOX=$(computer new --spec box.json)
BOX=$(computer new --spec box.json --size 1920x1080)
cat box.json | computer new --spec -
```

A flag overrides the same value in the file. The server refuses unknown keys, so a misspelled key gives an error.

`"gimp": {}` names a catalog application. An entry with fields defines your own.

The same file is the body of `POST /v1/boxes`. In Rust, see `examples/from_spec.rs`. For all fields, see [Create a box](../reference/rest-api.md#create-a-box).
