//! E2B over its own HTTP API.
//!
//! The only part of this vendor behind a feature, and the only part that needs
//! a client: the control plane is `https` on somebody else's host, so there is
//! no shelling out to a command that already knows how to reach it.
//!
//! Two planes, two hosts, two credentials. The control plane creates, lists,
//! kills and extends, and takes an API key. `envd` inside the sandbox runs
//! commands and moves files, and takes the token the control plane handed back
//! when the sandbox was created.

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

/// The key the control plane authenticates with.
pub const API_KEY_ENV: &str = "E2B_API_KEY";
/// The domain sandboxes live on, for a deployment that is not `e2b.app`.
pub const DOMAIN_ENV: &str = "E2B_DOMAIN";

const API_KEY_HEADER: &str = "X-API-Key";
/// What envd authenticates a data-plane call with. Lowercase because
/// `HeaderName::from_static` takes it that way; the wire does not care.
const ENVD_TOKEN_HEADER: &str = "x-access-token";
/// What the proxy in front of a secure sandbox demands. A browser cannot send
/// it, which is why a secure sandbox has no viewer URL.
const TRAFFIC_TOKEN_HEADER: &str = "e2b-traffic-access-token";

const CONNECT_JSON: &str = "application/connect+json";
const CONNECT_VERSION: &str = "Connect-Protocol-Version";

/// How long any one exchange may take.
///
/// Above [`crate::machine::DEFAULT_TIMEOUT`], so the bound a caller sees is the
/// one the runner applies rather than one buried in a client.
pub const TIMEOUT: Duration = Duration::from_secs(150);

pub struct Cloud {
    key: String,
    domain: String,
    http: Client,
    /// Who envd runs commands as. An account that is not in the template
    /// answers every exec with a refusal, so it is named rather than assumed.
    user: String,
    /// A command that kills a sandbox from `Drop`, where the caller named one.
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

    /// The key from the environment, as every E2B tool reads it.
    pub fn from_env() -> Result<Self> {
        let key = std::env::var(API_KEY_ENV).map_err(|_| Error::Unavailable {
            runtime: "e2b".to_string(),
            detail: format!("{API_KEY_ENV} is not set"),
        })?;

        let domain = std::env::var(DOMAIN_ENV).unwrap_or_else(|_| DEFAULT_DOMAIN.to_string());
        Self::at(key, domain)
    }

    /// Run commands as this user instead of [`DEFAULT_USER`].
    ///
    /// For a template built on a base that has its own account — E2B's own
    /// images use `user` — rather than on the one this crate carries.
    pub fn as_user(mut self, user: impl Into<String>) -> Self {
        self.user = user.into();
        self
    }

    /// Kill a dropped box with this command, `{}` standing for the sandbox ID.
    ///
    /// Unset, a dropped handle leaves the sandbox running until its deadline.
    /// That is survivable here in a way it is not on a hypervisor — the
    /// deadline is what stops a leak — so it is the default rather than a
    /// dependency on a CLI being installed.
    pub fn reaping_with<I, S>(mut self, program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.reaper = Some((program.into(), args.into_iter().map(Into::into).collect()));
        self
    }

    /// `e2b sandbox kill <id>`, for a host with the CLI on its path.
    pub fn reaping_with_cli(self) -> Self {
        self.reaping_with("e2b", ["sandbox", "kill", "{}"])
    }

    fn control(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", api_url(&self.domain)))
            .header(API_KEY_HEADER, &self.key)
    }

    /// A data-plane request, carrying both tokens a secure sandbox wants.
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

    /// The body of an answer, or the status turned into the right refusal.
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

    /// Every sandbox the control plane will list.
    ///
    /// The metadata filter is sent as a hint and the answer is filtered again
    /// in [`wire::carrying`]: the parameter is one encoded string, and a
    /// filter the server does not understand answers with everything.
    async fn listing(&self, key: &str, value: Option<&str>) -> Result<Value> {
        let query = value
            .map(|value| format!("?metadata={}", wire::metadata_query(key, value)))
            .unwrap_or_default();

        self.json(self.control(Method::GET, &format!("/v2/sandboxes{query}")))
            .await
    }
}

/// An error and everything under it, as one line.
///
/// `reqwest` reports "error sending request for url (…)" and keeps what
/// actually happened — a reset, a closed connection, a name that would not
/// resolve — one `source()` down. On its own that message sends a caller
/// nowhere.
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

/// A status, as the variant that says what the caller does next.
fn from_status(status: StatusCode, detail: &str) -> Error {
    let detail = detail.trim().to_string();

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Error::denied(format!("e2b refused the key: {detail}"))
        }
        StatusCode::NOT_FOUND => Error::Gone(detail),
        // A sandbox past its deadline answers through the proxy as a bad
        // gateway, which is gone rather than broken.
        StatusCode::BAD_GATEWAY => Error::Gone(detail),
        StatusCode::TOO_MANY_REQUESTS
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT => Error::transport(format!("{status}: {detail}"), true),
        _ => Error::transport(format!("{status}: {detail}"), false),
    }
}

/// A boundary no payload will contain.
///
/// The counter matters more than the clock: two uploads in the same
/// nanosecond are one call apart, not one tick.
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

        // Health is unauthenticated, so a bad key would otherwise arrive at
        // the first create as a sandbox that would not start.
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

        // A listing reports IDs and metadata and no credentials, so nothing
        // found that way could be driven. Connect answers with both tokens,
        // and resumes a sandbox that was paused.
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

        // Already gone is the outcome asked for.
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

/// A machine and its profile, on the key in the environment.
///
/// The whole wiring for the common case: `E2B_API_KEY`, the built-in X11
/// image, and a sandbox that is secure — so the desktop is driveable from here
/// and has no viewer URL. Call
/// [`public_viewer`](super::machine::E2bMachine::public_viewer) on the machine
/// to trade that.
pub fn pair_from_env() -> Result<(super::E2bMachine, std::sync::Arc<super::E2bProfile>)> {
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
