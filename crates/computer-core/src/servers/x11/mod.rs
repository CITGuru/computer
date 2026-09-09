//! Driving an X display, wherever it is running.
//!
//! [`X11Desktop`] turns each [`Desktop`] method into one command: `xdotool`
//! for input, `import` for a capture, `xclip` for the selections. It reaches
//! them through [`ScreenHost`], so the same driver works against a container, a
//! microVM, or anything else that can run a command on a display.
//!
//! # Coordinates
//!
//! Device pixels, top-left origin, against the frame the last screenshot
//! returned. A click resolved against a scaled or stale frame lands somewhere
//! else, and nothing in the result says so.

mod profile;

pub use profile::X11Profile;

use crate::error::{Error, Result};
use crate::machine::{MachineHost, ScreenHost};
use crate::screens::ControlGate;
use crate::servers::{settled, still_argv};
use crate::{
    Button, Clipboard, Delta, Desktop, DesktopFactory, DisplayServer, ExecResult, Held, Point,
    Rect, ScreenId, Selection,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Screen *i* is display `:i+1`.
///
/// Never `:0`, which on a host with a physical display is that display.
pub fn display_for(screen: ScreenId) -> String {
    format!(":{}", screen.0 + 1)
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

/// X keysym for a key as a caller is likely to name it.
///
/// Unrecognised names pass through unchanged. `xdotool` already understands
/// keysyms, so this translates only the names people get wrong.
pub fn keysym(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => "Return",
        "esc" | "escape" => "Escape",
        "space" => "space",
        "tab" => "Tab",
        "backspace" => "BackSpace",
        "delete" | "del" => "Delete",
        "insert" => "Insert",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "Prior",
        "pagedown" | "pgdn" => "Next",
        "ctrl" | "control" => "ctrl",
        "alt" | "option" => "alt",
        "shift" => "shift",
        "meta" | "cmd" | "command" | "super" | "win" => "super",
        _ => return key.to_string(),
    }
    .to_string()
}

/// Translate a chord such as `ctrl+shift+p` into what `xdotool key` takes.
pub fn chord(input: &str) -> String {
    input
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(keysym)
        .collect::<Vec<_>>()
        .join("+")
}

fn button_number(button: Button) -> &'static str {
    match button {
        Button::Left => "1",
        Button::Middle => "2",
        Button::Right => "3",
    }
}

/// `xdotool getmouselocation --shell` output.
///
/// `None` where either coordinate is missing, rather than a guess the caller
/// cannot tell from a measurement.
pub fn parse_cursor(output: &str) -> Option<Point> {
    let mut x = None;
    let mut y = None;

    for line in output.split_whitespace() {
        if let Some(value) = line.strip_prefix("X=") {
            x = value.parse().ok();
        } else if let Some(value) = line.strip_prefix("Y=") {
            y = value.parse().ok();
        }
    }

    Some(Point { x: x?, y: y? })
}

fn point_argv(command: &[&str], at: Point) -> Vec<String> {
    let mut args = argv(command);
    args.push(at.x.to_string());
    args.push(at.y.to_string());
    args
}

/// The modifiers pressed, as the first words of a chained `xdotool` command.
///
/// One command, because a press held across separate calls is released when
/// the first call's process exits — and `--clearmodifiers`, which the typing
/// path uses, would undo it even within one.
fn holding(held: &[Held]) -> Vec<String> {
    let mut args = argv(&["xdotool"]);

    for one in held {
        args.push("keydown".to_string());
        args.push(one.keysym().to_string());
    }
    args
}

/// The same modifiers released, last one first.
fn letting_go(held: &[Held]) -> Vec<String> {
    let mut args = Vec::new();

    for one in held.iter().rev() {
        args.push("keyup".to_string());
        args.push(one.keysym().to_string());
    }
    args
}

/// `+repage` after a crop, or the PNG carries the offset it was cut from and
/// a viewer honours it by drawing the picture in the wrong place.
fn capture_argv(area: Option<Rect>, scale: Option<u32>) -> Vec<String> {
    let mut args = argv(&["import", "-window", "root"]);

    if let Some(area) = area {
        args.push("-crop".to_string());
        args.push(format!(
            "{}x{}+{}+{}",
            area.width, area.height, area.at.x, area.at.y
        ));
        args.push("+repage".to_string());
    }
    if let Some(scale) = scale {
        // Area averaging, not the default Lanczos. Blurring a desktop's flat
        // colours into gradients is what a smaller picture is meant to avoid:
        // halved, the default costs 67KB against the full screen's 36KB, and
        // this costs 24KB with the text still readable.
        args.push("-filter".to_string());
        args.push("box".to_string());
        args.push("-resize".to_string());
        args.push(format!("{scale}%"));
    }

    // PNG bytes straight out of stdout: encoding them would need a decoder
    // whose flags differ between coreutils and BusyBox.
    args.push("png:-".to_string());
    args
}

