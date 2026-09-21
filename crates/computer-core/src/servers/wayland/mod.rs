mod profile;

pub use profile::WaylandProfile;

use crate::error::{Error, Result};
use crate::machine::MachineHost;
use crate::machine::ScreenHost;
use crate::motion::Step;
use crate::screens::ControlGate;
use crate::servers::{KeysDown, a11y, settled, still_argv};
use crate::{
    Button, Clipboard, Delta, Desktop, DesktopFactory, DisplayServer, ExecResult, Held, Node,
    NodeQuery, Point, Rect, ScreenId, Selection,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const INPUT_COMMAND: &str = "computer-input";
pub const POINTER_COMMAND: &str = "computer-pointer";

pub const DISPLAY_NAME: &str = "wayland-1";

pub fn runtime_dir(screen: ScreenId) -> String {
    format!("/tmp/computer/run-{}", screen.0 + 1)
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

fn button_name(button: Button) -> &'static str {
    match button {
        Button::Left => "left",
        Button::Middle => "middle",
        Button::Right => "right",
    }
}

pub fn keysym(key: &str) -> String {
    crate::servers::x11::keysym(key)
}

fn modifier(key: &str) -> Option<&'static str> {
    match key.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some("ctrl"),
        "alt" | "option" => Some("alt"),
        "shift" => Some("shift"),
        "meta" | "cmd" | "command" | "super" | "win" => Some("logo"),
        _ => None,
    }
}

pub fn chord(input: &str) -> Vec<String> {
    let parts: Vec<&str> = input
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();

    let held: Vec<&'static str> = parts.iter().filter_map(|part| modifier(part)).collect();
    let keys: Vec<String> = parts
        .iter()
        .filter(|part| modifier(part).is_none())
        .map(|part| keysym(part))
        .collect();

    let mut args = Vec::new();
    for name in &held {
        args.push("-M".to_string());
        args.push((*name).to_string());
    }
    for key in &keys {
        args.push("-k".to_string());
        args.push(key.clone());
    }
    for name in held.iter().rev() {
        args.push("-m".to_string());
        args.push((*name).to_string());
    }
    args
}

fn input_argv(verb: &str, parts: &[String]) -> Vec<String> {
    let mut args = vec![INPUT_COMMAND.to_string(), verb.to_string()];
    args.extend_from_slice(parts);
    args
}

fn holding(held: &[Held], verb: &str, parts: &[String]) -> Vec<String> {
    if held.is_empty() {
        return input_argv(verb, parts);
    }

    let names: Vec<&str> = held.iter().map(|one| one.keysym()).collect();
    let mut through = vec![names.join(","), verb.to_string()];
    through.extend_from_slice(parts);
    input_argv("with", &through)
}

fn key_parts(key: &str, down: bool) -> Result<Vec<String>> {
    let key = crate::servers::x11::one_key(key)?;

    Ok(match (modifier(&key), down) {
        (Some(name), true) => vec!["-M".to_string(), name.to_string()],
        (Some(name), false) => vec!["-m".to_string(), name.to_string()],
        (None, true) => vec!["-P".to_string(), key],
        (None, false) => vec!["-p".to_string(), key],
    })
}

fn button_parts(button: Button, at: Option<Point>) -> Vec<String> {
    let mut parts = vec![button_name(button).to_string()];
    parts.extend(at.map(point_parts).unwrap_or_default());
    parts
}

fn point_parts(at: Point) -> Vec<String> {
    vec![at.x.to_string(), at.y.to_string()]
}

/// Not `grim -c`: headless sway draws no cursor, even with a pointer on the seat.
fn pointing_argv(at: Point, area: Option<Rect>, scale: Option<u32>) -> Vec<String> {
    let mut draw = vec![
        "convert".to_string(),
        "png:-".to_string(),
        "-depth".to_string(),
        "8".to_string(),
    ];
    draw.extend(pointer_argv(at));
    draw.extend(shaping_argv(area, scale));
    draw.push("png:-".to_string());

    argv(&["sh", "-c"])
        .into_iter()
        .chain([format!("grim -t png - | {}", draw.join(" "))])
        .collect()
}

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

    vec![
        "-stroke".to_string(),
        "black".to_string(),
        "-strokewidth".to_string(),
        "1".to_string(),
        "-fill".to_string(),
        "white".to_string(),
        "-draw".to_string(),
        // Joined into a shell line, and the polygon carries spaces.
        format!("'polygon {arrow}'"),
    ]
}

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
        args.push("-filter".to_string());
        args.push("box".to_string());
        args.push("-resize".to_string());
        args.push(format!("{scale}%"));
    }

    args
}

