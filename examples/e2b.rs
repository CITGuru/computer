//! The same desktop, in an E2B sandbox instead of on this host.
//!
//! ```text
//! export E2B_API_KEY=...
//! cargo run --features e2b --example e2b -- <template-id> [--keep]
//! ```
//!
//! `--keep` leaves the sandbox running so the viewer URL is worth opening.
//! Without it the box goes away at the end and the URL dies with it.
//!
//! Build the template once. E2B builds templates and this crate builds
//! container images, and its builder is a Docker subset that this image does
//! not clear unchanged, so `images/context.py` derives one that does:
//!
//! ```text
//! python3 images/context.py images/desktop /tmp/e2b-ctx --for e2b
//! e2b template create computer-desktop -p /tmp/e2b-ctx -d Dockerfile \
//!     -c "/usr/local/bin/computer-desktop" --ready-cmd "true" \
//!     --cpu-count 2 --memory-mb 2048
//! ```
//!
//! Three things differ, and none of them is the driving:
//!
//! 1. A port is published as a subdomain rather than forwarded to a host port,
//!    so the viewer URL is the sandbox's own host.
//! 2. DevTools does not reach. The profile withdraws the claim rather than
//!    publishing a port nothing out here can open.
//! 3. The screen still has no password, and a sandbox URL is on the internet.
//!    `public_viewer(true)` below is what trades a watchable screen for that,
//!    and it is off by default.

use computer::sandboxes::e2b::{self, cloud::Cloud};
use computer::{Button, Computer, Point, X11Profile};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> computer::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keep = args.iter().any(|arg| arg == "--keep");

    let Some(template) = args.iter().find(|arg| !arg.starts_with("--")).cloned() else {
        eprintln!("usage: e2b <template-id> [--keep]");
        std::process::exit(2);
    };

    let (machine, profile) = e2b::pair(Arc::new(Cloud::from_env()?), Arc::new(X11Profile));

    println!("starting a sandbox …");
    let computer = Computer::builder()
        .machine(Arc::new(
            machine
                .public_viewer(true)
                .expiring_after(Duration::from_secs(15 * 60)),
        ))
        .profile(profile)
        .image(&template)
        .keep_on_drop(keep)
        .launch()
        .await?;

    println!("  runtime  {}", computer.runtime());
    match computer.viewer_url() {
        Some(url) => println!("  watch it {url}"),
        None => println!("  no viewer: this sandbox is secure"),
    }

    computer.open_url("https://example.com").await?;
    tokio::time::sleep(Duration::from_secs(4)).await;

    let frame = computer.screenshot().await?;
    std::fs::write("e2b.png", &frame).ok();
    println!("  screenshot: {} bytes -> e2b.png", frame.len());

    computer.click(Point::new(640, 400), Button::Left).await?;
    println!("  cursor: {:?}", computer.cursor().await?);

    let geometry = computer.primary().geometry().await?;
    println!("  geometry: {geometry:?}");

    if keep {
        // The deadline is what stops a kept box from running forever, so it is
        // worth saying out loud rather than leaving to be discovered.
        println!("\n  left running as {}", computer.name());
        println!("  it goes away on its own when its deadline runs out");
        return Ok(());
    }

    computer.shutdown().await
}
