# Modal

Modal runs each box in a gVisor container. You need no container runtime and no Docker on your host: Modal builds the image from Dockerfile lines that the server sends.

Modal has no REST API. This crate speaks Modal's gRPC API, which Modal says is not a public API: its messages can change without notice. If Modal changes them, this runtime can stop working until the crate is updated.

For how remote runtimes compare with host runtimes, see [Runtimes](../concepts/runtimes.md).

## What works

| Works | Not yet supported |
| --- | --- |
| Screenshots, mouse, keyboard, and waits | Pause and stop |
| Windows, applications, and native widgets | Browser profiles (`--profile`) |
| Files, commands, and clipboard | |
| Applications and packages at launch (`--app`), and applications installed into a running box | |
| Page tools and `holm cdp`, also after a server restart | A box that lives more than 24 hours |
| Viewer and takeover URLs in a browser | |
| CPUs and memory for each box | |
| Network policy (`--no-network`) | |

## 1. Get a token

Run `modal token new`, or create a token in the Modal dashboard under Settings, API Tokens. A token is a pair: an ID that starts with `ak-`, and a secret that starts with `as-`. Keep them out of shell history and out of files that go into git.

Release builds of `holm` and `holmd` include Modal. For a source build, add the `modal` feature:

```bash
cargo install --path . --locked --features modal
```

## 2. Add Modal to the server

**From the environment.** The runtime is named `modal`:

```bash
MODAL_TOKEN_ID=ak-... MODAL_TOKEN_SECRET=as-... HOLM_SERVER_SANDBOXES=modal holmd
```

**At run time, with the CLI.** The secret goes in as the runtime's key, and the ID as a field:

```bash
holm runtime add cloud --provider modal --field token_id=ak-... --api-key-env MODAL_TOKEN_SECRET
holm runtime ls
```

| Variable or field | Effect |
| --- | --- |
| `MODAL_TOKEN_ID`, or the `token_id` field | The token ID. Necessary. |
| `MODAL_TOKEN_SECRET`, or `api_key` | The token secret. Necessary. |
| `MODAL_ENVIRONMENT`, or the `environment` field | The Modal environment. Default: the workspace's default environment. |
| `MODAL_APP`, or the `app` field | The Modal app that holds the images and sandboxes. Default `holm`. The server creates it if it does not exist. |
| `MODAL_SERVER_URL`, or the `endpoint` field | The API address. Default `https://api.modal.com:443`. |

## 3. Create a box

```bash
BOX=$(holm new --runtime modal)
```

MCP: `launch_box` with `runtime: "modal"`. REST: `"placement": {"runtime": "modal"}`.

The first box waits for the image build, about one minute. Later boxes start in about 10 seconds.

## Images

### The built-in image

Modal builds an image from Dockerfile lines, with no build context. So the server changes each `COPY` of a file into a `RUN` that writes the file from base64, the same as for Daytona. Modal keeps each image by its recipe, so a later box with the same image starts from the image that exists.

### An image directory

An image directory (`image_dir` in the library) is sent the same way. See [Daytona](daytona.md#an-image-directory) for which `COPY` forms work.

### An image that you name

A named image is either a Modal image ID (it starts with `im-`), which is used as it is, or a public registry tag such as `debian:bookworm`, which Modal pulls. The image must contain the desktop: build it from the built-in Dockerfile.

## CPUs and memory

A box gets 2 CPUs and 4 GB of memory unless it asks for other values. `--cpus` takes fractions, such as `1.5`.

## Viewers and page tools

Modal publishes each port at a public HTTPS tunnel, with no gate of its own. The viewers need their own token. Page tools reach Chromium through the bridge in the box on port 9223, which refuses each request without the box's secret. When a restarted server takes a box back, it restarts the bridge with a new secret.

Commands and files go through Modal's command router, a separate address for each sandbox, with a short-lived token that the server refreshes.

## Lifetimes

A sandbox stops when it has no activity for the runtime's TTL, and it never lives more than 24 hours. That is Modal's limit, so the server does not accept a longer lifetime.

## Use Modal from Rust

Build with the `modal` feature, and set `MODAL_TOKEN_ID` and `MODAL_TOKEN_SECRET`.

```rust
use holm::sandboxes::{modal::cloud::Cloud, remote};
use holm::{Auth, Computer, X11Profile};

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
| No `image` | Build the built-in image at Modal. |
| `.image("im-...")` or `.image("<registry tag>")` | Start from this image. |
| `.image_dir(path)` | Build this directory at Modal. |
| `Cloud::environment(name)` | The Modal environment. |
| `Cloud::app(name)` | The Modal app. |

Run the example. With no argument, it uses the built-in image:

```bash
cargo run --features modal --example modal
cargo run --features modal --example modal -- path/to/image-dir --keep
```
