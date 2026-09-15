use super::api::{DEFAULT_DOMAIN, DEFAULT_USER, E2bApi, NAME_KEY, Sandbox, SandboxPlan, api_url};
use super::wire;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use async_trait::async_trait;
use reqwest::header::{
    AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, InvalidHeaderValue,
};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const API_KEY_ENV: &str = "E2B_API_KEY";
pub const DOMAIN_ENV: &str = "E2B_DOMAIN";

const API_KEY_HEADER: &str = "X-API-Key";
/// Lowercase because `HeaderName::from_static` panics otherwise.
const ENVD_TOKEN_HEADER: &str = "x-access-token";
/// A browser cannot send it, so a secure sandbox has no viewer URL.
const TRAFFIC_TOKEN_HEADER: &str = "e2b-traffic-access-token";

const CONNECT_JSON: &str = "application/connect+json";
const CONNECT_VERSION: &str = "Connect-Protocol-Version";

/// Above [`crate::machine::DEFAULT_TIMEOUT`], so the runner's bound is the one
/// a caller sees.
pub const TIMEOUT: Duration = Duration::from_secs(150);

pub struct Cloud {
    key: String,
    domain: String,
    http: Client,
    user: String,
    reaper: Option<(String, Vec<String>)>,
}

impl Cloud {
    pub fn new(key: impl Into<String>) -> Result<Self> {
        Self::at(key, DEFAULT_DOMAIN)
    }

    pub fn at(key: impl Into<String>, domain: impl Into<String>) -> Result<Self> {
        let http = Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| Error::transport(error.to_string(), false))?;

        Ok(Self {
            key: key.into(),
            domain: domain.into(),
            http,
            user: DEFAULT_USER.to_string(),
            reaper: None,
        })
    }

    pub fn from_env() -> Result<Self> {
        let key = std::env::var(API_KEY_ENV).map_err(|_| Error::Unavailable {
            runtime: "e2b".to_string(),
            detail: format!("{API_KEY_ENV} is not set"),
        })?;

        let domain = std::env::var(DOMAIN_ENV).unwrap_or_else(|_| DEFAULT_DOMAIN.to_string());
        Self::at(key, domain)
    }

    pub fn as_user(mut self, user: impl Into<String>) -> Self {
        self.user = user.into();
        self
    }

    /// `{}` stands for the sandbox ID. Unset, the deadline is what stops a leak.
    pub fn reaping_with<I, S>(mut self, program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.reaper = Some((program.into(), args.into_iter().map(Into::into).collect()));
        self
    }

    pub fn reaping_with_cli(self) -> Self {
        self.reaping_with("e2b", ["sandbox", "kill", "{}"])
    }

    fn control(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", api_url(&self.domain)))
            .header(API_KEY_HEADER, &self.key)
    }

    fn envd(&self, method: Method, sandbox: &Sandbox, path: &str) -> Result<RequestBuilder> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&wire::user_header(&self.user)).map_err(bad_header)?,
        );

        if let Some(token) = &sandbox.envd_token {
            headers.insert(
                HeaderName::from_static(ENVD_TOKEN_HEADER),
                HeaderValue::from_str(token).map_err(bad_header)?,
            );
        }
        if let Some(token) = &sandbox.traffic_token {
            headers.insert(
                HeaderName::from_static(TRAFFIC_TOKEN_HEADER),
                HeaderValue::from_str(token).map_err(bad_header)?,
            );
        }

        Ok(self
            .http
            .request(method, format!("{}{path}", sandbox.envd_url()))
            .headers(headers))
    }

    async fn send(&self, request: RequestBuilder) -> Result<Response> {
        request
            .send()
            .await
            .map_err(|error| Error::transport(chain(&error), error.is_timeout()))
    }

    async fn body(&self, response: Response) -> Result<Vec<u8>> {
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| Error::transport(error.to_string(), true))?
            .to_vec();

        match status.is_success() {
            true => Ok(bytes),
            false => Err(from_status(status, &String::from_utf8_lossy(&bytes))),
        }
    }

    async fn json(&self, request: RequestBuilder) -> Result<Value> {
        let bytes = self.body(self.send(request).await?).await?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }

        serde_json::from_slice(&bytes).map_err(|error| {
            Error::transport(format!("an answer that is not JSON: {error}"), false)
        })
    }

    /// The filter is only a hint; [`wire::carrying`] filters the answer again.
    async fn listing(&self, key: &str, value: Option<&str>) -> Result<Value> {
        let query = value
            .map(|value| format!("?metadata={}", wire::metadata_query(key, value)))
            .unwrap_or_default();

        self.json(self.control(Method::GET, &format!("/v2/sandboxes{query}")))
            .await
    }
}

