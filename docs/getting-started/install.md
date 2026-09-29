# Install

## Requirements

You need one runtime on the host:

| Runtime | Box type |
| --- | --- |
| Docker | Container (default) |
| Podman | Container |
| nerdctl | Container |
| smolvm | microVM |
| microsandbox | microVM |

You do not fetch a desktop image. The first box builds its image from source, which takes a few minutes. Later boxes with the same configuration start in seconds.

Cloud sandboxes such as E2B, Vercel, Daytona, and Modal need no local runtime, only a vendor API key. See [Runtimes](../concepts/runtimes.md).

## Install script

The script downloads a release build of `computer` and `computerd` for macOS or Linux on x86_64 or aarch64, and verifies its checksum:

```bash
curl -fsSL https://raw.githubusercontent.com/CITGuru/computer/main/scripts/install.sh | sh
```

| Setting | Flag | Default |
| --- | --- | --- |
| `COMPUTER_VERSION` | `--version <tag>` | The latest release |
| `COMPUTER_INSTALL_DIR` | `--dir <path>` | `~/.local/bin` |

Make sure that the install directory is on your `PATH`.

## Build from source

Install Rust 1.85 or newer, then:

```bash
git clone https://github.com/CITGuru/computer.git
cd computer
cargo install --path . --locked
```

Release builds include `e2b`, `vercel`, `daytona`, `modal`, `sqlite`, `postgres`, and `s3`. A source build adds them with features:

```bash
cargo install --path . --locked --features e2b,vercel,daytona,modal,microsandbox,sqlite,postgres,s3
```

| Feature | Adds |
| --- | --- |
| `e2b` | E2B cloud sandboxes |
| `vercel` | Vercel Sandbox |
| `daytona` | Daytona sandboxes |
| `modal` | Modal sandboxes |
| `microsandbox` | The microsandbox Rust library, for the library API. The server uses the `msb` CLI with no feature. |
| `sqlite`, `postgres`, `s3` | Durable storage for `computerd` |

## The two binaries

- `computer` controls boxes from a shell and serves MCP over stdio.
- `computerd` is the long-running server. It serves REST under `/v1` and MCP over HTTP at `/mcp`, and keeps traces, forks, and box expiry across commands.

You do not need `computerd` to start. When no server is running, `computer` starts one for the life of the command.

## Check the install

```bash
BOX=$(computer new)
computer screenshot "$BOX" screen.png
computer rm "$BOX"
```

The first `computer new` builds the image. When `screen.png` shows a desktop, the install is correct.

## Next

- [CLI quick start](quickstart-cli.md)
- [Rust quick start](quickstart-rust.md)
- [MCP quick start](quickstart-mcp.md)
