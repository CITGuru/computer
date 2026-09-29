# Daytona

Daytona runs each box in a container at Daytona. You need no container runtime and no Docker on your host: Daytona builds the image from a Dockerfile that the server sends.

For how remote runtimes compare with host runtimes, see [Runtimes](../concepts/runtimes.md).

## What works

| Works | Not yet supported |
| --- | --- |
| Screenshots, mouse, keyboard, and waits | Pause and fork. Daytona pauses only its Linux VM class, which needs a snapshot from a registry. |
| Windows, applications, and native widgets | Browser profiles (`--profile`) |
| Files, commands, and clipboard | |
| Applications and packages at launch (`--app`), and applications installed into a running box | |
| Page tools and `computer cdp`, also after a server restart | |
| Viewer and takeover URLs in a browser | |
| CPUs and memory for each box | |
| Network policy (`--no-network`) | |

## Network access

Daytona limits internet access on lower account tiers. On such an account, a sandbox reached github.com and pypi.org, but not example.com, google.com, or wikipedia.org, which failed with `ERR_CONNECTION_RESET` in the browser. A higher tier gives full internet access. The server cannot change this.

## 1. Get an API key

Create an API key in the Daytona dashboard, under Keys. Keep it out of shell history and out of files that go into git.

Release builds of `computer` and `computerd` include Daytona. For a source build, add the `daytona` feature:

```bash
cargo install --path . --locked --features daytona
```

## 2. Add Daytona to the server

**From the environment.** The runtime is named `daytona`:

```bash
DAYTONA_API_KEY=... COMPUTER_SERVER_SANDBOXES=daytona computerd
```

**At run time, with the CLI:**

```bash
computer runtime add cloud --provider daytona --api-key-env DAYTONA_API_KEY
computer runtime ls
```

| Variable or field | Effect |
| --- | --- |
| `DAYTONA_API_KEY`, or `api_key` | The key. Necessary. |
| `DAYTONA_API_URL`, or the `endpoint` field | The API URL. Default `https://app.daytona.io/api`. |
| `DAYTONA_TARGET`, or the `target` field | The region, such as `us` or `eu`. Default: Daytona's. |

## 3. Create a box

```bash
BOX=$(computer new --runtime daytona)
```

MCP: `launch_box` with `runtime: "daytona"`. REST: `"placement": {"runtime": "daytona"}`.

The first box waits for the image build, about one minute. Later boxes start in about 10 seconds.

## Images

### The built-in image

Daytona builds an image from a Dockerfile, but it has no build context for the files that the Dockerfile copies. So the server changes each `COPY` of a file into a `RUN` that writes the file from base64, and sends the result. Daytona keeps each build by the content of the Dockerfile, so a later box with the same image starts from the build that exists.

A changed image gives a different Dockerfile, so Daytona builds it again.

### An image directory

An image directory (`image_dir` in the library) is sent the same way. `COPY` of files and folders from the directory works, with `--chmod` and `--chown`. `COPY --from` of a build stage is left as it is. `ADD` is refused, because it can fetch and unpack files.

### An image that you name

A named image is a Daytona snapshot name. The server sends it as it is. The snapshot must contain the desktop: build it from the built-in Dockerfile.

## CPUs and memory

A box gets 2 CPUs, 4 GB of memory, and 10 GB of disk unless it asks for other values. Daytona's own default of 1 GB is too small for Chromium. `--memory` is rounded up to whole GB. The organization sets the most for each box.

## Viewers and page tools

Each published port gets a signed preview URL, with the access token in the host name. A browser opens it with no header. The URL is valid for 24 hours. The viewers still need their own token.

Page tools reach Chromium through the bridge in the box on port 9223. The bridge refuses each request without the box's secret. When a restarted server takes a box back, it restarts the bridge with a new secret.

## Lifetimes

Each sandbox stops when it has no activity for its TTL, and Daytona deletes it when it stops. While a box is in use, the server reports activity to Daytona. Set the server's limits on the runtime:

```toml
[runtimes.daytona]
lifetime = "1h"
max_lifetime = "8h"
```

## Use Daytona from Rust

Build with the `daytona` feature, and set `DAYTONA_API_KEY`.

```rust
use computer::sandboxes::{daytona::cloud::Cloud, remote};
use computer::{Auth, Computer, X11Profile};

let (machine, profile) = remote::pair(Arc::new(Cloud::from_env()?), Arc::new(X11Profile));

let computer = Computer::builder()
    .machine(Arc::new(machine.public_viewer(true)))
    .profile(profile)
    .auth(Auth::Token)
    .launch()
    .await?;
```

| Setting | Effect |
| --- | --- |
| No `image` | Send the built-in image as a Dockerfile. |
| `.image("<snapshot>")` | Start from this Daytona snapshot. |
| `.image_dir(path)` | Send this directory as a Dockerfile. |
| `Cloud::at(url)` | Another Daytona API URL. |
| `Cloud::target(region)` | The region. |

Run the example. With no argument, it uses the built-in image. It also takes a snapshot name or an image directory:

```bash
cargo run --features daytona --example daytona
cargo run --features daytona --example daytona -- path/to/image-dir --keep
```