fn capture_argv(area: Option<Rect>, scale: Option<u32>) -> Vec<String> {
    let mut args = argv(&["grim", "-t", "png"]);

    if let Some(area) = area {
        args.push("-g".to_string());
        args.push(format!(
            "{},{} {}x{}",
            area.at.x, area.at.y, area.width, area.height
        ));
    }
    if let Some(scale) = scale {
        args.push("-s".to_string());
        args.push(format!("{:.4}", f64::from(scale) / 100.0));
    }

    args.push("-".to_string());
    args
}

fn scroll_argv(at: Point, by: Delta) -> Vec<String> {
    let mut parts = point_parts(at);
    // A zero delta still scrolls one notch down.
    parts.push(notches(by.dy, by.dx == 0).to_string());
    parts.push(notches(by.dx, false).to_string());
    input_argv("scroll", &parts)
}

fn notches(delta: i32, floor: bool) -> i32 {
    if delta == 0 {
        return i32::from(floor);
    }

    let size = delta.unsigned_abs().clamp(1, 20) as i32;
    match delta < 0 {
        true => -size,
        false => size,
    }
}

#[derive(Debug, Clone, Copy)]
struct Tracked {
    at: Point,
    takeovers: u64,
}

pub struct WaylandDesktop {
    host: Arc<dyn ScreenHost>,
    screen: ScreenId,
    control: Arc<ControlGate>,
    pointer: Mutex<Option<Tracked>>,
    down: KeysDown,
}

