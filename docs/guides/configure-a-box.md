# Configure a box

This guide sets up a box before it starts: its screens, applications, packages, fonts, network, and browser data. For what each setting means, see [Boxes](../concepts/boxes.md). For a table of each setting in each interface, see [Configuration](../reference/configuration.md#box-settings).

## Image settings and runtime settings

Some settings go into the image. The first box with a new combination of them builds a new image, which takes a few minutes. Later boxes with the same combination start in seconds.

| Goes into the image | Does not |
| --- | --- |
| Applications | Screen size and number of screens |
| Packages | Network |
| Features: `wide_fonts`, `video`, `dock`, `accessibility`, and `x11_apps` on Wayland by default; `audio` | Memory, CPUs, lifetime |
| X11 or Wayland | Browser profile |

You cannot add a feature to a running box. To add an application to a running box, see [Add an application to a running box](#add-an-application-to-a-running-box).

To build an image before a box needs it:

```bash
holm image build docker --app vscode --package jq
```

## Screens

```bash
BOX=$(holm new --size 1920x1080 --screens 2)
```

Each screen has its own display, browser, and viewer. Screen 0 is the primary screen.

For Wayland instead of X11:

```bash
BOX=$(holm new --wayland)
```

A Wayland box has XWayland by default, so X11 applications also run on it. `--minimal` leaves it out.

## Applications

The catalog has these applications:

| Name | Application |
| --- | --- |
| `xterm` | xterm |
| `gimp` | GIMP |
| `files` | Thunar |
| `text-editor` | Mousepad |
| `vscode` | Visual Studio Code |

List them with `holm apps`, `GET /v1/catalog`, or the MCP tool `list_apps`.

Install applications when you create the box, then open one by name:

```bash
BOX=$(holm new --app gimp,vscode)
holm app "$BOX" gimp
```

`holm app` returns when the application's window has appeared and stopped changing. MCP: `launch_box` with `apps`, then `open_app`.

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
BOX=$(holm new --spec app.json)
holm app "$BOX" inkscape
```

An application with its own `source` makes the image build download and trust that repository's key. The server refuses it unless the spec also sets `"policy": {"custom_sources": true}`. The catalog's own sources, such as the one for `vscode`, need no policy.

### Add an application to a running box

The server can install a catalog application, or an application from the box's spec, into a running box:

```bash
holm app "$BOX" install gimp
holm app "$BOX" gimp
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

Add apt packages for commands that the agent runs with `run_command` or `holm exec`:

```bash
BOX=$(holm new --package jq --package ripgrep)
```

```rust
let computer = Computer::builder().packages(["jq", "ripgrep"]).launch().await?;
```

## Features

A box is one of two desktops. `--base` is the default, so the tools that need these features work without asking. `--minimal` is the bare desktop, for when size or build time matters more:

| Feature | Adds | Without it | `--base` | `--minimal` |
| --- | --- | --- | --- | --- |
| Wide fonts | Noto CJK and color emoji fonts | Chinese, Japanese, Korean, and emoji show as empty boxes, and the screenshot still looks correct at a glance. | Yes | No |
| Video | ffmpeg | `holm record` and the MCP `record` tool fail. | Yes | No |
| Dock | A tint2 dock | A person has no launcher. | Yes | No |
| Accessibility | AT-SPI | Native windows cannot be driven by widget name. See [Control modes](../concepts/control-modes.md#accessibility). | Yes | No |
| X11 apps | XWayland | X11 applications do not open on Wayland. | On Wayland | No |
| Audio | PulseAudio | A page or application that needs a sound server does not work. | Add with `--audio` | Add with `--audio` |

```bash
BOX=$(holm new --minimal)
BOX=$(holm new --minimal --audio)
```

For any other set of features, list them in a `--spec` file.

## Network

```bash
BOX=$(holm new --no-network)
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
BOX=$(holm new --profile work)
```

The volume is `holm-profile-work`. The next box with `--profile work` starts with the same browser data. The server refuses a second box that asks for a profile that another box holds, running or stopped, and names that box.

Before the server stops or removes a box with a profile, it closes the browser cleanly, so recent cookies are written. Session cookies end when the browser closes, as on any computer. A site's "remember me" cookie is the one that keeps a login. A fork does not take the profile.

### Saved state

```bash
holm browser "$BOX" state save login.json --origin https://mail.example.com
NEW=$(holm new)
holm browser "$NEW" state load login.json
```

`--name NAME` keeps the state on the server instead of in a file, until the server restarts. A state file contains live logins, and the CLI writes it with mode `0600`.

MCP: `save_state` and `load_state`.

## Start from a spec file

Put a full spec and placement in a file, and pass it to `holm new`:

```json
{
  "spec": {
    "desktop": {
      "server": "x11",
      "width": 1280,
      "height": 800,
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
BOX=$(holm new --spec box.json)
BOX=$(holm new --spec box.json --size 1920x1080)
cat box.json | holm new --spec -
```

A flag overrides the same value in the file. The server refuses unknown keys, so a misspelled key gives an error.

`"gimp": {}` names a catalog application. An entry with fields defines your own.

The same file is the body of `POST /v1/boxes`. In Rust, see `examples/from_spec.rs`. For all fields, see [Create a box](../reference/rest-api.md#create-a-box).