/// A wheel notch is button 4 up, 5 down, 6 left and 7 right. `xdotool` has no
/// scroll distance, so a delta becomes a repeat count, bounded at twenty.
///
/// Both axes travel as one chained command, because two commands are two
/// round trips into the box with the pointer sitting still between them.
fn scroll_argv(at: Point, by: Delta) -> Vec<String> {
    let mut args = point_argv(&["xdotool", "mousemove", "--"], at);

    // A delta with no distance in it is still a scroll: one notch down, which
    // is the only thing a caller that named neither axis can have meant.
    if by.dy != 0 || by.dx == 0 {
        args.extend(wheel_argv(by.dy, "4", "5"));
    }
    if by.dx != 0 {
        args.extend(wheel_argv(by.dx, "6", "7"));
    }

    args
}

fn wheel_argv(delta: i32, back: &str, forward: &str) -> Vec<String> {
    let mut args = argv(&["click", "--repeat"]);
    args.push(delta.unsigned_abs().clamp(1, 20).to_string());
    args.push(match delta < 0 {
        true => back.to_string(),
        false => forward.to_string(),
    });
    args
}

/// The driver this crate ships, and the default every box gets.
///
/// Separate from [`X11Desktop`] because a box picks a driver once and opens
/// every screen with it: the choice is configured, not the screen.
#[derive(Debug, Clone, Copy, Default)]
pub struct X11Driver;

impl DesktopFactory for X11Driver {
    fn display_server(&self) -> DisplayServer {
        DisplayServer::X11
    }

    fn open(&self, host: Arc<MachineHost>, screen: ScreenId) -> Arc<dyn Desktop> {
        Arc::new(X11Desktop::new(host as Arc<dyn ScreenHost>, screen))
    }
}

/// One screen, driven through synthetic X input.
pub struct X11Desktop {
    host: Arc<dyn ScreenHost>,
    screen: ScreenId,
    control: Arc<ControlGate>,
}

impl X11Desktop {
    pub fn new(host: Arc<dyn ScreenHost>, screen: ScreenId) -> Self {
        Self {
            host,
            screen,
            control: Arc::new(ControlGate::new()),
        }
    }

    /// Share a gate, so a takeover started elsewhere stops this driver too.
    pub fn with_control(mut self, control: Arc<ControlGate>) -> Self {
        self.control = control;
        self
    }

    pub fn screen(&self) -> ScreenId {
        self.screen
    }

    pub fn host(&self) -> &Arc<dyn ScreenHost> {
        &self.host
    }

    async fn run(&self, args: Vec<String>) -> Result<ExecResult> {
        let result = self.host.run(&args, self.screen).await?;
        if result.code != 0 {
            return Err(Error::Failed {
                code: result.code,
                stderr: result.stderr_utf8().trim().to_string(),
            });
        }
        Ok(result)
    }

    /// Every input path, and the only place the takeover rule is applied.
    ///
    /// Reads do not come through here. A run is not paused while a person
    /// drives; it may watch and may not act.
    /// A capture, refused rather than returned empty: a zero-byte PNG reaches
    /// a caller as a picture of nothing rather than as a failure.
    async fn import(&self, args: Vec<String>) -> Result<Vec<u8>> {
        let result = self.run(args).await?;

        if result.stdout.is_empty() {
            return Err(Error::denied("the screen capture returned no image"));
        }
        Ok(result.stdout)
    }

    async fn act(&self, args: Vec<String>) -> Result<()> {
        self.control.may_act()?;
        self.run(args).await.map(|_| ())
    }
}

#[async_trait]
impl Desktop for X11Desktop {
    async fn screenshot(&self) -> Result<Vec<u8>> {
        self.import(capture_argv(None, None)).await
    }

    async fn capture(&self, area: Option<Rect>, scale: Option<u32>) -> Result<Vec<u8>> {
        self.import(capture_argv(area, scale)).await
    }

