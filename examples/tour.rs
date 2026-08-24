//! A long drive: two screens, real pages, and the evidence assembled in the box.
//!
//! ```text
//! cargo run --example tour                     # open a box, drive it, keep it
//! cargo run --example tour -- <box-name>       # drive one that is already up
//! ```
//!
//! What it exercises, in order: attaching, keyboard navigation, find-in-page,
//! scrolling, double-click and drag selection, a context menu, a second screen
//! with its own browser and its own pointer, capture from both, ImageMagick
//! inside the box, and a file coming back out.
//!
//! Every frame comes back through `screenshot`, so the recording is the API
//! working rather than a picture of it. The pointer is never in a frame —
//! `import` captures the root window, which does not include the cursor — so
//! each step moves something that is visible instead.

use computer::{Button, Computer, Delta, Point, ScreenId};
use std::time::{Duration, Instant};

const FRAMES: &str = "/tmp/tour";
const GIF: &str = "/tmp/tour.gif";
const SIDE_BY_SIDE: &str = "/tmp/tour-screens.png";

#[tokio::main]
async fn main() -> computer::Result<()> {
    let started = Instant::now();

    let computer = match std::env::args().nth(1) {
        Some(name) => {
            println!("attaching to {name} …");
            Computer::attach(name).await?
        }
        None => {
            println!("opening a box …");
            Computer::builder().keep_on_drop(true).launch().await?
        }
    };

    if let Some(url) = computer.viewer_url() {
        println!("  watch along: {url}\n");
    }

    let mut film = Film::new(&computer);
    computer.exec(["rm", "-rf", FRAMES]).await?;
    computer.exec(["mkdir", "-p", FRAMES]).await?;

    tour(&computer, &mut film).await?;
    let second = second_screen(&computer, &mut film).await?;
    assemble(&computer, &second).await?;

    // The screen goes away and the box stays: the caller may still want it.
    computer.close_screen(ScreenId(1)).await?;

    println!(
        "\n{} frames in {:.1}s → tour.gif, tour-screens.png",
        film.count,
        started.elapsed().as_secs_f32()
    );
    println!("  the box is still up: {}", computer.name());
    Ok(())
}

/// The whole drive on screen 0.
async fn tour(computer: &Computer, film: &mut Film<'_>) -> computer::Result<()> {
    let screen = computer.primary();

    // A tab and an address by keyboard, not by coordinates: a click at a
    // guessed pixel lands wherever the window happens to be.
    screen.key("ctrl+t").await?;
    settle(600).await;
    screen.key("ctrl+l").await?;
    screen.type_text("pawrly.dev").await?;
    screen.key("enter").await?;
    settle(5_000).await;
    film.take("the page").await?;

    // Find-in-page. The match highlights, which is the change a frame can show.
    screen.key("ctrl+f").await?;
    settle(500).await;
    screen.type_text("SQL").await?;
    settle(800).await;
    film.take("found in page").await?;
    screen.key("escape").await?;
    settle(400).await;

    // Two words of the heading, selected by dragging across them.
    screen
        .drag(Point::new(320, 375), Point::new(950, 380), Button::Left)
        .await?;
    settle(600).await;
    film.take("dragged a selection").await?;

    // One word, taken by a double click.
    screen
        .double_click(Point::new(600, 556), Button::Left)
        .await?;
    settle(600).await;
    film.take("double-clicked a word").await?;

    // The pointer is not in any of these frames, so this is the only way to
    // know where it is.
    let at = screen.cursor().await?;
    println!("  the pointer is at {},{}", at.x, at.y);

    for notch in 1..=3 {
        screen.scroll(Point::new(640, 500), Delta::down(4)).await?;
        settle(500).await;
        film.take(&format!("scrolled {notch}")).await?;
    }

    screen.key("End").await?;
    settle(1_200).await;
    film.take("the foot of the page").await?;

    // Chromium's own menu is the largest visible change this desktop can be
    // asked for.
    screen.click(Point::new(400, 500), Button::Right).await?;
    settle(800).await;
    film.take("context menu").await?;
    screen.key("escape").await?;
    settle(400).await;

    screen.key("Home").await?;
    settle(800).await;
    film.take("back to the top").await?;

    Ok(())
}

