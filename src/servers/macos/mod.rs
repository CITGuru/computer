//! Driving a macOS desktop through Quartz.

mod profile;

pub use profile::MacProfile;

use crate::error::{Error, Result};
use crate::machine::{MachineHost, ScreenHost};
use crate::screens::ControlGate;
use crate::{
    Button, Clipboard, Delta, Desktop, DesktopFactory, DisplayServer, ExecResult, Point, ScreenId,
    Selection,
};
use async_trait::async_trait;
use std::sync::Arc;

/// The command the image installs for every input it accepts.
pub const INPUT_COMMAND: &str = "computer-input";

/// Capture to a file and hand the bytes back on stdout.
const CAPTURE: &str = "f=$(mktemp -t computer).png; trap 'rm -f \"$f\"' EXIT; \
                       screencapture -x -t png \"$f\" && cat \"$f\"";

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

/// What the helper calls a mouse button.
fn button_name(button: Button) -> &'static str {
    match button {
        Button::Left => "left",
        Button::Middle => "middle",
        Button::Right => "right",
    }
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
fn scroll_argv(at: Point, by: Delta) -> Vec<String> {
    let notches = by.dy.unsigned_abs().clamp(1, 20) as i32;
    let signed = if by.dy < 0 { -notches } else { notches };

    let mut parts = point_parts(at);
    parts.push(signed.to_string());
    input_argv("scroll", &parts)
}

/// The driver for a macOS box.
#[derive(Debug, Clone, Copy, Default)]
pub struct QuartzDriver;

impl DesktopFactory for QuartzDriver {
    fn display_server(&self) -> DisplayServer {
        DisplayServer::Quartz
    }

    fn open(&self, host: Arc<MachineHost>, screen: ScreenId) -> Arc<dyn Desktop> {
        Arc::new(QuartzDesktop::new(host as Arc<dyn ScreenHost>, screen))
    }
}

/// One screen, driven through synthetic Quartz events.
pub struct QuartzDesktop {
    host: Arc<dyn ScreenHost>,
    screen: ScreenId,
    control: Arc<ControlGate>,
}

impl QuartzDesktop {
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
    async fn act(&self, args: Vec<String>) -> Result<()> {
        self.control.may_act()?;
        self.run(args).await.map(|_| ())
    }
}

#[async_trait]
impl Desktop for QuartzDesktop {
    async fn screenshot(&self) -> Result<Vec<u8>> {
        let result = self.run(argv(&["sh", "-c", CAPTURE])).await?;

        if result.stdout.is_empty() {
            return Err(Error::denied("the screen capture returned no image"));
        }
        Ok(result.stdout)
    }

    async fn move_to(&self, at: Point) -> Result<()> {
        self.act(input_argv("move", &point_parts(at))).await
    }

    async fn click(&self, at: Point, button: Button) -> Result<()> {
        let mut parts = point_parts(at);
        parts.push(button_name(button).to_string());
        self.act(input_argv("click", &parts)).await
    }

    /// The helper's own click count, not two calls: macOS tells a double click
    /// from two single ones by a field on the event, and two round trips
    /// through the runtime arrive as two separate clicks whatever the timing.
    async fn double_click(&self, at: Point, button: Button) -> Result<()> {
        let mut parts = point_parts(at);
        parts.push(button_name(button).to_string());
        self.act(input_argv("double", &parts)).await
    }

    /// One command: a press held across separate calls is released when the
    /// first call's process exits.
    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()> {
        let mut parts = point_parts(from);
        parts.extend(point_parts(to));
        parts.push(button_name(button).to_string());
        self.act(input_argv("drag", &parts)).await
    }

    /// The characters themselves, not keystrokes for them.
    async fn type_text(&self, text: &str) -> Result<()> {
        self.act(input_argv("type", &[text.to_string()])).await
    }

    /// The chord is passed through whole.
    async fn key(&self, chord: &str) -> Result<()> {
        self.act(input_argv("key", &[chord.to_string()])).await
    }

