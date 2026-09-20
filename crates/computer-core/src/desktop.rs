use crate::ScreenId;
use crate::error::{Error, Result};
use crate::machine::MachineHost;
use crate::screens::ControlGate;
use async_trait::async_trait;
pub use computer_types::{Button, Held, Keys, Node, NodeQuery, Point, Rect, Selection};

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub struct Press<'a> {
    on: &'a dyn Desktop,
    chords: Vec<String>,
    held: Vec<Held>,
}

impl<'a> Press<'a> {
    pub(crate) fn new(on: &'a dyn Desktop, keys: impl Keys) -> Self {
        Self {
            on,
            chords: keys.chords(),
            held: Vec::new(),
        }
    }

    pub fn holding(mut self, held: impl AsRef<[Held]>) -> Self {
        self.held = held.as_ref().to_vec();
        self
    }
}

impl<'a> std::future::IntoFuture for Press<'a> {
    type Output = Result<()>;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.on.press(&self.chords, &self.held).await })
    }
}

pub struct Typing<'a> {
    on: &'a dyn Desktop,
    text: String,
    delay: Option<Duration>,
}

impl<'a> Typing<'a> {
    pub(crate) fn new(on: &'a dyn Desktop, text: impl Into<String>) -> Self {
        Self {
            on,
            text: text.into(),
            delay: None,
        }
    }

