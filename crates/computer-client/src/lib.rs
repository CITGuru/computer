//! Talking to a `computer-server`.
//!
//! Every endpoint is here, because REST being the complete surface is the
//! promise the server makes — a client that had to reach past it for one verb
//! would mean the promise was not kept.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use computer_api::*;
use computer_types::{Placement, Point, Selection, Spec};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server understood and refused, and said why.
    #[error("{}: {}", .0.code.as_str(), .0.message)]
    Refused(ErrorBody),
    /// Never reached it, or it never finished.
    #[error("transport: {0}")]
    Transport(String),
    /// It answered something this client cannot read, which is a version skew
    /// rather than a refusal.
    #[error("{status} answered with {body}")]
    Unreadable { status: u16, body: String },
}

impl Error {
    /// Whether sending it again could work.
    pub fn retryable(&self) -> bool {
        match self {
            Self::Refused(body) => body.retryable,
            Self::Transport(_) => true,
            Self::Unreadable { .. } => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Client {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Client {
    /// `http://127.0.0.1:8080`, with or without a trailing slash.
    /// Which server this talks to, for a caller that resolved one rather than
    /// naming it and has to say which it found.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            token: None,
            http: reqwest::Client::new(),
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Checks what answered, not merely that something did: a client that
    /// guesses a port has to tell this apart from whatever else is on it.
    pub async fn health(&self) -> Result<()> {
        let health: Health = self
            .send(reqwest::Method::GET, "/v1/health", None, &[])
            .await?;

        if !health.ok || health.service != computer_api::SERVICE {
            return Err(Error::Transport(format!(
                "something is answering here, and it is not a box server: {}",
                health.service
            )));
        }
        Ok(())
    }

    pub async fn create(
        &self,
        spec: &Spec,
        placement: &Placement,
        idempotency: Option<&str>,
    ) -> Result<BoxView> {
        let body = serde_json::json!({ "spec": spec, "placement": placement });
        self.send(
            reqwest::Method::POST,
            "/v1/boxes",
            Some(body),
            &idempotency_header(idempotency),
        )
        .await
    }

    pub async fn list(&self) -> Result<Vec<BoxView>> {
        let listed: BoxList = self
            .send(reqwest::Method::GET, "/v1/boxes", None, &[])
            .await?;
        Ok(listed.boxes)
    }

    pub async fn get(&self, id: &str) -> Result<BoxView> {
        self.send(reqwest::Method::GET, &format!("/v1/boxes/{id}"), None, &[])
            .await
    }

    /// Takes the confirmation header for you: the caller reached for a method
    /// called `delete`, which is the confirmation the header exists to get.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.nothing(
            reqwest::Method::DELETE,
            &format!("/v1/boxes/{id}"),
            None,
            &[("x-computer-confirm-delete", "yes".to_string())],
        )
        .await
    }

    pub async fn act(
        &self,
        id: &str,
        screen: u32,
        batch: &ActionBatch,
        idempotency: Option<&str>,
    ) -> Result<BatchResult> {
        self.send(
            reqwest::Method::POST,
            &format!("/v1/boxes/{id}/screens/{screen}/actions"),
            Some(serde_json::to_value(batch).map_err(|error| Error::Transport(error.to_string()))?),
            &idempotency_header(idempotency),
        )
        .await
    }

    /// What the page in front is showing, as text.
    pub async fn page(
        &self,
        id: &str,
        format: Reading,
        limit: Option<usize>,
        max_links: Option<usize>,
    ) -> Result<PageText> {
        let format = match format {
            Reading::Markdown => "markdown",
            Reading::Text => "text",
            Reading::Raw => "raw",
        };
        let mut path = format!("/v1/boxes/{id}/page?format={format}");
        if let Some(limit) = limit {
            path.push_str(&format!("&limit={limit}"));
        }
        if let Some(max_links) = max_links {
            path.push_str(&format!("&max_links={max_links}"));
        }

        self.send(reqwest::Method::GET, &path, None, &[]).await
    }

    /// What on the page matches, best first.
    pub async fn find(
        &self,
        id: &str,
        query: &str,
        limit: Option<usize>,
        scroll: Option<bool>,
        exact: Option<bool>,
    ) -> Result<Vec<Element>> {
        let mut path = format!("/v1/boxes/{id}/page/find?q={}", query_value(query));
        if let Some(limit) = limit {
            path.push_str(&format!("&limit={limit}"));
        }
        if let Some(scroll) = scroll {
            path.push_str(&format!("&scroll={scroll}"));
        }
        if let Some(exact) = exact {
            path.push_str(&format!("&exact={exact}"));
        }

        self.send(reqwest::Method::GET, &path, None, &[]).await
    }

    /// `settle_ms` is how long the page is given to stop moving before the URL
    /// it ended on is read. Zero measures immediately.
    ///
    /// `tab` names which page, or the one on screen where it names none.
    pub async fn on_element(
        &self,
        id: &str,
        what: &OnElement,
        settle_ms: u64,
        tab: Option<&str>,
    ) -> Result<ElementResult> {
        let mut query = vec![("settle_ms", settle_ms.to_string())];
        if let Some(tab) = tab {
            query.push(("tab", tab.to_string()));
        }

        self.send(
            reqwest::Method::POST,
            &format!("/v1/boxes/{id}/page/element"),
            Some(serde_json::to_value(what).map_err(|error| Error::Transport(error.to_string()))?),
            &query,
        )
        .await
    }

    /// Every page this box has open, and which of them is on screen.
    pub async fn tabs(&self, id: &str) -> Result<Vec<Tab>> {
        self.send(
            reqwest::Method::GET,
            &format!("/v1/boxes/{id}/pages"),
            None,
            &[],
        )
        .await
    }

    /// Bring one to the front.
    pub async fn focus_tab(&self, id: &str, tab: &str) -> Result<()> {
        self.nothing(
            reqwest::Method::POST,
            &format!("/v1/boxes/{id}/pages/{tab}/focus"),
            None,
            &[],
        )
        .await
    }

    pub async fn close_tab(&self, id: &str, tab: &str) -> Result<()> {
        self.nothing(
            reqwest::Method::DELETE,
            &format!("/v1/boxes/{id}/pages/{tab}"),
            None,
            &[],
        )
        .await
    }

    /// The app names this server can open.
    pub async fn catalog(&self) -> Result<Vec<String>> {
        let apps: std::collections::BTreeMap<String, serde_json::Value> = self
            .send(reqwest::Method::GET, "/v1/catalog", None, &[])
            .await?;

        Ok(apps.into_keys().collect())
    }

    /// What is on a screen, whoever opened it.
    pub async fn windows(&self, id: &str, screen: u32) -> Result<Vec<Window>> {
        self.send(
            reqwest::Method::GET,
            &format!("/v1/boxes/{id}/screens/{screen}/windows"),
            None,
            &[],
        )
        .await
    }

    pub async fn focus_window(&self, id: &str, screen: u32, window: &str) -> Result<()> {
        self.nothing(
            reqwest::Method::POST,
            &format!(
                "/v1/boxes/{id}/screens/{screen}/windows/{}/focus",
                query_value(window)
            ),
            None,
            &[],
        )
        .await
    }

    pub async fn arrange_window(
        &self,
        id: &str,
        screen: u32,
        window: &str,
        how: Arrange,
    ) -> Result<Window> {
        self.send(
            reqwest::Method::POST,
            &format!(
                "/v1/boxes/{id}/screens/{screen}/windows/{}/arrange",
                query_value(window)
            ),
            Some(serde_json::to_value(how).map_err(|error| Error::Transport(error.to_string()))?),
            &[],
        )
        .await
    }

    pub async fn active_window(&self, id: &str, screen: u32) -> Result<Option<Window>> {
        self.send(
            reqwest::Method::GET,
            &format!("/v1/boxes/{id}/screens/{screen}/windows/active"),
            None,
            &[],
        )
        .await
    }

    pub async fn wait_for_window(
        &self,
        id: &str,
        screen: u32,
        class: &str,
        within: Option<Duration>,
    ) -> Result<Window> {
        let body = AwaitWindow {
            class: class.to_string(),
            within_ms: within.map(|within| within.as_millis() as u64),
        };

        self.send(
            reqwest::Method::POST,
            &format!("/v1/boxes/{id}/screens/{screen}/windows/wait"),
            Some(serde_json::to_value(body).map_err(|error| Error::Transport(error.to_string()))?),
            &[],
        )
        .await
    }

    pub async fn close_window(&self, id: &str, screen: u32, window: &str) -> Result<()> {
        self.nothing(
            reqwest::Method::DELETE,
            &format!(
                "/v1/boxes/{id}/screens/{screen}/windows/{}",
                query_value(window)
            ),
            None,
            &[],
        )
        .await
    }

    /// One action and a look, which is most of what an agent ever asks for.
    pub async fn act_once(&self, id: &str, screen: u32, action: Action) -> Result<BatchResult> {
        self.act(
            id,
            screen,
            &ActionBatch {
                actions: vec![action],
                want: vec![Want::Frame],
                ..ActionBatch::default()
            },
            None,
        )
        .await
    }

    pub async fn frame(&self, id: &str, screen: u32, have: Option<&str>) -> Result<Frame> {
        self.capture(id, screen, &Shot::default(), have).await
    }

    /// A `Shot` naming nothing is the whole screen at full size, which is what
    /// `frame` asks for.
    pub async fn capture(
        &self,
        id: &str,
        screen: u32,
        shot: &Shot,
        have: Option<&str>,
    ) -> Result<Frame> {
        let mut asked: Vec<String> = Vec::new();

        if let Some(hash) = have {
            asked.push(format!("have={}", query_value(hash)));
        }
        if let Some(window) = &shot.window {
            asked.push(format!("window={}", query_value(window)));
        }
        if let Some(area) = &shot.region {
            asked.push(format!(
                "x={}&y={}&width={}&height={}",
                area.at.x, area.at.y, area.width, area.height
            ));
        }
        if let Some(scale) = shot.scale {
            asked.push(format!("scale={scale}"));
        }

        let path = match asked.is_empty() {
            true => format!("/v1/boxes/{id}/screens/{screen}/frame"),
            false => format!("/v1/boxes/{id}/screens/{screen}/frame?{}", asked.join("&")),
        };

        self.send(reqwest::Method::GET, &path, None, &[]).await
    }

    pub async fn cursor(&self, id: &str, screen: u32) -> Result<Point> {
        self.send(
            reqwest::Method::GET,
            &format!("/v1/boxes/{id}/screens/{screen}/cursor"),
            None,
            &[],
        )
        .await
    }

    pub async fn clipboard(&self, id: &str, screen: u32, selection: Selection) -> Result<String> {
        let view: ClipboardView = self
            .send(
                reqwest::Method::GET,
                &format!(
                    "/v1/boxes/{id}/screens/{screen}/clipboard?selection={}",
                    name_of(selection)
                ),
                None,
                &[],
            )
            .await?;
        Ok(view.text)
    }

    pub async fn set_clipboard(
        &self,
        id: &str,
        screen: u32,
        text: &str,
        selection: Selection,
    ) -> Result<()> {
        self.nothing(
            reqwest::Method::PUT,
            &format!("/v1/boxes/{id}/screens/{screen}/clipboard"),
            Some(serde_json::json!({ "text": text, "selection": name_of(selection) })),
            &[],
        )
        .await
    }

    pub async fn takeover(&self, id: &str, screen: u32, shared: bool) -> Result<TakeoverView> {
        self.send(
            reqwest::Method::POST,
            &format!("/v1/boxes/{id}/screens/{screen}/takeover"),
            Some(serde_json::json!({ "shared": shared })),
            &[],
        )
        .await
    }

    pub async fn end_takeover(&self, id: &str, screen: u32) -> Result<()> {
        self.nothing(
            reqwest::Method::DELETE,
            &format!("/v1/boxes/{id}/screens/{screen}/takeover"),
            None,
            &[],
        )
        .await
    }

    pub async fn viewers(&self, id: &str, screen: u32) -> Result<ViewersView> {
        self.send(
            reqwest::Method::GET,
            &format!("/v1/boxes/{id}/screens/{screen}/viewers"),
            None,
            &[],
        )
        .await
    }

    pub async fn exec(
        &self,
        id: &str,
        argv: &[String],
        timeout_ms: Option<u64>,
    ) -> Result<ExecResponse> {
        self.send(
            reqwest::Method::POST,
            &format!("/v1/boxes/{id}/exec"),
            Some(serde_json::json!({ "argv": argv, "timeout_ms": timeout_ms })),
            &[],
        )
        .await
    }

    pub async fn read_file(&self, id: &str, path: &str) -> Result<Vec<u8>> {
        let read: ReadFile = self
            .send(
                reqwest::Method::GET,
                &format!("/v1/boxes/{id}/files?path={}", query_value(path)),
                None,
                &[],
            )
            .await?;
        decode(&read.contents_base64)
    }

    pub async fn write_file(&self, id: &str, path: &str, bytes: &[u8]) -> Result<()> {
        self.nothing(
            reqwest::Method::PUT,
            &format!("/v1/boxes/{id}/files"),
            Some(serde_json::json!({
                "path": path,
                "contents_base64": BASE64.encode(bytes),
            })),
            &[],
        )
        .await
    }

    pub async fn fork(
        &self,
        id: &str,
        request: &ForkRequest,
        idempotency: Option<&str>,
    ) -> Result<ForkResult> {
        self.send(
            reqwest::Method::POST,
            &format!("/v1/boxes/{id}/fork"),
            Some(
                serde_json::to_value(request)
                    .map_err(|error| Error::Transport(error.to_string()))?,
            ),
            &idempotency_header(idempotency),
        )
        .await
    }

    pub async fn trace(
        &self,
        id: &str,
        after: Option<u64>,
        limit: Option<usize>,
    ) -> Result<TraceView> {
        let mut path = format!("/v1/boxes/{id}/trace?");
        if let Some(after) = after {
            path.push_str(&format!("after={after}&"));
        }
        if let Some(limit) = limit {
            path.push_str(&format!("limit={limit}"));
        }
        self.send(reqwest::Method::GET, &path, None, &[]).await
    }

    /// A frame out of a trace, as the PNG itself.
    pub async fn trace_frame(&self, id: &str, hash: &str) -> Result<Vec<u8>> {
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/v1/boxes/{id}/trace/frames/{hash}"),
                None,
                &[],
            )
            .await?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        if status.is_success() {
            return Ok(bytes.to_vec());
        }
        Err(refusal(status.as_u16(), &bytes))
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
        headers: &[(&str, String)],
    ) -> Result<reqwest::Response> {
        let mut request = self.http.request(method, format!("{}{path}", self.base));

        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }

        request
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
        headers: &[(&str, String)],
    ) -> Result<T> {
        let response = self.request(method, path, body, headers).await?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;

        if !(200..300).contains(&status) {
            return Err(refusal(status, &bytes));
        }

        serde_json::from_slice(&bytes).map_err(|_| Error::Unreadable {
            status,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }

    async fn nothing(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
        headers: &[(&str, String)],
    ) -> Result<()> {
        let response = self.request(method, path, body, headers).await?;
        let status = response.status().as_u16();

        if (200..300).contains(&status) {
            return Ok(());
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;
        Err(refusal(status, &bytes))
    }
}

/// The picture out of a frame, where one came. `None` where the caller already
/// held it.
///
/// A free function rather than a method: `Frame` belongs to `computer-api`.
pub fn frame_png(frame: &Frame) -> Result<Option<Vec<u8>>> {
    frame.png_base64.as_deref().map(decode).transpose()
}

/// Percent-encodes one query value.
///
/// A path is the caller's, and `&`, `#` or `?` in one ends the value early:
/// the request then names a different file and the wrong bytes come back as a
/// success.
fn query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());

    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(char::from(*byte))
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }

    out
}

fn decode(value: &str) -> Result<Vec<u8>> {
    BASE64
        .decode(value.as_bytes())
        .map_err(|error| Error::Transport(format!("the server sent base64 that is not: {error}")))
}

fn refusal(status: u16, bytes: &[u8]) -> Error {
    match serde_json::from_slice::<ErrorBody>(bytes) {
        Ok(body) => Error::Refused(body),
        Err(_) => Error::Unreadable {
            status,
            body: String::from_utf8_lossy(bytes).into_owned(),
        },
    }
}

fn idempotency_header(key: Option<&str>) -> Vec<(&'static str, String)> {
    key.map(|key| vec![("idempotency-key", key.to_string())])
        .unwrap_or_default()
}

fn name_of(selection: Selection) -> &'static str {
    match selection {
        Selection::Clipboard => "clipboard",
        Selection::Primary => "primary",
    }
}
