# holm-cli

The `holm` command creates and drives desktop boxes through `holmd`.

```bash
BOX=$(holm new --url https://example.com)
holm screenshot "$BOX" screen.png
holm browser "$BOX" snapshot --urls
holm rm "$BOX"
```

Run `holm --help` for the complete command and flag reference.

## Server selection

Most commands use the REST server. The client chooses it in this order:

1. `--server URL`
2. `HOLM_SERVER_URL`
3. a compatible service on `http://127.0.0.1:8080`
4. an embedded server on an operating-system-selected port

The embedded server lives only for the command. Nothing must be running for a local quick start.

Use a remote server:

```bash
export HOLM_SERVER_URL=https://boxes.example.com
export HOLM_SERVER_TOKEN=...
holm ls
```

`--server` overrides `HOLM_SERVER_URL`. The token is sent as a bearer credential.

The health probe verifies that the service on port 8080 is `holm-server`; an unrelated process is not treated as a desktop fleet.

## Run a persistent server

Use `holmd` when state must survive individual CLI commands:

```bash
holmd
```

The daemon keeps traces used by `trace` and `fork`, reaps expired boxes, serves REST under `/v1`, and serves MCP at `/mcp`.

Bind outside loopback only with a token:

```bash
HOLM_SERVER_ADDR=0.0.0.0:8080 \
HOLM_SERVER_TOKEN="$(openssl rand -hex 32)" \
holmd
```

A new server adopts boxes left on the configured runtimes. Its default in-memory trace does not survive a server restart. See the [server guide](../holm-server/README.md) for durable storage and deployment settings.

## `--local`

`--local` bypasses REST and drives the local runtime from the CLI process:

```bash
BOX=$(holm --local new --name my-box)
holm --local screenshot "$BOX" out.png
```

It supports the direct local operations: `new`, `ls`, `screenshot`, `open`, `mouse`, `keyboard`, `wait`, `clip`, `takeover`, `release`, `exec`, `rm`, and `sweep`.

Server-managed operations such as `box`, `cdp`, lifecycle state, applications, windows, widgets, browser elements, recording, files, batches, forks, and traces are not available with `--local`.

```
$ holm --local trace "$BOX"
trace needs a server to remember what was done, and --local has none.
Run it without --local.
```

`mouse down` and `mouse up` also need the server. It owns the timeout that releases a button if the second command never arrives.

## Common workflows

Create and inspect a box:

```bash
BOX=$(holm new --size 1920x1080 --ttl 60)
holm box "$BOX"
holm ls
```

Open and drive a page:

```bash
TAB=$(holm open "$BOX" https://example.com)
holm browser "$BOX" snapshot --tab "$TAB"
holm browser "$BOX" click "More information" --tab "$TAB"
```

Drive desktop input:

```bash
holm mouse "$BOX" move 640 400 --human --seed 42
holm mouse "$BOX" click 640 400 left
holm keyboard "$BOX" press ctrl+l
holm keyboard "$BOX" type "https://example.org" --delay 20
holm keyboard "$BOX" press enter
```

Hold a key or a button across commands. The server lets it go after `--hold`, 10 seconds unless told:

```bash
holm keyboard "$BOX" down shift --hold 20
holm mouse "$BOX" click 300 200
holm keyboard "$BOX" up shift
```

Run several dependent actions under one screen lock:

```bash
holm batch "$BOX" actions.json --settle 400
```

Give the screen to a person:

```bash
holm takeover "$BOX"
holm release "$BOX"
```

## Output conventions

Commands print machine-readable primary values, such as a new box ID or a URL, to standard output. Status text goes to standard error. This keeps command substitution safe:

```bash
holm screenshot "$(holm new)" out.png
```

`holm sweep` is always local and checks the default Docker runtime, even when `HOLM_SERVER_URL` is set. Fleet expiry is handled by `holmd`.