    async fn move_to(&self, at: Point) -> Result<()> {
        self.act(point_argv(&["xdotool", "mousemove", "--"], at))
            .await
    }

    async fn click(&self, at: Point, button: Button) -> Result<()> {
        self.click_with(at, button, &[]).await
    }

    async fn click_with(&self, at: Point, button: Button, held: &[Held]) -> Result<()> {
        let mut args = holding(held);
        args.extend(point_argv(&["mousemove", "--"], at));
        args.push("click".to_string());
        args.push(button_number(button).to_string());
        args.extend(letting_go(held));

        self.act(args).await
    }

    /// `xdotool`'s own repeat, not two calls: two round trips through the
    /// runtime are far enough apart that the application sees two single
    /// clicks, which is a different gesture.
    async fn double_click(&self, at: Point, button: Button) -> Result<()> {
        let mut args = point_argv(&["xdotool", "mousemove", "--"], at);
        args.extend(argv(&["click", "--repeat", "2", "--delay", "80"]));
        args.push(button_number(button).to_string());
        self.act(args).await
    }

    /// One command: a press held across separate calls may be released when
    /// the first call's process exits.
    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()> {
        self.drag_with(from, to, button, &[]).await
    }

    async fn drag_with(&self, from: Point, to: Point, button: Button, held: &[Held]) -> Result<()> {
        let number = button_number(button);
        let mut args = holding(held);
        args.extend(point_argv(&["mousemove", "--"], from));
        args.extend(argv(&["mousedown", number]));
        // Through the middle, because an application that tracks motion sees
        // nothing in a drag that teleports.
        let middle = Point {
            x: from.x.midpoint(to.x),
            y: from.y.midpoint(to.y),
        };
        args.extend(point_argv(&["mousemove", "--"], middle));
        args.extend(point_argv(&["mousemove", "--"], to));
        args.extend(argv(&["mouseup", number]));
        args.extend(letting_go(held));

        self.act(args).await
    }

    /// Bounded by the box's own command timeout, so a `within` longer than
    /// that ends as a transport failure rather than as this one's answer.
    async fn wait_until_still(&self, settle: Duration, within: Duration) -> Result<()> {
        let watched = self
            .run(still_argv(
                "import -window root png:- 2>/dev/null",
                settle,
                within,
            ))
            .await?;

        settled(watched, within)
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        // `--` first, or text beginning with a dash is read as a flag.
        let mut args = argv(&["xdotool", "type", "--clearmodifiers", "--"]);
        args.push(text.to_string());
        self.act(args).await
    }

    async fn key(&self, keys: &str) -> Result<()> {
        let mut args = argv(&["xdotool", "key", "--clearmodifiers"]);
        args.push(chord(keys));
        self.act(args).await
    }

    async fn scroll(&self, at: Point, by: Delta) -> Result<()> {
        self.act(scroll_argv(at, by)).await
    }

    async fn cursor(&self) -> Result<Point> {
        let result = self
            .run(argv(&["xdotool", "getmouselocation", "--shell"]))
            .await?;

        parse_cursor(&result.stdout_utf8())
            .ok_or_else(|| Error::denied("the cursor position could not be read"))
    }

    async fn geometry(&self) -> Result<(u32, u32)> {
        let result = self.run(argv(&["xdotool", "getdisplaygeometry"])).await?;
        let text = result.stdout_utf8();
        let mut parts = text.split_whitespace();

        match (
            parts.next().and_then(|w| w.parse().ok()),
            parts.next().and_then(|h| h.parse().ok()),
        ) {
            (Some(width), Some(height)) => Ok((width, height)),
            _ => Err(Error::denied("the screen geometry could not be read")),
        }
    }

    async fn alive(&self) -> Result<()> {
        self.run(argv(&["xdpyinfo"]))
            .await
            .map(|_| ())
            .map_err(|_| Error::Gone(format!("no X server on {}", display_for(self.screen))))
    }

    fn control(&self) -> &Arc<ControlGate> {
        &self.control
    }

    fn as_clipboard(&self) -> Option<&dyn Clipboard> {
        Some(self)
    }
}

