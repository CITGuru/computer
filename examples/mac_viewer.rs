//! Start a box and hold its viewer open, so the filtered port can be tried.

use computer::mac::{MacMachine, VIEWER_PORT, ViewerMode};
use computer::machine::Machine;
use computer::runtime::Config;
use std::time::Duration;

const NAME: &str = "computer-viewer";

#[tokio::main]
async fn main() -> computer::Result<()> {
    let image = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ghcr.io/cirruslabs/ubuntu:latest".to_string());

    let machine = match std::env::var("COMPUTER_VIEWER").as_deref() {
        Ok("screen-sharing") => MacMachine::default().viewing_with(ViewerMode::ScreenSharing),
        Ok("none") => MacMachine::default().without_viewer(),
        _ => MacMachine::default(),
    };
    let config = Config {
        image,
        bundle: None,
        image_dir: None,
        publish: Vec::new(),
        ..Config::default()
    };

    machine.preflight().await?;
    machine.ensure_image(&config).await?;

    let ports = machine.start(NAME, &config).await?;
    let viewer = machine.viewer(NAME);

    println!();
    match (ports.get(&VIEWER_PORT), viewer.as_ref()) {
        (Some(port), Some(viewer)) => {
            println!("  WATCH (input filtered out, loopback only)");
            println!("    vnc://127.0.0.1:{port}");
            println!("    password: {}", viewer.password.expose());
            println!();
            println!("  On macOS:  open vnc://127.0.0.1:{port}");
            println!();
            println!("  Tart's own server, which accepts input and is NOT filtered,");
            println!(
                "  is on {}:{} — and it binds every interface.",
                viewer.host, viewer.port
            );
            println!();
            println!("  A client reaching that port directly can still crash the guest:");
            println!("  the filter advertises DesktopSize for clients that omit it,");
            println!("  and nothing does that for a connection which bypasses it.");
        }
        _ => println!("  no viewer was announced"),
    }
    println!();
    println!("  Ctrl-C to take the box away.");
    println!();

    // The relay is in this process. Leaving keeps the port open; the caller
    // stops it, and `stop` below runs on the way out.
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}
