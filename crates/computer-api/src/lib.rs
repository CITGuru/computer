//! The HTTP contract: what a client sends, and what it gets back.
//!
//! Nothing here may reach for a web framework or for the engine. Both ends of
//! the wire depend on these types, and a client that has to compile a server's
//! dependencies to send a request is a client nobody uses.
//!
//! Every type goes both ways. The server never sends a request and never reads
//! a reply, so half of each pair is unused here — but a client does both, and a
//! protocol only one end can construct is not one.

pub use computer_types::{
    App, Auth, Bind, Button, Desktop, DisplayServer, Feature, Placement, Point, Policy, Selection,
    Spec,
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
    Gone,
}

/// What `/v1/health` answers, so a client can tell this apart from whatever
/// else happens to be listening on the port it guessed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    /// Names the server, because `{"ok": true}` is a thing many services say.
    pub service: String,
}

pub const SERVICE: &str = "computer-server";

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub index: usize,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Text rather than base64: an agent reads this. Binary belongs in a file,
/// read back through `GET /boxes/{id}/files`.
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
    /// `false` hands the screen over exclusively and holds this API's input
    /// back. `true` lets both drive, and both can race.
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

/// `Person` marks custody, never input. A person's keystrokes arrive over VNC
/// and never reach this server, so what is recorded is the interval a screen
/// was theirs.
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
    /// Carries the spec whole, so a box can be rebuilt from its record after
    /// the box itself is gone.
    ///
    /// Boxed: a trace holds thousands of entries and every one of them would
    /// otherwise be sized to hold a spec.
    BoxCreated {
        spec_digest: String,
        spec: Box<Spec>,
        placement: Box<Placement>,
        width: u32,
        height: u32,
        screens: u32,
    },
    /// Removed by something other than a request: its deadline passed, or the
    /// runtime stopped holding it.
    Gone {
        why: String,
    },
    /// Found running after a restart. Everything before it is gone: the trace
    /// lived in this process and the box did not.
    Adopted {
        runtime: String,
    },
    ForkedFrom {
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        up_to: Option<u64>,
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
    /// The actor is whoever held the screen, not whoever changed it: an agent
    /// can act during a handover, so a person-held frame may show its work.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceView {
    pub entries: Vec<TraceEntry>,
    /// Pass back as `after` to continue. `None` where the end was reached.
    pub next: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkRequest {
    #[serde(default)]
    pub mode: ForkMode,
    /// Replay up to and including this trace sequence. `None` replays all of
    /// it.
    #[serde(default)]
    pub up_to: Option<u64>,
    /// Where the copy runs. `None` puts it where the original was.
    #[serde(default)]
    pub placement: Option<Placement>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkMode {
    /// Launch from the same spec and do again what was done. Works wherever a
    /// box runs, and reconstructs rather than copies — see [`ReplayReport`].
    #[default]
    Replay,
    /// Copy the running machine. Needs a substrate that can freeze one.
    Snapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkResult {
    #[serde(rename = "box")]
    pub created: BoxView,
    pub replay: ReplayReport,
}

/// What the replay managed.
///
/// A replay reconstructs, it does not copy. The same actions against a page
/// that has since changed, or a slower network, or a dialog that appeared this
/// time, land somewhere else — so this reports what was attempted rather than
/// promising the two boxes match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayReport {
    pub attempted: usize,
    pub ok: usize,
    /// The source trace sequence that failed, if one did. Nothing after it was
    /// attempted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<u64>,
    /// Whether the replay ran out of time before reaching the end.
    pub truncated: bool,
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
