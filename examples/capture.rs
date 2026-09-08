//! Capture part of a screen, at a size.
//!
//! ```bash
//! cargo run --example capture -- <box>
//! ```

use computer::{Computer, Of, Point, Rect, Shot};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::args().nth(1).ok_or("usage: capture <box>")?;
    let computer = Computer::attach(&name).await?;
    let screen = computer.primary();

    let whole = screen.screenshot().await?;
    println!("whole screen        {} bytes", whole.len());

    let region = screen
        .capture(&Shot::region(Rect::new(Point::new(100, 80), 400, 300)))
        .await?;
    println!("400x300 at 100,80   {} bytes", region.len());

    let half = screen.capture(&Shot::of(Of::Screen).scaled(50)).await?;
    println!("half size           {} bytes", half.len());

    let windows = screen.windows().await?;
    for window in &windows {
        let shot = screen.capture(&Shot::window(&window.id)).await?;
        let small = screen.capture(&Shot::window(&window.id).scaled(25)).await?;
        println!(
            "{:<10} {}x{}  {} bytes, quarter size {} bytes",
            window.class,
            window.width,
            window.height,
            shot.len(),
            small.len()
        );
    }

    for wrong in [
        Shot::of(Of::Screen).scaled(0),
        Shot::of(Of::Screen).scaled(9_999),
        Shot::window("999"),
        Shot::region(Rect::new(Point::new(0, 0), 0, 10)),
    ] {
        match screen.capture(&wrong).await {
            Ok(_) => println!("refused nothing: {wrong:?}"),
            Err(error) => println!("refused: {error}"),
        }
    }

    Ok(())
}
