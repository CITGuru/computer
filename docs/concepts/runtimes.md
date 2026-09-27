# Runtimes

A runtime is where a box runs. All desktop operations are the same on every runtime. Startup time, isolation, image handling, networking, and lifecycle support are different.

## Two properties

Each runtime has a **place** and an **environment**.

| Place | Meaning |
| --- | --- |
| Host | The box runs on the same machine as the server. The server finds the runtime at start. |
| Remote | The box runs at a cloud vendor. You configure the runtime, because it needs an API key. |

| Environment | Meaning |
| --- | --- |
| Container | The box shares the host kernel, in its own namespaces. |
| MicroVM | The box has its own kernel. The isolation is stronger, and startup is slower. |
| VM | The box is in a full virtual machine. |
| Unknown | The provider does not say. |

The two properties are independent. For example, Docker gives a container on runc, but a microVM when its default runtime is Kata.

## Available runtimes

| Runtime | Place | Environment | Notes |
| --- | --- | --- | --- |
| `docker` | Host | Container | The default. |
| `podman` | Host | Container | |
| `nerdctl` | Host | Container | containerd. |
| `smolvm` | Host | MicroVM | libkrun: Hypervisor.framework on macOS, KVM on Linux. |
| `microsandbox` | Host | MicroVM | libkrun, through the `msb` CLI. |
| `e2b` | Remote | MicroVM | Firecracker. Needs an API key. A source build needs the `e2b` feature. |

To see the runtimes a server has, and what each can do:

```bash
computer runtime ls
curl -s localhost:8080/v1/runtimes
```

An MCP agent calls `list_runtimes`.

## Select a runtime

Name the runtime when you create a box. With no name, the box goes on the server's default, which is `docker` unless you change it.

```bash
computer new --runtime smolvm
```

```json
{ "spec": {}, "placement": { "runtime": "smolvm" } }
```

MCP: `launch_box` with `runtime: "smolvm"`.

The server refuses a name that it does not have, and gives the names that it has. A box stays on its runtime for its full life. A fork goes on the same runtime.

From the Rust library with no server, `runtime()` selects a container engine:

```rust
let computer = Computer::builder().runtime("podman").launch().await?;
```

For a microVM or a cloud sandbox, give the builder a `machine()`. See `examples/microvm.rs` and `examples/e2b.rs`.

## Host runtimes

At start, `computerd` offers each host runtime whose program answers. To offer fewer, set `COMPUTER_SERVER_RUNTIMES`:

```bash
COMPUTER_SERVER_RUNTIMES=docker,smolvm computerd
```

Configure an engine with its own settings where you can:

- `DOCKER_HOST` or `docker context` point Docker at a different host.
- `default-runtime` in `daemon.json` puts all Docker containers on gVisor or Kata.

### The runtimes file

Other settings go in a TOML file. Set `COMPUTER_SERVER_CONFIG` to its path. The server reads it at start, so a change needs a restart.

```toml
default = "hardened"

[runtimes.docker]
memory = "4g"

[runtimes.smolvm]
program = "/opt/smolvm/bin/smolvm"
memory = "4g"

[runtimes.hardened]
provider = "docker"
isolation = "runsc"

[runtimes.gpu-host]
provider = "docker"
context = "gpu-1"

[runtimes.podman]
enabled = false
```

- A table with no `provider` changes the runtime with that name.
- A table with a `provider` adds a new runtime on that engine. Use this to offer one engine two times with different settings.
- `default` is the runtime for a box that names none.

| Field | Runtimes | Effect |
| --- | --- | --- |
| `enabled` | All host | `false` hides a runtime that the server found. |
| `memory`, `cpus` | All | The default for each box. A placement can override it. |
| `lifetime` | All | The lifetime of a box that does not give one. Default one hour. |
| `max_lifetime` | All | The longest lifetime a box can ask for. |
| `isolation` | docker, podman, nerdctl | The OCI runtime for each box, such as `runsc` for gVisor. |
| `context` | docker | A Docker context from `docker context create`. |
| `program` | smolvm, microsandbox | The hypervisor program, when it is not on the `PATH`. |
| `build_with` | smolvm, microsandbox | The engine that builds the image. Default `docker`. |

Write times as `24h`, `90m`, `3600s`, or a number of seconds. The server refuses an unknown field.

## Remote runtimes

A remote runtime is a vendor account with a name. You can have more than one for a vendor, such as two E2B accounts.

