//! cargo run --example demo -- media/demo.gif

use computer::{Button, Computer, Delta, Point};
use std::time::Duration;

const FRAMES: &str = "/tmp/demo";

struct Step {
    caption: &'static str,
    settle: u64,
}

#[tokio::main]
async fn main() -> computer::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "demo.gif".into());

    println!("opening a box …");
    let computer = Computer::builder().launch().await?;
    let screen = computer.primary();

    computer.exec(["rm", "-rf", FRAMES]).await?;
    computer.exec(["mkdir", "-p", FRAMES]).await?;

    let mut frame = 0usize;
    let shot =
        async |step: Step, from: &computer::Screen, frame: &mut usize| -> computer::Result<()> {
            tokio::time::sleep(Duration::from_millis(step.settle)).await;

            let image = from.screenshot().await?;
            let raw = format!("{FRAMES}/{:02}-raw.png", *frame);
            computer.write_file(&raw, &image).await?;

            // Two passes, so the caption sits on new canvas under the frame.
            let captioned = format!("{FRAMES}/{:02}.png", *frame);
            let annotate = computer
                .exec([
                    "convert",
                    &raw,
                    "-resize",
                    "900",
                    "-background",
                    "#111113",
                    "-fill",
                    "#e8e8ea",
                    "-font",
                    "DejaVu-Sans-Mono",
                    "-pointsize",
                    "17",
                    "-gravity",
                    "south",
                    "-splice",
                    "0x40",
                    "-annotate",
                    "+0+11",
                    step.caption,
                    &captioned,
                ])
                .await?;

            if !annotate.ok() {
                return Err(computer::Error::denied(format!(
                    "convert failed: {}",
                    annotate.stderr_utf8().trim()
                )));
            }

            println!("  {:>2}  {}", *frame, step.caption);
            *frame += 1;
            Ok(())
        };

    shot(
        Step {
            caption: "let computer = Computer::launch().await?;",
            settle: 500,
        },
        screen,
        &mut frame,
    )
    .await?;

    computer.open_url("https://example.com").await?;
    shot(
        Step {
            caption: "computer.open_url(\"https://example.com\").await?;",
            settle: 5_000,
        },
        screen,
        &mut frame,
    )
    .await?;

    screen.press("ctrl+l").await?;
    screen
        .type_text("en.wikipedia.org/wiki/Rust_(programming_language)")
        .await?;
    shot(
        Step {
            caption: "computer.key(\"ctrl+l\").await?;  computer.type_text(url).await?;",
            settle: 900,
        },
        screen,
        &mut frame,
    )
    .await?;

    screen.press("enter").await?;
    shot(
        Step {
            caption: "computer.key(\"enter\").await?;",
            settle: 6_000,
        },
        screen,
        &mut frame,
    )
    .await?;

    for _ in 0..2 {
        screen.scroll(Point::new(640, 500), Delta::down(5)).await?;
    }
    shot(
        Step {
            caption: "computer.scroll((640, 500), Delta::down(5)).await?;",
            settle: 900,
        },
        screen,
        &mut frame,
    )
    .await?;

    screen
        .drag(Point::new(360, 300), Point::new(900, 320), Button::Left)
        .await?;
    shot(
        Step {
            caption: "computer.drag((360, 300), (900, 320), Button::Left).await?;",
            settle: 900,
        },
        screen,
        &mut frame,
    )
    .await?;

    screen.click(Point::new(640, 400), Button::Right).await?;
    shot(
        Step {
            caption: "computer.click((640, 400), Button::Right).await?;",
            settle: 1_200,
        },
        screen,
        &mut frame,
    )
    .await?;

    screen.press("escape").await?;
    computer.set_clipboard("driven from rust").await?;
    screen.press("ctrl+l").await?;
    screen.press("ctrl+v").await?;
    shot(
        Step {
            caption: "computer.set_clipboard(text).await?;  computer.key(\"ctrl+v\").await?;",
            settle: 1_000,
        },
        screen,
        &mut frame,
    )
    .await?;

    let second = computer.screen(computer::ScreenId(1)).await?;
    second.open_url("https://doc.rust-lang.org/book/").await?;
    shot(
        Step {
            caption: "let second = computer.screen(ScreenId(1)).await?;  // its own desktop",
            settle: 7_000,
        },
        &second,
        &mut frame,
    )
    .await?;

    println!("  stitching …");
    let gif = computer
        .exec_within(
            [
                "convert",
                "-delay",
                "170",
                "-loop",
                "0",
                &format!("{FRAMES}/*[0-9].png"),
                "-layers",
                "Optimize",
                "-colors",
                "128",
                "/tmp/demo.gif",
            ],
            Duration::from_secs(180),
        )
        .await?;

    if !gif.ok() {
        return Err(computer::Error::denied(format!(
            "convert failed: {}",
            gif.stderr_utf8().trim()
        )));
    }

    computer.download("/tmp/demo.gif", &out).await?;
    println!("\n{frame} frames → {out}");

    computer.shutdown().await
}
