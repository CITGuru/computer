mod profile;

pub use profile::X11Profile;

use crate::error::{Error, Result};
use crate::machine::{MachineHost, ScreenHost};
use crate::motion::Step;
use crate::screens::ControlGate;
use crate::servers::{a11y, settled, still_argv};
use crate::{
    Button, Clipboard, Delta, Desktop, DesktopFactory, DisplayServer, ExecResult, Held, Node,
    NodeQuery, Point, Rect, ScreenId, Selection,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Never `:0`, which on a host with a physical display is that display.
pub fn display_for(screen: ScreenId) -> String {
    format!(":{}", screen.0 + 1)
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

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

fn button_argv(at: Option<Point>, verb: &str, button: Button) -> Vec<String> {
    let mut args = match at {
        Some(at) => point_argv(&["xdotool", "mousemove", "--"], at),
        None => argv(&["xdotool"]),
    };
    args.extend(argv(&[verb, button_number(button)]));
    args
}

fn point_argv(command: &[&str], at: Point) -> Vec<String> {
    let mut args = argv(command);
    args.push(at.x.to_string());
    args.push(at.y.to_string());
    args
}

/// `sleep` takes seconds. A pause under a millisecond is not worth the word.
fn along(steps: &[Step]) -> Vec<String> {
    let mut args = Vec::new();

    for step in steps {
        args.extend(point_argv(&["mousemove", "--"], step.at));
        if step.pause >= Duration::from_millis(1) {
            args.push("sleep".to_string());
            args.push(format!("{:.3}", step.pause.as_secs_f64()));
        }
    }
    args
}

/// One chained command: a held key is released when the process that pressed it exits.
fn holding(held: &[Held]) -> Vec<String> {
    let mut args = argv(&["xdotool"]);

    for one in held {
        args.push("keydown".to_string());
        args.push(one.keysym().to_string());
    }
    args
}

fn letting_go(held: &[Held]) -> Vec<String> {
    let mut args = Vec::new();

    for one in held.iter().rev() {
        args.push("keyup".to_string());
        args.push(one.keysym().to_string());
    }
    args
}

/// `import` cannot see the X11 cursor, so a stand-in arrow is drawn.
fn pointer_argv(at: Point) -> Vec<String> {
    let (x, y) = (i64::from(at.x), i64::from(at.y));
    let arrow = [
        (0, 0),
        (0, 17),
        (4, 13),
        (7, 19),
        (10, 18),
        (7, 12),
        (12, 12),
    ]
    .iter()
    .map(|(dx, dy)| format!("{},{}", x + dx, y + dy))
    .collect::<Vec<_>>()
    .join(" ");

    argv(&[
        "-stroke",
        "black",
        "-strokewidth",
        "1",
        "-fill",
        "white",
        "-draw",
    ])
    .into_iter()
    .chain([format!("polygon {arrow}")])
    .collect()
}

/// `+repage`, or the PNG keeps its crop offset and a viewer draws it misplaced.
fn shaping_argv(area: Option<Rect>, scale: Option<u32>) -> Vec<String> {
    let mut args = Vec::new();

    if let Some(area) = area {
        args.push("-crop".to_string());
        args.push(format!(
            "{}x{}+{}+{}",
            area.width, area.height, area.at.x, area.at.y
        ));
        args.push("+repage".to_string());
    }
    if let Some(scale) = scale {
        // Box, not the default Lanczos: halved, Lanczos is 67KB and box is 24KB.
        args.push("-filter".to_string());
        args.push("box".to_string());
        args.push("-resize".to_string());
        args.push(format!("{scale}%"));
    }

    args
}

fn capture_argv(area: Option<Rect>, scale: Option<u32>) -> Vec<String> {
    let mut args = argv(&["import", "-window", "root"]);
    args.extend(shaping_argv(area, scale));

    args.push("png:-".to_string());
    args
}

/// `import` takes no drawing options, and `convert` reads the root at 16 bits unless told.
fn pointing_argv(at: Point, area: Option<Rect>, scale: Option<u32>) -> Vec<String> {
    let mut args = argv(&["convert", "x:root", "-depth", "8"]);
    args.extend(pointer_argv(at));
    args.extend(shaping_argv(area, scale));
    args.push("png:-".to_string());
    args
}

fn scroll_argv(at: Point, by: Delta) -> Vec<String> {
    let mut args = point_argv(&["xdotool", "mousemove", "--"], at);

    // A zero delta still scrolls one notch down.
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

    async fn capture_pointing(&self, area: Option<Rect>, scale: Option<u32>) -> Result<Vec<u8>> {
        let at = self.cursor().await?;
        self.import(pointing_argv(at, area, scale)).await
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

    /// One command: two round trips arrive far enough apart to read as two single clicks.
    async fn double_click(&self, at: Point, button: Button) -> Result<()> {
        let mut args = point_argv(&["xdotool", "mousemove", "--"], at);
        args.extend(argv(&["click", "--repeat", "2", "--delay", "80"]));
        args.push(button_number(button).to_string());
        self.act(args).await
    }

    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()> {
        self.drag_with(from, to, button, &[]).await
    }

    async fn drag_with(&self, from: Point, to: Point, button: Button, held: &[Held]) -> Result<()> {
        let number = button_number(button);
        let mut args = holding(held);
        args.extend(point_argv(&["mousemove", "--"], from));
        args.extend(argv(&["mousedown", number]));
        // Through the middle: an app that tracks motion ignores a drag that teleports.
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

    async fn button_down(&self, at: Option<Point>, button: Button) -> Result<()> {
        self.act(button_argv(at, "mousedown", button)).await
    }

    async fn button_up(&self, at: Option<Point>, button: Button) -> Result<()> {
        self.act(button_argv(at, "mouseup", button)).await
    }

    async fn let_go(&self, button: Button) -> Result<()> {
        self.run(button_argv(None, "mouseup", button))
            .await
            .map(|_| ())
    }

    async fn move_along(&self, steps: &[Step]) -> Result<()> {
        let mut args = argv(&["xdotool"]);
        args.extend(along(steps));
        self.act(args).await
    }

    async fn drag_along(
        &self,
        from: Point,
        steps: &[Step],
        button: Button,
        held: &[Held],
    ) -> Result<()> {
        let number = button_number(button);
        let mut args = holding(held);
        args.extend(point_argv(&["mousemove", "--"], from));
        args.extend(argv(&["mousedown", number]));
        args.extend(along(steps));
        args.extend(argv(&["mouseup", number]));
        args.extend(letting_go(held));

        self.act(args).await
    }

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

    async fn type_text(&self, text: &str, delay: Option<Duration>) -> Result<()> {
        let mut args = argv(&["xdotool", "type", "--clearmodifiers"]);

        if let Some(delay) = delay {
            args.push("--delay".to_string());
            args.push(delay.as_millis().to_string());
        }
        // `--` last, or text beginning with a dash is read as a flag.
        args.push("--".to_string());
        args.push(text.to_string());
        self.act(args).await
    }

    /// `--clearmodifiers` only when nothing is held, as it would drop the held keys.
    async fn press(&self, chords: &[String], held: &[Held]) -> Result<()> {
        if let ([one], true) = (chords, held.is_empty()) {
            let mut args = argv(&["xdotool", "key", "--clearmodifiers"]);
            args.push(chord(one));
            return self.act(args).await;
        }

        let mut args = holding(held);
        for one in chords {
            args.push("key".to_string());
            args.push(chord(one));
        }
        args.extend(letting_go(held));

        self.act(args).await
    }

    async fn scroll(&self, at: Point, by: Delta) -> Result<()> {
        self.act(scroll_argv(at, by)).await
    }

    async fn nodes(&self, app: Option<&str>, depth: Option<u32>) -> Result<Vec<Node>> {
        a11y::tree(&self.host, self.screen, app, depth).await
    }

    async fn find_nodes(&self, query: &NodeQuery, limit: Option<usize>) -> Result<Vec<Node>> {
        a11y::find(&self.host, self.screen, query, limit).await
    }

    async fn focus_node(&self, query: &NodeQuery) -> Result<Node> {
        self.control.may_act()?;
        a11y::focus(&self.host, self.screen, query).await
    }

    async fn invoke_node(&self, query: &NodeQuery, action: Option<&str>) -> Result<Node> {
        self.control.may_act()?;
        a11y::invoke(&self.host, self.screen, query, action).await
    }

    async fn set_node(&self, query: &NodeQuery, value: &str) -> Result<Node> {
        self.control.may_act()?;
        a11y::set(&self.host, self.screen, query, value).await
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
    async fn text(&self, selection: Selection) -> Result<String> {
        let result = self
            .run(argv(&["xclip", "-selection", selection.name(), "-o"]))
            .await;

        match result {
            Ok(result) => Ok(result.stdout_utf8()),
            // No owner answers "not available"; any other failure stays an error.
            Err(Error::Failed { stderr, .. }) if stderr.contains("not available") => {
                Ok(String::new())
            }
            Err(error) => Err(error),
        }
    }

    /// `setsid`: X keeps no copy of a selection, so `xclip` must outlive the command.
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

    async fn bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>> {
        let mut args = argv(&["xclip", "-selection", selection.name(), "-t"]);
        args.push(target.to_string());
        args.push("-o".to_string());

        match self.run(args).await {
            Ok(result) => Ok(result.stdout),
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

/// Under `bash`: `/dev/tcp` is a bash feature and Debian's `sh` is dash.
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
    fn test_the_pointer_is_drawn_before_the_picture_is_cut_down() {
        let args = pointing_argv(
            Point::new(100, 50),
            Some(Rect::new(Point::new(80, 40), 200, 200)),
            Some(50),
        );
        let at = |what: &str| args.iter().position(|arg| arg == what);

        assert_eq!(args[0], "convert", "import takes no drawing options");
        assert_eq!(args[1], "x:root");
        assert!(
            at("-draw") < at("-crop") && at("-crop") < at("-resize"),
            "the arrow is placed in root coordinates, so it goes on before \
             anything moves or resizes the picture: {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg.starts_with("polygon 100,50 ")),
            "the arrow starts at the pointer: {args:?}"
        );
        assert!(
            args.windows(2).any(|pair| pair == ["-depth", "8"]),
            "convert reads the root at sixteen bits and import does not"
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
