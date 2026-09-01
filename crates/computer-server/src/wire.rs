//! The wire contract: what a client sends, and what it gets back.
//!
//! Nothing here may reach for the HTTP framework or for the engine. These
//! types are what a future `computer-api` crate hands to both this server and
//! a client, and a client that compiles axum to send a request is a client
//! nobody uses. The conversions live in [`crate::spec`] and [`crate::error`].
//!
//! The shared half is re-exported from `computer-types`, which describes a
//! desktop and how to address one without knowing an API exists.

pub use computer_types::{
    App, Auth, Bind, Button, Desktop, DisplayServer, Feature, Placement, Point, Policy, Selection,
    Spec,
};
use serde::{Deserialize, Serialize};

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
        button: Button,
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
    pub selection: Selection,
}

/// Who asked for it.
///
/// `Person` marks custody, never input. A person's keystrokes arrive over VNC
/// and never reach this server, so what is recorded is the interval a screen
/// was theirs.
///
/// On a [`TraceEvent::Frame`] it means less again: who *held* the screen when
/// it was observed, not who changed it. An agent can still act during a
/// handover — `open_url` does not go through the gate that refuses synthetic
/// input — so a person-held frame may be showing an agent's work. The agent's
/// own entries are in the same interval, which is what the two together are
/// for.
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
    BoxCreated {
        spec_digest: String,
        width: u32,
        height: u32,
        screens: u32,
    },
    /// One action, with what it did. The action is carried whole so the run can
    /// be replayed against a fresh box.
    Acted {
        screen: u32,
        action: Action,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorBody>,
    },
    /// The screen changed. Written only when it did, so a caller polling a
    /// still screen adds nothing, and no frame entry between a takeover's two
    /// ends means nothing visible happened while it was held.
    ///
    /// The actor is whoever held the screen, not whoever changed it.
    Frame {
        screen: u32,
    },
    Executed {
        argv: Vec<String>,
        code: i32,
        timed_out: bool,
    },
    FileWritten {
        path: String,
        bytes: usize,
    },
    FileRead {
        path: String,
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
    /// From here until `TakeoverEnded` the screen was a person's, and this
    /// API's input was refused.
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
    /// The frame this entry left behind, by content. Fetch it from
    /// `/v1/boxes/{id}/trace/frames/{hash}`, which answers 404 once it has
    /// aged out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceView {
    pub entries: Vec<TraceEntry>,
    /// Pass back as `after` to continue. `None` where the end was reached.
    pub next: Option<u64>,
}