impl WaylandDesktop {
    pub fn new(host: Arc<dyn ScreenHost>, screen: ScreenId) -> Self {
        Self {
            host,
            screen,
            control: Arc::new(ControlGate::new()),
            pointer: Mutex::new(None),
            down: KeysDown::default(),
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

    async fn grim(&self, args: Vec<String>) -> Result<Vec<u8>> {
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

    /// The pointer client takes one pause for the whole path; a path has one.
    fn pause_ms(steps: &[Step]) -> String {
        steps
            .first()
            .map(|step| step.pause.as_millis())
            .unwrap_or(0)
            .to_string()
    }

    fn moved_to(&self, at: Point) {
        if let Ok(mut pointer) = self.pointer.lock() {
            *pointer = Some(Tracked {
                at,
                takeovers: self.control.takeovers(),
            });
        }
    }
}

#[async_trait]
impl Desktop for WaylandDesktop {
    async fn screenshot(&self) -> Result<Vec<u8>> {
        self.grim(capture_argv(None, None)).await
    }

    async fn capture(&self, area: Option<Rect>, scale: Option<u32>) -> Result<Vec<u8>> {
        self.grim(capture_argv(area, scale)).await
    }

    async fn capture_pointing(&self, area: Option<Rect>, scale: Option<u32>) -> Result<Vec<u8>> {
        let at = self.find_cursor().await?;
        self.grim(pointing_argv(at, area, scale)).await
    }

    async fn move_to(&self, at: Point) -> Result<()> {
        self.act(input_argv("move", &point_parts(at))).await?;
        self.moved_to(at);
        Ok(())
    }

    async fn click(&self, at: Point, button: Button) -> Result<()> {
        self.click_with(at, button, &[]).await
    }

    async fn click_with(&self, at: Point, button: Button, held: &[Held]) -> Result<()> {
        let mut parts = point_parts(at);
        parts.push(button_name(button).to_string());
        self.act(holding(&self.down.without(held), "click", &parts))
            .await?;
        self.moved_to(at);
        Ok(())
    }

    async fn double_click(&self, at: Point, button: Button) -> Result<()> {
        let mut parts = point_parts(at);
        parts.push(button_name(button).to_string());
        self.act(input_argv("dblclick", &parts)).await?;
        self.moved_to(at);
        Ok(())
    }

    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()> {
        self.drag_with(from, to, button, &[]).await
    }

    async fn drag_with(&self, from: Point, to: Point, button: Button, held: &[Held]) -> Result<()> {
        let mut parts = point_parts(from);
        parts.extend(point_parts(to));
        parts.push(button_name(button).to_string());
        self.act(holding(&self.down.without(held), "drag", &parts))
            .await?;
        self.moved_to(to);
        Ok(())
    }

    async fn move_along(&self, steps: &[Step]) -> Result<()> {
        let Some(last) = steps.last() else {
            return Ok(());
        };
        let mut parts = vec![Self::pause_ms(steps)];
        for step in steps {
            parts.extend(point_parts(step.at));
        }
        self.act(input_argv("path", &parts)).await?;
        self.moved_to(last.at);
        Ok(())
    }

    async fn drag_along(
        &self,
        from: Point,
        steps: &[Step],
        button: Button,
        held: &[Held],
    ) -> Result<()> {
        let Some(last) = steps.last() else {
            return self.click_with(from, button, held).await;
        };
        let mut parts = vec![button_name(button).to_string(), Self::pause_ms(steps)];
        parts.extend(point_parts(from));
        for step in steps {
            parts.extend(point_parts(step.at));
        }
        self.act(holding(&self.down.without(held), "sweep", &parts))
            .await?;
        self.moved_to(last.at);
        Ok(())
    }

    async fn button_down(&self, at: Option<Point>, button: Button) -> Result<()> {
        self.act(input_argv("down", &button_parts(button, at)))
            .await?;
        if let Some(at) = at {
            self.moved_to(at);
        }
        Ok(())
    }

    async fn button_up(&self, at: Option<Point>, button: Button) -> Result<()> {
        self.act(input_argv("up", &button_parts(button, at)))
            .await?;
        if let Some(at) = at {
            self.moved_to(at);
        }
        Ok(())
    }

    async fn let_go(&self, button: Button) -> Result<()> {
        self.run(vec![
            POINTER_COMMAND.to_string(),
            "up".to_string(),
            button_name(button).to_string(),
        ])
        .await
        .map(|_| ())
    }

    async fn key_down(&self, key: &str) -> Result<()> {
        self.act(input_argv("key", &key_parts(key, true)?)).await?;
        self.down.set(&crate::servers::x11::one_key(key)?, true);
        Ok(())
    }

    async fn key_up(&self, key: &str) -> Result<()> {
        self.act(input_argv("key", &key_parts(key, false)?)).await?;
        self.down.set(&crate::servers::x11::one_key(key)?, false);
        Ok(())
    }

    async fn let_key_go(&self, key: &str) -> Result<()> {
        let mut args = vec![POINTER_COMMAND.to_string(), "key".to_string()];
        args.extend(key_parts(key, false)?);
        self.down.set(&crate::servers::x11::one_key(key)?, false);
        self.run(args).await.map(|_| ())
    }

    async fn let_keys_go(&self) -> Result<()> {
        self.down.clear();
        self.run(vec![POINTER_COMMAND.to_string(), "release".to_string()])
            .await
            .map(|_| ())
    }

    async fn type_text(&self, text: &str, delay: Option<Duration>) -> Result<()> {
        match delay {
            Some(delay) => {
                let parts = [delay.as_millis().to_string(), text.to_string()];
                self.act(input_argv("paced", &parts)).await
            }
            None => self.act(input_argv("type", &[text.to_string()])).await,
        }
    }

    /// One run, so the modifiers come up in the command that put them down.
    async fn press(&self, chords: &[String], held: &[Held]) -> Result<()> {
        let held = &self.down.without(held);
        let mut parts = Vec::new();

        for one in held {
            parts.push("-M".to_string());
            parts.push(one.keysym().to_string());
        }
        for one in chords {
            parts.extend(chord(one));
        }
        for one in held.iter().rev() {
            parts.push("-m".to_string());
            parts.push(one.keysym().to_string());
        }

        self.act(input_argv("key", &parts)).await
    }

    async fn scroll(&self, at: Point, by: Delta) -> Result<()> {
        self.act(scroll_argv(at, by)).await?;
        self.moved_to(at);
        Ok(())
    }

    async fn wait_until_still(&self, settle: Duration, within: Duration) -> Result<()> {
        let watched = self
            .run(still_argv("grim -t png - 2>/dev/null", settle, within))
            .await?;

        settled(watched, within)
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
        let tracked = self
            .pointer
            .lock()
            .ok()
            .and_then(|pointer| *pointer)
            .ok_or(Error::Unsupported {
                gaps: vec!["cursor before the first move"],
            })?;

        if tracked.takeovers == self.control.takeovers() {
            Ok(tracked.at)
        } else {
            Err(Error::Unsupported {
                gaps: vec!["cursor after a person drove the screen"],
            })
        }
    }

    /// Moves the pointer to the middle when its position is unknown, since nothing can read it.
    async fn find_cursor(&self) -> Result<Point> {
        if let Ok(at) = self.cursor().await {
            return Ok(at);
        }

        let (width, height) = self.geometry().await?;
        let middle = Point::new(width / 2, height / 2);
        self.move_to(middle).await?;

        Ok(middle)
    }

    async fn geometry(&self) -> Result<(u32, u32)> {
        let result = self
            .run(argv(&[
                "bash",
                "-c",
                "grim -t png - | head -c 24 | od -An -tu1 -j16 -N8",
            ]))
            .await?;

        parse_png_size(&result.stdout_utf8())
            .ok_or_else(|| Error::denied("the screen geometry could not be read"))
    }

    /// Asks the compositor, since a dead one leaves its socket file behind.
    async fn alive(&self) -> Result<()> {
        let sockfile = format!("/tmp/computer/screen-{}.sway", self.screen.0);
        let mut args = argv(&["bash", "-c"]);
        args.push(format!(
            "swaymsg -s \"$(cat {sockfile})\" -t get_version >/dev/null 2>&1"
        ));

        self.run(args)
            .await
            .map(|_| ())
            .map_err(|_| Error::Gone(format!("no compositor in {}", runtime_dir(self.screen))))
    }

    fn control(&self) -> &Arc<ControlGate> {
        &self.control
    }

    fn as_clipboard(&self) -> Option<&dyn Clipboard> {
        Some(self)
    }
}

fn selection_flag(selection: Selection) -> &'static [&'static str] {
    match selection {
        Selection::Clipboard => &[],
        Selection::Primary => &["-p"],
    }
}

fn empty_selection(stderr: &str) -> bool {
    stderr.contains("No selection")
}

#[async_trait]
impl Clipboard for WaylandDesktop {
    async fn text(&self, selection: Selection) -> Result<String> {
        let mut args = argv(&["wl-paste", "-n"]);
        args.extend(argv(selection_flag(selection)));

        match self.run(args).await {
            Ok(result) => Ok(result.stdout_utf8()),
            Err(Error::Failed { stderr, .. }) if empty_selection(&stderr) => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    /// The path is `$0`, so no character in it can become shell syntax.
    async fn set_from(&self, selection: Selection, path: &str) -> Result<()> {
        let mut args = argv(&["bash", "-c", "wl-copy \"$@\" < \"$0\"", path, "--"]);
        args.extend(argv(selection_flag(selection)));
        self.act(args).await
    }

    async fn bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>> {
        let mut args = argv(&["wl-paste"]);
        args.extend(argv(selection_flag(selection)));
        args.push("-t".to_string());
        args.push(target.to_string());

        match self.run(args).await {
            Ok(result) => Ok(result.stdout),
            Err(Error::Failed { stderr, .. }) if empty_selection(&stderr) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    async fn set_bytes_from(&self, selection: Selection, target: &str, path: &str) -> Result<()> {
        let mut args = argv(&["bash", "-c", "wl-copy \"$@\" < \"$0\"", path, "--"]);
        args.extend(argv(selection_flag(selection)));
        args.push("-t".to_string());
        args.push(target.to_string());
        self.act(args).await
    }

    async fn targets(&self, selection: Selection) -> Result<Vec<String>> {
        let mut args = argv(&["wl-paste", "-l"]);
        args.extend(argv(selection_flag(selection)));

        let listed = match self.run(args).await {
            Ok(result) => result.stdout_utf8(),
            Err(Error::Failed { stderr, .. }) if empty_selection(&stderr) => String::new(),
            Err(error) => return Err(error),
        };

        Ok(listed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }
}

pub fn parse_png_size(output: &str) -> Option<(u32, u32)> {
    let bytes: Vec<u32> = output
        .split_whitespace()
        .filter_map(|byte| byte.parse().ok())
        .collect();

    let [w0, w1, w2, w3, h0, h1, h2, h3] = bytes.get(..8)? else {
        return None;
    };
    let width = (w0 << 24) | (w1 << 16) | (w2 << 8) | w3;
    let height = (h0 << 24) | (h1 << 16) | (h2 << 8) | h3;

    (width > 0 && height > 0).then_some((width, height))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WaylandDriver;

impl DesktopFactory for WaylandDriver {
    fn display_server(&self) -> DisplayServer {
        DisplayServer::Wayland
    }

    fn open(&self, host: Arc<MachineHost>, screen: ScreenId) -> Arc<dyn Desktop> {
        Arc::new(WaylandDesktop::new(host as Arc<dyn ScreenHost>, screen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoHost;

    #[async_trait]
    impl ScreenHost for NoHost {
        async fn run(&self, _argv: &[String], _screen: ScreenId) -> Result<ExecResult> {
            Err(Error::transport_public("no box here"))
        }
    }

    #[tokio::test]
    async fn test_the_pointer_is_placed_only_where_it_was_asked_about() {
        let desktop = WaylandDesktop::new(Arc::new(NoHost) as Arc<dyn ScreenHost>, ScreenId(0));

        assert!(
            desktop.cursor().await.is_err(),
            "reporting the pointer never moves it: a batch told to report one, \
             and a click with no point of its own, both come through here"
        );

        let reached = desktop.find_cursor().await.expect_err("no host here");
        assert!(
            !reached.to_string().contains("before the first move"),
            "it went on to place the pointer rather than giving up: {reached}"
        );
    }

    #[test]
    fn test_the_pointer_is_drawn_before_the_picture_is_cut_down() {
        let args = pointing_argv(
            Point::new(100, 50),
            Some(Rect::new(Point::new(80, 40), 200, 200)),
            Some(50),
        );

        assert_eq!(args[0], "sh", "a capture and a draw are two programs");
        let line = &args[2];

        let at = |what: &str| line.find(what);
        assert!(
            at("-draw") < at("-crop") && at("-crop") < at("-resize"),
            "the arrow is placed in screen coordinates, so it goes on before \
             anything moves or resizes the picture: {line}"
        );
        assert!(
            line.contains("'polygon 100,50 "),
            "the arrow starts at the pointer, quoted for the shell: {line}"
        );
    }

    #[test]
    fn test_screens_are_told_apart_by_directory_and_not_by_socket_name() {
        assert_eq!(runtime_dir(ScreenId(0)), "/tmp/computer/run-1");
        assert_eq!(runtime_dir(ScreenId(7)), "/tmp/computer/run-8");
        assert_eq!(
            DISPLAY_NAME, "wayland-1",
            "a compositor takes the first free name in the directory it is \
             given, and every screen gets its own directory"
        );
    }

    #[test]
    fn test_a_chord_holds_its_modifiers_and_lets_them_go() {
        assert_eq!(
            chord("ctrl+c"),
            vec!["-M", "ctrl", "-k", "c", "-m", "ctrl"],
            "a modifier left held arrives on every keystroke after it"
        );
    }

    #[test]
    fn test_modifiers_are_released_in_the_order_they_were_taken() {
        assert_eq!(
            chord("ctrl+shift+p"),
            vec![
                "-M", "ctrl", "-M", "shift", "-k", "p", "-m", "shift", "-m", "ctrl"
            ]
        );
    }

    #[test]
    fn test_a_chord_uses_the_same_key_names_as_the_x11_driver() {
        assert_eq!(chord("enter"), vec!["-k", "Return"]);
        assert_eq!(
            chord("cmd+enter"),
            vec!["-M", "logo", "-k", "Return", "-m", "logo"],
            "the keyboard in the box knows the super key as logo too, and a caller should \
             not have to"
        );
    }

    #[test]
    fn test_a_chord_tolerates_spacing() {
        assert_eq!(
            chord("ctrl + c"),
            vec!["-M", "ctrl", "-k", "c", "-m", "ctrl"]
        );
        assert_eq!(chord("ctrl+"), vec!["-M", "ctrl", "-m", "ctrl"]);
    }

    #[test]
    fn test_a_plain_capture_is_the_whole_screen_at_full_size() {
        assert_eq!(capture_argv(None, None), argv(&["grim", "-t", "png", "-"]));
    }

    #[test]
    fn test_a_capture_carries_its_rectangle_in_grims_own_spelling() {
        let args = capture_argv(Some(Rect::new(Point::new(10, 20), 400, 300)), None);

        assert!(args.contains(&"10,20 400x300".to_string()), "{args:?}");
    }

    #[test]
    fn test_a_scale_reaches_grim_as_a_factor() {
        let args = capture_argv(None, Some(50));

        assert!(args.contains(&"0.5000".to_string()), "{args:?}");
    }

    #[test]
    fn test_scrolling_up_is_a_negative_count_and_down_a_positive_one() {
        assert_eq!(tail(Delta::up(3)), ["-3", "0"]);
        assert_eq!(tail(Delta::down(3)), ["3", "0"]);
    }

    #[test]
    fn test_scrolling_left_is_a_negative_count_and_right_a_positive_one() {
        assert_eq!(tail(Delta::left(3)), ["0", "-3"]);
        assert_eq!(tail(Delta::right(3)), ["0", "3"]);
    }

    #[test]
    fn test_a_sideways_scroll_does_not_also_go_down() {
        assert_eq!(tail(Delta::right(2))[0], "0");
    }

    #[test]
    fn test_both_axes_travel_as_one_run() {
        assert_eq!(tail(Delta { dx: 2, dy: 3 }), ["3", "2"]);
    }

    fn tail(by: Delta) -> [String; 2] {
        let args = scroll_argv(Point::new(5, 5), by);
        let mut last = args.iter().rev().take(2);
        let right = last.next().cloned().expect("a horizontal count");
        let down = last.next().cloned().expect("a vertical count");

        [down, right]
    }

    #[test]
    fn test_a_scroll_distance_is_bounded_in_both_directions() {
        assert!(
            scroll_argv(Point::new(0, 0), Delta { dx: 0, dy: 9_999 }).contains(&"20".to_string()),
            "a runaway count would hold the screen for minutes"
        );
        assert!(
            scroll_argv(Point::new(0, 0), Delta { dx: 0, dy: -9_999 }).contains(&"-20".to_string())
        );
    }

    #[test]
    fn test_a_zero_scroll_still_moves_one_notch() {
        let args = scroll_argv(Point::new(0, 0), Delta { dx: 0, dy: 0 });
        assert!(args.contains(&"1".to_string()));
    }

    #[test]
    fn test_a_size_is_read_out_of_the_frames_own_header() {
        assert_eq!(
            parse_png_size(" 0 0 5 0 0 0 3 32\n"),
            Some((1280, 800)),
            "the frame is what the coordinates are against"
        );
    }

    #[test]
    fn test_a_short_header_is_none_rather_than_a_guess() {
        assert_eq!(parse_png_size(" 0 0 5 0"), None);
        assert_eq!(parse_png_size(""), None);
        assert_eq!(
            parse_png_size(" 0 0 0 0 0 0 0 0"),
            None,
            "a zero-sized screen is a capture that failed, not a screen"
        );
    }
}
