# Rust quick start

This page launches a desktop from a Rust program. You need Rust 1.85 or newer and a container runtime running.

## Add the dependency

Disable the default `cli` feature when your program needs only the library:

```toml
[dependencies]
computer = { git = "https://github.com/CITGuru/computer", default-features = false }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Launch a desktop

```rust
use computer::{Button, Computer, Point};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let computer = Computer::launch().await?;

    if let Some(url) = computer.viewer_url() {
        eprintln!("watch it at {url}");
    }

    computer.open_url("https://example.com").await?;
    computer.click(Point::new(640, 81), Button::Left).await?;
    computer.type_text("driven from rust").await?;

    let png = computer.screenshot().await?;
    std::fs::write("screen.png", &png)?;

    computer.shutdown().await?;
    Ok(())
}
```

`Computer::launch()` starts one 1280x800 screen on Docker. The first launch builds the image, which takes a few minutes.

`viewer_url()` returns a URL where you can watch the desktop in a browser. `shutdown()` stops and removes the box.

## Change the defaults

```rust
let computer = Computer::builder()
    .size(1920, 1080)
    .network(false)
    .memory("2g")
    .runtime("podman")
    .expires_after(std::time::Duration::from_secs(3600))
    .launch()
    .await?;
```

| Method | Effect |
| --- | --- |
| `size(w, h)` | Screen size |
| `network(false)` | Block outbound network access |
| `memory(size)` | Memory limit |
| `runtime(name)` | `docker`, `podman`, or `nerdctl` |
| `keep_on_drop(true)` | Keep the box when the handle is dropped |
| `expires_after(duration)` | Remove the box after a fixed time |
| `expires_when_idle(duration)` | Remove the box after a time with no use |

`Computer::builder().config()?` returns the resolved image, ports, environment, and boot command, and does not start a desktop.

## Optional features

| Feature | Adds |
| --- | --- |
| `e2b` | E2B cloud sandboxes |
| `vercel` | Vercel Sandbox |
| `daytona` | Daytona sandboxes |
| `modal` | Modal sandboxes |
| `smol` | smol cloud machines |
| `microsandbox` | microVM boxes |
| `sqlite`, `postgres`, `s3` | Storage backends for the server |

## Run the example

From a clone of this repository:

```bash
cargo run --example quickstart
```

The `examples/` directory has more programs, such as `browser.rs`, `takeover.rs`, and `microvm.rs`.