    async fn scroll(&self, at: Point, by: Delta) -> Result<()> {
        self.act(scroll_argv(at, by)).await
    }

    async fn cursor(&self) -> Result<Point> {
        let result = self.run(input_argv("cursor", &[])).await?;

        // The same `X= Y=` the X11 driver reads, so there is one parser.
        crate::servers::x11::parse_cursor(&result.stdout_utf8())
            .ok_or_else(|| Error::denied("the cursor position could not be read"))
    }

    async fn geometry(&self) -> Result<(u32, u32)> {
        let result = self.run(input_argv("geometry", &[])).await?;
        let text = result.stdout_utf8();
        let mut parts = text.split_whitespace();

        match (
            parts.next().and_then(|width| width.parse().ok()),
            parts.next().and_then(|height| height.parse().ok()),
        ) {
            (Some(width), Some(height)) => Ok((width, height)),
            _ => Err(Error::denied("the screen geometry could not be read")),
        }
    }

    /// Whether there is a session with a screen in it.
    async fn alive(&self) -> Result<()> {
        self.run(input_argv("geometry", &[]))
            .await
            .map(|_| ())
            .map_err(|error| Error::Gone(format!("no macOS session with a screen: {error}")))
    }

    fn control(&self) -> &Arc<ControlGate> {
        &self.control
    }

    fn as_clipboard(&self) -> Option<&dyn Clipboard> {
        Some(self)
    }
}

/// Text only, through `pbpaste` and `pbcopy`.
#[async_trait]
impl Clipboard for QuartzDesktop {
    async fn text(&self, selection: Selection) -> Result<String> {
        primary_is_not_a_thing(selection)?;

        let result = self.run(argv(&["pbpaste"])).await?;
        Ok(result.stdout_utf8())
    }

    async fn set_from(&self, selection: Selection, path: &str) -> Result<()> {
        primary_is_not_a_thing(selection)?;

        // The pasteboard is a server, so nothing has to stay alive to own the
        // selection the way an X client does.
        self.run(argv(&["sh", "-c", &format!("pbcopy < '{path}'")]))
            .await
            .map(|_| ())
    }

    async fn bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>> {
        primary_is_not_a_thing(selection)?;

        if !is_text(target) {
            return Err(Error::Unsupported {
                gaps: vec!["a pasteboard type other than text"],
            });
        }
        self.text(selection).await.map(String::into_bytes)
    }

    async fn set_bytes_from(&self, selection: Selection, target: &str, path: &str) -> Result<()> {
        primary_is_not_a_thing(selection)?;

        if !is_text(target) {
            return Err(Error::Unsupported {
                gaps: vec!["a pasteboard type other than text"],
            });
        }
        self.set_from(selection, path).await
    }

    /// What `pbpaste` can hand back, which is text and nothing else.
    async fn targets(&self, selection: Selection) -> Result<Vec<String>> {
        primary_is_not_a_thing(selection)?;
        Ok(vec!["text/plain".to_string()])
    }
}

fn is_text(target: &str) -> bool {
    target.starts_with("text/") || target == "UTF8_STRING" || target == "STRING"
}

