//! The smallest useful thing: open a box, look at it, drive it.
//!
//! ```text
//! cargo run --example quickstart
//! ```
//!
//! The first run builds the image, which takes minutes. Every run after it
//! starts in a few seconds.

use computer::{Button, Computer, Point};

#[tokio::main]
async fn main() -> computer::Result<()> {
    println!("opening a box (the first run builds the image) …");
    let computer = Computer::launch().await?;

    if let Some(url) = computer.viewer_url() {
        println!("watch it at {url}");
    }

    computer.open_url("https://example.com").await?;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Coordinates are device pixels against the frame this returns, with the
    // origin at the top left.
    let frame = computer.screenshot().await?;
    std::fs::write("screen.png", &frame).ok();
    println!("captured {} bytes to screen.png", frame.len());

    computer.click(Point::new(640, 81), Button::Left).await?;
    computer.type_text("driven from rust").await?;

    // The pointer is never in a screenshot, so this is the only way to know.
    let at = computer.cursor().await?;
    println!("the pointer is at {},{}", at.x, at.y);

    computer.shutdown().await
}
