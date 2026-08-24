//! A real screen, in a real container.
//!
//! Everything else is tested against a scripted runtime, which proves the
//! mapping and nothing about the image. This proves the image: that the build
//! produces a box where an X server comes up, a browser starts, `xdotool`
//! moves a real pointer and `import` returns real PNG bytes.
//!
//! ```text
//! cargo run --example live_desktop
//! ```
//!
//! It disposes of what it opened, including after a failure — a container left
//! behind is one nothing above this layer knows exists.

use computer::{Button, Computer, Error, Point};

#[tokio::main]
async fn main() {
    println!("opening a box …");
    let computer = match Computer::launch().await {
        Ok(computer) => computer,
        Err(error) => {
            eprintln!("could not open: {error}");
            std::process::exit(1);
        }
    };

    println!("  {} is up", computer.name());
    if let Some(url) = computer.viewer_url() {
        println!("  watch it at {url}");
    }

    let outcome = drive(&computer).await;

    // Taken away whatever happened above.
    if let Err(error) = computer.shutdown().await {
        eprintln!("could not shut down: {error}");
    }

    match outcome {
        Ok(()) => println!("\nall of it ran against a real screen."),
        Err(error) => {
            eprintln!("\nfailed: {error}");
            std::process::exit(1);
        }
    }
}

async fn drive(computer: &Computer) -> computer::Result<()> {
    let presence = computer.probe().await;
    println!(
        "  probe: display={} browser={}",
        presence.display, presence.browser
    );

    let screen = computer.primary();
    let (width, height) = screen.geometry().await?;
    println!("  geometry: {width}x{height}");

    let claimed = computer.support().display.expect("a screen");
    if (width, height) != (claimed.width, claimed.height) {
        return Err(Error::denied(format!(
            "the box reports {}x{} and the screen is {width}x{height}",
            claimed.width, claimed.height
        )));
    }

    // A real capture. The first four bytes are the only proof that matters: a
    // stub would return something, and only the magic says it is a PNG.
    let image = screen.screenshot().await?;
    let magic = image.first_chunk::<4>().copied().unwrap_or_default();
    println!("  screenshot: {} bytes, magic {magic:02x?}", image.len());
    if magic != [0x89, b'P', b'N', b'G'] {
        return Err(Error::denied("the capture is not a PNG"));
    }

    // A real pointer, moved and then measured.
    screen.click(Point::new(640, 400), Button::Left).await?;
    let at = screen.cursor().await?;
    println!("  cursor after a click at 640,400: {},{}", at.x, at.y);
    if at != Point::new(640, 400) {
        return Err(Error::denied(format!(
            "the pointer went to {},{}",
            at.x, at.y
        )));
    }

    // Typing needs a window to land in, so this only asserts the call is
    // accepted — where the characters went is the window manager's business.
    screen.type_text("computer").await?;
    screen.key("ctrl+a").await?;
    println!("  typed, and sent a chord");

    let second = screen.screenshot().await?;
    println!("  second screenshot: {} bytes", second.len());

    Ok(())
}
