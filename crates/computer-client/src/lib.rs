//! Talking to a `computer-server`.
//!
//! Every endpoint is here, because REST being the complete surface is the
//! promise the server makes — a client that had to reach past it for one verb
//! would mean the promise was not kept.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use computer_api::*;
use computer_types::{Placement, Point, Selection, Spec};

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

    pub async fn health(&self) -> Result<()> {
        self.send::<serde_json::Value>(reqwest::Method::GET, "/v1/health", None, &[])
            .await?;
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
        let path = match have {
            Some(hash) => format!("/v1/boxes/{id}/screens/{screen}/frame?have={hash}"),
            None => format!("/v1/boxes/{id}/screens/{screen}/frame"),
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
                &format!("/v1/boxes/{id}/files?path={path}"),
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