    pub fn every(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
}

impl<'a> std::future::IntoFuture for Typing<'a> {
    type Output = Result<()>;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.on.type_text(&self.text, self.delay).await })
    }
}

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
    #[serde(default)]
    pub cdp: bool,
    #[serde(default)]
    pub headed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Control {
    Owner,
    Human { since: SystemTime },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewer {
    pub kind: ViewerKind,
    #[serde(default)]
    pub takeover: bool,
    pub control: Control,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DesktopSupport {
    pub display: Option<Display>,
    pub input: bool,
    pub browser: Option<Browser>,
    pub clipboard: bool,
    pub viewer: Option<Viewer>,
    pub max_screens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DesktopPresence {
    pub display: bool,
    pub browser: bool,
    pub detail: Option<String>,
}

impl DesktopPresence {
    pub fn ready(&self) -> bool {
        self.display && self.browser
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Viewers {
    pub watching: usize,
    pub driving: usize,
}

impl Viewers {
    pub fn person_present(&self) -> bool {
        self.driving > 0
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DesktopNeed {
    pub display: bool,
    pub input: bool,
    pub browser: bool,
    pub min_size: Option<(u32, u32)>,
}

impl DesktopNeed {
    pub fn browser() -> Self {
        Self {
            browser: true,
            ..Self::default()
        }
    }

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
                Some(_) => gaps.push("display size"),
                None if !self.display => gaps.push("display"),
                None => {}
            }
        }

        gaps
    }

    pub fn check(&self, support: &DesktopSupport) -> Result<()> {
        let gaps = self.unsupported_by(support);
        if gaps.is_empty() {
            Ok(())
        } else {
            Err(Error::Unsupported { gaps })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserEndpoint {
    pub http_url: String,
    pub ws_url: String,
}

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

    pub const fn right(notches: i32) -> Self {
        Self {
            dx: notches.abs(),
            dy: 0,
        }
    }

    pub const fn left(notches: i32) -> Self {
        Self {
            dx: -notches.abs(),
            dy: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shot {
    pub of: Of,
    /// Percent of full size.
    pub scale: Option<u32>,
    pub pointer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Of {
    #[default]
    Screen,
    Window(String),
    Region(Rect),
}

impl Shot {
    pub fn of(of: Of) -> Self {
        Self {
            of,
            scale: None,
            pointer: false,
        }
    }

    pub fn window(id: impl Into<String>) -> Self {
        Self::of(Of::Window(id.into()))
    }

    pub fn region(area: Rect) -> Self {
        Self::of(Of::Region(area))
    }

    pub fn scaled(mut self, percent: u32) -> Self {
        self.scale = Some(percent);
        self
    }
}

#[async_trait]
pub trait Clipboard: Send + Sync {
    /// Empty rather than an error where nothing was written.
    async fn text(&self, selection: Selection) -> Result<String>;

    /// A path inside the box: arbitrary content does not fit on a command line.
    async fn set_from(&self, selection: Selection, path: &str) -> Result<()>;

    async fn bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>>;

    async fn set_bytes_from(&self, selection: Selection, target: &str, path: &str) -> Result<()>;

    async fn targets(&self, selection: Selection) -> Result<Vec<String>>;
}

#[async_trait]
pub trait Desktop: Send + Sync {
    /// PNG.
    async fn screenshot(&self) -> Result<Vec<u8>>;

    /// A driver that cannot crop or scale must refuse, not return the whole screen.
    async fn capture(&self, area: Option<Rect>, scale: Option<u32>) -> Result<Vec<u8>> {
        match (area, scale) {
            (None, None) => self.screenshot().await,
            _ => Err(Error::Unsupported {
                gaps: vec!["capture"],
            }),
        }
    }

    /// A driver that cannot draw the pointer must refuse rather than leave it out.
    async fn capture_pointing(&self, area: Option<Rect>, scale: Option<u32>) -> Result<Vec<u8>> {
        let _ = (area, scale);
        Err(Error::Unsupported {
            gaps: vec!["the pointer in a capture"],
        })
    }

    async fn move_to(&self, at: Point) -> Result<()>;

    async fn click(&self, at: Point, button: Button) -> Result<()>;

    async fn click_with(&self, at: Point, button: Button, held: &[Held]) -> Result<()> {
        match held.is_empty() {
            true => self.click(at, button).await,
            false => Err(Error::Unsupported {
                gaps: vec!["modifiers"],
            }),
        }
    }

    /// Not defaulted: two `click` calls arrive as two single clicks.
    async fn double_click(&self, at: Point, button: Button) -> Result<()>;

    /// Not defaulted: a press held across calls may be released between them.
    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()>;

    async fn drag_with(&self, from: Point, to: Point, button: Button, held: &[Held]) -> Result<()> {
        match held.is_empty() {
            true => self.drag(from, to, button).await,
            false => Err(Error::Unsupported {
                gaps: vec!["modifiers"],
            }),
        }
    }

    /// A driver that spawns a process per move should send the whole path as
    /// one, or the pauses are the spawns.
    async fn move_along(&self, steps: &[crate::motion::Step]) -> Result<()> {
        for step in steps {
            self.move_to(step.at).await?;
            tokio::time::sleep(step.pause).await;
        }
        Ok(())
    }

    /// Not defaulted: a press held across calls may be released between them.
    async fn drag_along(
        &self,
        from: Point,
        steps: &[crate::motion::Step],
        button: Button,
        held: &[Held],
    ) -> Result<()> {
        let _ = (from, steps, button, held);
        Err(Error::Unsupported {
            gaps: vec!["a drag along a path"],
        })
    }

    async fn button_down(&self, at: Option<Point>, button: Button) -> Result<()> {
        let _ = (at, button);
        Err(Error::Unsupported {
            gaps: vec!["a button held across steps"],
        })
    }

    async fn button_up(&self, at: Option<Point>, button: Button) -> Result<()> {
        let _ = (at, button);
        Err(Error::Unsupported {
            gaps: vec!["a button held across steps"],
        })
    }

    async fn let_go(&self, button: Button) -> Result<()> {
        let _ = button;
        Ok(())
    }

    /// A desktop that cannot pace keystrokes must refuse a `delay`.
    async fn type_text(&self, text: &str, delay: Option<Duration>) -> Result<()>;

    /// A desktop that cannot hold a modifier must refuse a non-empty `held`.
    async fn press(&self, chords: &[String], held: &[Held]) -> Result<()>;
    async fn scroll(&self, at: Point, by: Delta) -> Result<()>;

    async fn wait_until_still(&self, settle: Duration, within: Duration) -> Result<()> {
        let _ = (settle, within);

        Err(Error::Unsupported {
            gaps: vec!["waiting"],
        })
    }

    async fn nodes(&self, app: Option<&str>, depth: Option<u32>) -> Result<Vec<Node>> {
        let _ = (app, depth);

        Err(Error::Unsupported {
            gaps: vec!["the accessibility tree"],
        })
    }

    /// Also matches a label beside the widget: a GTK entry's own name is empty.
    async fn find_nodes(&self, query: &NodeQuery, limit: Option<usize>) -> Result<Vec<Node>> {
        let _ = (query, limit);

        Err(Error::Unsupported {
            gaps: vec!["the accessibility tree"],
        })
    }

    async fn focus_node(&self, query: &NodeQuery) -> Result<Node> {
        let _ = query;

        Err(Error::Unsupported {
            gaps: vec!["the accessibility tree"],
        })
    }

    /// The widget's own action: no pointer moves, so a covered widget is reachable.
    async fn invoke_node(&self, query: &NodeQuery, action: Option<&str>) -> Result<Node> {
        let _ = (query, action);

        Err(Error::Unsupported {
            gaps: vec!["the accessibility tree"],
        })
    }

    async fn set_node(&self, query: &NodeQuery, value: &str) -> Result<Node> {
        let _ = (query, value);

        Err(Error::Unsupported {
            gaps: vec!["the accessibility tree"],
        })
    }

    /// Never moves the pointer.
    async fn cursor(&self) -> Result<Point>;

    /// May move the pointer where that is the only way to read it.
    async fn find_cursor(&self) -> Result<Point> {
        self.cursor().await
    }

    /// Read from the display, not from what the box was asked for.
    async fn geometry(&self) -> Result<(u32, u32)>;

    async fn alive(&self) -> Result<()>;

    fn control(&self) -> &Arc<ControlGate>;

    /// Must agree with [`DesktopSupport::clipboard`].
    fn as_clipboard(&self) -> Option<&dyn Clipboard> {
        None
    }
}

pub trait DesktopFactory: Send + Sync {
    fn display_server(&self) -> DisplayServer;

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
