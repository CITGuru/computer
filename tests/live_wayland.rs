//! The Wayland image against a real container: `cargo test --test live_wayland -- --ignored`.

use computer::{Button, Computer, Delta, Point, Rect, ScreenId, Shot, WaylandProfile};
use std::sync::Arc;
use std::time::Duration;

fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let width = u32::from_be_bytes(png.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(png.get(20..24)?.try_into().ok()?);

    Some((width, height))
}

#[tokio::test]
#[ignore = "needs a container runtime, and builds the Wayland image"]
async fn a_real_wayland_box_does_everything_the_x11_one_does() {
    let computer = Computer::builder()
        .profile(Arc::new(WaylandProfile))
        .launch()
        .await
        .expect("a box");
    println!("  {} is up", computer.name());

    let outcome = exercise(&computer).await;

    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn exercise(computer: &Computer) -> computer::Result<()> {
    let presence = computer.probe().await;
    assert!(presence.ready(), "the box said it was ready: {presence:?}");

    let screen = computer.primary();
    assert_eq!(
        screen.geometry().await?,
        (1280, 800),
        "the compositor must be the size the descriptor claims"
    );

    let frame = screen.screenshot().await?;
    assert_eq!(
        frame.first_chunk::<4>(),
        Some(&[0x89, b'P', b'N', b'G']),
        "grim did not return a PNG"
    );
    println!("  screenshot: {} bytes", frame.len());

    let region = screen
        .capture(&Shot::region(Rect::new(Point::new(100, 80), 400, 300)))
        .await?;
    assert_eq!(png_size(&region), Some((400, 300)), "grim ignored -g");

    let quarter = screen.capture(&Shot::default().scaled(25)).await?;
    assert_eq!(
        png_size(&quarter),
        Some((320, 200)),
        "grim takes a factor, and a percentage handed to it asks for a \
         picture many times the screen"
    );
    println!(
        "  cropped to {} bytes, quartered to {}",
        region.len(),
        quarter.len()
    );

    screen.set_wallpaper(&frame).await?;
    println!("  wallpaper changed from uploaded image bytes");

    // No Wayland protocol reports the pointer, so this is the driver's own memory.
    screen.move_to(Point::new(100, 120)).await?;
    assert_eq!(screen.cursor().await?, Point::new(100, 120));

    browser(computer).await?;
    input(computer).await?;
    clipboard(computer).await?;
    takeover(computer).await?;
    second_screen(computer).await?;

    // Last: every `DesktopSupport` claim has to prove itself against a real box.
    let audit = computer::audit::audit_strictly(computer, Duration::from_secs(60)).await?;
    println!("  audit: {audit}");

    Ok(())
}

async fn browser(computer: &Computer) -> computer::Result<()> {
    computer.open_url("https://example.com").await?;

    // A screenshot before the window appears reads as a broken browser.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let windows = computer
        .exec_on(
            ScreenId(0),
            [
                "bash",
                "-c",
                "swaymsg -s \"$(cat /tmp/computer/screen-0.sway)\" -t get_tree",
            ],
        )
        .await?;

    assert!(
        windows.stdout_utf8().contains("Chromium"),
        "chromium has no window on the compositor: {}",
        windows.stderr_utf8()
    );
    println!("  chromium has a window on wayland-1");

    assert!(
        computer.devtools().is_some(),
        "the DevTools bridge is not reachable from out here"
    );
    Ok(())
}

/// Handlers go on through [`RECORDERS`]: a mis-escaped `data:` URL records nothing.
const PROBE: &str = "data:text/html,\
<input%20autofocus%20style=\"width:90%25;font-size:40px\">\
<div%20style=\"width:4000px;height:4000px\"></div>";

/// `moves` counts only motion with a button held, which is what makes a drag a drag.
const RECORDERS: &str = r#"
    window.seen = { click: null, down: null, up: null, moves: 0, doubles: 0 };
    onclick     = e => { seen.click = e.clientX + ',' + e.clientY; };
    onmousedown = e => { seen.down = e.clientX + ',' + e.clientY; };
    onmousemove = e => { if (e.buttons) { seen.moves++; } };
    onmouseup   = e => { seen.up = e.clientX + ',' + e.clientY; };
    ondblclick  = () => { seen.doubles++; };
    "installed"
"#;

/// Only x: screen and viewport coordinates differ vertically by the browser chrome.
fn column(recorded: &serde_json::Value) -> Option<u32> {
    recorded
        .as_str()?
        .split(',')
        .next()
        .and_then(|part| part.parse().ok())
}

/// Checked through DevTools: sway and `wtype` exit zero after doing nothing.
async fn input(computer: &Computer) -> computer::Result<()> {
    let devtools = computer
        .browser()
        .expect("the descriptor claims a DevTools endpoint");
    let mut page = devtools.open_page(PROBE, Duration::from_secs(30)).await?;
    page.wait_for_load(Duration::from_secs(20)).await?;

    // Coordinates reach the page in front, and this one opened behind it.
    page.bring_to_front().await?;
    assert!(
        page.visible().await?,
        "the probe page is not the one on screen"
    );

    // The window has to be mapped before anything is sent to it.
    tokio::time::sleep(Duration::from_secs(3)).await;
    page.evaluate(RECORDERS).await?;

    computer.type_text("KEYBOARD").await?;
    let typed = page
        .evaluate("document.querySelector('input').value")
        .await?;
    assert_eq!(
        typed.as_str(),
        Some("KEYBOARD"),
        "the keyboard dropped or mangled what it sent — a missing first \
         character means the keymap was not ready when the first key went out"
    );
    println!("  the keyboard delivered every character");

    computer.press("ctrl+a").await?;
    computer.type_text("replaced").await?;
    let after = page
        .evaluate("document.querySelector('input').value")
        .await?;
    assert_eq!(
        after.as_str(),
        Some("replaced"),
        "ctrl+a did not select, so the modifier never reached the page"
    );
    println!("  a chord selected, and the typing replaced it");

    computer.click(Point::new(700, 400), Button::Left).await?;
    let clicked = page.evaluate("seen.click").await?;
    assert_eq!(
        column(&clicked),
        Some(700),
        "no click reached the page, or it landed elsewhere ({clicked}): the \
         pointer is a device the compositor has to make, and a command that \
         only moves the seat's own cursor is accepted and does nothing"
    );
    println!("  a click landed at {clicked}");

    computer
        .scroll(Point::new(640, 500), Delta::down(5))
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let scrolled = page.evaluate("window.scrollY").await?;
    assert!(
        scrolled.as_f64().unwrap_or(0.0) > 0.0,
        "the page did not move, so the wheel notches never arrived"
    );
    println!("  the wheel scrolled the page to {scrolled}");

    // No held modifiers: this compositor's pointer makes no virtual keyboard.
    computer
        .primary()
        .wait_until_still(Duration::from_millis(300), Duration::from_secs(15))
        .await?;
    println!("  the screen settled rather than being slept on");

    let refused = computer
        .primary()
        .click_with(Point::new(10, 10), Button::Left, &[computer::Held::Shift])
        .await;
    assert!(
        refused.is_err(),
        "a modifier this compositor cannot hold has to be refused, not dropped"
    );

    computer
        .scroll(Point::new(640, 500), Delta::right(5))
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let sideways = page.evaluate("window.scrollX").await?;
    let still = page.evaluate("window.scrollY").await?;
    assert!(
        sideways.as_f64().unwrap_or(0.0) > 0.0,
        "the horizontal axis never arrived"
    );
    assert_eq!(
        still, scrolled,
        "a sideways scroll also moved the page down"
    );
    println!("  and sideways to {sideways}");

    page.evaluate("seen.down = seen.up = null; seen.moves = 0; 'reset'")
        .await?;
    computer
        .drag(Point::new(200, 300), Point::new(400, 380), Button::Left)
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let down = page.evaluate("seen.down").await?;
    let up = page.evaluate("seen.up").await?;
    let moves = page.evaluate("seen.moves").await?.as_u64().unwrap_or(0);

    assert_eq!(column(&down), Some(200), "the drag pressed at {down}");
    assert_eq!(column(&up), Some(400), "the drag released at {up}");
    assert!(
        moves >= 2,
        "the pointer teleported: {moves} moves with the button held, and a \
         drag through the middle should show at least the midpoint and the end"
    );
    println!("  a drag pressed at {down}, moved {moves} times, released at {up}");

    page.evaluate("seen.doubles = 0; 'reset'").await?;
    computer
        .double_click(Point::new(700, 400), Button::Left)
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let doubles = page.evaluate("seen.doubles").await?.as_u64().unwrap_or(0);
    assert_eq!(
        doubles, 1,
        "the page saw {doubles} double clicks: two presses too far apart are \
         two single clicks, which is a different gesture"
    );
    println!("  a double click arrived as one gesture");

    Ok(())
}

async fn clipboard(computer: &Computer) -> computer::Result<()> {
    let text = "clipboard \"round trip\", with a newline\nand a $dollar";

    computer.set_clipboard(text).await?;
    assert_eq!(computer.clipboard().await?, text);
    println!("  the clipboard holds what was put on it");
    Ok(())
}

async fn takeover(computer: &Computer) -> computer::Result<()> {
    let handed = computer.hand_over().await?;
    assert!(
        handed.url().is_some(),
        "a takeover with no URL is one nobody can reach"
    );

    assert!(
        computer
            .click(Point::new(10, 10), Button::Left)
            .await
            .is_err(),
        "the gate in this process let the owner act during a takeover"
    );

    let raw = computer
        .exec_on(ScreenId(0), ["computer-input", "move", "10", "10"])
        .await?;
    assert_eq!(
        raw.code,
        3,
        "computer-input let a caller past the takeover: {}",
        raw.stderr_utf8()
    );
    println!("  the box refuses input, not only the SDK");

    computer.screenshot().await?;

    handed.end().await?;
    assert!(
        matches!(
            computer.cursor().await,
            Err(computer::Error::Unsupported { .. })
        ),
        "a person drove the screen, so the tracked pointer is stale and \
         nothing in Wayland will say where it went"
    );

    computer.move_to(Point::new(50, 50)).await?;
    assert_eq!(computer.cursor().await?, Point::new(50, 50));
    println!("  the cursor is answerable again after a fresh move");
    Ok(())
}

async fn second_screen(computer: &Computer) -> computer::Result<()> {
    let second = computer.screen(ScreenId(1)).await?;
    assert_eq!(second.geometry().await?, (1280, 800));

    second.open_url("https://example.org").await?;
    let frame = second.screenshot().await?;
    assert_eq!(frame.first_chunk::<4>(), Some(&[0x89, b'P', b'N', b'G']));

    assert_ne!(
        second.viewer_url(),
        computer.viewer_url(),
        "two screens sharing a viewer port is one screen"
    );
    println!("  screen 1 came up beside screen 0");
    Ok(())
}
