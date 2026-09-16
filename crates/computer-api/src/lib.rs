pub use computer_types::{
    App, Arrange, Auth, Bind, Button, Desktop, DisplayServer, Feature, Held, Node, NodeQuery,
    Placement, Point, Policy, Rect, Selection, Spec, Window,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBox {
    #[serde(default)]
    pub spec: Spec,
    #[serde(default)]
    pub placement: Placement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxView {
    pub id: String,
    pub spec_digest: String,
    pub state: BoxState,
    pub screens: u32,
    pub width: u32,
    pub height: u32,
    pub viewer_url: Option<String>,
    pub devtools_url: Option<String>,
    pub created_at_ms: u64,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxState {
    Ready,
    /// Calls that reach into a paused box hang rather than fail.
    Paused,
    /// Starting it again gives a fresh desktop on new ports.
    Stopped,
    Gone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub service: String,
}

pub const SERVICE: &str = "computer-server";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxList {
    pub boxes: Vec<BoxView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Move {
        to: Point,
    },
    Click {
        #[serde(default)]
        at: Option<Point>,
        #[serde(default)]
        button: Button,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        held: Vec<Held>,
    },
    DoubleClick {
        #[serde(default)]
        at: Option<Point>,
        #[serde(default)]
        button: Button,
    },
    Drag {
        from: Point,
        to: Point,
        #[serde(default)]
        button: Button,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        held: Vec<Held>,
    },
    Type {
        text: String,
        /// Milliseconds between keystrokes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_ms: Option<u64>,
    },
    Press {
        chord: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        then: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        held: Vec<Held>,
    },
    /// In notches: positive `dy` down, positive `dx` right.
    Scroll {
        at: Point,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    OpenUrl {
        #[serde(default)]
        target: OpenIn,
        url: String,
    },
    Wait {
        ms: u64,
    },
    WaitStill {
        #[serde(default)]
        settle_ms: Option<u64>,
        #[serde(default)]
        within_ms: Option<u64>,
    },
    /// Nested because `deny_unknown_fields` does not work with `flatten`.
    OnPage {
        what: OnElement,
    },
    OnNode {
        what: OnNode,
    },
    /// `app` is a catalog name: an argv here would be `exec` in disguise.
    Launch {
        app: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum OnNode {
    Tree {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
    },
    Find {
        node: NodeQuery,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    Focus {
        node: NodeQuery,
    },
    Invoke {
        node: NodeQuery,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
    },
    Set {
        node: NodeQuery,
        value: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeResult {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<Node>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageText {
    pub url: String,
    pub title: String,
    pub text: String,
    pub truncated: bool,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub text: String,
    pub href: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    pub text: String,
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Point>,
    pub width: u32,
    pub height: u32,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum OnElement {
    Click {
        query: String,
        #[serde(default)]
        button: Button,
        #[serde(default)]
        double: bool,
    },
    Fill {
        query: String,
        text: String,
    },
    Options {
        query: String,
    },
    Choose {
        query: String,
        option: String,
    },
    Upload {
        query: String,
        paths: Vec<String>,
    },
    WaitFor {
        query: String,
        #[serde(default)]
        gone: bool,
        #[serde(default)]
        within_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        or: Vec<String>,
        #[serde(default)]
        exact: bool,
    },
    Hover {
        query: String,
    },
    History {
        go: Where,
    },
    Scroll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        #[serde(default)]
        to: ScrollTo,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Where {
    Back,
    Forward,
    Reload,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollTo {
    #[default]
    By,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ElementResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<Element>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub navigated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Point>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reading {
    #[default]
    Markdown,
    Text,
    Raw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwaitWindow {
    pub class: String,
    #[serde(default)]
    pub within_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Want {
    Frame,
    Cursor,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionBatch {
    pub actions: Vec<Action>,
    #[serde(default)]
    pub settle_ms: Option<u64>,
    #[serde(default)]
    pub want: Vec<Want>,
    #[serde(default)]
    pub have_frame: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    pub results: Vec<ActionResult>,
    pub stopped_at: Option<usize>,
    pub frame: Option<Frame>,
    pub cursor: Option<Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<Window>,
    /// Empty on a box that publishes no DevTools port.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenIn {
    #[default]
    Blank,
    Current,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Find {
    pub query: String,
    pub limit: Option<usize>,
    pub scroll: Option<bool>,
    pub exact: Option<bool>,
    pub role: Option<String>,
    pub tab: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evaluate {
    pub expression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evaluated {
    pub json: String,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub url: String,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub index: usize,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Ignored when a window is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Rect>,
    /// A percentage of full size, 1 to 400.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pointer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
}

impl Shot {
    pub fn is_whole(&self) -> bool {
        self.window.is_none() && self.region.is_none() && self.scale.is_none() && !self.pointer
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageShot {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Picture>,
    /// JPEG only, 1 to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Picture {
    #[default]
    Png,
    Jpeg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Captured {
    pub format: Picture,
    pub bytes: usize,
    pub image_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub hash: String,
    pub unchanged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub png_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    Gone,
    Denied,
    ScreenUnavailable,
    Unsupported,
    Failed,
    Timeout,
    Unavailable,
    Transport,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResponse {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteFile {
    pub path: String,
    pub contents_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFile {
    pub path: String,
    pub contents_base64: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeoverRequest {
    #[serde(default)]
    pub shared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakeoverView {
    pub url: Option<String>,
    pub exclusive: bool,
    pub screen: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewersView {
    pub watching: usize,
    pub driving: usize,
    pub person_driving: bool,
    /// The screen was handed to a person and not yet reclaimed, whether or not they are connected.
    #[serde(default)]
    pub taken_over: bool,
}

/// Opens the viewer socket of one screen for a while, from a browser that can send no header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerTicket {
    pub ticket: String,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingView {
    pub recording: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRecording {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardView {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetClipboard {
    pub text: String,
    #[serde(default)]
    pub selection: Selection,
}

/// `Person` marks custody, never input: a person's keystrokes go over VNC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Agent,
    Person,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceEvent {
    /// Boxed: a trace holds thousands of entries.
    BoxCreated {
        spec_digest: String,
        spec: Box<Spec>,
        placement: Box<Placement>,
        width: u32,
        height: u32,
        screens: u32,
    },
    Gone {
        why: String,
    },
    Adopted {
        runtime: String,
    },
    ForkedFrom {
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        up_to: Option<u64>,
    },
    Acted {
        screen: u32,
        action: Action,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorBody>,
    },
    /// The actor is whoever held the screen, not whoever changed it.
    Frame {
        screen: u32,
    },
    Executed {
        argv: Vec<String>,
        code: i32,
        timed_out: bool,
    },
    AppLaunched {
        screen: u32,
        app: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        window: String,
    },
    FileWritten {
        path: String,
        bytes: usize,
    },
    FileRead {
        path: String,
        bytes: usize,
    },
    BoxPaused,
    BoxResumed,
    BoxStopped,
    BoxStarted,
    PageCaptured {
        full: bool,
        bytes: usize,
    },
    ClipboardSet {
        screen: u32,
        selection: Selection,
    },
    ClipboardRead {
        screen: u32,
        selection: Selection,
    },
    TakeoverStarted {
        screen: u32,
        exclusive: bool,
    },
    TakeoverEnded {
        screen: u32,
    },
    BoxDeleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub seq: u64,
    pub at_ms: u64,
    pub actor: Actor,
    pub event: TraceEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceView {
    pub entries: Vec<TraceEntry>,
    pub next: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkRequest {
    #[serde(default)]
    pub mode: ForkMode,
    #[serde(default)]
    pub up_to: Option<u64>,
    #[serde(default)]
    pub placement: Option<Placement>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkMode {
    #[default]
    Replay,
    Snapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkResult {
    #[serde(rename = "box")]
    pub created: BoxView,
    pub replay: ReplayReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayReport {
    pub attempted: usize,
    pub ok: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<u64>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skipped {
    pub seq: u64,
    pub kind: String,
    pub why: String,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::NotFound => "not_found",
            Self::Gone => "gone",
            Self::Denied => "denied",
            Self::ScreenUnavailable => "screen_unavailable",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::Transport => "transport",
            Self::Internal => "internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_pointer_is_not_the_plain_whole_screen() {
        assert!(Shot::default().is_whole());
        assert!(
            !Shot {
                pointer: true,
                ..Shot::default()
            }
            .is_whole(),
            "the plain path cannot draw one, so a pointer shot must not take it"
        );
    }

    #[test]
    fn test_a_wait_without_an_alternative_is_still_a_wait() {
        let sent: OnElement =
            serde_json::from_str(r#"{"op":"wait_for","query":"Public rate"}"#).expect("parses");

        assert!(
            matches!(sent, OnElement::WaitFor { ref or, .. } if or.is_empty()),
            "an older caller sends no `or` and means none"
        );
    }

    #[test]
    fn test_a_wait_can_carry_what_else_to_stop_for() {
        let sent: OnElement = serde_json::from_str(
            r#"{"op":"wait_for","query":"Public rate","or":["unavailable on our site"]}"#,
        )
        .expect("parses");

        match sent {
            OnElement::WaitFor { or, .. } => assert_eq!(or, vec!["unavailable on our site"]),
            other => panic!("read as {other:?}"),
        }
    }

    #[test]
    fn test_a_click_without_a_button_is_a_left_click() {
        let sent: OnElement =
            serde_json::from_str(r#"{"op":"click","query":"Submit"}"#).expect("parses");

        assert!(
            matches!(sent, OnElement::Click { button, .. } if button == Button::Left),
            "an older caller names no button and means the left one"
        );
    }

    #[test]
    fn test_a_click_can_name_another_button() {
        let sent: OnElement =
            serde_json::from_str(r#"{"op":"click","query":"Row","button":"right"}"#)
                .expect("parses");

        match sent {
            OnElement::Click { button, .. } => assert_eq!(button, Button::Right),
            other => panic!("read as {other:?}"),
        }
    }

    #[test]
    fn test_a_url_opens_in_a_tab_of_its_own_unless_told_otherwise() {
        let sent: Action =
            serde_json::from_str(r#"{"type":"open_url","url":"https://example.com"}"#)
                .expect("parses");

        assert!(
            matches!(
                sent,
                Action::OpenUrl {
                    target: OpenIn::Blank,
                    ..
                }
            ),
            "an older caller sends no target and gets what it always got"
        );

        let here: Action = serde_json::from_str(
            r#"{"type":"open_url","url":"https://example.com","target":"current"}"#,
        )
        .expect("parses");

        assert!(matches!(
            here,
            Action::OpenUrl {
                target: OpenIn::Current,
                ..
            }
        ));
    }

    #[test]
    fn test_a_batch_from_a_box_with_no_debugger_names_no_tabs() {
        let answered = r#"{"results":[],"stopped_at":null,"frame":null,"cursor":null}"#;
        let result: BatchResult = serde_json::from_str(answered).expect("parses");

        assert!(result.tabs.is_empty());
    }

    #[test]
    fn test_a_click_on_an_element_is_single_unless_asked() {
        let once: OnElement =
            serde_json::from_str(r#"{"op":"click","query":"Report.pdf"}"#).expect("parses");
        assert!(matches!(once, OnElement::Click { double: false, .. }));

        let twice: OnElement =
            serde_json::from_str(r#"{"op":"click","query":"Report.pdf","double":true}"#)
                .expect("parses");
        assert!(matches!(twice, OnElement::Click { double: true, .. }));
    }

    #[test]
    fn test_a_wait_matches_loosely_unless_told_otherwise() {
        let sent: OnElement =
            serde_json::from_str(r#"{"op":"wait_for","query":"Public rate"}"#).expect("parses");

        assert!(matches!(sent, OnElement::WaitFor { exact: false, .. }));

        let strict: OnElement =
            serde_json::from_str(r#"{"op":"wait_for","query":"Public rate","exact":true}"#)
                .expect("parses");

        assert!(matches!(strict, OnElement::WaitFor { exact: true, .. }));
    }

    #[test]
    fn test_an_element_with_no_point_goes_both_ways() {
        let out = Element {
            text: "Top".to_string(),
            tag: "a".to_string(),
            kind: None,
            role: None,
            states: Vec::new(),
            selector: None,
            label: None,
            visible: false,
            at: None,
            width: 10,
            height: 10,
            enabled: true,
            value: None,
        };

        let wire = serde_json::to_string(&out).expect("serialises");
        assert!(!wire.contains("\"at\""), "absence is left out: {wire}");
        assert_eq!(
            serde_json::from_str::<Element>(&wire).expect("parses").at,
            None
        );
    }

    #[test]
    fn test_a_result_says_where_the_page_ended_up() {
        let answered = r#"{"url":"https://example.com/africa","navigated":true}"#;
        let result: ElementResult = serde_json::from_str(answered).expect("parses");

        assert_eq!(result.url.as_deref(), Some("https://example.com/africa"));
        assert!(result.navigated);
    }

    #[test]
    fn test_a_result_from_an_older_server_still_parses() {
        let result: ElementResult = serde_json::from_str("{}").expect("parses");

        assert!(result.url.is_none());
        assert!(!result.navigated, "and does not claim the page moved");
        assert!(result.matched.is_none());
    }
}
