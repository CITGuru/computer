//! What the driver actually sends, checked without a container.
//!
//! Every method here becomes one command line inside the box. A wrong flag or
//! a missing `--` produces a refusal that reads like a broken tool, and a
//! container is the slowest possible place to discover that — so the mapping
//! is pinned here and the image test is left to prove the image.

use computer::servers::x11::{X11Desktop, port_listening};
use computer::testing::{ScriptedDesktop, ScriptedHost};
use computer::{
    Button, ControlGate, Delta, Desktop, ExecResult, Point, ScreenHost, ScreenId, Selection,
};

/// The clipboard is an optional capability, so a driver that has one has to
/// hand it over rather than answer a method that every driver must carry.
fn clipboard(screen: &X11Desktop) -> &dyn computer::Clipboard {
    screen
        .as_clipboard()
        .expect("the X11 driver has a clipboard")
}
use std::sync::Arc;
use std::time::SystemTime;

fn driver(host: Arc<ScriptedHost>) -> X11Desktop {
    X11Desktop::new(host as Arc<dyn ScreenHost>, ScreenId(0))
}

fn png() -> ExecResult {
    ExecResult {
        stdout: vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a],
        ..ExecResult::default()
    }
}

#[tokio::test]
async fn a_screenshot_comes_back_as_the_bytes_the_capture_wrote() {
    let host = Arc::new(ScriptedHost::new().replying(png()));
    let screen = driver(Arc::clone(&host));

    let image = screen.screenshot().await.expect("a capture");

    assert_eq!(host.last_line(), "import -window root png:-");
    assert_eq!(
        image.first_chunk::<4>(),
        Some(&[0x89, b'P', b'N', b'G']),
        "raw PNG, not an encoding whose decoder differs between images"
    );
}

#[tokio::test]
async fn an_empty_capture_is_an_error_rather_than_an_empty_image() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(host);

    let error = screen.screenshot().await.expect_err("nothing was captured");
    assert!(error.to_string().contains("no image"));
}

#[tokio::test]
async fn a_click_moves_the_pointer_and_presses_in_one_command() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));

    screen
        .click(Point::new(640, 400), Button::Left)
        .await
        .expect("a click");

    assert_eq!(
        host.last_line(),
        "xdotool mousemove -- 640 400 click 1",
        "two commands would let something else move the pointer in between"
    );
}

#[tokio::test]
async fn each_button_has_its_own_number() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));

    for (button, number) in [
        (Button::Left, "1"),
        (Button::Middle, "2"),
        (Button::Right, "3"),
    ] {
        screen
            .click(Point::new(1, 2), button)
            .await
            .expect("a click");
        assert!(host.last_line().ends_with(number), "{button:?} is {number}");
    }
}

#[tokio::test]
async fn hovering_does_not_press_anything() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));

    screen.move_to(Point::new(10, 20)).await.expect("a move");

    assert_eq!(host.last_line(), "xdotool mousemove -- 10 20");
    assert!(
        !host.last_line().contains("click"),
        "a hover that clicks re-opens the menu it was meant to move down"
    );
}

#[tokio::test]
async fn typed_text_is_passed_after_a_double_dash() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));

    screen.type_text("--version").await.expect("typing");

    assert_eq!(
        host.last(),
        Some(vec![
            "xdotool".to_string(),
            "type".to_string(),
            "--clearmodifiers".to_string(),
            "--".to_string(),
            "--version".to_string(),
        ]),
        "without the -- this is read as a flag, and the refusal is undiagnosable"
    );
}

#[tokio::test]
async fn a_chord_is_translated_before_it_is_sent() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));

    screen.key("cmd+enter").await.expect("a chord");

    assert_eq!(
        host.last_line(),
        "xdotool key --clearmodifiers super+Return"
    );
}

#[tokio::test]
async fn scrolling_is_a_wheel_button_repeated() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));

    screen
        .scroll(Point::new(100, 200), Delta::down(3))
        .await
        .expect("a scroll");

    assert_eq!(
        host.last_line(),
        "xdotool mousemove -- 100 200 click --repeat 3 5"
    );
}

#[tokio::test]
async fn the_cursor_is_read_back_because_no_frame_shows_it() {
    let host = Arc::new(ScriptedHost::new().saying("X=42\nY=99\nSCREEN=0\nWINDOW=1\n"));
    let screen = driver(Arc::clone(&host));

    let at = screen.cursor().await.expect("a position");

    assert_eq!(at, Point::new(42, 99));
    assert_eq!(host.last_line(), "xdotool getmouselocation --shell");
}

