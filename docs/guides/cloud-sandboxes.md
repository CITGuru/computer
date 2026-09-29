# Cloud sandboxes

A cloud sandbox runs a box at a vendor, not on your host. The vendor supplies the machine, and you need no container runtime. This guide sets up E2B. For the other vendors that are built in, see [Vercel Sandbox](vercel-sandbox.md), [Daytona](daytona.md), and [Modal](modal.md). At the end, this guide shows how to add a different vendor.

For how remote runtimes compare with host runtimes, see [Runtimes](../concepts/runtimes.md).

## What works

| Works | Not yet supported |
| --- | --- |
| Screenshots, mouse, keyboard, and waits | Browser profiles (`--profile`) |
| Windows, applications, and native widgets | Stop. Pause works. |
| Files, commands, and clipboard | Memory and CPUs for each box. They are set when the template is built. |
| Page tools and `computer cdp`, with a template built from the current image, also after a server restart. See [Control modes](../concepts/control-modes.md#browser-mode-on-remote-runtimes). | |
| Human control, through the MCP Apps live screen. Viewer and takeover URLs open in a browser only when the runtime sets `public_traffic`. | |
| Pause and resume | |
| Network policy (`--no-network`) | |

## 1. Get an API key

Make an account at E2B and create an API key. Keep it out of shell history and out of files that go into git.

Release builds of `computer` and `computerd` include E2B. For a source build, add the `e2b` feature:

```bash
cargo install --path . --locked --features e2b
```

## 2. Add E2B to the server

There are two ways. Use one.

**From the environment.** The runtime is named `e2b`:

```bash
E2B_API_KEY=... COMPUTER_SERVER_SANDBOXES=e2b computerd
```

**At run time, with the CLI.** You choose the name. The server keeps the runtime in its store:

```bash
printf '%s' "$E2B_API_KEY" | computer runtime add cloud --provider e2b --api-key
computer runtime ls
```

The CLI reads the key from standard input, or from a variable with `--api-key-env E2B_API_KEY`. The key does not go on the command line. To keep the key, the server needs `COMPUTER_SERVER_SECRET_KEY` or `COMPUTER_SERVER_SECRET_FILE`, and durable storage to keep the runtime after a restart. See [Remote runtimes](../concepts/runtimes.md#remote-runtimes).

| | Environment | CLI or API |
| --- | --- | --- |
| Name | `e2b` | Any |
| More than one account | No | Yes |
| Fields such as `image` and `public_traffic` | No | Yes |
| Lifetimes | In the runtimes file | `--field` |
| Change the key | Restart the server | `computer runtime set NAME --api-key` |

## 3. Create a box

```bash
BOX=$(computer new --runtime cloud)
```

MCP: `launch_box` with `runtime: "cloud"`. REST: `"placement": {"runtime": "cloud"}`.

To make E2B the default, set `default = "cloud"` in the [runtimes file](../concepts/runtimes.md#the-runtimes-file).

The server refuses `--memory` and `--cpus` for an E2B box, because E2B sets them when it builds the template.

## Templates

E2B starts a sandbox from a template, not from a container image.

### Automatic templates

When a box needs a template that does not exist, the server makes one:

1. It converts the built-in Dockerfile, with the spec's applications and packages, into E2B build steps.
2. It uploads the files that the Dockerfile copies.
3. It starts the build at E2B, with 2 CPUs and 2048 MiB of memory, and waits for it to finish.
4. It records the template. Later boxes with the same spec use it.

Each different set of applications and packages is a different template. The first box with a new set waits for the build. To build it before a box needs it:

```bash
computer image build cloud --app vscode
computer image ls cloud
```

### A template that you build

Build a template yourself when you want to keep it, change its size, or see what E2B receives. Write the build context, then use the E2B CLI:

```bash
python3 crates/computer-core/images/context.py \
  crates/computer-core/images/desktop \
  /tmp/e2b-ctx \
  --for e2b

e2b template create computer-desktop \
  -p /tmp/e2b-ctx \
  -d Dockerfile \
  -c "/usr/local/bin/computer-desktop" \
  --ready-cmd "true" \
  --cpu-count 2 \
  --memory-mb 2048
```

The conversion removes the Dockerfile instructions that E2B refuses or replaces.

Then tell the runtime to use your template for all boxes:

```bash
computer runtime set cloud --field image=<template-id>
```

## Lifetimes

E2B limits how long a sandbox can live. The limit depends on your E2B plan. The server does not know your plan, so set the limit on the runtime:

```toml
[runtimes.e2b]
lifetime = "1h"
max_lifetime = "24h"
```

For a runtime added with the CLI, use `--field lifetime_secs=3600 --field max_lifetime_secs=86400`.

The server refuses a box that asks for a lifetime above `max_lifetime`, and gives both numbers. While a box is in use, the server extends its deadline at E2B.

## Viewers

E2B publishes each port of a sandbox at its own URL. The server creates each sandbox as secure, so each port needs E2B's traffic token in a request header. A browser cannot send that header.

| Viewer | Works |
| --- | --- |
| MCP Apps live screen | Yes. It goes through `computerd`, which sends the traffic token. |
| Viewer and takeover URLs in a browser | Only when the runtime has `public_traffic=true` |

With `public_traffic=true`, anyone who has a port's URL can reach that port without the traffic token. The viewers still need their own token: the server always sets token access on a remote box with no other viewer access.

```bash
computer runtime set cloud --field public_traffic=true
```

The environment runtime (`COMPUTER_SERVER_SANDBOXES=e2b`) takes no fields, so it cannot set `public_traffic`. Add the runtime with the CLI instead.

## Self-hosted E2B

For E2B that runs on your own infrastructure:

- **Environment runtime and the library:** set `E2B_DOMAIN` to your domain. The default is `e2b.app`.
- **CLI or API runtime:** set the `endpoint` field. Through the API, it must be `https` and a public address.

```bash
printf '%s' "$E2B_API_KEY" | computer runtime add onprem --provider e2b --field endpoint=https://e2b.example.com --api-key
```

## Use E2B from Rust

The library uses E2B with no server. Build with the `e2b` feature, and set `E2B_API_KEY`.

```rust
use computer::sandboxes::e2b::{self, cloud::Cloud};
use computer::{Auth, Computer, X11Profile};

let (machine, profile) = e2b::pair(Arc::new(Cloud::from_env()?), Arc::new(X11Profile));

let computer = Computer::builder()
    .machine(Arc::new(
        machine
            .public_viewer(true)
            .expiring_after(Duration::from_secs(15 * 60)),
    ))
    .profile(profile)
    .auth(Auth::Token)
    .launch()
    .await?;
```

| Setting | Effect |
| --- | --- |
| `.image("<template-id>")` | Start from this template. With no `image`, the library builds a template from the built-in image, as the server does. |
| `public_viewer(true)` | Give the box a viewer URL. The launch needs `Auth::Token` or `Auth::Password`. |
| `public_traffic(true)` | Let a browser open the ports without the traffic token. |
| `expiring_after(duration)` | How long the sandbox lives with no activity. Activity extends it. |
| `Cloud::at(key, domain)` | Use a self-hosted E2B. |

Run the examples:

```bash
export E2B_API_KEY=...
cargo run --features e2b --example e2b -- <template-id>
cargo run --features e2b --example e2b_takeover -- <template-id> "search text"
```

## Add a different vendor

A vendor adapter implements the `RemoteApi` trait. Its required methods create, find, and kill a sandbox, run a command, and read and write a file. The other methods have defaults that claim nothing, such as `pause` and `keep_alive`. Override them when the vendor can do more, and say so in `can()`.

`RemoteMachine` adds the lifetime, naming, and keep-alive behavior that all vendors share. `RemoteProfile` maps the vendor's port URLs and removes what a remote sandbox cannot expose.

```rust
use computer::sandboxes::remote::{self, RemoteApi};
use computer::{Computer, X11Profile};

let (machine, profile) = remote::pair(Arc::new(MyVendor::new()), Arc::new(X11Profile));
let computer = Computer::builder()
    .machine(Arc::new(machine))
    .profile(profile)
    .launch()
    .await?;
```

`examples/custom_sandbox.rs` is a complete adapter that runs sandboxes as local Docker containers. Run it before you connect a real API:

```bash
cargo run --example custom_sandbox
```

For tests with no account or network, use `computer::testing::ScriptedRemote`.

`computerd` builds only the vendors that it knows. To offer your own vendor from a server, build a server with your own implementation of the `computer_server::runtimes::Vendors` trait, and pass it to `AppState::serving`.