#[async_trait]
impl Clipboard for X11Desktop {
    /// `-o` prints the selection and exits. Owning one means staying alive to
    /// serve it, which is why writing has a different shape from reading.
    async fn text(&self, selection: Selection) -> Result<String> {
        let result = self
            .run(argv(&["xclip", "-selection", selection.name(), "-o"]))
            .await;

        match result {
            Ok(result) => Ok(result.stdout_utf8()),
            // Nothing owns the selection: `xclip` answers "target STRING not
            // available" and exits non-zero. Anything else — no display, no
            // xclip — is broken rather than empty, and stays an error.
            Err(Error::Failed { stderr, .. }) if stderr.contains("not available") => {
                Ok(String::new())
            }
            Err(error) => Err(error),
        }
    }

    /// Own a selection, serving what is in a file already inside the box.
    ///
    /// `setsid`, because X keeps no copy of a selection: `xclip` has to outlive
    /// the command that started it or the next paste finds nothing. The path is
    /// a positional argument, so a space in it cannot become shell syntax.
    async fn set_from(&self, selection: Selection, path: &str) -> Result<()> {
        let mut args = argv(&[
            "bash",
            "-c",
            "setsid xclip -selection \"$2\" -i \"$1\" >/dev/null 2>&1 &",
            "--",
        ]);
        args.push(path.to_string());
        args.push(selection.name().to_string());
        self.act(args).await
    }

