//! What a box can show, and be driven through.
//!
//! [`DesktopSupport`] is what an image claims, [`DesktopPresence`] is what
//! answers right now, and [`DesktopNeed`] is what a caller requires. `browser`
//! sits beside `display` because a browser needs no framebuffer, and a
//! framebuffer needs an X server and a viewer.
//!
//! [`Desktop`] is everything a screen is driven through, and [`Clipboard`] is
//! the part a box may not have.

use crate::ScreenId;
use crate::error::{Error, Result};
use crate::machine::MachineHost;
use crate::screens::ControlGate;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayServer {
    X11,
    Wayland,
    Quartz,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerKind {
    Vnc,
    Cdp,
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    pub server: DisplayServer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Browser {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    /// A DevTools endpoint, so it is drivable with no desktop at all.
    #[serde(default)]
    pub cdp: bool,
    /// A real window on the display, not headless only.
    #[serde(default)]
    pub headed: bool,
}

/// Who is driving the input right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Control {
    Owner,
    Human { since: SystemTime },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewer {
    pub kind: ViewerKind,
    /// A person can take the input, not merely watch.
    #[serde(default)]
    pub takeover: bool,
    pub control: Control,
}

/// What it can show. Stable — cache it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DesktopSupport {
    pub display: Option<Display>,
    pub input: bool,
    pub browser: Option<Browser>,
    pub clipboard: bool,
    pub viewer: Option<Viewer>,
    /// Screens it can run at once. Zero where it has no desktop at all.
    pub max_screens: u32,
}

/// What it can serve right now.
///
/// A configured `DISPLAY` is not a running one: an X server that has exited
/// leaves the variable set.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DesktopPresence {
    pub display: bool,
    pub browser: bool,
    pub detail: Option<String>,
}

impl DesktopPresence {
    /// Both halves up. The screen stays blank until the browser has a window
    /// on it.
    pub fn ready(&self) -> bool {
        self.display && self.browser
    }
}

/// Who is looking at a screen, and who is on the input.
///
/// Counted from live connections. Both servers keep listening whether or not
/// anyone is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Viewers {
    pub watching: usize,
    pub driving: usize,
}

impl Viewers {
    /// Whether a person is on the input right now.
    pub fn person_present(&self) -> bool {
        self.driving > 0
    }

    /// `watching=0 driving=1`, as the image reports it.
    pub fn parse(output: &str) -> Option<Self> {
        let mut viewers = Self::default();
        let mut seen = 0;

        for field in output.split_whitespace() {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            let Ok(count) = value.parse() else { continue };
            match key {
                "watching" => {
                    viewers.watching = count;
                    seen += 1;
                }
                "driving" => {
                    viewers.driving = count;
                    seen += 1;
                }
                _ => {}
            }
        }

        (seen == 2).then_some(viewers)
    }
}

/// What the work needs, checked against what a box offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DesktopNeed {
    pub display: bool,
    pub input: bool,
    pub browser: bool,
    pub min_size: Option<(u32, u32)>,
}

impl DesktopNeed {
    /// Just a browser — no framebuffer, no window manager, any container.
    pub fn browser() -> Self {
        Self {
            browser: true,
            ..Self::default()
        }
    }

    /// A screen the caller can see and drive.
    pub fn desktop() -> Self {
        Self {
            display: true,
            input: true,
            ..Self::default()
        }
    }

    pub fn at_least(mut self, width: u32, height: u32) -> Self {
        self.min_size = Some((width, height));
        self
    }

    /// Which parts of this need `support` cannot meet. Empty means placeable.
    pub fn unsupported_by(&self, support: &DesktopSupport) -> Vec<&'static str> {
        let mut gaps = Vec::new();

        if self.display && support.display.is_none() {
            gaps.push("display");
        }
        if self.input && !support.input {
            gaps.push("input");
        }
        if self.browser && support.browser.is_none() {
            gaps.push("browser");
        }
        if let Some((width, height)) = self.min_size {
            match support.display {
                Some(display) if display.width >= width && display.height >= height => {}
                // A display too small is reported as the size, not as a missing
                // display: the caller asked for a screen and there is one.
                Some(_) => gaps.push("display size"),
                None if !self.display => gaps.push("display"),
                None => {}
            }
        }

        gaps
    }

    /// The refusal a caller gets before anything is started.
    pub fn check(&self, support: &DesktopSupport) -> Result<()> {
        let gaps = self.unsupported_by(support);
        if gaps.is_empty() {
            Ok(())
        } else {
            Err(Error::Unsupported { gaps })
        }
    }
}

/// A DevTools endpoint reachable from the caller, not merely from inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserEndpoint {
    pub http_url: String,
    pub ws_url: String,
}