#[tokio::test]
async fn a_double_click_is_one_command_and_not_two_round_trips() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));

    screen
        .double_click(Point::new(5, 6), Button::Left)
        .await
        .expect("a double click");

    assert_eq!(host.count(), 1, "two execs are two single clicks");
    assert!(host.last_line().contains("--repeat 2"));
}

#[tokio::test]
async fn a_drag_presses_moves_through_the_middle_and_releases() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));

    screen
        .drag(Point::new(0, 0), Point::new(100, 50), Button::Left)
        .await
        .expect("a drag");

    assert_eq!(
        host.last_line(),
        "xdotool mousemove -- 0 0 mousedown 1 mousemove -- 50 25 mousemove -- 100 50 mouseup 1",
        "a drag that teleports is one some applications never register"
    );
}

#[tokio::test]
async fn the_clipboard_is_read_with_one_command_and_written_from_a_file() {
    let host = Arc::new(ScriptedHost::new().saying("what was copied"));
    let screen = driver(Arc::clone(&host));

    assert_eq!(
        clipboard(&screen)
            .text(Selection::Clipboard)
            .await
            .expect("a clipboard"),
        "what was copied"
    );
    assert_eq!(host.last_line(), "xclip -selection clipboard -o");

    clipboard(&screen)
        .set_from(Selection::Clipboard, "/tmp/computer/clipboard-0")
        .await
        .expect("a paste");

    let sent = host.last().expect("a call");
    assert_eq!(sent[0], "bash", "the text is a file, not an argument");
    assert!(
        sent.contains(&"/tmp/computer/clipboard-0".to_string()),
        "the path is a positional argument, so a space or a quotation mark in \
         it cannot become shell syntax: {sent:?}"
    );
    assert!(
        !sent[2].contains("/tmp/computer/clipboard-0"),
        "the path must not be interpolated into the command"
    );
    assert!(
        sent[2].contains("setsid"),
        "X asks whoever owns a selection for it, so an owner that dies with \
         the command that set it takes the paste with it"
    );
    assert!(
        sent[2].trim_end().ends_with('&'),
        "xclip has to keep running to own the selection it just took"
    );
}

#[tokio::test]
async fn each_selection_is_read_and_written_by_name() {
    let host = Arc::new(ScriptedHost::new().saying("what the mouse selected"));
    let screen = driver(Arc::clone(&host));

    assert_eq!(
        clipboard(&screen)
            .text(Selection::Primary)
            .await
            .expect("a selection"),
        "what the mouse selected"
    );
    assert_eq!(
        host.last_line(),
        "xclip -selection primary -o",
        "PRIMARY is what dragging the mouse fills, and it is not CLIPBOARD"
    );

    clipboard(&screen)
        .set_from(Selection::Primary, "/tmp/computer/primary-0")
        .await
        .expect("a write");

    let sent = host.last().expect("a call");
    assert!(
        sent.contains(&"primary".to_string()),
        "the selection is a positional argument too: {sent:?}"
    );
}

#[tokio::test]
async fn an_empty_clipboard_is_nothing_rather_than_a_failure() {
    let host = Arc::new(ScriptedHost::new().failing(1, "Error: target STRING not available"));
    let screen = driver(host);

    assert_eq!(
        clipboard(&screen)
            .text(Selection::Clipboard)
            .await
            .expect("an empty clipboard"),
        "",
        "a caller asking what is on an empty clipboard wants nothing, not an error"
    );
}

#[tokio::test]
async fn a_failing_command_carries_its_status_and_its_stderr() {
    let host = Arc::new(ScriptedHost::new().failing(1, "Can't open display :1"));
    let screen = driver(host);

    let error = screen
        .click(Point::new(1, 1), Button::Left)
        .await
        .expect_err("no display");

    let message = error.to_string();
    assert!(message.contains("status 1"));
    assert!(message.contains("open display"));
}

#[tokio::test]
async fn a_person_driving_stops_the_input_and_not_the_reading() {
    let gate = Arc::new(ControlGate::new());
    let host = Arc::new(ScriptedHost::new().replying(png()).replying(png()));
    let screen = driver(Arc::clone(&host)).with_control(Arc::clone(&gate));

    gate.hand_over("token", SystemTime::now());

    screen
        .screenshot()
        .await
        .expect("looking is still allowed while a person drives");
    let error = screen
        .click(Point::new(1, 1), Button::Left)
        .await
        .expect_err("two pointers on one cursor");

    assert!(error.to_string().contains("observe, do not act"));
    assert_eq!(host.count(), 1, "the refused click never reached the box");
}

