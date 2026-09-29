# smol cloud

smol cloud runs each box in a libkrun microVM at smol machines. The server builds the desktop image once, pushes it to your account's smol registry, and starts each box from it.

For how remote runtimes compare with host runtimes, see [Runtimes](../concepts/runtimes.md). The `smolvm` runtime is different: it runs microVMs on the server's own host.

## What works

| Works | Not yet supported |
| --- | --- |
| Screenshots, mouse, keyboard, and waits | The live viewer and takeover URLs |
| Windows, applications, and native widgets | Page tools and `computer cdp` |
| Files, commands, and clipboard | Pause and fork |
| Applications at launch (`--app`) and installed into a running box | Browser profiles (`--profile`) |
| CPUs and memory for each box | |
| Network policy (`--no-network`) | |

Everything in the first column goes through smol's exec API. A box publishes no ports. smol reaches a port only through `https://api.smolmachines.com/v1/machines/<id>/connect/<port>` with the account's API key, and its proxy forwards that key into the machine, where any program in the box could read it. Its proxy also refuses the WebSocket answer that page tools need. The viewer and page tools come back when smol changes this.

## 1. Get an API key

Create an API key in the smol cloud console. Keys start with `smk_`. Keep it out of shell history and out of files that go into git.

Release builds of `computer` and `computerd` include smol cloud. For a source build, add the `smol` feature:

```bash
cargo install --path . --locked --features smol
```

The server also needs:

- **Docker with buildx**, to build the desktop image the first time. After that, the image is in your smol registry and Docker is not used.
- **The `smolvm` CLI**, optional, to make a `.smolmachine` pack. Without it, boxes still start, only more slowly.

## 2. Add smol cloud to the server

**From the environment.** The runtime is named `smol`:

```bash
SMOL_CLOUD_TOKEN=smk_... COMPUTER_SERVER_SANDBOXES=smol computerd
```

**At run time, with the CLI:**

```bash
computer runtime add cloud --provider smol --api-key-env SMOL_CLOUD_TOKEN
computer runtime ls
```

| Variable or field | Effect |
| --- | --- |
| `SMOL_CLOUD_TOKEN`, or `api_key` | The API key. Necessary. |
| `SMOL_CLOUD_URL`, or the `endpoint` field | The API address. Default `https://api.smolmachines.com`. |
| `SMOL_CLOUD_PACK=0`, or the field `pack=false` | Do not make `.smolmachine` packs. |

## 3. Create a box

```bash
BOX=$(computer new --runtime smol)
```

MCP: `launch_box` with `runtime: "smol"`. REST: `"placement": {"runtime": "smol"}`.

## Images

### The built-in image

1. The server tags the image `registry.smolmachines.com/<namespace>/computer-desktop:<fingerprint>-x86_64`. The namespace is your account's, from `GET /v1/me`. The fingerprint comes from the image files, so a changed image gets a new tag.
2. If the registry does not have that tag, the server builds the image for `linux/amd64` with Docker on its host and pushes it. On a Mac with Apple Silicon, an amd64 build runs under emulation and can take 15 minutes the first time.
3. With the `smolvm` CLI, it packs the image into a `.smolmachine` artifact whose main process is idle, and pushes it as `…/computer-pack:pack-<digest>`. The image's `ENV` and `WORKDIR` go with each box, because a pack does not keep them.

A later box finds the image and the pack, and starts in about 15 seconds.

### Without a pack

With no `smolvm` CLI, `SMOL_CLOUD_PACK=0`, or a pack that fails, a box starts from the image itself. smol then runs the image's `CMD`, which starts the desktop, and the server's own start command runs as well. The image's start script takes a lock, so the second start waits and then finds the desktop running. A box from the image starts in about 40 seconds.

### An image that you name

A named image is used as it is, and packed when the `smolvm` CLI is there. It can be in any registry that smol can pull from. It must contain the desktop: build it from the built-in Dockerfile.

## CPUs and memory

A box gets 2 CPUs and 4 GB of memory unless it asks for other values.

## Lifetimes

A box stops when it has no activity for its TTL, and smol deletes it when it stops.

## Restart

smol keeps no labels on a machine. The server writes the box's labels to `/tmp/computer-labels.json` in the box, and reads that file to take the box back after a restart. It finds a box by its name, which is unique in the account.

## Use smol cloud from Rust

Build with the `smol` feature, and set `SMOL_CLOUD_TOKEN`.

```rust
use computer::sandboxes::{remote, smol::cloud::Cloud};
use computer::{Computer, X11Profile};

let (machine, profile) = remote::pair(Arc::new(Cloud::from_env()?), Arc::new(X11Profile));

let computer = Computer::builder()
    .machine(Arc::new(machine))
    .profile(profile)
    .launch()
    .await?;
```

| Setting | Effect |
| --- | --- |
| No `image` | Build the built-in image into your smol registry, then pack it. |
| `.image("<reference>")` | Start from this image, packed when possible. |
| `.image_dir(path)` | Build this directory into your smol registry, then pack it. |
| `Cloud::packing(false)` | Start from the image, with no pack. |
| `Cloud::at(url)` | Another API address. |

Run the example:

```bash
cargo run --features smol --example smol
```
