use computer_types::Motion;
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
        #[serde(default, skip_serializing_if = "Motion::is_instant")]
        motion: Motion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pause_ms: Option<u64>,
    },
    Click {
        #[serde(default)]
        at: Option<Point>,
        #[serde(default)]
        button: Button,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        held: Vec<Held>,
        #[serde(default, skip_serializing_if = "Motion::is_instant")]
        motion: Motion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u64>,
    },
    DoubleClick {
        #[serde(default)]
        at: Option<Point>,
        #[serde(default)]
        button: Button,
        #[serde(default, skip_serializing_if = "Motion::is_instant")]
        motion: Motion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u64>,
    },
    Drag {
        from: Point,
        to: Point,
        #[serde(default)]
        button: Button,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        held: Vec<Held>,
        #[serde(default, skip_serializing_if = "Motion::is_instant")]
        motion: Motion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u64>,
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
    /// One press, through every point, one release. A drag for each leg draws
    /// that many strokes; this draws one.
    Path {
        through: Vec<Point>,
        #[serde(default)]
        button: Button,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        held: Vec<Held>,
        #[serde(default, skip_serializing_if = "Motion::is_instant")]
        motion: Motion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u64>,
    },
    MouseDown {
        #[serde(default)]
        at: Option<Point>,
        #[serde(default)]
        button: Button,
        #[serde(default, skip_serializing_if = "Motion::is_instant")]
        motion: Motion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold_ms: Option<u64>,
    },
    MouseUp {
        #[serde(default)]
        at: Option<Point>,
        #[serde(default)]
        button: Button,
    },
    KeyDown {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hold_ms: Option<u64>,
    },
    KeyUp {
        key: String,
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
    Evaluate {
        what: Evaluate,
    },
    Look {
        what: Find,
    },
    Snapshot {
        what: SnapshotOptions,
    },
    Read {
        what: PageRead,
    },
    PageShot {
        what: PageShot,
    },
    Capture {
        what: Shot,
    },
    Cursor,
    Windows {
        /// Only the one the keyboard reaches.
        #[serde(default)]
        active: bool,
    },
    AwaitWindow {
        what: AwaitWindow,
    },
    OnWindow {
        window: String,
        what: WindowOp,
    },
    Tabs,
    OnTab {
        tab: String,
        #[serde(default)]
        close: bool,
    },
    Exec {
        what: ExecRequest,
    },
    ReadFile {
        path: String,
    },
    WriteFile {
        what: WriteFile,
    },
    ListFiles {
        path: String,
    },
    Grep {
        what: computer_types::Search,
    },
    Glob {
        what: Globbing,
    },
    /// Reads the selection, or sets it when `text` is given.
    Clipboard {
        #[serde(default)]
        selection: Selection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    Record {
        what: RecordOp,
    },
    Apps,
}

/// What a reading step answers with. Typed rather than free JSON, so this
/// crate stays a contract and needs no JSON library of its own.
// Adjacently tagged: an internal tag cannot carry a variant that holds a
// sequence, and half of these are lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "is", content = "saw", rename_all = "snake_case")]
pub enum Out {
    Value(Evaluated),
    Elements(Vec<Element>),
    Snapshot(Box<Snapshot>),
    Text(Box<PageText>),
    Picture(Captured),
    /// The desktop, which answers with a hash so an unchanged screen costs
    /// nothing; a page capture answers with the bytes every time.
    Frame(Frame),
    At(Point),
    Windows(Vec<Window>),
    Window(Option<Window>),
    Tabs(Vec<Tab>),
    Ran(ExecResponse),
    File(ReadFile),
    Listing(Listing),
    Found(Found),
    Globbed(Globbed),
    Clipboard(ClipboardView),
    Recording(RecordingView),
    Apps(Vec<String>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRead {
    #[serde(default)]
    pub format: Reading,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_links: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case", deny_unknown_fields)]
pub enum WindowOp {
    Focus,
    Close,
    Arrange { how: Arrange },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordOp {
    Start {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<u32>,
    },
    Stop,
    Status,
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
    /// Given by the last `snapshot`; `@e12` names it in a query.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

impl Element {
    pub fn role_word(&self) -> String {
        if let Some(role) = self.role.as_deref() {
            return role.to_string();
        }

        let word = match self.tag.as_str() {
            "a" | "area" => "link",
            "button" | "summary" => "button",
            "select" => "combobox",
            "textarea" => "textbox",
            "input" => match self.kind.as_deref().unwrap_or("") {
                "submit" | "button" | "reset" | "image" => "button",
                "" | "text" | "search" | "email" | "url" | "tel" | "password" => "textbox",
                other => other,
            },
            tag => tag,
        };

        word.to_string()
    }

    pub fn brief(&self, with_href: bool) -> String {
        let mut line = match (&self.r#ref, &self.selector) {
            (Some(numbered), _) => format!("@{numbered}"),
            (None, Some(selector)) => selector.clone(),
            (None, None) => self.tag.clone(),
        };

        line.push(' ');
        line.push_str(&self.role_word());

        let name = self
            .label
            .as_deref()
            .filter(|label| !label.is_empty())
            .unwrap_or(&self.text);
        if !name.is_empty() {
            line.push_str(&format!(" {name:?}"));
        }

        // A checkbox's value is "on"; its state is what says anything.
        let field = matches!(self.tag.as_str(), "input" | "textarea" | "select")
            && !matches!(
                self.kind.as_deref(),
                Some("checkbox" | "radio" | "submit" | "button" | "reset" | "image")
            );
        if field {
            if let Some(value) = self.value.as_deref().filter(|value| !value.is_empty()) {
                if value != name {
                    line.push_str(&format!(" = {value:?}"));
                }
            }
        }

        if !self.states.is_empty() {
            line.push_str(&format!(" [{}]", self.states.join(" ")));
        }

        if with_href {
            if let Some(href) = &self.href {
                line.push(' ');
                line.push_str(href);
            }
        }

        line
    }
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
        #[serde(default, skip_serializing_if = "Motion::is_instant")]
        motion: Motion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u64>,
    },
    /// Both ends must fit in the window at once.
    Drag {
        from: String,
        to: String,
        #[serde(default)]
        button: Button,
        #[serde(default, skip_serializing_if = "Motion::is_instant")]
        motion: Motion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u64>,
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
        /// The whole selection, for a dropdown that takes several.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<String>,
        /// Take those named out of the selection instead, or empty it when
        /// none is named.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        drop: bool,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quiet_ms: Option<u64>,
        /// Not disabled. A query matches a button that cannot be pressed.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        enabled: bool,
        /// Wait for the document to finish loading.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        load: bool,
        /// Javascript, waited on until it is truthy.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<String>,
    },
    Focus {
        query: String,
    },
    Check {
        query: String,
        /// Ticked, or cleared. Already in that state is not a click.
        on: bool,
    },
    Hover {
        query: String,
        #[serde(default, skip_serializing_if = "Motion::is_instant")]
        motion: Motion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u64>,
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
    pub changed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Point>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<Changes>,
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
    /// Run the rest after a step is refused. A form wants the default, where
    /// step four does not run on the assumption that step three worked; a
    /// drawing wants this, where one refused stroke costs only that stroke.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_going: bool,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub released: Vec<Button>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holding: Vec<Holding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub released_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holding_keys: Vec<HoldingKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holding {
    pub button: Button,
    pub until_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldingKey {
    pub key: String,
    pub until_ms: u64,
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
pub struct SnapshotOptions {
    /// A query as `find` takes one; the listing is what its first match holds.
    pub scope: Option<String>,
    pub limit: Option<usize>,
    pub tab: Option<String>,
    #[serde(default)]
    pub delta: bool,
    #[serde(default)]
    pub quiet_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub url: String,
    pub title: String,
    /// What the page offers; the listing stops at the limit.
    pub total: usize,
    /// Empty when a delta was asked for and there was a snapshot to compare with.
    pub elements: Vec<Element>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<Changes>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Changes {
    /// Nothing to compare with, so the whole listing came instead.
    #[serde(default)]
    pub first: bool,
    #[serde(default)]
    pub added: Vec<Element>,
    #[serde(default)]
    pub changed: Vec<Element>,
    /// As they were, since they are no longer on the page.
    #[serde(default)]
    pub gone: Vec<Element>,
    #[serde(default)]
    pub same: usize,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.gone.is_empty()
    }

    pub fn lines(&self, with_href: bool) -> Vec<String> {
        if self.is_empty() {
            return vec![format!(
                "unchanged since the last snapshot: {} controls",
                self.same
            )];
        }

        let mut lines = vec![format!(
            "since the last snapshot: {} appeared, {} changed, {} left, {} the same",
            self.added.len(),
            self.changed.len(),
            self.gone.len(),
            self.same
        )];
        lines.extend(
            self.added
                .iter()
                .map(|one| format!("+ {}", one.brief(with_href))),
        );
        lines.extend(
            self.changed
                .iter()
                .map(|one| format!("~ {}", one.brief(with_href))),
        );
        lines.extend(
            self.gone
                .iter()
                .map(|one| format!("- {}", one.brief(with_href))),
        );
        lines
    }
}

impl Snapshot {
    pub fn lines(&self, with_href: bool) -> Vec<String> {
        let mut lines = vec![match self.title.trim().is_empty() {
            true => self.url.clone(),
            false => format!("{}  {}", self.title, self.url),
        }];

        match &self.delta {
            Some(delta) if !delta.first => {
                lines.extend(delta.lines(with_href));
                return lines;
            }
            Some(_) => {
                lines.push("first snapshot of this page, so the whole listing follows".to_string())
            }
            None => {}
        }

        if self.elements.is_empty() {
            lines.push("no controls on the page".to_string());
        }
        lines.extend(self.elements.iter().map(|one| one.brief(with_href)));

        let left = self.total.saturating_sub(self.elements.len());
        if left > 0 {
            lines.push(format!(
                "({left} more: narrow the scope or raise the limit)"
            ));
        }

        lines
    }
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
    /// What the step read. Absent for a step that only acts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<Out>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Globbing {
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Found {
    pub matches: Vec<computer_types::Match>,
    /// There were more; the cap stopped it.
    pub cut: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Globbed {
    pub paths: Vec<String>,
    pub cut: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listing {
    pub path: String,
    pub entries: Vec<computer_types::DirEntry>,
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
pub struct CdpToken {
    pub url: String,
    pub ws_url: String,
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
    fn test_a_button_goes_down_and_up_where_the_pointer_is_unless_told() {
        let down: Action = serde_json::from_str(r#"{"type":"mouse_down"}"#).expect("parses");
        assert_eq!(
            down,
            Action::MouseDown {
                at: None,
                button: Button::Left,
                motion: Motion::default(),
                seed: None,
                hold_ms: None,
            }
        );

        let up: Action =
            serde_json::from_str(r#"{"type":"mouse_up","at":{"x":30,"y":40},"button":"right"}"#)
                .expect("parses");
        assert_eq!(
            up,
            Action::MouseUp {
                at: Some(Point { x: 30, y: 40 }),
                button: Button::Right,
            }
        );

        assert_eq!(
            serde_json::to_string(&down).expect("writes"),
            r#"{"type":"mouse_down","at":null,"button":"left"}"#
        );
    }

    #[test]
    fn test_a_press_outlives_its_call_only_when_told_how_long() {
        let held: Action =
            serde_json::from_str(r#"{"type":"mouse_down","hold_ms":10000}"#).expect("parses");
        assert!(matches!(
            held,
            Action::MouseDown {
                hold_ms: Some(10_000),
                ..
            }
        ));

        let plain: Action = serde_json::from_str(r#"{"type":"mouse_down"}"#).expect("parses");
        assert!(matches!(plain, Action::MouseDown { hold_ms: None, .. }));

        let said = serde_json::to_string(&BatchResult {
            results: Vec::new(),
            stopped_at: None,
            frame: None,
            cursor: None,
            windows: Vec::new(),
            tabs: Vec::new(),
            released: Vec::new(),
            holding: vec![Holding {
                button: Button::Left,
                until_ms: 1_700_000_000_000,
            }],
            released_keys: Vec::new(),
            holding_keys: Vec::new(),
        })
        .expect("writes");
        assert!(
            said.contains(r#""holding":[{"button":"left","until_ms":1700000000000}]"#),
            "{said}"
        );
    }

    #[test]
    fn test_the_file_steps_take_what_the_file_routes_take() {
        let listed: Action =
            serde_json::from_str(r#"{"type":"list_files","path":"/tmp"}"#).expect("parses");
        assert_eq!(
            listed,
            Action::ListFiles {
                path: "/tmp".to_string()
            }
        );

        let searched: Action = serde_json::from_str(
            r#"{"type":"grep","what":{"pattern":"TODO","path":"/src","include":"*.rs"}}"#,
        )
        .expect("parses");
        assert!(matches!(
            &searched,
            Action::Grep { what } if what.pattern == "TODO" && what.include.as_deref() == Some("*.rs")
        ));

        let globbed: Action =
            serde_json::from_str(r#"{"type":"glob","what":{"pattern":"**/*.png"}}"#)
                .expect("parses");
        assert_eq!(
            globbed,
            Action::Glob {
                what: Globbing {
                    pattern: "**/*.png".to_string(),
                    path: None,
                    limit: None,
                }
            }
        );
        assert!(
            serde_json::from_str::<Action>(r#"{"type":"glob","what":{"patern":"*"}}"#).is_err(),
            "a misspelt field would glob for nothing and say it found nothing"
        );

        let said = serde_json::to_string(&Out::Globbed(Globbed {
            paths: vec!["/tmp/a.png".to_string()],
            cut: false,
        }))
        .expect("writes");
        assert_eq!(
            said,
            r#"{"is":"globbed","saw":{"paths":["/tmp/a.png"],"cut":false}}"#
        );
    }

    #[test]
    fn test_a_key_goes_down_and_stays_down_only_when_told_how_long() {
        let plain: Action =
            serde_json::from_str(r#"{"type":"key_down","key":"shift"}"#).expect("parses");
        assert_eq!(
            plain,
            Action::KeyDown {
                key: "shift".to_string(),
                hold_ms: None,
            }
        );
        assert_eq!(
            serde_json::to_string(&plain).expect("writes"),
            r#"{"type":"key_down","key":"shift"}"#
        );

        let held: Action =
            serde_json::from_str(r#"{"type":"key_down","key":"space","hold_ms":5000}"#)
                .expect("parses");
        assert!(matches!(
            held,
            Action::KeyDown {
                hold_ms: Some(5000),
                ..
            }
        ));

        let up: Action =
            serde_json::from_str(r#"{"type":"key_up","key":"shift"}"#).expect("parses");
        assert_eq!(
            up,
            Action::KeyUp {
                key: "shift".to_string()
            }
        );
        assert!(
            serde_json::from_str::<Action>(r#"{"type":"key_down"}"#).is_err(),
            "a key hold with no key would hold nothing"
        );
    }

    #[test]
    fn test_a_batch_names_a_key_it_let_go_or_still_holds_only_when_it_did() {
        let quiet = BatchResult {
            results: Vec::new(),
            stopped_at: None,
            frame: None,
            cursor: None,
            windows: Vec::new(),
            tabs: Vec::new(),
            released: Vec::new(),
            holding: Vec::new(),
            released_keys: Vec::new(),
            holding_keys: Vec::new(),
        };
        let said = serde_json::to_string(&quiet).expect("writes");
        assert!(
            !said.contains("released_keys") && !said.contains("holding_keys"),
            "{said}"
        );

        let said = serde_json::to_string(&BatchResult {
            released_keys: vec!["shift".to_string()],
            holding_keys: vec![HoldingKey {
                key: "space".to_string(),
                until_ms: 1_700_000_000_000,
            }],
            ..quiet
        })
        .expect("writes");
        assert!(said.contains(r#""released_keys":["shift"]"#), "{said}");
        assert!(
            said.contains(r#""holding_keys":[{"key":"space","until_ms":1700000000000}]"#),
            "{said}"
        );
    }

    #[test]
    fn test_a_move_can_pause_so_a_drawing_program_sees_every_point() {
        let paced: Action =
            serde_json::from_str(r#"{"type":"move","to":{"x":10,"y":20},"pause_ms":40}"#)
                .expect("parses");
        assert!(matches!(
            paced,
            Action::Move {
                pause_ms: Some(40),
                ..
            }
        ));

        let plain: Action =
            serde_json::from_str(r#"{"type":"move","to":{"x":10,"y":20}}"#).expect("parses");
        assert!(matches!(plain, Action::Move { pause_ms: None, .. }));
        assert_eq!(
            serde_json::to_string(&plain).expect("writes"),
            r#"{"type":"move","to":{"x":10,"y":20}}"#,
            "a move with no pause is the move an older server already takes"
        );
    }

    #[test]
    fn test_a_batch_names_a_button_it_let_go_only_when_it_did() {
        let quiet = BatchResult {
            results: Vec::new(),
            stopped_at: None,
            frame: None,
            cursor: None,
            windows: Vec::new(),
            tabs: Vec::new(),
            released: Vec::new(),
            holding: Vec::new(),
            released_keys: Vec::new(),
            holding_keys: Vec::new(),
        };
        assert!(
            !serde_json::to_string(&quiet)
                .expect("writes")
                .contains("released")
        );

        let said = serde_json::to_string(&BatchResult {
            released: vec![Button::Left],
            ..quiet
        })
        .expect("writes");
        assert!(said.contains(r#""released":["left"]"#), "{said}");

        let older: BatchResult =
            serde_json::from_str(r#"{"results":[],"stopped_at":null,"frame":null,"cursor":null}"#)
                .expect("an answer from a server that has no such field");
        assert!(older.released.is_empty());
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
    fn test_a_dropdown_takes_one_option_or_several() {
        let one: OnElement =
            serde_json::from_str(r#"{"op":"choose","query":"Colour","options":["Blue"]}"#)
                .expect("parses");
        assert!(matches!(one, OnElement::Choose { drop: false, .. }));

        let clear: OnElement =
            serde_json::from_str(r#"{"op":"choose","query":"Tags","drop":true}"#).expect("parses");
        assert!(matches!(
            clear,
            OnElement::Choose {
                drop: true,
                ref options,
                ..
            } if options.is_empty()
        ));

        let wire = serde_json::to_string(&OnElement::Choose {
            query: "Colour".to_string(),
            options: vec!["Blue".to_string()],
            drop: false,
        })
        .expect("serialises");
        assert!(!wire.contains("drop"), "nothing new when defaulted: {wire}");
    }

    #[test]
    fn test_a_wait_can_be_for_the_page_to_settle() {
        let sent: OnElement =
            serde_json::from_str(r#"{"op":"wait_for","query":"","quiet_ms":500}"#).expect("parses");
        assert!(matches!(
            sent,
            OnElement::WaitFor {
                quiet_ms: Some(500),
                ..
            }
        ));

        let plain = OnElement::WaitFor {
            query: "Calendar".to_string(),
            gone: false,
            within_ms: None,
            or: Vec::new(),
            exact: false,
            quiet_ms: None,
            enabled: false,
            load: false,
            until: None,
        };
        let wire = serde_json::to_string(&plain).expect("serialises");
        for added in ["quiet_ms", "enabled", "load", "until"] {
            assert!(
                !wire.contains(added),
                "an older server sees nothing new: {wire}"
            );
        }
    }

    fn numbered(tag: &str, kind: Option<&str>, text: &str) -> Element {
        Element {
            text: text.to_string(),
            tag: tag.to_string(),
            kind: kind.map(str::to_string),
            role: None,
            states: Vec::new(),
            selector: Some("#one".to_string()),
            label: None,
            visible: true,
            at: None,
            width: 10,
            height: 10,
            enabled: true,
            value: None,
            r#ref: Some("e7".to_string()),
            href: None,
        }
    }

    #[test]
    fn test_a_brief_line_names_the_ref_the_kind_and_the_words() {
        let button = numbered("button", None, "Sign in");
        assert_eq!(button.brief(false), "@e7 button \"Sign in\"");

        let heading = numbered("h2", None, "Account");
        assert_eq!(heading.brief(false), "@e7 h2 \"Account\"");

        let unnumbered = Element {
            r#ref: None,
            ..numbered("a", None, "Docs")
        };
        assert_eq!(
            unnumbered.brief(false),
            "#one link \"Docs\"",
            "without a number the selector names it"
        );
    }

    #[test]
    fn test_a_field_shows_its_value_and_a_checkbox_does_not() {
        let email = Element {
            label: Some("Email".to_string()),
            value: Some("toby@example.com".to_string()),
            states: vec!["required".to_string()],
            ..numbered("input", Some("email"), "toby@example.com")
        };
        assert_eq!(
            email.brief(false),
            "@e7 textbox \"Email\" = \"toby@example.com\" [required]"
        );

        let ticked = Element {
            label: Some("Remember me".to_string()),
            value: Some("on".to_string()),
            states: vec!["checked".to_string()],
            ..numbered("input", Some("checkbox"), "on")
        };
        assert_eq!(
            ticked.brief(false),
            "@e7 checkbox \"Remember me\" [checked]",
            "\"on\" says nothing; checked says it all"
        );

        let bare = Element {
            value: Some("hello".to_string()),
            ..numbered("input", None, "hello")
        };
        assert_eq!(
            bare.brief(false),
            "@e7 textbox \"hello\"",
            "a field with no label is named by its value, and not twice"
        );
    }

    #[test]
    fn test_a_link_address_comes_only_when_asked() {
        let link = Element {
            href: Some("https://example.com/docs".to_string()),
            ..numbered("a", None, "Docs")
        };

        assert_eq!(link.brief(false), "@e7 link \"Docs\"");
        assert_eq!(
            link.brief(true),
            "@e7 link \"Docs\" https://example.com/docs"
        );
    }

    #[test]
    fn test_a_role_the_page_set_wins_over_the_tag() {
        let styled = Element {
            role: Some("tab".to_string()),
            ..numbered("div", None, "Billing")
        };
        assert_eq!(styled.role_word(), "tab");

        assert_eq!(
            numbered("input", Some("submit"), "Go").role_word(),
            "button"
        );
        assert_eq!(numbered("input", Some("date"), "").role_word(), "date");
        assert_eq!(numbered("select", None, "").role_word(), "combobox");
        assert_eq!(numbered("textarea", None, "").role_word(), "textbox");
    }

    fn listing(
        title: &str,
        elements: Vec<Element>,
        total: usize,
        delta: Option<Changes>,
    ) -> Snapshot {
        Snapshot {
            url: "https://example.com/".to_string(),
            title: title.to_string(),
            total,
            elements,
            delta,
        }
    }

    #[test]
    fn test_a_listing_reads_as_one_line_per_control() {
        let email = Element {
            label: Some("Email".to_string()),
            value: Some("toby@example.com".to_string()),
            states: vec!["required".to_string()],
            ..numbered("input", Some("email"), "toby@example.com")
        };
        let said = listing(
            "Example",
            vec![numbered("h1", None, "Example Domain"), email],
            3,
            None,
        )
        .lines(false)
        .join("\n");

        assert!(said.starts_with("Example  https://example.com/\n"));
        assert!(said.contains("\n@e7 h1 \"Example Domain\"\n"));
        assert!(said.contains("\n@e7 textbox \"Email\" = \"toby@example.com\" [required]\n"));
        assert!(
            said.ends_with("(1 more: narrow the scope or raise the limit)"),
            "{said}"
        );
    }

    #[test]
    fn test_a_page_without_a_title_is_named_by_its_address() {
        let said = listing("", Vec::new(), 0, None).lines(false);

        assert_eq!(
            said[0], "https://example.com/",
            "no gap where a title would go"
        );
        assert_eq!(said[1], "no controls on the page");
    }

    #[test]
    fn test_a_delta_lists_what_moved_and_nothing_else() {
        let delta = Changes {
            first: false,
            added: vec![numbered("button", None, "Confirm")],
            changed: Vec::new(),
            gone: vec![numbered("input", Some("checkbox"), "on")],
            same: 11,
        };
        let said = listing("Example", Vec::new(), 12, Some(delta)).lines(false);

        assert_eq!(
            said[1],
            "since the last snapshot: 1 appeared, 0 changed, 1 left, 11 the same"
        );
        assert_eq!(said[2], "+ @e7 button \"Confirm\"");
        assert_eq!(said[3], "- @e7 checkbox \"on\"");
        assert_eq!(
            said.len(),
            4,
            "the listing itself is not repeated: {said:?}"
        );
    }

    #[test]
    fn test_an_unchanged_delta_is_one_line() {
        let delta = Changes {
            same: 13,
            ..Changes::default()
        };
        let said = listing("Example", Vec::new(), 13, Some(delta)).lines(false);

        assert_eq!(said[1], "unchanged since the last snapshot: 13 controls");
        assert_eq!(said.len(), 2);
    }

    #[test]
    fn test_a_first_delta_is_the_whole_listing_and_says_so() {
        let delta = Changes {
            first: true,
            ..Changes::default()
        };
        let said = listing(
            "Example",
            vec![numbered("button", None, "Go")],
            1,
            Some(delta),
        )
        .lines(false);

        assert_eq!(
            said[1],
            "first snapshot of this page, so the whole listing follows"
        );
        assert_eq!(said[2], "@e7 button \"Go\"");
    }

    #[test]
    fn test_a_result_from_an_older_server_carries_no_delta() {
        let result: ElementResult = serde_json::from_str("{}").expect("parses");
        assert!(result.delta.is_none());

        let wire = serde_json::to_string(&ElementResult::default()).expect("serialises");
        assert!(
            !wire.contains("delta"),
            "and nothing new is sent to one: {wire}"
        );

        let told: ElementResult = serde_json::from_str(
            r#"{"delta":{"added":[{"text":"Confirm","tag":"button","ref":"e14","width":1,"height":1,"enabled":true}]}}"#,
        )
        .expect("parses");
        let delta = told.delta.expect("a delta");
        assert!(!delta.first, "left out means false");
        assert_eq!(delta.added[0].r#ref.as_deref(), Some("e14"));
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
            r#ref: None,
            href: None,
        };

        let wire = serde_json::to_string(&out).expect("serialises");
        assert!(!wire.contains("\"at\""), "absence is left out: {wire}");
        assert!(
            !wire.contains("\"ref\""),
            "and so is a number it was never given"
        );
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
        assert!(result.changed.is_none(), "nor that nothing on it changed");
        assert!(result.matched.is_none());
    }

    #[test]
    fn test_a_result_says_whether_the_page_changed_under_the_press() {
        let answered = r#"{"url":"https://example.com/","navigated":false,"changed":true}"#;
        let result: ElementResult = serde_json::from_str(answered).expect("parses");

        assert_eq!(result.changed, Some(true));

        let wire = serde_json::to_string(&ElementResult::default()).expect("serialises");
        assert!(!wire.contains("changed"), "unwatched is left out: {wire}");
    }
}
