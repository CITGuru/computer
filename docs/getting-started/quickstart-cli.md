# CLI quick start

This page opens a box, drives it, and removes it. You need `holm` [installed](install.md) and a container runtime running.

## Open a box

```bash
BOX=$(holm new --url https://example.com)
```

`holm new` prints the box ID on standard output and the viewer URL on standard error. Open the viewer URL in a browser to watch the desktop.

The first box builds the image, which takes a few minutes.

## Drive the desktop

Coordinates are in device pixels, and `(0, 0)` is the top-left corner. The default screen is 1280x800.

```bash
holm screenshot "$BOX" screen.png
holm mouse "$BOX" click 640 81 left
holm keyboard "$BOX" type "driven from the CLI"
holm keyboard "$BOX" press enter
```

## Drive a page

On a web page, act on controls by name. This is more reliable than coordinates, because the page can move after a screenshot.

```bash
TAB=$(holm open "$BOX" https://example.com)
holm browser "$BOX" snapshot --tab "$TAB"
holm browser "$BOX" click "More information" --tab "$TAB"
```

`snapshot` lists the controls on the page and gives each one a reference such as `@e12`. You can use that reference as a query in a later command.

## Give the screen to a person

```bash
holm takeover "$BOX"
holm release "$BOX"
```

`takeover` prints a URL where a person can use the mouse and keyboard. `release` gives control back.

## Remove the box

```bash
holm rm "$BOX"
```

A box stays after the command that opened it exits. Remove it, or give it a lifetime when you open it:

```bash
BOX=$(holm new --ttl 60)
```

## Common options for `holm new`

| Option | Effect |
| --- | --- |
| `--size WxH` | Screen size |
| `--screens N` | Number of screens |
| `--app NAME` | Install an application from the catalog, such as `gimp` or `vscode` |
| `--package PKG` | Install a system package |
| `--minimal` | The bare desktop, without the default fonts, video, dock, and accessibility |
| `--no-network` | Block outbound network access |
| `--runtime NAME` | Select a runtime, such as `podman` |
| `--ttl MINUTES` | Remove the box after this time |
| `--idle MINUTES` | Remove the box after this time with no use |
| `--profile NAME` | Keep browser logins, cookies, and history in a named volume |
| `--spec FILE` | Read the full box specification from a JSON file |

A box has wide fonts, video, a dock, and accessibility by default. You cannot change them or `--wayland` after a box opens.

See the [CLI reference](../reference/cli.md) for all commands.

## Use a server

With no server, each command starts a server that stops when the command exits. Start `holmd` when you need traces, forks, or box expiry:

```bash
holmd
```

`holm` finds a server in this order:

1. `--server URL`
2. `HOLM_SERVER_URL`
3. A `holmd` on `http://127.0.0.1:8080`
4. A server for the life of the command

To use a remote server:

```bash
export HOLM_SERVER_URL=https://boxes.example.com
export HOLM_SERVER_TOKEN=...
holm ls
```

## Output

Commands write primary values, such as a box ID or a URL, to standard output, and status text to standard error. You can put one command inside another:

```bash
holm screenshot "$(holm new)" out.png
```