/// `reqwest` keeps the real cause one `source()` below its top-level message.
fn chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut under = error.source();

    while let Some(cause) = under {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        under = cause.source();
    }
    message
}

fn bad_header(error: InvalidHeaderValue) -> Error {
    Error::denied(format!("a token that cannot be a header: {error}"))
}

fn from_status(status: StatusCode, detail: &str) -> Error {
    let detail = detail.trim().to_string();

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Error::denied(format!("e2b refused the key: {detail}"))
        }
        StatusCode::NOT_FOUND => Error::Gone(detail),
        // A sandbox past its deadline answers through the proxy as 502.
        StatusCode::BAD_GATEWAY => Error::Gone(detail),
        StatusCode::TOO_MANY_REQUESTS
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT => Error::transport(format!("{status}: {detail}"), true),
        _ => Error::transport(format!("{status}: {detail}"), false),
    }
}

/// The counter, not the clock, keeps two uploads in one nanosecond apart.
fn boundary() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();

    format!(
        "computer{:x}{:x}",
        nanos,
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[async_trait]
impl E2bApi for Cloud {
    async fn available(&self) -> Result<()> {
        let response = self.send(self.control(Method::GET, "/health")).await?;
        let status = response.status();

        if !status.is_success() {
            return Err(Error::Unavailable {
                runtime: "e2b".to_string(),
                detail: status.to_string(),
            });
        }

        // Health is unauthenticated, so the key is checked with a listing.
        let listed = self
            .send(self.control(Method::GET, "/v2/sandboxes"))
            .await?;
        match listed.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::Unavailable {
                runtime: "e2b".to_string(),
                detail: format!("{API_KEY_ENV} was refused"),
            }),
            _ => Ok(()),
        }
    }

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox> {
        let body = wire::new_sandbox(plan);
        let answer = self
            .json(
                self.control(Method::POST, "/sandboxes")
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.to_string()),
            )
            .await?;

        wire::sandbox_from(&answer)
    }

    async fn find(&self, name: &str) -> Result<Option<Sandbox>> {
        let listing = self.listing(NAME_KEY, Some(name)).await?;

        let Some((id, _)) = wire::carrying(&listing, NAME_KEY)
            .into_iter()
            .find(|(_, listed)| listed == name)
        else {
            return Ok(None);
        };

        // A listing carries no tokens; connect does, and resumes a paused box.
        let answer = self
            .json(
                self.control(Method::POST, &format!("/sandboxes/{id}/connect"))
                    .header(CONTENT_TYPE, "application/json")
                    .body(r#"{"timeout":300}"#),
            )
            .await?;

        wire::sandbox_from(&answer).map(Some)
    }

    async fn kill(&self, id: &str) -> Result<()> {
        let response = self
            .send(self.control(Method::DELETE, &format!("/sandboxes/{id}")))
            .await?;

        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Ok(()),
            status => Err(from_status(status, "")),
        }
    }

    async fn keep_alive(&self, id: &str, ttl: Duration) -> Result<()> {
        let request = self
            .control(Method::POST, &format!("/sandboxes/{id}/timeout"))
            .header(CONTENT_TYPE, "application/json")
            .body(format!(r#"{{"timeout":{}}}"#, ttl.as_secs()));

        self.body(self.send(request).await?).await.map(|_| ())
    }

    async fn logs(&self, id: &str) -> Result<String> {
        let answer = self
            .json(self.control(Method::GET, &format!("/sandboxes/{id}/logs")))
            .await?;

        let Some(lines) = answer.get("logs").and_then(Value::as_array) else {
            return Ok(answer.to_string());
        };

        Ok(lines
            .iter()
            .filter_map(|entry| entry.get("line").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>> {
        Ok(wire::carrying(&self.listing(key, None).await?, key))
    }

    async fn exec(
        &self,
        sandbox: &Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        let body = wire::start_request(argv, env)?;

        let request = self
            .envd(Method::POST, sandbox, "/process.Process/Start")?
            .header(CONTENT_TYPE, CONNECT_JSON)
            .header(CONNECT_VERSION, "1")
            .body(wire::enveloped(body.to_string().as_bytes()));

        let bytes = self.body(self.send(request).await?).await?;
        wire::parse_events(&bytes)
    }

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>> {
        let query = format!(
            "/files?path={}&username={}",
            wire::escape(path),
            wire::escape(&self.user)
        );
        let request = self.envd(Method::GET, sandbox, &query)?;

        self.body(self.send(request).await?).await
    }

    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()> {
        let name = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());

        let boundary = boundary();
        let query = format!(
            "/files?path={}&username={}",
            wire::escape(path),
            wire::escape(&self.user)
        );

        let request = self
            .envd(Method::POST, sandbox, &query)?
            .header(
                CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(wire::multipart(&name, bytes, &boundary));

        self.body(self.send(request).await?).await.map(|_| ())
    }

    fn reaper(&self, id: &str) -> Option<(String, Vec<String>)> {
        let (program, args) = self.reaper.clone()?;
        Some((
            program,
            args.into_iter().map(|arg| arg.replace("{}", id)).collect(),
        ))
    }
}

/// The sandbox is secure, so it has no viewer URL until
/// [`public_viewer`](crate::sandboxes::remote::RemoteMachine::public_viewer).
pub fn pair_from_env() -> Result<(
    crate::sandboxes::remote::RemoteMachine,
    std::sync::Arc<crate::sandboxes::remote::RemoteProfile>,
)> {
    Ok(super::pair(
        std::sync::Arc::new(Cloud::from_env()?),
        std::sync::Arc::new(crate::X11Profile),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_status_says_what_the_caller_does_next() {
        assert!(!from_status(StatusCode::UNAUTHORIZED, "nope").needs_another_place());
        assert!(from_status(StatusCode::NOT_FOUND, "gone").needs_another_place());
        assert!(from_status(StatusCode::BAD_GATEWAY, "expired").needs_another_place());
        assert!(from_status(StatusCode::TOO_MANY_REQUESTS, "slow down").retryable());
        assert!(!from_status(StatusCode::BAD_REQUEST, "bad").retryable());
    }

    #[test]
    fn test_a_boundary_is_not_reused() {
        assert_ne!(boundary(), boundary());
    }

    #[test]
    fn test_the_reaper_substitutes_the_sandbox_id() {
        let cloud = Cloud::new("key").expect("a client").reaping_with_cli();
        let (program, args) = E2bApi::reaper(&cloud, "sbx-9").expect("a command");

        assert_eq!(program, "e2b");
        assert_eq!(args, vec!["sandbox", "kill", "sbx-9"]);
    }

    #[test]
    fn test_a_dropped_handle_leaks_nothing_worse_than_a_deadline() {
        let cloud = Cloud::new("key").expect("a client");
        assert!(
            E2bApi::reaper(&cloud, "sbx-9").is_none(),
            "the deadline is what stops the leak, so the CLI is opt-in"
        );
    }
}