/// Top-left origin, device pixels, and the same coordinates the screenshot came
/// back in — a click against a scaled frame lands somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

impl Point {
    pub const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }
}

impl From<(u32, u32)> for Point {
    fn from((x, y): (u32, u32)) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

/// A wheel movement. Positive `dy` scrolls down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    pub dx: i32,
    pub dy: i32,
}

impl Delta {
    pub const fn down(notches: i32) -> Self {
        Self {
            dx: 0,
            dy: notches.abs(),
        }
    }

    pub const fn up(notches: i32) -> Self {
        Self {
            dx: 0,
            dy: -notches.abs(),
        }
    }
}

/// Which X selection is meant.
///
/// Copy and paste uses `CLIPBOARD`. Dragging the mouse over text fills
/// `PRIMARY`, which a middle click pastes. They hold different text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    /// What copy and paste uses, and what a caller means when it says nothing.
    #[default]
    Clipboard,
    /// What selecting text with the mouse fills, and a middle click pastes.
    Primary,
}

impl Selection {
    /// The name `xclip` takes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::Primary => "primary",
        }
    }
}

/// Reading and writing the selection a paste comes from.
///
/// Its own trait because [`DesktopSupport::clipboard`] calls the clipboard
/// optional. `None` from [`Desktop::as_clipboard`] is something a caller can
/// check; a required method that only ever fails is not.
#[async_trait]
pub trait Clipboard: Send + Sync {
    /// What is on one selection, or an empty string where nothing is.
    ///
    /// A selection nobody has written to is empty rather than an error.
    async fn text(&self, selection: Selection) -> Result<String>;

    /// Own a selection, serving what is in a file already inside the box.
    ///
    /// A path rather than the text, because the content is arbitrary and a
    /// command line is no place for a document. Staging the file is the
    /// caller's job: only the caller can reach the box.
    async fn set_from(&self, selection: Selection, path: &str) -> Result<()>;

    /// The selection as one of the types it is offered in.
    ///
    /// A page that copies a picture offers `image/png` beside the text of its
    /// `alt` attribute, and asking for the wrong one gets the wrong thing.
    async fn bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>>;

    /// Own the selection, offering it as this type.
    async fn set_bytes_from(&self, selection: Selection, target: &str, path: &str) -> Result<()>;

    /// The types the selection can be read as, as its owner advertises them.
    async fn targets(&self, selection: Selection) -> Result<Vec<String>>;
}

/// A screen that can be looked at and driven.
///
/// Everything a screen is driven through is here, so a second display
/// server is another implementation rather than an edit to
/// [`crate::Screen`].
#[async_trait]
pub trait Desktop: Send + Sync {
    /// PNG bytes of the whole screen.
    async fn screenshot(&self) -> Result<Vec<u8>>;

    /// Move the pointer without pressing anything.
    ///
    /// Hovering is its own action: a menu highlights, a tooltip appears, a
    /// control reveals itself.
    async fn move_to(&self, at: Point) -> Result<()>;

    async fn click(&self, at: Point, button: Button) -> Result<()>;

    /// Press twice close enough together to count as one gesture.
    ///
    /// No default implementation. Two `click` calls are two round trips to the
    /// box, far enough apart that the application sees two single clicks.
    async fn double_click(&self, at: Point, button: Button) -> Result<()>;

    /// Press at one point, move, release at another.
    ///
    /// Required rather than defaulted: a press held across separate calls may
    /// be released when the first call's process exits.
    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()>;

    async fn type_text(&self, text: &str) -> Result<()>;
    async fn key(&self, chord: &str) -> Result<()>;
    async fn scroll(&self, at: Point, by: Delta) -> Result<()>;

    /// Where the pointer is.
    ///
    /// A root-window capture does not include the cursor, so no screenshot
    /// shows this. A display server that cannot read the pointer position
    /// answers [`Error::Unsupported`] rather than guessing.
    async fn cursor(&self) -> Result<Point>;

    /// The screen's own idea of its size.
    ///
    /// Read from the display rather than from what the box was asked for.
    /// Coordinates are against the screen that came up.
    async fn geometry(&self) -> Result<(u32, u32)>;

    /// Whether the screen answers now.
    ///
    /// `Ok` means it does, and the error says why not. A configured display is
    /// not a running one: a server that has exited leaves its variable set.
    async fn alive(&self) -> Result<()>;

    /// Whether the owner may act, and where a takeover is recorded.
    ///
    /// On the trait because the takeover rule belongs to this crate rather
    /// than to any one display server. A driver holding its own gate would
    /// send input into a session a person is already driving.
    fn control(&self) -> &Arc<ControlGate>;

