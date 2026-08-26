//! Driving a Wayland compositor, wherever it is running.
//!
//! The same shape as [`crate::servers::x11`], against a display server that refuses
//! three things X11 allowed.
//!
//! Synthetic input is a compositor privilege, so there is no `xdotool`. The
//! image supplies `computer-input`, which reaches sway's IPC for the pointer
//! and `wtype` for the keyboard; neither needs `/dev/uinput`, so the box keeps
//! the isolation it started with.
//!
//! There is no root window to capture, so `grim` asks the compositor.
//!
//! No client can read the global pointer position, so this driver remembers
//! where it put the pointer and refuses to answer once a person has driven the
//! screen. See [`WaylandDesktop::cursor`].
//!
//! # Coordinates
//!
//! Device pixels, top-left origin, against the frame the last screenshot
//! returned, as everywhere else in this crate.

mod profile;

pub use profile::WaylandProfile;

use crate::error::{Error, Result};
use crate::machine::MachineHost;
use crate::machine::ScreenHost;
use crate::screens::ControlGate;
use crate::{
    Button, Clipboard, Delta, Desktop, DesktopFactory, DisplayServer, ExecResult, Point, ScreenId,
    Selection,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// The command the image installs for every input it accepts.
pub const INPUT_COMMAND: &str = "computer-input";

/// What every compositor in this image calls its socket.
///
/// One name for all of them: a compositor is reached through the directory
/// holding the socket, so the separation is the directory.
pub const DISPLAY_NAME: &str = "wayland-1";

/// The runtime directory screen *i*'s compositor lives in.
///
/// One per screen. Two sharing a directory would fight for one socket name.
pub fn runtime_dir(screen: ScreenId) -> String {
    format!("/tmp/computer/run-{}", screen.0 + 1)
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

/// What `computer-pointer` calls a mouse button.
///
/// A name rather than a Linux event code, so `0x110` lives in one place.
fn button_name(button: Button) -> &'static str {
    match button {
        Button::Left => "left",
        Button::Middle => "middle",
        Button::Right => "right",
    }
}

/// `wtype`'s name for a key, as a caller is likely to name it.
///
/// `wtype -k` takes an xkb keysym, the same vocabulary `xdotool` takes.
pub fn keysym(key: &str) -> String {
    crate::servers::x11::keysym(key)
}

/// Whether a key names a modifier `wtype` can hold down.
fn modifier(key: &str) -> Option<&'static str> {
    match key.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some("ctrl"),
        "alt" | "option" => Some("alt"),
        "shift" => Some("shift"),
        "meta" | "cmd" | "command" | "super" | "win" => Some("logo"),
        _ => None,
    }
}

/// Turn a chord such as `ctrl+shift+p` into the `wtype` arguments for it.
///
/// `wtype` has no chord syntax: `-M` holds a modifier, `-m` releases it.
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
    // Released in reverse, so the last one held is the first one let go.
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

fn point_parts(at: Point) -> Vec<String> {
    vec![at.x.to_string(), at.y.to_string()]
}

/// A signed count of wheel notches, negative for up.
///
/// The protocol takes a direction and a distance, so there is no button 4
/// and 5 here.
fn scroll_argv(at: Point, by: Delta) -> Vec<String> {
    let notches = by.dy.unsigned_abs().clamp(1, 20) as i32;
    let signed = if by.dy < 0 { -notches } else { notches };

    let mut parts = point_parts(at);
    parts.push(signed.to_string());
    input_argv("scroll", &parts)
}

/// Where the pointer was last put, and which takeover that was true under.
///
/// A person on the input moves a pointer this driver did not move, so the
/// remembered position stops being an answer once one has driven.
#[derive(Debug, Clone, Copy)]
struct Tracked {
    at: Point,
    takeovers: u64,
}

/// One screen, driven through a compositor.
pub struct WaylandDesktop {
    host: Arc<dyn ScreenHost>,
    screen: ScreenId,
    control: Arc<ControlGate>,
    pointer: Mutex<Option<Tracked>>,
}