    /// `xclip -selection NAME -t TARGET -o`, and the bytes are returned raw.
    ///
    /// Raw, because a picture through a `String` loses every byte that is not
    /// valid UTF-8.
    async fn bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>> {
        let mut args = argv(&["xclip", "-selection", selection.name(), "-t"]);
        args.push(target.to_string());
        args.push("-o".to_string());

        match self.run(args).await {
            Ok(result) => Ok(result.stdout),
            // Nothing owns it, or its owner cannot offer this type. Empty for
            // the same reason as in `text`.
            Err(Error::Failed { stderr, .. }) if stderr.contains("not available") => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    async fn set_bytes_from(&self, selection: Selection, target: &str, path: &str) -> Result<()> {
        let mut args = argv(&[
            "bash",
            "-c",
            "setsid xclip -selection \"$2\" -t \"$3\" -i \"$1\" >/dev/null 2>&1 &",
            "--",
        ]);
        args.push(path.to_string());
        args.push(selection.name().to_string());
        args.push(target.to_string());
        self.act(args).await
    }

    async fn targets(&self, selection: Selection) -> Result<Vec<String>> {
        let text = self.bytes(selection, "TARGETS").await?;

        Ok(String::from_utf8_lossy(&text)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }
}

/// Whether something is listening on a port inside the box.
///
/// Runs under `bash`, not `sh`. `/dev/tcp` is a bash feature, and Debian's
/// `sh` is dash, which answers "Directory nonexistent" for it.
pub async fn port_listening(host: &dyn ScreenHost, screen: ScreenId, port: u16) -> bool {
    let probe = format!("(echo > /dev/tcp/127.0.0.1/{port}) 2>/dev/null");
    let mut args = argv(&["bash", "-c"]);
    args.push(probe);

    host.run(&args, screen)
        .await
        .map(|result| result.code == 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screen_zero_is_display_one() {
        assert_eq!(display_for(ScreenId(0)), ":1");
        assert_eq!(display_for(ScreenId(7)), ":8");
    }

    #[test]
    fn test_friendly_key_names_become_keysyms() {
        assert_eq!(keysym("enter"), "Return");
        assert_eq!(keysym("ESC"), "Escape");
        assert_eq!(keysym("pageup"), "Prior");
        assert_eq!(keysym("cmd"), "super");
    }

    #[test]
    fn test_an_unknown_key_is_passed_through_untouched() {
        assert_eq!(
            keysym("F11"),
            "F11",
            "xdotool already knows keysyms; a second keymap would only be wrong"
        );
        assert_eq!(keysym("a"), "a");
    }

    #[test]
    fn test_a_chord_translates_every_part() {
        assert_eq!(chord("ctrl+shift+p"), "ctrl+shift+p");
        assert_eq!(chord("cmd+enter"), "super+Return");
    }

    #[test]
    fn test_a_chord_tolerates_spacing() {
        assert_eq!(chord("ctrl + c"), "ctrl+c");
        assert_eq!(chord("ctrl+"), "ctrl");
    }

    #[test]
    fn test_a_cursor_position_is_read_from_the_shell_form() {
        let parsed = parse_cursor("X=100\nY=250\nSCREEN=0\nWINDOW=12345\n");
        assert_eq!(parsed, Some(Point { x: 100, y: 250 }));
    }

    #[test]
    fn test_a_partial_cursor_reading_is_none_rather_than_a_guess() {
        assert_eq!(parse_cursor("X=100\nSCREEN=0\n"), None);
        assert_eq!(parse_cursor(""), None);
    }

    const SETTLE: Duration = Duration::from_millis(400);

    #[test]
    fn test_a_held_modifier_is_pressed_before_and_released_after() {
        let args = holding(&[Held::Ctrl, Held::Shift]);
        let back = letting_go(&[Held::Ctrl, Held::Shift]);

        assert_eq!(
            args,
            argv(&["xdotool", "keydown", "ctrl", "keydown", "shift"])
        );
        assert_eq!(back, argv(&["keyup", "shift", "keyup", "ctrl"]));
    }

    #[test]
    fn test_holding_nothing_is_the_command_that_was_always_sent() {
        assert_eq!(holding(&[]), argv(&["xdotool"]));
        assert!(letting_go(&[]).is_empty());
    }

    #[test]
    fn test_a_watch_stops_on_stillness_and_says_so() {
        let args = still_argv("import -window root png:-", SETTLE, SETTLE);
        let script = args.last().expect("a script");

        assert!(script.contains("cksum"), "{script}");
        assert!(script.contains("echo still"), "{script}");
        assert!(script.contains("echo moving"), "{script}");
    }

    #[test]
    fn test_a_plain_capture_is_the_whole_screen_at_full_size() {
        assert_eq!(
            capture_argv(None, None),
            argv(&["import", "-window", "root", "png:-"])
        );
    }

    #[test]
    fn test_a_cropped_capture_forgets_where_it_was_cut_from() {
        let args = capture_argv(Some(Rect::new(Point::new(10, 20), 400, 300)), None);

        assert!(args.contains(&"400x300+10+20".to_string()), "{args:?}");
        assert!(args.contains(&"+repage".to_string()), "{args:?}");
    }

    #[test]
    fn test_a_scaled_capture_averages_rather_than_blurs() {
        let args = capture_argv(None, Some(50));

        assert!(args.contains(&"box".to_string()), "{args:?}");
        assert!(args.contains(&"50%".to_string()), "{args:?}");
    }

    #[test]
    fn test_scrolling_up_is_button_four_and_down_is_five() {
        let up = scroll_argv(Point::new(5, 5), Delta::up(3));
        assert_eq!(up.last().map(String::as_str), Some("4"));

        let down = scroll_argv(Point::new(5, 5), Delta::down(3));
        assert_eq!(down.last().map(String::as_str), Some("5"));
    }

    #[test]
    fn test_scrolling_left_is_button_six_and_right_is_seven() {
        let left = scroll_argv(Point::new(5, 5), Delta::left(3));
        assert_eq!(left.last().map(String::as_str), Some("6"));

        let right = scroll_argv(Point::new(5, 5), Delta::right(3));
        assert_eq!(right.last().map(String::as_str), Some("7"));
    }

    #[test]
    fn test_a_sideways_scroll_does_not_also_go_down() {
        // `dy` is zero here and means zero, not the default: a caller asking
        // to go right and landing lower is a page in the wrong place.
        let args = scroll_argv(Point::new(0, 0), Delta::right(2));
        let buttons: Vec<&String> = args.iter().skip(4).collect();

        assert!(!buttons.contains(&&"4".to_string()), "{args:?}");
        assert!(!buttons.contains(&&"5".to_string()), "{args:?}");
    }

    #[test]
    fn test_both_axes_travel_as_one_command() {
        let args = scroll_argv(Point::new(5, 5), Delta { dx: 2, dy: 3 });

        assert_eq!(args.first().map(String::as_str), Some("xdotool"));
        assert_eq!(
            args.iter().filter(|word| *word == "click").count(),
            2,
            "one xdotool, two chained clicks: {args:?}"
        );
    }

    #[test]
    fn test_a_scroll_distance_is_bounded() {
        let args = scroll_argv(Point::new(0, 0), Delta { dx: 0, dy: 9_999 });
        assert!(
            args.contains(&"20".to_string()),
            "a runaway repeat count would hold the display for minutes"
        );
    }

    #[test]
    fn test_a_zero_scroll_still_moves_one_notch() {
        let args = scroll_argv(Point::new(0, 0), Delta { dx: 0, dy: 0 });
        assert!(args.contains(&"1".to_string()));
    }
}