    /// The clipboard, where the box has one.
    ///
    /// `None` and [`DesktopSupport::clipboard`] have to agree.
    fn as_clipboard(&self) -> Option<&dyn Clipboard> {
        None
    }
}

/// Which display server a box is driven through.
///
/// [`Desktop`] is what a screen can do; this is who does it. A box picks one
/// and opens every screen with it.
pub trait DesktopFactory: Send + Sync {
    /// What this drives, so [`Display::server`] reports the driver in use
    /// rather than the one the image constants were written for.
    fn display_server(&self) -> DisplayServer;

    /// A driver for one screen.
    ///
    /// Takes the host rather than the box's name: the whole coupling is
    /// something that runs a command against one screen.
    fn open(&self, host: Arc<MachineHost>, screen: ScreenId) -> Arc<dyn Desktop>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headless_browser() -> DesktopSupport {
        DesktopSupport {
            browser: Some(Browser {
                name: "chromium".to_string(),
                version: None,
                cdp: true,
                headed: false,
            }),
            ..DesktopSupport::default()
        }
    }

    fn full_desktop() -> DesktopSupport {
        DesktopSupport {
            display: Some(Display {
                width: 1280,
                height: 800,
                server: DisplayServer::X11,
            }),
            input: true,
            max_screens: 8,
            ..headless_browser()
        }
    }

    #[test]
    fn test_a_browser_need_is_met_without_a_desktop() {
        assert!(
            DesktopNeed::browser()
                .unsupported_by(&headless_browser())
                .is_empty()
        );
    }

    #[test]
    fn test_a_desktop_need_is_not_met_by_a_browser() {
        let gaps = DesktopNeed::desktop().unsupported_by(&headless_browser());
        assert_eq!(gaps, vec!["display", "input"]);
    }

    #[test]
    fn test_a_full_desktop_meets_both() {
        assert!(
            DesktopNeed::desktop()
                .unsupported_by(&full_desktop())
                .is_empty()
        );
        assert!(
            DesktopNeed::browser()
                .unsupported_by(&full_desktop())
                .is_empty()
        );
    }

    #[test]
    fn test_a_screen_too_small_is_reported_as_the_size() {
        let need = DesktopNeed::desktop().at_least(1920, 1080);
        assert_eq!(need.unsupported_by(&full_desktop()), vec!["display size"]);
    }

    #[test]
    fn test_nothing_needed_is_met_by_nothing_offered() {
        assert!(
            DesktopNeed::default()
                .unsupported_by(&DesktopSupport::default())
                .is_empty()
        );
    }

    #[test]
    fn test_a_size_alone_still_needs_a_screen_to_measure() {
        let need = DesktopNeed {
            min_size: Some((800, 600)),
            ..DesktopNeed::default()
        };
        assert_eq!(
            need.unsupported_by(&DesktopSupport::default()),
            vec!["display"]
        );
    }

    #[test]
    fn test_a_check_refuses_with_the_gaps_named() {
        let error = DesktopNeed::desktop()
            .check(&DesktopSupport::default())
            .expect_err("nothing is offered");
        assert!(error.to_string().contains("display"));
    }

    #[test]
    fn test_the_two_selections_are_not_the_same_one() {
        assert_eq!(Selection::Clipboard.name(), "clipboard");
        assert_eq!(Selection::Primary.name(), "primary");
        assert_eq!(
            Selection::default(),
            Selection::Clipboard,
            "copy and paste is what a caller means when it does not say"
        );
    }

    #[test]
    fn test_viewers_are_read_from_the_image_report() {
        let viewers = Viewers::parse("watching=2 driving=1\n").expect("both counts");
        assert_eq!((viewers.watching, viewers.driving), (2, 1));
        assert!(viewers.person_present());
    }

    #[test]
    fn test_half_a_report_is_none_rather_than_a_zero() {
        assert_eq!(
            Viewers::parse("watching=2"),
            None,
            "a missing count read as zero says nobody is driving, which is the \
             one answer that lets the owner act over a person"
        );
        assert_eq!(Viewers::parse(""), None);
    }

    #[test]
    fn test_a_display_alone_is_not_ready() {
        let half_up = DesktopPresence {
            display: true,
            browser: false,
            detail: None,
        };
        assert!(
            !half_up.ready(),
            "an X server with no window on it is a blank screen"
        );
    }

    #[test]
    fn test_a_scroll_direction_reads_the_way_a_page_moves() {
        assert_eq!(Delta::down(3).dy, 3);
        assert_eq!(Delta::up(3).dy, -3);
        assert_eq!(Delta::up(-3).dy, -3, "a sign mistake must not reverse it");
    }
}