fn primary_is_not_a_thing(selection: Selection) -> Result<()> {
    match selection {
        Selection::Clipboard => Ok(()),
        Selection::Primary => Err(Error::Unsupported {
            gaps: vec!["the primary selection, which macOS does not have"],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ScriptedHost;

    fn driver(host: Arc<ScriptedHost>) -> QuartzDesktop {
        QuartzDesktop::new(host as Arc<dyn ScreenHost>, ScreenId(0))
    }

    #[tokio::test]
    async fn test_a_capture_cleans_up_after_itself_however_it_ends() {
        let host = Arc::new(ScriptedHost::new().replying(ExecResult {
            stdout: vec![0x89, b'P', b'N', b'G'],
            ..ExecResult::default()
        }));
        driver(Arc::clone(&host))
            .screenshot()
            .await
            .expect("a capture");

        let sent = host.last().expect("a call");
        assert!(
            sent[2].contains("trap 'rm -f"),
            "screencapture has no stdout form, so a refused capture would \
             otherwise leave the file behind: {}",
            sent[2]
        );
        assert!(sent[2].contains("screencapture -x -t png"));
    }

    #[tokio::test]
    async fn test_a_double_click_is_one_command_and_not_two_clicks() {
        let host = Arc::new(ScriptedHost::new());
        driver(Arc::clone(&host))
            .double_click(Point::new(10, 20), Button::Left)
            .await
            .expect("a double click");

        assert_eq!(
            host.last().expect("a call"),
            ["computer-input", "double", "10", "20", "left"],
            "macOS reads the click count off the event, so two round trips \
             arrive as two single clicks whatever the timing"
        );
        assert_eq!(host.count(), 1);
    }

    #[tokio::test]
    async fn test_a_drag_is_one_command_so_the_button_stays_down() {
        let host = Arc::new(ScriptedHost::new());
        driver(Arc::clone(&host))
            .drag(Point::new(1, 2), Point::new(3, 4), Button::Left)
            .await
            .expect("a drag");

        assert_eq!(
            host.last().expect("a call"),
            ["computer-input", "drag", "1", "2", "3", "4", "left"]
        );
        assert_eq!(
            host.count(),
            1,
            "a press held across two calls is released when the first one's \
             process exits"
        );
    }

    #[tokio::test]
    async fn test_text_is_sent_whole_rather_than_as_keystrokes() {
        let host = Arc::new(ScriptedHost::new());
        driver(Arc::clone(&host))
            .type_text("naïve — ✓")
            .await
            .expect("typing");

        assert_eq!(
            host.last().expect("a call"),
            ["computer-input", "type", "naïve — ✓"],
            "the helper posts a Unicode string; typing it as keycodes is what \
             mangles non-ASCII"
        );
    }

    #[tokio::test]
    async fn test_a_chord_is_not_translated_twice() {
        let host = Arc::new(ScriptedHost::new());
        driver(Arc::clone(&host))
            .key("cmd+shift+p")
            .await
            .expect("a chord");

        assert_eq!(
            host.last().expect("a call"),
            ["computer-input", "key", "cmd+shift+p"],
            "the helper owns the key table, so a second one here could only \
             disagree with it"
        );
    }

    #[test]
    fn test_a_scroll_carries_its_direction_and_is_bounded() {
        let down = scroll_argv(Point::new(5, 6), Delta::down(3));
        let up = scroll_argv(Point::new(5, 6), Delta::up(3));

        assert_eq!(down, ["computer-input", "scroll", "5", "6", "3"]);
        assert_eq!(up, ["computer-input", "scroll", "5", "6", "-3"]);
        assert_eq!(
            scroll_argv(Point::new(0, 0), Delta::down(500)).pop(),
            Some("20".to_string()),
            "a runaway delta is a scroll that never ends"
        );
    }

    #[tokio::test]
    async fn test_the_primary_selection_is_refused_and_not_faked() {
        let host = Arc::new(ScriptedHost::new().saying("something"));
        let screen = driver(Arc::clone(&host));
        let clipboard = screen.as_clipboard().expect("a clipboard");

        assert!(clipboard.text(Selection::Clipboard).await.is_ok());
        assert!(
            clipboard.text(Selection::Primary).await.is_err(),
            "macOS has no primary selection, and handing back the clipboard \
             for it is the wrong text rather than no text"
        );
    }

    #[tokio::test]
    async fn test_a_screen_with_no_session_is_gone_rather_than_broken() {
        let host = Arc::new(ScriptedHost::new().failing(1, "no display"));

        let error = driver(host).alive().await.expect_err("no session");
        assert!(
            matches!(error, Error::Gone(_)),
            "a guest at the login window has no window server, and every \
             command against it succeeds while moving nothing: {error}"
        );
    }
}