impl WaylandDesktop {
    pub fn new(host: Arc<dyn ScreenHost>, screen: ScreenId) -> Self {
        Self {
            host,
            screen,
            control: Arc::new(ControlGate::new()),
            pointer: Mutex::new(None),
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

    /// Every input path, and the only place the takeover rule is applied here.
    ///
    /// Reads do not come through it: a run may watch and may not act.
    async fn act(&self, args: Vec<String>) -> Result<()> {
        self.control.may_act()?;
        self.run(args).await.map(|_| ())
    }

    /// Record where the pointer now is, against the takeover it is true under.
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
        // PNG bytes straight out of stdout, as with the X11 image's `import`.
        let result = self.run(argv(&["grim", "-t", "png", "-"])).await?;

        if result.stdout.is_empty() {
            return Err(Error::denied("the screen capture returned no image"));
        }
        Ok(result.stdout)
    }

    async fn move_to(&self, at: Point) -> Result<()> {
        self.act(input_argv("move", &point_parts(at))).await?;
        self.moved_to(at);
        Ok(())
    }

    async fn click(&self, at: Point, button: Button) -> Result<()> {
        let mut parts = point_parts(at);
        parts.push(button_name(button).to_string());
        self.act(input_argv("click", &parts)).await?;
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
        let mut parts = point_parts(from);
        parts.extend(point_parts(to));
        parts.push(button_name(button).to_string());
        self.act(input_argv("drag", &parts)).await?;
        self.moved_to(to);
        Ok(())
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        self.act(input_argv("type", &[text.to_string()])).await
    }

    async fn key(&self, keys: &str) -> Result<()> {
        self.act(input_argv("key", &chord(keys))).await
    }

    async fn scroll(&self, at: Point, by: Delta) -> Result<()> {
        self.act(scroll_argv(at, by)).await?;
        self.moved_to(at);
        Ok(())
    }

    /// Where this driver last put the pointer.
    ///
    /// Wayland reports no global pointer position to any client, so this is
    /// remembered rather than read, and refused once a person has driven.
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

    async fn geometry(&self) -> Result<(u32, u32)> {
        let result = self
            .run(argv(&[
                "bash",
                "-c",
                // The compositor's own idea of the size: coordinates are
                // against the screen that came up.
                "grim -t png - | head -c 24 | od -An -tu1 -j16 -N8",
            ]))
            .await?;

        parse_png_size(&result.stdout_utf8())
            .ok_or_else(|| Error::denied("the screen geometry could not be read"))
    }

    /// Asked of the compositor rather than of a socket file: one that died
    /// leaves the file behind, and every check that reads configuration passes
    /// while the first screenshot fails.
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

/// `wl-copy` and `wl-paste` name the primary selection with a flag rather than
/// with a name, so there is nothing to pass for the clipboard.
fn selection_flag(selection: Selection) -> &'static [&'static str] {
    match selection {
        Selection::Clipboard => &[],
        Selection::Primary => &["-p"],
    }
}

/// Whether a failure means nothing owns the selection.
fn empty_selection(stderr: &str) -> bool {
    stderr.contains("No selection")
}

#[async_trait]
impl Clipboard for WaylandDesktop {
    /// `-n` drops the trailing newline `wl-paste` adds, so what
    /// comes back is what was copied.
    async fn text(&self, selection: Selection) -> Result<String> {
        let mut args = argv(&["wl-paste", "-n"]);
        args.extend(argv(selection_flag(selection)));

        match self.run(args).await {
            Ok(result) => Ok(result.stdout_utf8()),
            Err(Error::Failed { stderr, .. }) if empty_selection(&stderr) => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    /// `wl-copy` forks and holds the selection itself, so unlike `xclip` this
    /// needs no detaching.
    ///
    /// The path is a positional argument rather than part of the command, so a
    /// space or a quotation mark in it cannot become shell syntax.
    async fn set_from(&self, selection: Selection, path: &str) -> Result<()> {
        let mut args = argv(&["bash", "-c", "wl-copy \"$@\" < \"$0\"", path, "--"]);
        args.extend(argv(selection_flag(selection)));
        self.act(args).await
    }

    /// The selection as one of the types its owner offers, returned raw.
    ///
    /// Raw, because a picture through a `String` loses every byte that is not
    /// valid UTF-8.
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

    /// `-l` lists the types, one per line — the same question `TARGETS` asks
    /// an X selection.
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

/// The size out of a PNG's header, as `od` prints those bytes.
///
/// Eight big-endian bytes: four of width, four of height.
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

/// The Wayland driver, for a box running the compositor image.
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
            "wtype calls the super key logo, and a caller should not have to"
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
    fn test_scrolling_up_is_a_negative_count_and_down_a_positive_one() {
        assert_eq!(
            scroll_argv(Point::new(5, 5), Delta::up(3)).last().cloned(),
            Some("-3".to_string())
        );
        assert_eq!(
            scroll_argv(Point::new(5, 5), Delta::down(3))
                .last()
                .cloned(),
            Some("3".to_string())
        );
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
        // 1280x800, as od prints those eight bytes.
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
