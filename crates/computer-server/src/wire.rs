//! The wire contract: what a client sends, and what it gets back.
//!
//! Nothing here may reach for the HTTP framework or for the engine. These
//! types are what a future `computer-api` crate hands to both this server and
//! a client, and a client that compiles axum to send a request is a client
//! nobody uses. The conversions live in [`crate::spec`] and [`crate::error`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `deny_unknown_fields` throughout: a spec is written by hand, and a
/// misspelled key that is quietly ignored hands back a box missing the thing
/// it was misspelled for, with nothing anywhere saying so.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default)]
    pub desktop: Desktop,
    /// Refused while there is no catalog behind them — see
    /// [`crate::spec::builder_for`].
    #[serde(default)]
    pub apps: BTreeMap<String, App>,
    #[serde(default)]
    pub policy: Policy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Desktop {
    #[serde(default)]
    pub server: DisplayServer,
    /// One size for every screen, because the engine takes one size for the
    /// box. Per-screen geometry would be a promise the image cannot keep.
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    /// Screen 0 always starts.
    #[serde(default = "default_screens")]
    pub screens: u32,
    #[serde(default)]
    pub features: Vec<Feature>,
    #[serde(default)]
    pub packages: Vec<String>,
}

impl Default for Desktop {
    fn default() -> Self {
        Self {
            server: DisplayServer::default(),
            width: default_width(),
            height: default_height(),
            screens: default_screens(),
            features: Vec::new(),
            packages: Vec::new(),
        }
    }
}

fn default_width() -> u32 {
    computer::image::WIDTH
}

fn default_height() -> u32 {
    computer::image::HEIGHT
}

fn default_screens() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayServer {
    #[default]
    X11,
    Wayland,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Chinese, Japanese, Korean and emoji. Without it those pages render as
    /// empty boxes and the screenshot still looks like a working page.
    WideFonts,
    Audio,
    Video,
    Dock,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default = "default_network")]
    pub network: bool,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub bind: Bind,
    /// The host to put in a viewer URL, where it is not the one bound to.
    #[serde(default)]
    pub advertise: Option<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            network: default_network(),
            auth: Auth::default(),
            bind: Bind::default(),
            advertise: None,
        }
    }
}

fn default_network() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    #[default]
    None,
    Password,
    Token,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bind {
    #[default]
    Loopback,
    Any,
}

/// Where the box runs and for how long. Deliberately not part of [`Spec`]:
/// two identical desktops that differ only in a memory limit are one desktop,
/// and hashing the placement in would build the same image twice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// `docker`, `podman` or `nerdctl`.
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub cpus: Option<String>,
    #[serde(default)]
    pub expires_after_secs: Option<u64>,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBox {
    #[serde(default)]
    pub spec: Spec,
    #[serde(default)]
    pub placement: Placement,
}

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxState {
    Ready,
    Gone,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoxList {
    pub boxes: Vec<BoxView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

/// One step an agent takes. `at` is optional wherever the pointer is already
/// where it should be, so a move and a click in one batch need not repeat the
/// coordinate.
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
        button: MouseButton,
    },
    DoubleClick {
        #[serde(default)]
        at: Option<Point>,
        #[serde(default)]
        button: MouseButton,
    },
    Drag {
        from: Point,
        to: Point,
        #[serde(default)]
        button: MouseButton,
    },
    Type {
        text: String,
    },
    Key {
        chord: String,
    },
    Scroll {
        at: Point,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    OpenUrl {
        url: String,
    },
    Wait {
        ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Want {
    Frame,
    Cursor,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionBatch {
    pub actions: Vec<Action>,
    /// How long to leave the screen alone before capturing. Waited inside the
    /// box, where a client's guess at a sleep cannot be wrong about the
    /// network.
    #[serde(default)]
    pub settle_ms: Option<u64>,
    #[serde(default)]
    pub want: Vec<Want>,
    /// The frame hash the caller already holds. A screen that has not moved
    /// answers `unchanged` and carries no picture.
    #[serde(default)]
    pub have_frame: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchResult {
    pub results: Vec<ActionResult>,
    /// The index of the action that stopped the batch, if one did. Everything
    /// after it was not attempted: a click that follows a move which failed
    /// lands wherever the pointer happened to be, and nothing in the frame
    /// afterwards says so.
    pub stopped_at: Option<usize>,
    pub frame: Option<Frame>,
    pub cursor: Option<Point>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionResult {
    pub index: usize,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Frame {
    pub hash: String,
    pub unchanged: bool,
    /// Omitted when `unchanged`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub png_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Text rather than base64: an agent reads this. Binary belongs in a file,
/// read back through `GET /boxes/{id}/files`.
#[derive(Debug, Clone, Serialize)]
pub struct ExecResponse {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteFile {
    pub path: String,
    pub contents_base64: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadFile {
    pub path: String,
    pub contents_base64: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeoverRequest {
    /// `false` hands the screen over exclusively and holds this API's input
    /// back. `true` lets both drive, and both can race.
    #[serde(default)]
    pub shared: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TakeoverView {
    pub url: Option<String>,
    pub exclusive: bool,
    pub screen: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewersView {
    pub watching: usize,
    pub driving: usize,
    pub person_driving: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClipboardView {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetClipboard {
    pub text: String,
    #[serde(default)]
    pub selection: ClipboardSelection,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardSelection {
    #[default]
    Clipboard,
    Primary,
}
