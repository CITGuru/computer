//! The Wayland image against a real container. Ignored by default.
//!
//! ```text
//! cargo test --test live_wayland -- --ignored --nocapture
//! ```
//!
//! Nothing else in the suite builds this image. `tests/wayland_image.rs`
//! reads it as text and proves the code and the files agree about names, ports
//! and verbs; it cannot prove that sway comes up headless, that `wtype` finds
//! a virtual keyboard, or that chromium starts on Ozone. Only this can, and
//! only on a machine with a container runtime.
//!
//! One box for the whole test: opening one costs a build the first time and
//! seconds after that, and every step below is independent of the others.

use computer::{Button, Computer, Delta, Point, ScreenId, WaylandProfile};
use std::sync::Arc;
use std::time::Duration;

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

    // Taken away whatever happened above.
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

    // A real capture: only the magic says it is a PNG, since a stub would
    // return bytes too.
    let frame = screen.screenshot().await?;
    assert_eq!(
        frame.first_chunk::<4>(),
        Some(&[0x89, b'P', b'N', b'G']),
        "grim did not return a PNG"
    );
    println!("  screenshot: {} bytes", frame.len());

    screen.set_wallpaper(&frame).await?;
    println!("  wallpaper changed from uploaded image bytes");

    // The pointer this driver moved, remembered because no Wayland protocol
    // will report it.
    screen.move_to(Point::new(100, 120)).await?;
    assert_eq!(screen.cursor().await?, Point::new(100, 120));

    browser(computer).await?;
    input(computer).await?;
    clipboard(computer).await?;
    takeover(computer).await?;
    second_screen(computer).await?;

    // Last, and the reason it is here at all: `DesktopSupport` is written when
    // the descriptor is designed rather than when the capability is built, so
    // a flag can stay true beside a method nothing serves. This image shipped
    // exactly that — a pointer that returned success and moved nothing — and
    // the audit is what asks every claim to prove itself against a real box.
    let audit = computer::audit::audit_strictly(computer, Duration::from_secs(60)).await?;
    println!("  audit: {audit}");

    Ok(())
}

/// Chromium on Ozone, which is the flag this image exists to prove.
async fn browser(computer: &Computer) -> computer::Result<()> {
    computer.open_url("https://example.com").await?;

    // The window takes a moment to appear, and a screenshot before it does is
    // a blank screen that reads as a broken browser.
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

/// A page with room to scroll and something to type into.
///
/// The handlers go on afterwards through [`RECORDERS`] rather than in the URL:
/// a `data:` URL has to be escaped character by character, and an escape that
/// is wrong produces a page that loads and quietly records nothing.
const PROBE: &str = "data:text/html,\
<input%20autofocus%20style=\"width:90%25;font-size:40px\">\
<div%20style=\"height:4000px\"></div>";

/// What the page writes down about the input it receives.
///
/// `moves` counts only motion with a button held, because that is what makes a
/// drag a drag: applications that track motion rather than the endpoints never
/// see one that teleports.
const RECORDERS: &str = r#"
    window.seen = { click: null, down: null, up: null, moves: 0, doubles: 0 };
    onclick     = e => { seen.click = e.clientX + ',' + e.clientY; };
    onmousedown = e => { seen.down = e.clientX + ',' + e.clientY; };
    onmousemove = e => { if (e.buttons) { seen.moves++; } };
    onmouseup   = e => { seen.up = e.clientX + ',' + e.clientY; };
    ondblclick  = () => { seen.doubles++; };
    "installed"
"#;

/// The x of a `clientX,clientY` the page wrote down.
///
/// The x rather than the pair: a screen coordinate and a viewport one differ
/// by the browser's own chrome vertically, and by nothing horizontally.
fn column(recorded: &serde_json::Value) -> Option<u32> {
    recorded
        .as_str()?
        .split(',')
        .next()
        .and_then(|part| part.parse().ok())
}

/// Input, checked by what it did rather than by what it returned.
///
/// Every tool in this path succeeds and does nothing: `sway`'s `seat cursor`
/// returns zero against a seat with no pointer device, and `wtype` returns zero
/// after dropping a keystroke. Reading exit codes passes against a box whose
/// screen never moves, which is how this image once shipped with no working
/// pointer — so the box is asked what happened through DevTools, which shares
/// no path with the input being tested.
async fn input(computer: &Computer) -> computer::Result<()> {
    let devtools = computer
        .browser()
        .expect("the descriptor claims a DevTools endpoint");
    let mut page = devtools.open_page(PROBE, Duration::from_secs(30)).await?;
    page.wait_for_load(Duration::from_secs(20)).await?;

    // The page a coordinate addresses is the one in front, and opening this
    // put another behind it.
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

    // A chord, proving the modifiers are held and released around the key
    // rather than sent as bare letters.
    computer.key("ctrl+a").await?;
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

    // A click, at a point the page can report back.
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

    // A scroll, proving the wheel notches are axis events and not buttons.
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

    // A drag: pressed at one point, moved *while pressed*, released at
    // another. The motion is the part worth checking — an application that
    // tracks it rather than the endpoints never sees a drag that teleports.
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

    // A double click, which is one gesture rather than two clicks: two runs
    // are far enough apart that the page sees two singles.
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

/// The clipboard is claimed by the descriptor, so it has to be real.
async fn clipboard(computer: &Computer) -> computer::Result<()> {
    let text = "clipboard \"round trip\", with a newline\nand a $dollar";

    computer.set_clipboard(text).await?;
    assert_eq!(computer.clipboard().await?, text);
    println!("  the clipboard holds what was put on it");
    Ok(())
}

/// The read-only viewer and the control viewer are two servers, and the input
/// gate is in the box rather than only in this process.
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

    // Past the API, which is the case the gate in this process cannot cover.
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

    // A read stays open while a person drives.
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

/// A second compositor, in its own runtime directory.
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