/// A second display, with its own browser and its own pointer.
async fn second_screen(
    computer: &Computer,
    film: &mut Film<'_>,
) -> computer::Result<computer::LeasedScreen> {
    println!("  starting screen 1 …");
    let second = computer.screen(ScreenId(1)).await?;
    println!("  screen 1 is on {}", second.display());

    second.open_url("https://news.ycombinator.com").await?;
    settle(6_000).await;

    // Two pointers, and neither is the other. If one position appeared on both
    // screens they would be one display under two names.
    second.move_to(Point::new(200, 300)).await?;
    computer.primary().move_to(Point::new(900, 600)).await?;
    println!(
        "  screen 1 pointer {:?}, screen 0 pointer {:?}",
        second.cursor().await?,
        computer.primary().cursor().await?
    );

    second.scroll(Point::new(640, 400), Delta::down(3)).await?;
    settle(600).await;

    let frame = second.screenshot().await?;
    computer
        .write_file(format!("{FRAMES}/screen-1.png"), &frame)
        .await?;
    println!("  screen 1: {} bytes", frame.len());

    // The last frame of the film is the second screen, so the recording ends
    // where the drive did.
    film.write(frame, "screen 1").await?;

    Ok(second)
}

/// Stitch the evidence inside the box, and bring it out.
///
/// ImageMagick is in the image and is not required on the host, which is the
/// sort of thing a box is for.
async fn assemble(computer: &Computer, second: &computer::LeasedScreen) -> computer::Result<()> {
    println!("  stitching inside the box …");

    let gif = computer
        .exec([
            "convert",
            "-delay",
            "80",
            "-loop",
            "0",
            &format!("{FRAMES}/*.png"),
            GIF,
        ])
        .await?;
    if !gif.ok() {
        return Err(computer::Error::denied(format!(
            "convert failed: {}",
            gif.stderr_utf8().trim()
        )));
    }

    // Both screens in one image, side by side.
    let now = computer.primary().screenshot().await?;
    computer
        .write_file(format!("{FRAMES}/final-0.png"), &now)
        .await?;
    let other = second.screenshot().await?;
    computer
        .write_file(format!("{FRAMES}/final-1.png"), &other)
        .await?;

    let joined = computer
        .exec([
            "convert",
            &format!("{FRAMES}/final-0.png"),
            &format!("{FRAMES}/final-1.png"),
            "+append",
            "-resize",
            "1600",
            SIDE_BY_SIDE,
        ])
        .await?;
    if !joined.ok() {
        return Err(computer::Error::denied(format!(
            "append failed: {}",
            joined.stderr_utf8().trim()
        )));
    }

    let out_gif = std::env::var("TOUR_GIF").unwrap_or_else(|_| "tour.gif".to_string());
    let out_png = std::env::var("TOUR_PNG").unwrap_or_else(|_| "tour-screens.png".to_string());
    computer.download(GIF, &out_gif).await?;
    computer.download(SIDE_BY_SIDE, &out_png).await?;

    Ok(())
}

/// The frames, numbered in the order they were taken.
struct Film<'a> {
    computer: &'a Computer,
    count: usize,
}

impl<'a> Film<'a> {
    fn new(computer: &'a Computer) -> Self {
        Self { computer, count: 0 }
    }

    async fn take(&mut self, label: &str) -> computer::Result<()> {
        let frame = self.computer.screenshot().await?;
        self.write(frame, label).await
    }

    async fn write(&mut self, frame: Vec<u8>, label: &str) -> computer::Result<()> {
        let path = format!("{FRAMES}/{:03}.png", self.count);
        self.computer.write_file(&path, &frame).await?;
        println!("  {:>3}  {label:<24} {} bytes", self.count, frame.len());
        self.count += 1;
        Ok(())
    }
}

/// Long enough for the window manager and the page to draw what was asked for.
/// A screenshot taken in the same millisecond catches the state before it.
async fn settle(millis: u64) {
    tokio::time::sleep(Duration::from_millis(millis)).await;
}
