//! Against a real container. Ignored by default.
//!
//! ```text
//! cargo test --test live -- --ignored --nocapture
//! ```
//!
//! The rest of the suite proves the mapping against a scripted runtime, which
//! says nothing about the image. This proves the image: that the build
//! produces a box where an X server comes up, a browser starts, a second
//! screen can be added, a person can be handed the keyboard, and files go in
//! and come back out.
//!
//! One box for the whole test, because opening one costs seconds and every
//! step below is independent of the others.

use computer::{
    Arrange, Button, Computer, Delta, Held, Point, ProfileBuilder, Rect, ScreenId, Shot, X11Profile,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn a_real_box_does_everything_the_readme_claims() {
    let computer = Computer::launch().await.expect("a box");
    println!("  {} is up", computer.name());

    let outcome = exercise(&computer).await;

    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

/// A local build context, reached the way a custom image is meant to reach one.
///
/// The profile carries the directory, so the image and the contract it
/// implements arrive together rather than as two things a caller pairs by
/// hand. One box, because a fourth desktop running beside the others is what
/// makes this suite flake.
#[tokio::test]
#[ignore = "needs a container runtime and builds the Ubuntu image"]
async fn a_local_image_directory_can_be_driven() {
    let directory = Path::new(computer::bundle::IMAGES).join("ubuntu");
    let profile = ProfileBuilder::new(X11Profile)
        .image_dir(&directory)
        .build();

    // Named on the builder, the same directory has to resolve to the same
    // image: one path is the profile's own, the other overrides it.
    let by_profile = Computer::builder()
        .profile(Arc::new(profile))
        .config()
        .expect("the profile's image");
    let by_builder = Computer::builder()
        .image_dir(&directory)
        .config()
        .expect("the builder's image");
    assert_eq!(by_profile.image, by_builder.image);
    assert_eq!(by_profile.image_dir, by_builder.image_dir);

    let profile = ProfileBuilder::new(X11Profile)
        .image_dir(&directory)
        .build();
    let computer = Computer::builder()
        .profile(Arc::new(profile))
        .launch()
        .await
        .expect("an Ubuntu box from a derived profile");

    let outcome = exercise(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn a_window_knows_what_it_is_and_where() {
    let computer = Computer::launch().await.expect("a box");
    let outcome = window_facts(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn window_facts(computer: &Computer) -> computer::Result<()> {
    let screen = computer.primary();
    let (width, height) = screen.geometry().await?;

    let windows = screen.windows().await?;
    for window in &windows {
        println!(
            "  {:<12} {}x{} at {},{}  {:?}",
            window.class, window.width, window.height, window.at.x, window.at.y, window.title
        );
    }

    let browser = windows
        .iter()
        .find(|window| window.class.eq_ignore_ascii_case("chromium"))
        .expect("the browser is on the screen and says which it is");

    assert!(
        browser.width > 0 && browser.height > 0,
        "a window has a size"
    );
    assert!(
        browser.width <= width && browser.height <= height,
        "a window is not larger than the screen it is on: {}x{} against {width}x{height}",
        browser.width,
        browser.height
    );

    Ok(())
}

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn a_capture_can_be_cropped_and_shrunk() {
    let computer = Computer::launch().await.expect("a box");
    let outcome = captures(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn captures(computer: &Computer) -> computer::Result<()> {
    let screen = computer.primary();
    let (width, height) = screen.geometry().await?;

    let whole = screen.screenshot().await?;
    assert_eq!(png_size(&whole), Some((width, height)));

    let region = screen
        .capture(&Shot::region(Rect::new(Point::new(100, 80), 400, 300)))
        .await?;
    assert_eq!(
        png_size(&region),
        Some((400, 300)),
        "a crop answers with the rectangle that was asked for"
    );

    let quarter = screen.capture(&Shot::default().scaled(25)).await?;
    assert_eq!(png_size(&quarter), Some((width / 4, height / 4)));
    assert!(
        quarter.len() < whole.len(),
        "a smaller picture that costs more bytes is not a saving: {} against {}",
        quarter.len(),
        whole.len()
    );

    let browser = screen
        .windows()
        .await?
        .into_iter()
        .find(|window| window.class.eq_ignore_ascii_case("chromium"))
        .expect("the browser is on screen");

    let one = screen.capture(&Shot::window(&browser.id)).await?;
    assert_eq!(
        png_size(&one),
        Some((browser.width, browser.height)),
        "a window is captured at the size it reports"
    );

    let small = screen
        .capture(&Shot::window(&browser.id).scaled(50))
        .await?;
    assert_eq!(
        png_size(&small),
        Some((browser.width / 2, browser.height / 2))
    );

    println!(
        "  whole {} bytes, quarter {}, window {} at {}x{}",
        whole.len(),
        quarter.len(),
        one.len(),
        browser.width,
        browser.height
    );

    assert!(screen.capture(&Shot::window("999")).await.is_err());
    assert!(screen.capture(&Shot::default().scaled(0)).await.is_err());
    assert!(
        screen
            .capture(&Shot::region(Rect::new(Point::new(0, 0), 0, 10)))
            .await
            .is_err()
    );

    Ok(())
}

/// The width and height a PNG declares in its own header.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let width = u32::from_be_bytes(png.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(png.get(20..24)?.try_into().ok()?);

    Some((width, height))
}

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn a_click_can_hold_a_modifier_and_a_wait_can_end_itself() {
    let computer = Computer::launch().await.expect("a box");
    let outcome = held_and_still(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

/// A page that writes down which modifiers each click arrived with.
const WITNESS: &str = "data:text/html,\
<body%20style=\"margin:0\"><div%20id=\"pad\"%20style=\"width:100%25;height:100vh\"></div>";

const WATCH: &str = r#"
    window.seen = [];
    document.getElementById('pad').onclick = e => {
        seen.push([e.shiftKey && 'shift', e.ctrlKey && 'ctrl', e.altKey && 'alt',
                   e.metaKey && 'super'].filter(Boolean).join('+') || 'none');
    };
    "installed"
"#;

async fn held_and_still(computer: &Computer) -> computer::Result<()> {
    let screen = computer.primary();
    let devtools = computer.browser().expect("a published DevTools port");

    let mut page = devtools.open_page(WITNESS, Duration::from_secs(30)).await?;
    page.wait_for_load(Duration::from_secs(20)).await?;
    page.bring_to_front().await?;

    // Both the thing being tested and the thing needed to test it.
    screen
        .wait_until_still(Duration::from_millis(400), Duration::from_secs(15))
        .await?;
    page.evaluate(WATCH).await?;

    let at = Point::new(640, 500);
    screen.click(at, Button::Left).await?;
    screen.click_with(at, Button::Left, &[Held::Shift]).await?;
    screen
        .click_with(at, Button::Left, &[Held::Ctrl, Held::Shift])
        .await?;
    screen.click(at, Button::Left).await?;

    let seen = page.evaluate("seen.join(' ')").await?;
    println!("  clicks arrived as: {seen}");
    assert_eq!(
        seen.as_str(),
        // The page names them in its own order, not the order they were asked
        // for, so this says both arrived rather than which came first.
        Some("none shift shift+ctrl none"),
        "a modifier did not reach the page, or one was left held afterwards"
    );

    let never = screen
        .wait_until_still(Duration::from_secs(30), Duration::from_secs(2))
        .await;
    assert!(never.is_err(), "a settle longer than the wait cannot pass");

    page.close().await.ok();
    Ok(())
}

/// A page wide enough and tall enough to move on either axis.
const WIDE: &str = "data:text/html,<div%20style=\"width:4000px;height:4000px\"></div>";

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn the_wheel_turns_on_both_axes() {
    let computer = Computer::launch().await.expect("a box");
    let outcome = wheel(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn wheel(computer: &Computer) -> computer::Result<()> {
    let devtools = computer.browser().expect("a published DevTools port");
    let mut page = devtools.open_page(WIDE, Duration::from_secs(30)).await?;
    page.wait_for_load(Duration::from_secs(20)).await?;

    // A coordinate addresses the page in front, and opening this put one
    // behind it.
    page.bring_to_front().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let screen = computer.primary();
    let at = Point::new(640, 400);
    screen.scroll(at, Delta::down(5)).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (x, y) = scrolled(&mut page).await?;
    println!("  after down(5):  {x},{y}");
    assert!(y > 0.0, "the wheel notches never arrived");
    assert_eq!(x, 0.0, "a scroll down also went sideways");

    screen.scroll(at, Delta::right(5)).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (sideways, still) = scrolled(&mut page).await?;
    println!("  after right(5): {sideways},{still}");
    assert!(
        sideways > 0.0,
        "buttons 6 and 7 never reached the page, so a sideways scroll is \
         accepted and does nothing"
    );
    assert_eq!(still, y, "a sideways scroll also moved the page down");

    screen.scroll(at, Delta { dx: -2, dy: -2 }).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (back_x, back_y) = scrolled(&mut page).await?;
    println!("  after -2,-2:    {back_x},{back_y}");
    assert!(
        back_x < sideways && back_y < still,
        "one axis did not come back"
    );

    page.close().await.ok();
    Ok(())
}

async fn scrolled(page: &mut computer::Page) -> computer::Result<(f64, f64)> {
    let x = page
        .evaluate("window.scrollX")
        .await?
        .as_f64()
        .unwrap_or(0.0);
    let y = page
        .evaluate("window.scrollY")
        .await?
        .as_f64()
        .unwrap_or(0.0);

    Ok((x, y))
}

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn a_window_goes_where_it_is_put() {
    let computer = Computer::launch().await.expect("a box");
    let outcome = window_control(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn window_control(computer: &Computer) -> computer::Result<()> {
    let screen = computer.primary();

    // Already on screen, so this returns at once. The wait is for the windows
    // nothing here started, and this proves it finds one either way.
    let browser = screen
        .wait_for_window("chromium", Duration::from_secs(20))
        .await?;
    println!("  waited for {} ({})", browser.class, browser.id);

    screen.focus(&browser.id).await?;
    let active = screen.active_window().await?;
    println!("  active: {active:?}");
    assert_eq!(
        active.as_ref().map(|window| window.id.clone()),
        Some(browser.id.clone()),
        "the window that was raised is the one holding the keyboard"
    );

    let sized = screen
        .arrange(
            &browser.id,
            Arrange::Size {
                width: 800,
                height: 600,
            },
        )
        .await?;
    println!("  sized: {}x{}", sized.width, sized.height);
    assert_eq!((sized.width, sized.height), (800, 600));

    let moved = screen
        .arrange(
            &browser.id,
            Arrange::At {
                to: Point::new(120, 90),
            },
        )
        .await?;
    println!("  moved: {},{}", moved.at.x, moved.at.y);
    assert!(
        near(moved.at.x, 120) && near(moved.at.y, 90),
        "a window ends up where it was put, allowing for its own frame: {:?}",
        moved.at
    );

    let big = screen.arrange(&browser.id, Arrange::Maximise).await?;
    println!("  maximised: {}x{}", big.width, big.height);
    assert!(
        big.width > sized.width,
        "maximising makes it wider than 800: {}",
        big.width
    );

    screen.arrange(&browser.id, Arrange::Minimise).await?;
    let back = screen.arrange(&browser.id, Arrange::Restore).await?;
    println!(
        "  back: {}x{} at {},{}",
        back.width, back.height, back.at.x, back.at.y
    );

    assert_eq!(back.id, browser.id);

    let gone = screen.arrange("1", Arrange::Maximise).await;
    assert!(gone.is_err(), "a window id nothing answers to is a failure");

    Ok(())
}

/// A decorated window is placed by its frame, and its contents land a title
/// bar below where the move asked for.
fn near(got: u32, wanted: u32) -> bool {
    got.abs_diff(wanted) <= 64
}

async fn exercise(computer: &Computer) -> computer::Result<()> {
    let presence = computer.probe().await;
    assert!(presence.ready(), "the box said it was ready: {presence:?}");

    let screen = computer.primary();
    assert_eq!(
        screen.geometry().await?,
        (1280, 800),
        "the X server must be the size the descriptor claims"
    );

    // A real capture: only the magic says it is a PNG, since a stub would
    // return bytes too.
    let frame = screen.screenshot().await?;
    assert_eq!(
        frame.first_chunk::<4>(),
        Some(&[0x89, b'P', b'N', b'G']),
        "the capture is not a PNG"
    );
    println!("  screenshot: {} bytes", frame.len());

    screen.set_wallpaper(&frame).await?;
    println!("  wallpaper changed from uploaded image bytes");

    // A real pointer, moved and then measured, since no frame shows it.
    screen.click(Point::new(640, 400), Button::Left).await?;
    assert_eq!(screen.cursor().await?, Point::new(640, 400));

    screen.move_to(Point::new(100, 120)).await?;
    assert_eq!(screen.cursor().await?, Point::new(100, 120));

    screen.type_text("computer-rs").await?;
    screen.key("ctrl+a").await?;
    screen.scroll(Point::new(640, 400), Delta::down(2)).await?;
    println!("  typed, chorded and scrolled");

    // The URL goes to the browser already running on that screen, not to a
    // second instance fighting it for the profile lock.
    computer.open_url("https://example.com").await?;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let after = screen.screenshot().await?;
    assert_ne!(
        after, frame,
        "the screen did not change after a page was opened"
    );
    println!("  a page loaded and the screen changed");

    clipboard(computer).await?;
    files(computer).await?;
    devtools(computer).await?;
    browser(computer).await?;
    browser_groups(computer).await?;
    takeover(computer).await?;
    second_screen(computer).await?;
    the_last_screen(computer).await?;

    // Last, because it checks that every flag in the descriptor means
    // something, and it only means anything against a real box.
    leases(computer).await?;

    let audit = computer::audit::audit_strictly(computer, Duration::from_secs(60)).await?;
    println!("  audit: {audit}");

    Ok(())
}

/// A screen is held, so a second caller cannot be handed the same one.
async fn leases(computer: &Computer) -> computer::Result<()> {
    let screen = computer.screen(ScreenId(2)).await?;
    assert_eq!(screen.display(), ":3");

    let other = computer
        .claim(&computer::HolderId::new("somebody-else"), 1)
        .await?;
    assert_ne!(
        other.id(),
        screen.id(),
        "two callers were handed one screen, which is what a lease prevents"
    );

    // Given back on drop, so the next caller can have it.
    let taken = screen.id();
    drop(screen);
    drop(other);

    let again = computer.screen(taken).await?;
    assert_eq!(again.id(), taken, "a screen nobody holds is free again");

    computer.close_screen(ScreenId(2)).await?;
    println!("  screens are leased, and given back when dropped");
    Ok(())
}

/// The clipboard is claimed by the descriptor, so it has to be real.
async fn clipboard(computer: &Computer) -> computer::Result<()> {
    // Arbitrary text, including the characters that would end an argument if
    // this went through a command line instead of a file.
    let text = "clipboard \"round trip\", with a newline\nand a $dollar";

    computer.set_clipboard(text).await?;
    assert_eq!(computer.clipboard().await?, text);

    // PRIMARY is a different selection rather than another name for the same
    // one: a caller that wrote to one and read the other sees stale text.
    let selected = "what the mouse dragged over";
    computer
        .set_selection(computer::Selection::Primary, selected)
        .await?;

    assert_eq!(
        computer.selection(computer::Selection::Primary).await?,
        selected
    );
    assert_eq!(
        computer.clipboard().await?,
        text,
        "writing PRIMARY must not disturb CLIPBOARD"
    );

    // A picture is offered as image/png, which a text-only clipboard could
    // not carry.
    let picture = computer.screenshot().await?;
    computer
        .set_clipboard_bytes(computer::Selection::Clipboard, "image/png", &picture)
        .await?;

    let offered = computer
        .clipboard_targets(computer::Selection::Clipboard)
        .await?;
    assert!(
        offered.iter().any(|target| target == "image/png"),
        "the owner offers no picture: {offered:?}"
    );

    let back = computer
        .clipboard_bytes(computer::Selection::Clipboard, "image/png")
        .await?;
    assert_eq!(back, picture, "the picture changed on the clipboard");

    println!("  both selections hold what was put on them, pictures included");
    Ok(())
}

/// The last screen the image allows, and the one past it.
async fn the_last_screen(computer: &Computer) -> computer::Result<()> {
    let last = computer.screen(ScreenId(7)).await?;
    assert_eq!(last.display(), ":8");
    assert_eq!(last.ports().view, 6094);

    let frame = last.screenshot().await?;
    assert_eq!(frame.first_chunk::<4>(), Some(&[0x89, b'P', b'N', b'G']));

    last.move_to(Point::new(11, 22)).await?;
    assert_eq!(last.cursor().await?, Point::new(11, 22));
    assert_eq!(
        computer.primary().cursor().await?,
        Point::new(700, 500),
        "screen 0 kept the pointer it had"
    );

    assert!(
        computer.screen(ScreenId(8)).await.is_err(),
        "a ninth screen is refused rather than computed onto somebody else's port"
    );

    computer.close_screen(ScreenId(7)).await?;
    println!("  screen 7 ran, and screen 8 was refused");
    Ok(())
}

async fn files(computer: &Computer) -> computer::Result<()> {
    let frame = computer.screenshot().await?;
    computer.write_file("/tmp/live/frame.png", &frame).await?;

    let listed = computer.exec(["ls", "/tmp/live"]).await?;
    assert!(listed.stdout_utf8().contains("frame.png"));

    let back = computer.read_file("/tmp/live/frame.png").await?;
    assert_eq!(back, frame, "the bytes changed on the way through");
    println!("  {} bytes went in and came back", back.len());

    Ok(())
}

/// The DevTools endpoint has to answer from here, not merely inside the box.
///
/// Chromium binds the debugging port to loopback whatever
/// `--remote-debugging-address` says, so a host port forwarded straight onto it
/// accepts the connection and closes it. That empty reply reads as a browser
/// without DevTools, and the probe inside the box passes the whole time.
async fn devtools(computer: &Computer) -> computer::Result<()> {
    let endpoint = computer.devtools().expect("a published endpoint");
    let port: u16 = endpoint
        .http_url
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .expect("a host port");

    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the bridge is listening");

    let request = format!(
        "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    socket
        .write_all(request.as_bytes())
        .await
        .expect("a request");

    // Read with a limit rather than to end of file. Chromium answers with a
    // Content-Length and then holds the connection open, so waiting for it to
    // close waits for ever.
    let mut answer = String::new();
    let mut buffer = [0u8; 4096];
    loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(read)) => answer.push_str(&String::from_utf8_lossy(&buffer[..read])),
            Ok(Err(error)) => panic!("devtools closed the connection: {error}"),
        }
        if answer.contains("}") {
            break;
        }
    }

    assert!(answer.contains("200 OK"), "devtools answered: {answer}");
    assert!(answer.contains("\"Browser\""));
    println!("  devtools answers on {}", endpoint.http_url);

    Ok(())
}

/// Which page a coordinate actually addresses.
///
/// The desktop API points at pixels, the browser thinks in pages, and nothing
/// joins them: `open_url` opens a *new* tab and raises it, so coordinates from
/// an earlier screenshot now belong to a page that is gone — and the click
/// lands on the new one with nothing in the next frame to say so.
async fn which_page_the_pixels_belong_to(
    computer: &Computer,
    page: &mut computer::Page,
) -> computer::Result<()> {
    page.bring_to_front().await?;
    assert!(
        page.visible().await?,
        "a page told to come forward is the one the screen is showing"
    );

    // Anything else opening takes the screen, which is the whole hazard.
    computer.open_url("https://example.net").await?;
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert!(
        !page.visible().await?,
        "open_url raised a new tab, so the page a caller was driving is no \
         longer the one its coordinates reach"
    );

    let showing = computer
        .browser()
        .expect("a published DevTools port")
        .visible_page()
        .await?;
    let mut showing = showing.expect("something is on the screen");
    assert!(
        showing.url().await?.contains("example.net"),
        "the page in front is the one that was just opened"
    );

    // And back, which is what a caller does before using coordinates again.
    page.bring_to_front().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(page.visible().await?);
    assert!(
        !showing.visible().await?,
        "two pages cannot both be the one on screen"
    );
    println!("  the page in front is known, and can be chosen");
    Ok(())
}

/// The browser, driven through the protocol rather than through the screen.
///
/// None of this touches the display: it is the half of the box that works when
/// there is no screen at all.
async fn browser(computer: &Computer) -> computer::Result<()> {
    let browser = computer.browser().expect("a published DevTools port");

    // `open_page`, not `open` then wait: a tab that exists is showing
    // about:blank, whose readyState is already complete, so a wait right after
    // opening is answered by the blank page.
    let mut page = browser
        .open_page("https://example.com", Duration::from_secs(20))
        .await?;

    assert_eq!(page.title().await?, "Example Domain");
    assert!(page.url().await?.starts_with("https://example.com"));

    // A question no screenshot can answer.
    let links = page
        .evaluate("Array.from(document.links).map(a => a.href).length")
        .await?;
    assert_eq!(links.as_u64(), Some(1));

    // The page as the browser renders it, with no window frame around it.
    let shot = page.screenshot().await?;
    assert_eq!(
        shot.first_chunk::<4>(),
        Some(&[0x89, b'P', b'N', b'G']),
        "the capture came back but is not a PNG"
    );

    page.navigate("https://example.org").await?;
    page.wait_for_load(Duration::from_secs(20)).await?;
    assert!(page.url().await?.starts_with("https://example.org"));

    // A navigation the browser refuses is a failure, not a silent no-op.
    assert!(
        page.navigate("http://no-such-host.invalid").await.is_err(),
        "a refused navigation left the caller looking at the old page"
    );

    which_page_the_pixels_belong_to(computer, &mut page).await?;

    let id = page.target().id.clone();
    browser.close(&id).await?;
    println!("  the browser drove itself: {} bytes captured", shot.len());
    Ok(())
}

async fn browser_groups(computer: &Computer) -> computer::Result<()> {
    let browser = computer.browser().expect("a published DevTools port");
    let mut default = browser
        .open_page("https://example.com", Duration::from_secs(20))
        .await?;
    default
        .evaluate(
            "document.cookie = 'computer_group=default; path=/'; \
             localStorage.setItem('computer_group', 'default')",
        )
        .await?;

    let one = browser.create_group().await?;
    let two = browser.create_group().await?;
    let one_id = one.id().to_string();
    let two_id = two.id().to_string();
    let (one_page, two_page) = tokio::join!(
        one.open_page("https://example.com", Duration::from_secs(20)),
        two.open_page("https://example.com", Duration::from_secs(20)),
    );
    let mut one_page = one_page?;
    let mut two_page = two_page?;

    one_page
        .evaluate(
            "document.cookie = 'computer_group=one; path=/'; \
             localStorage.setItem('computer_group', 'one')",
        )
        .await?;
    two_page
        .evaluate(
            "document.cookie = 'computer_group=two; path=/'; \
             localStorage.setItem('computer_group', 'two')",
        )
        .await?;

    assert_eq!(
        default
            .evaluate("localStorage.getItem('computer_group')")
            .await?,
        "default"
    );
    assert_eq!(
        one_page
            .evaluate("localStorage.getItem('computer_group')")
            .await?,
        "one"
    );
    assert_eq!(
        two_page
            .evaluate("localStorage.getItem('computer_group')")
            .await?,
        "two"
    );
    assert!(
        one_page
            .evaluate("document.cookie")
            .await?
            .as_str()
            .unwrap_or_default()
            .contains("computer_group=one")
    );

    let one_target = one_page.target().id.clone();
    let two_target = two_page.target().id.clone();
    assert!(one.pages().await?.iter().any(|page| page.id == one_target));
    assert!(two.pages().await?.iter().any(|page| page.id == two_target));

    let groups = browser.groups().await?;
    assert!(groups.iter().any(|group| group.id() == one_id));
    assert!(groups.iter().any(|group| group.id() == two_id));
    let duplicate = groups
        .into_iter()
        .find(|group| group.id() == one_id)
        .expect("a second handle to group one");

    drop(one_page);
    one.close().await?;
    duplicate
        .close()
        .await
        .expect("closing the same context twice is idempotent");
    assert!(
        !browser
            .groups()
            .await?
            .iter()
            .any(|group| group.id() == one_id)
    );
    assert_eq!(two_page.title().await?, "Example Domain");

    drop(two_page);
    two.close().await?;
    let default_target = default.target().id.clone();
    browser.close(&default_target).await?;
    println!("  browser groups isolate sessions and clean up their pages");
    Ok(())
}

async fn takeover(computer: &Computer) -> computer::Result<()> {
    let takeover = computer.hand_over().await?;
    println!("  handed over: {:?}", takeover.url());

    // Looking is still allowed while a person drives …
    computer
        .screenshot()
        .await
        .expect("the run is not paused, it just may not touch anything");

    // … and touching is not.
    let refused = computer.click(Point::new(1, 1), Button::Left).await;
    assert!(refused.is_err(), "the gate let a click through");

    let control = computer
        .exec(["bash", "-c", "echo > /dev/tcp/127.0.0.1/6081"])
        .await?;
    assert!(
        control.ok(),
        "the control viewer is not listening, so nobody can take the keyboard"
    );

    // A second handle, opened while the person still has the screen. The
    // gate is per process, so this one has to ask the box.
    let other = Computer::attach(computer.name())
        .await
        .expect("the same box, a fresh handle");
    assert!(
        other.person_driving().await,
        "the box knows a person has it"
    );
    assert!(
        other.click(Point::new(5, 5), Button::Left).await.is_err(),
        "a handle that did not start the takeover must still respect it"
    );
    println!("  a second handle found the person and stood back");

    // The box keeps the token, so a release carrying an invented one is
    // refused even after the caller that took the screen has exited.
    let replacement = computer.hand_over().await?;
    assert!(
        replacement.url().is_some(),
        "the second takeover reuses the same server"
    );
    assert!(
        takeover_ends_stale(computer).await,
        "a stale release must not take the keyboard from whoever is driving now"
    );
    replacement.end().await?;
    let takeover = computer.hand_over().await?;

    // The box refuses the bypass as well as the API: an owner that runs
    // xdotool through exec meets the guard on the path.
    let bypass = computer
        .exec(["xdotool", "mousemove", "--", "1", "1"])
        .await?;
    assert_eq!(
        bypass.code,
        3,
        "the image let a shell drive a screen a person is holding: {}",
        bypass.stderr_utf8()
    );

    // Reads are still allowed, because a run that may not act may still watch.
    let watching = computer.exec(["xdotool", "getdisplaygeometry"]).await?;
    assert!(watching.ok(), "observation must survive a takeover");
    println!("  the box itself refused a shell that tried to drive");

    // Nobody has actually connected, so the count says so even though the
    // server is up. A run that waited on "the server is listening" would wait
    // for a person who left long ago.
    let viewers = computer.viewers().await?;
    assert!(
        !viewers.person_present(),
        "no browser is attached: {viewers:?}"
    );

    takeover.end().await?;
    computer
        .click(Point::new(640, 400), Button::Left)
        .await
        .expect("the owner drives again");

    // Shared: the same server, and the gate left open.
    let shared = computer.share().await?;
    assert!(!shared.exclusive());
    computer
        .click(Point::new(300, 300), Button::Left)
        .await
        .expect("both may drive in a shared session");
    shared.end().await?;
    println!("  shared control lets the owner keep driving");

    // Released means released: the second server is gone and the read-only
    // one is untouched.
    let still_there = computer
        .exec(["bash", "-c", "echo > /dev/tcp/127.0.0.1/6081"])
        .await?;
    assert!(
        !still_there.ok(),
        "the control viewer outlived the takeover"
    );

    let watching = computer
        .exec(["bash", "-c", "echo > /dev/tcp/127.0.0.1/6080"])
        .await?;
    assert!(watching.ok(), "whoever was watching stopped being able to");
    println!("  taken back, and the read-only viewer never went away");

    Ok(())
}

/// Whether the box refuses a release carrying a token it never recorded.
///
/// The token lives in the box rather than in the caller, so the refusal
/// survives a caller that has exited — which is the case that matters, because
/// that caller's gate went with it.
async fn takeover_ends_stale(computer: &Computer) -> bool {
    let refused = computer
        .exec(["computer-screen", "release", "0", "a-token-nobody-issued"])
        .await;

    matches!(refused, Ok(result) if result.code == 3)
}

async fn second_screen(computer: &Computer) -> computer::Result<()> {
    let second = computer.screen(ScreenId(1)).await?;
    assert_eq!(second.display(), ":2");

    let frame = second.screenshot().await?;
    assert_eq!(frame.first_chunk::<4>(), Some(&[0x89, b'P', b'N', b'G']));

    // Two screens, two pointers. A position set on one must not appear on the
    // other, or they are the same display under two names.
    second.move_to(Point::new(20, 30)).await?;
    computer.primary().move_to(Point::new(700, 500)).await?;

    assert_eq!(second.cursor().await?, Point::new(20, 30));
    assert_eq!(computer.primary().cursor().await?, Point::new(700, 500));
    println!("  screen 1 is its own display, with its own pointer");

    computer.close_screen(ScreenId(1)).await?;
    Ok(())
}

/// A box that nobody touches goes away on its own.
///
/// Its own test rather than a step in the long one, because it has to sit
/// still for the idle period, and the long test is never still.
#[tokio::test]
#[ignore = "needs a container runtime"]
async fn an_idle_box_takes_itself_away() {
    let idle = Duration::from_secs(5);

    let computer = Computer::builder()
        .expires_when_idle(idle)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let name = computer.name().to_string();
    let machine = Arc::clone(computer.machine());

    // Busy: the deadline moves with every call, so this must not go away.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        computer.screenshot().await.expect("a frame");
    }
    assert!(
        machine.running(&name).await.expect("a state"),
        "a box being worked on was taken away under its caller"
    );

    // Quiet: nothing is asked of it, and it goes.
    tokio::time::sleep(idle + Duration::from_secs(4)).await;
    assert!(
        !machine.running(&name).await.expect("a state"),
        "a box nobody has touched holds a core and gigabytes until somebody \
         notices, and nothing else notices"
    );

    println!("  an idle box removed itself after {idle:?}");
}