#[tokio::test]
async fn the_owner_drives_again_once_the_screen_is_handed_back() {
    let gate = Arc::new(ControlGate::new());
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host)).with_control(Arc::clone(&gate));

    gate.hand_over("token", SystemTime::now());
    assert!(screen.key("ctrl+a").await.is_err());

    assert!(gate.hand_back("token"));
    screen.key("ctrl+a").await.expect("the owner has it back");
}

#[tokio::test]
async fn every_command_goes_to_the_screen_the_driver_was_built_for() {
    let host = Arc::new(ScriptedHost::new());
    let screen = X11Desktop::new(Arc::clone(&host) as Arc<dyn ScreenHost>, ScreenId(3));

    screen.move_to(Point::new(1, 1)).await.expect("a move");

    assert_eq!(
        host.screens(),
        vec![ScreenId(3)],
        "the display is chosen by the host, from the screen it was told"
    );
}

#[tokio::test]
async fn the_geometry_is_read_from_the_x_server_and_not_from_the_descriptor() {
    let host = Arc::new(ScriptedHost::new().saying("1920 1080\n"));
    let screen = driver(Arc::clone(&host));

    assert_eq!(screen.geometry().await.expect("a size"), (1920, 1080));
    assert_eq!(host.last_line(), "xdotool getdisplaygeometry");
}

#[tokio::test]
async fn presence_asks_the_server_rather_than_reading_the_environment() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));

    screen.alive().await.expect("the server answered");
    assert_eq!(
        host.last(),
        Some(vec!["xdpyinfo".to_string()]),
        "a configured DISPLAY is not a running one, so the server is asked"
    );
}

#[tokio::test]
async fn a_dead_display_says_which_display_it_was() {
    let host = Arc::new(ScriptedHost::new().failing(1, "unable to open display"));
    let screen = X11Desktop::new(host as Arc<dyn ScreenHost>, ScreenId(2));

    let error = screen.alive().await.expect_err("nothing is listening");
    assert!(error.to_string().contains(":3"), "{error}");
}

#[tokio::test]
async fn a_browser_is_looked_for_where_it_actually_listens() {
    let host = ScriptedHost::new();

    assert!(port_listening(&host, ScreenId(0), 9222).await);
    let sent = host.last().expect("a call");

    assert_eq!(
        sent[0], "bash",
        "/dev/tcp is a bash feature; under dash a listening browser is \
         reported as absent"
    );
    assert!(sent[2].contains("/dev/tcp/127.0.0.1/9222"));
}

#[tokio::test]
async fn a_driver_with_no_clipboard_says_so_instead_of_failing_one() {
    let scripted = ScriptedDesktop::new(ScreenId(0));

    assert!(
        scripted.as_clipboard().is_none(),
        "a refusal a descriptor never predicted is one a caller cannot plan \
         around; None is the answer it can check"
    );
}

#[tokio::test]
async fn the_descriptor_and_the_driver_agree_about_the_clipboard() {
    let screen = driver(Arc::new(ScriptedHost::new()));

    assert_eq!(
        computer::image::support().clipboard,
        screen.as_clipboard().is_some(),
        "the claim and the capability drift apart silently, and the caller \
         only finds out at the paste"
    );
}

#[tokio::test]
async fn a_takeover_stops_a_driver_that_holds_no_x_server() {
    let scripted = ScriptedDesktop::new(ScreenId(0));
    scripted.control().hand_over("a person", SystemTime::now());

    assert!(
        scripted
            .click(Point::new(1, 2), Button::Left)
            .await
            .is_err(),
        "the gate is on the trait so the rule holds for every driver, not \
         only the one that happens to run xdotool"
    );
    assert!(
        scripted.screenshot().await.is_ok(),
        "reads stay open while a person drives: the run may look, not touch"
    );
}

#[tokio::test]
async fn every_gesture_a_screen_needs_is_on_the_trait() {
    let host = Arc::new(ScriptedHost::new());
    let screen = driver(Arc::clone(&host));
    // Through the trait object, which is what `Screen` holds.
    let desktop: &dyn Desktop = &screen;

    desktop
        .double_click(Point::new(5, 6), Button::Left)
        .await
        .expect("a double click");
    assert!(host.last_line().contains("--repeat 2"));

    desktop
        .drag(Point::new(0, 0), Point::new(10, 10), Button::Left)
        .await
        .expect("a drag");
    assert!(host.last_line().contains("mouseup 1"));

    assert!(desktop.control().may_act().is_ok());
}
