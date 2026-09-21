# computer-cli

The `computer` command creates and drives desktop boxes through `computerd`.

```bash
BOX=$(computer new --url https://example.com)
computer screenshot "$BOX" screen.png
computer browser "$BOX" snapshot --urls
computer rm "$BOX"
```

Run `computer --help` for the complete command and flag reference.

## Server selection

Most commands use the REST server. The client chooses it in this order:

1. `--server URL`
2. `COMPUTER_SERVER_URL`
3. a compatible service on `http://127.0.0.1:8080`
4. an embedded server on an operating-system-selected port

The embedded server lives only for the command. Nothing must be running for a local quick start.

Use a remote server:

```bash
export COMPUTER_SERVER_URL=https://boxes.example.com
export COMPUTER_SERVER_TOKEN=...
computer ls
```

`--server` overrides `COMPUTER_SERVER_URL`. The token is sent as a bearer credential.

The health probe verifies that the service on port 8080 is `computer-server`; an unrelated process is not treated as a desktop fleet.

## Run a persistent server

Use `computerd` when state must survive individual CLI commands:

```bash
computerd
```

The daemon keeps traces used by `trace` and `fork`, reaps expired boxes, serves REST under `/v1`, and serves MCP at `/mcp`.

Bind outside loopback only with a token:

```bash
COMPUTER_SERVER_ADDR=0.0.0.0:8080 \
COMPUTER_SERVER_TOKEN="$(openssl rand -hex 32)" \
computerd
```

A new server adopts boxes left on the configured runtimes. Its default in-memory trace does not survive a server restart. See the [server guide](../computer-server/README.md) for durable storage and deployment settings.

## `--local`

`--local` bypasses REST and drives the local runtime from the CLI process:

```bash
BOX=$(computer --local new --name my-box)
computer --local screenshot "$BOX" out.png
```

It supports the direct local operations: `new`, `ls`, `screenshot`, `open`, `mouse`, `keyboard`, `wait`, `clip`, `takeover`, `release`, `exec`, `rm`, and `sweep`.

Server-managed operations such as `box`, `cdp`, lifecycle state, applications, windows, widgets, browser elements, recording, files, batches, forks, and traces are not available with `--local`.

```
$ computer --local trace "$BOX"
trace needs a server to remember what was done, and --local has none.
Run it without --local.
```

`mouse down` and `mouse up` also need the server. It owns the timeout that releases a button if the second command never arrives.

## Common workflows

Create and inspect a box:

```bash
BOX=$(computer new --size 1920x1080 --ttl 60)
computer box "$BOX"
computer ls
```

Open and drive a page:

```bash
TAB=$(computer open "$BOX" https://example.com)
computer browser "$BOX" snapshot --tab "$TAB"
computer browser "$BOX" click "More information" --tab "$TAB"
```

Drive desktop input:

```bash
computer mouse "$BOX" move 640 400 --human --seed 42
computer mouse "$BOX" click 640 400 left
computer keyboard "$BOX" press ctrl+l
computer keyboard "$BOX" type "https://example.org" --delay 20
computer keyboard "$BOX" press enter
```

Run several dependent actions under one screen lock:

```bash
computer batch "$BOX" actions.json --settle 400
```

Give the screen to a person:

```bash
computer takeover "$BOX"
computer release "$BOX"
```

## Output conventions

Commands print machine-readable primary values, such as a new box ID or a URL, to standard output. Status text goes to standard error. This keeps command substitution safe:

```bash
computer screenshot "$(computer new)" out.png
```

`computer sweep` is always local and checks the default Docker runtime, even when `COMPUTER_SERVER_URL` is set. Fleet expiry is handled by `computerd`.