There are three ways to add one.

**Environment**, for a runtime named after the vendor:

```bash
COMPUTER_SERVER_SANDBOXES=e2b E2B_API_KEY=... computerd
```

**Runtimes file**, to set its lifetimes:

```toml
[runtimes.e2b]
lifetime = "4h"
max_lifetime = "24h"
```

**CLI or API**, while the server runs:

```bash
printf '%s' "$E2B_API_KEY" | computer runtime add cloud --provider e2b --api-key
computer runtime set cloud --field max_lifetime_secs=86400
computer runtime rm cloud
```

The CLI reads the key from standard input, or from a variable with `--api-key-env`. The key does not go on the command line, where `ps` and the shell history can show it.

A key added through the CLI or API:

- Is encrypted before the server stores it. The server key comes from `COMPUTER_SERVER_SECRET_KEY`, or from the file at `COMPUTER_SERVER_SECRET_FILE`, which the server makes if it does not exist. With neither, the server refuses to store a key.
- Is never returned. `GET /v1/runtimes` gives only the names of the secrets, such as `["api_key"]`.
- Moves to the running boxes when you replace it, so they continue to work.

If the server cannot decrypt a stored key, the runtime shows `state: unavailable` with the reason.

The API can add only remote vendors. It cannot name a host program, socket, or engine. A self-hosted vendor endpoint added through the API must use `https` and a public address. The runtimes file can name any endpoint.

MCP tools cannot add a runtime, because a key given to an agent goes through its context.

## Lifecycle support

| Runtime | Pause (memory kept) | Stop (files kept) | Browser profile |
| --- | --- | --- | --- |
| docker, podman, nerdctl | Yes | Yes | Yes |
| smolvm | No | Yes | No |
| e2b | Yes | No | No |

After a stop, a start gives a new desktop with new ports and a new viewer URL.

A browser profile needs a Docker volume, so only container runtimes support `--profile`. On other runtimes, use `save_state` and `load_state` to move a login between boxes.

On E2B, memory and CPUs are set when the template is built, not when the box is created.

On E2B, every port of a box refuses a request without the sandbox's traffic token, so a browser cannot open the viewer URL directly: watch and take over through `computerd`. Add a runtime with `--field public_traffic=true` to open the ports and get direct viewer and takeover URLs back.

On E2B, page tools reach Chromium through a bridge in the box. See [Control modes](control-modes.md#browser-mode-on-remote-runtimes).

A runtime says what it can do in `GET /v1/runtimes/{name}` under `can`. The server refuses a request that the runtime cannot do before it starts the box.

## Images

Each runtime gets its image in a different way:

- **Container engine:** the server builds the image on the host from the bundled Dockerfile.
- **MicroVM:** a hypervisor cannot read an engine's images. The server builds the image with `build_with`, then gives it to the hypervisor one time. Later boxes start from that copy.
- **E2B:** the server makes an E2B template from the bundled image, uploads the files, and waits for the build. It makes one template for each box specification.

Each set of applications and packages is a different image. The first box with a new set waits for a build. To build before a box needs it:

```bash
computer image build smolvm --app vscode
computer image ls
```

## Lifetime

A box lives one hour unless it asks for a different time. Set `lifetime` in the runtimes file to change the default, and `max_lifetime` to limit what a box can ask for. The server refuses a request above the limit, and gives both numbers.

Give a box its own lifetime:

- CLI: `--ttl MINUTES` and `--idle MINUTES`
- REST: `expires_after_secs` and `idle_timeout_secs` in the placement. Values under 60 seconds are refused.
- MCP: `ttl_minutes` and `idle_minutes` on `launch_box`

## Restart

When `computerd` restarts, it takes back each box that it recorded, through the runtime in the record. Then it scans each runtime for boxes that an earlier server labeled. A box whose runtime is missing shows `unreachable` with the reason, and its record stays.

## Display servers

Separate from the runtime, a box runs X11 (the default) or Wayland:

```bash
computer new --wayland
```

| Display | Components |
| --- | --- |
| X11 | Xvfb, fluxbox, x11vnc, ImageMagick, xdotool |
| Wayland | Headless sway, wayvnc, grim, and a virtual pointer and keyboard for each screen |

The desktop API is the same for both. On Wayland, `cursor` cannot read the pointer after a person moves it. It returns `Unsupported` until the agent moves the pointer again.
