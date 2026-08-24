//! Drive a box somebody else opened.
//!
//! ```text
//! cargo run --example serve
//! cargo run --example attach -- <box-name> pawrly.dev
//! ```
//!
//! A box picked up this way keeps its windows, its browser profile and its
//! files, and it is not taken away when this program ends: this process
//! did not create it.

use computer::Computer;
use std::time::Duration;

#[tokio::main]
async fn main() -> computer::Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args
        .next()
        .expect("usage: attach <box-name> <what to type>");
    let typed: String = args.collect::<Vec<_>>().join(" ");

    let computer = Computer::attach(&name).await?;
    println!("attached to {}", computer.name());

    let presence = computer.probe().await;
    println!(
        "  display={} browser={}",
        presence.display, presence.browser
    );

    if !typed.is_empty() {
        // A new tab and the address bar by keyboard, not by coordinates: a
        // click at a guessed pixel lands wherever the window happens to be.
        computer.key("ctrl+t").await?;
        tokio::time::sleep(Duration::from_millis(600)).await;

        computer.key("ctrl+l").await?;
        tokio::time::sleep(Duration::from_millis(300)).await;

        computer.type_text(&typed).await?;
        tokio::time::sleep(Duration::from_millis(400)).await;
        computer.key("enter").await?;
        println!("  typed {typed:?} and pressed enter");

        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    let frame = computer.screenshot().await?;
    let out = std::env::var("COMPUTER_SHOT").unwrap_or_else(|_| "attached.png".to_string());
    std::fs::write(&out, &frame).ok();
    println!("  {} bytes → {out}", frame.len());

    Ok(())
}
