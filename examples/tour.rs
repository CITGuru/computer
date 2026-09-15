//! ```text
//! cargo run --example tour                     # open a box, drive it, keep it
//! cargo run --example tour -- <box-name>       # drive one that is already up
//! ```

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

    computer.close_screen(ScreenId(1)).await?;

    println!(
        "\n{} frames in {:.1}s → tour.gif, tour-screens.png",
        film.count,
        started.elapsed().as_secs_f32()
    );
    println!("  the box is still up: {}", computer.name());
    Ok(())
}

async fn tour(computer: &Computer, film: &mut Film<'_>) -> computer::Result<()> {
    let screen = computer.primary();

    screen.press("ctrl+t").await?;
    settle(600).await;
    screen.press("ctrl+l").await?;
    screen.type_text("pawrly.dev").await?;
    screen.press("enter").await?;
    settle(5_000).await;
    film.take("the page").await?;

    screen.press("ctrl+f").await?;
    settle(500).await;
    screen.type_text("SQL").await?;
    settle(800).await;
    film.take("found in page").await?;
    screen.press("escape").await?;
    settle(400).await;

    screen
        .drag(Point::new(320, 375), Point::new(950, 380), Button::Left)
        .await?;
    settle(600).await;
    film.take("dragged a selection").await?;

    screen
        .double_click(Point::new(600, 556), Button::Left)
        .await?;
    settle(600).await;
    film.take("double-clicked a word").await?;

    let at = screen.cursor().await?;
    println!("  the pointer is at {},{}", at.x, at.y);

    for notch in 1..=3 {
        screen.scroll(Point::new(640, 500), Delta::down(4)).await?;
        settle(500).await;
        film.take(&format!("scrolled {notch}")).await?;
    }

    screen.press("End").await?;
    settle(1_200).await;
    film.take("the foot of the page").await?;

    screen.click(Point::new(400, 500), Button::Right).await?;
    settle(800).await;
    film.take("context menu").await?;
    screen.press("escape").await?;
    settle(400).await;

    screen.press("Home").await?;
    settle(800).await;
    film.take("back to the top").await?;

    Ok(())
}

async fn second_screen(
    computer: &Computer,
    film: &mut Film<'_>,
) -> computer::Result<computer::LeasedScreen> {
    println!("  starting screen 1 …");
    let second = computer.screen(ScreenId(1)).await?;
    println!("  screen 1 is on {}", second.display());

    second.open_url("https://news.ycombinator.com").await?;
    settle(6_000).await;

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

    film.write(frame, "screen 1").await?;

    Ok(second)
}

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

/// A screenshot in the same millisecond catches the state before it is drawn.
async fn settle(millis: u64) {
    tokio::time::sleep(Duration::from_millis(millis)).await;
}
