//! cargo run --example recording -- out.gif

use computer::{Button, Computer, Error, Point};
use std::time::Duration;

const FRAMES: &str = "/tmp/computer-frames";
const GIF: &str = "/tmp/computer.gif";

#[tokio::main]
async fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "computer.gif".into());

    println!("opening a box …");
    let computer = match Computer::launch().await {
        Ok(computer) => computer,
        Err(error) => {
            eprintln!("could not open: {error}");
            std::process::exit(1);
        }
    };

    if let Some(url) = computer.viewer_url() {
        println!("  watching along at {url}");
    }

    let outcome = record(&computer, &out).await;

    if let Err(error) = computer.shutdown().await {
        eprintln!("could not shut down: {error}");
    }

    match outcome {
        Ok(frames) => println!("\n{frames} frames → {out}"),
        Err(error) => {
            eprintln!("\nfailed: {error}");
            std::process::exit(1);
        }
    }
}

async fn record(computer: &Computer, out: &str) -> computer::Result<usize> {
    computer.exec(["mkdir", "-p", FRAMES]).await?;

    let screen = computer.primary();
    let mut frame = 0usize;

    // A screenshot in the same millisecond catches the state before it is drawn.
    let capture = async |label: &str, frame: &mut usize| -> computer::Result<()> {
        let image = screen.screenshot().await?;
        let path = format!("{FRAMES}/{:03}.png", *frame);
        computer.write_file(&path, &image).await?;
        println!("  {:>3}  {label:<28} {} bytes", *frame, image.len());
        *frame += 1;
        Ok(())
    };

    capture("the desktop as it starts", &mut frame).await?;

    computer.open_url("https://example.com").await?;
    settle(Duration::from_secs(4)).await;
    capture("a page", &mut frame).await?;

    // chromium comes up maximised at 0,38 1280x778, so the address bar sits at y≈81.
    screen.click(Point::new(640, 81), Button::Left).await?;
    settle(Duration::from_millis(400)).await;
    capture("address bar focused", &mut frame).await?;

    for chunk in ["a computer", " in a box", ", driven"] {
        screen.type_text(chunk).await?;
        settle(Duration::from_millis(400)).await;
        capture(&format!("typed {chunk:?}"), &mut frame).await?;
    }

    screen.press("ctrl+a").await?;
    settle(Duration::from_millis(400)).await;
    capture("select all", &mut frame).await?;

    screen.press("BackSpace").await?;
    settle(Duration::from_millis(400)).await;
    capture("cleared", &mut frame).await?;

    screen.click(Point::new(400, 500), Button::Right).await?;
    settle(Duration::from_millis(600)).await;
    capture("context menu", &mut frame).await?;

    screen.press("Escape").await?;
    settle(Duration::from_millis(400)).await;
    capture("dismissed", &mut frame).await?;

    println!("  stitching …");
    let convert = computer
        .exec([
            "convert",
            "-delay",
            "60",
            "-loop",
            "0",
            &format!("{FRAMES}/*.png"),
            GIF,
        ])
        .await?;

    if !convert.ok() {
        return Err(Error::denied(format!(
            "convert failed: {}",
            convert.stderr_utf8().trim()
        )));
    }

    computer.download(GIF, out).await?;
    Ok(frame)
}

async fn settle(how_long: Duration) {
    tokio::time::sleep(how_long).await;
}
