//! ```text
//! export E2B_API_KEY=...
//! cargo run --features e2b --example e2b -- <template-id> [--keep]
//! ```
//!
//! `--keep` leaves the sandbox running. Build the template once with:
//!
//! ```text
//! python3 images/context.py images/desktop /tmp/e2b-ctx --for e2b
//! e2b template create computer-desktop -p /tmp/e2b-ctx -d Dockerfile \
//!     -c "/usr/local/bin/computer-desktop" --ready-cmd "true" \
//!     --cpu-count 2 --memory-mb 2048
//! ```

use computer::sandboxes::e2b::{self, cloud::Cloud};
use computer::{Auth, Button, Computer, Point, X11Profile};
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
        .auth(Auth::Token)
        .keep_on_drop(keep)
        .launch()
        .await?;

    println!("  runtime  {}", computer.provider());
    match computer.viewer_url() {
        Some(url) => println!("  watch it {url}"),
        None => println!("  no viewer: this sandbox is secure"),
    }

    computer.open_url("https://example.com").await?;
    tokio::time::sleep(Duration::from_secs(4)).await;

    let frame = computer.screenshot().await?;
    std::fs::write("e2b.png", &frame).ok();
    println!("  screenshot: {} bytes -> e2b.png", frame.len());

    match computer.browser() {
        Some(devtools) => {
            let started = std::time::Instant::now();
            let pages = devtools.pages().await?;
            let target = pages
                .iter()
                .find(|page| page.url.contains("example.com"))
                .or(pages.first())
                .ok_or_else(|| computer::Error::denied("the browser has no page"))?;
            let mut page = devtools.attach(target).await?;
            let snapshot = page.snapshot(None, Some(20)).await?;
            println!(
                "  devtools: {:?} with {} controls in {:?}",
                snapshot.title,
                snapshot.total,
                started.elapsed()
            );
        }
        None => println!("  devtools: none reaches this box"),
    }

    computer.click(Point::new(640, 400), Button::Left).await?;
    println!("  cursor: {:?}", computer.cursor().await?);

    let geometry = computer.primary().geometry().await?;
    println!("  geometry: {geometry:?}");

    if keep {
        println!("\n  left running as {}", computer.name());
        println!("  it goes away on its own when its deadline runs out");
        return Ok(());
    }

    computer.shutdown().await
}
