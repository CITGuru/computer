use super::{API_URL, build, wire};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::sandboxes::remote::{self, RemoteApi, Sandbox, SandboxPlan};
use async_trait::async_trait;
use computer_types::{Arch, Capabilities, Environment, PortReach, Resources, Start};
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const TOKEN_ENV: &str = "VERCEL_TOKEN";
pub const TEAM_ENV: &str = "VERCEL_TEAM_ID";
pub const PROJECT_ENV: &str = "VERCEL_PROJECT_ID";
pub const TEAM_SLUG_ENV: &str = "VERCEL_TEAM_SLUG";

pub const TIMEOUT: Duration = Duration::from_secs(150);

pub const IMAGE_WAIT: Duration = Duration::from_secs(10 * 60);

pub const MOST_LIFETIME: Duration = Duration::from_secs(45 * 60);

pub struct Cloud {
    token: String,
    team: Option<String>,
    team_slug: Option<String>,
    project: String,
    base: String,
    http: Client,
    sessions: Mutex<BTreeMap<String, String>>,
}

impl Cloud {
    pub fn new(
        token: impl Into<String>,
        team: Option<String>,
        project: impl Into<String>,
    ) -> Result<Self> {
        let http = Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| Error::transport(error.to_string(), false))?;

        Ok(Self {
            token: token.into(),
            team: team.filter(|team| !team.is_empty()),
            team_slug: None,
            project: project.into(),
            base: API_URL.to_string(),
            http,
            sessions: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn from_env() -> Result<Self> {
        let needed = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| Error::Unavailable {
                    provider: "vercel".to_string(),
                    detail: format!("{name} is not set"),
                })
        };

        let cloud = Self::new(
            needed(TOKEN_ENV)?,
            std::env::var(TEAM_ENV).ok(),
            needed(PROJECT_ENV)?,
        )?;

        Ok(match std::env::var(TEAM_SLUG_ENV) {
            Ok(slug) => cloud.team_slug(slug),
            Err(_) => cloud,
        })
    }

    pub fn team_slug(mut self, slug: impl Into<String>) -> Self {
        self.team_slug = Some(slug.into()).filter(|slug| !slug.is_empty());
        self
    }

    pub fn at(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);

        match &self.team {
            Some(team) => request.query(&[("teamId", team)]),
            None => request,
        }
    }

    fn named(&self, method: Method, name: &str) -> RequestBuilder {
        self.request(method, &format!("/v2/sandboxes/{}", escape(name)))
            .query(&[("projectId", &self.project)])
    }

    fn in_session(&self, method: Method, session: &str, path: &str) -> RequestBuilder {
        self.request(method, &format!("/v2/sandboxes/sessions/{session}{path}"))
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

    fn remember(&self, name: &str, session: String) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(name.to_string(), session);
        }
    }

    fn forget(&self, name: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(name);
        }
    }

    async fn described(&self, name: &str) -> Result<Option<Value>> {
        let response = self.send(self.named(Method::GET, name)).await?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let bytes = self.body(response).await?;
        let answer: Value = serde_json::from_slice(&bytes).map_err(|error| {
            Error::transport(format!("an answer that is not JSON: {error}"), false)
        })?;

        Ok(wire::live(&answer).then_some(answer))
    }

    async fn session(&self, name: &str) -> Result<String> {
        if let Some(session) = self
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(name).cloned())
        {
            return Ok(session);
        }

        let answer = self
            .described(name)
            .await?
            .ok_or_else(|| Error::Gone(name.to_string()))?;
        let (_, session) = wire::sandbox_from(&answer)?;

        self.remember(name, session.clone());
        Ok(session)
    }
}

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

fn from_status(status: StatusCode, detail: &str) -> Error {
    let detail = detail.trim().to_string();

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Error::denied(format!("vercel refused the token: {detail}"))
        }
        StatusCode::NOT_FOUND | StatusCode::GONE => Error::Gone(detail),
        StatusCode::TOO_MANY_REQUESTS
        | StatusCode::BAD_GATEWAY
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT => Error::transport(format!("{status}: {detail}"), true),
        _ => Error::transport(format!("{status}: {detail}"), false),
    }
}

fn escape(segment: &str) -> String {
    segment
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn tarred(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let failed = |error: std::io::Error| Error::transport(format!("a tar: {error}"), false);
    let mtime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();

    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(gzip);
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(mtime);
        archive
            .append_data(&mut header, path.trim_start_matches('/'), bytes.as_slice())
            .map_err(failed)?;
    }

    archive
        .into_inner()
        .and_then(|gzip| gzip.finish())
        .map_err(failed)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[async_trait]
impl RemoteApi for Cloud {
    fn vendor(&self) -> &str {
        "vercel"
    }

    fn environment(&self) -> Environment {
        Environment::MicroVm(serde_json::json!({ "hypervisor": "firecracker" }))
    }

    fn can(&self) -> Capabilities {
        Capabilities {
            start: Start::AfterEveryStart,
            reach: PortReach::VendorUrl,
            resources: Resources::AtCreate,
            max_lifetime_secs: Some(MOST_LIFETIME.as_secs()),
            ports: Some(super::MOST_PORTS as u32),
            arch: vec![Arch::Amd64],
            ..Capabilities::default()
        }
    }

    fn sweepable(&self) -> bool {
        true
    }

    async fn available(&self) -> Result<()> {
        let response = self
            .send(
                self.request(Method::GET, "/v2/sandboxes")
                    .query(&[("project", self.project.as_str()), ("limit", "1")]),
            )
            .await?;

        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::Unavailable {
                provider: "vercel".to_string(),
                detail: format!("{TOKEN_ENV} was refused for this team and project"),
            }),
            status => Err(Error::Unavailable {
                provider: "vercel".to_string(),
                detail: status.to_string(),
            }),
        }
    }

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox> {
        let body = wire::new_sandbox(&self.project, plan)?.to_string();
        let deadline = Instant::now() + IMAGE_WAIT;

        let answer = loop {
            let response = self
                .send(
                    self.request(Method::POST, "/v3/sandboxes")
                        .header(CONTENT_TYPE, "application/json")
                        .body(body.clone()),
                )
                .await?;

            let status = response.status();
            let bytes = response
                .bytes()
                .await
                .map_err(|error| Error::transport(error.to_string(), true))?;
            let text = String::from_utf8_lossy(&bytes);

            if status.is_success() {
                break serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                    Error::transport(format!("an answer that is not JSON: {error}"), false)
                })?;
            }

            if !wire::image_not_ready(&text) {
                return Err(from_status(status, &text));
            }

            if Instant::now() >= deadline {
                return Err(Error::Timeout {
                    after: IMAGE_WAIT,
                    detail: format!("{} was still being prepared", plan.image),
                });
            }

            tracing::info!(image = %plan.image, "vercel is still preparing the image");
            tokio::time::sleep(Duration::from_secs(10)).await;
        };

        let (sandbox, session) = wire::sandbox_from(&answer)?;
        self.remember(&sandbox.id, session);
        Ok(sandbox)
    }

    async fn find(&self, name: &str) -> Result<Option<Sandbox>> {
        let Some(answer) = self.described(name).await? else {
            return Ok(None);
        };

        let (sandbox, session) = wire::sandbox_from(&answer)?;
        self.remember(name, session);
        Ok(Some(sandbox))
    }

    async fn kill(&self, id: &str) -> Result<()> {
        let response = self.send(self.named(Method::DELETE, id)).await?;
        self.forget(id);

        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Ok(()),
            status => Err(from_status(status, "")),
        }
    }

    async fn keep_alive(&self, id: &str, ttl: Duration) -> Result<()> {
        let session = self.session(id).await?;
        let answer = self
            .json(self.in_session(Method::GET, &session, ""))
            .await?;

        let Some(short) = wire::shortfall(&answer, ttl, now_ms()) else {
            return Ok(());
        };

        let request = self
            .in_session(Method::POST, &session, "/extend-timeout")
            .header(CONTENT_TYPE, "application/json")
            .body(serde_json::json!({ "duration": short }).to_string());

        self.body(self.send(request).await?).await.map(|_| ())
    }

    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>> {
        let mut found = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut request = self
                .request(Method::GET, "/v2/sandboxes")
                .query(&[("project", self.project.as_str()), ("limit", "100")]);
            if let Some(cursor) = &cursor {
                request = request.query(&[("cursor", cursor)]);
            }

            let listing = self.json(request).await?;
            found.extend(wire::carrying(&listing, key));

            match wire::next_page(&listing) {
                Some(next) => cursor = Some(next),
                None => return Ok(found),
            }
        }
    }

    async fn exec(
        &self,
        sandbox: &Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        let session = self.session(&sandbox.id).await?;
        let request = self
            .in_session(Method::POST, &session, "/cmd")
            .header(CONTENT_TYPE, "application/json")
            .body(wire::command(argv, env)?.to_string());

        let bytes = self.body(self.send(request).await?).await?;
        wire::parse_command(&bytes)
    }

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>> {
        let session = self.session(&sandbox.id).await?;
        let request = self
            .in_session(Method::POST, &session, "/fs/read")
            .header(CONTENT_TYPE, "application/json")
            .body(serde_json::json!({ "path": path }).to_string());

        self.body(self.send(request).await?).await
    }

    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()> {
        let session = self.session(&sandbox.id).await?;
        let request = self
            .in_session(Method::POST, &session, "/fs/write")
            .header(CONTENT_TYPE, "application/gzip")
            .header("x-cwd", "/")
            .body(tarred(&[(path.to_string(), bytes.to_vec())])?);

        self.body(self.send(request).await?).await.map(|_| ())
    }

    async fn ensure_image(&self, config: &Config) -> Result<Option<String>> {
        let Some(bundle) = config.bundle.as_ref().filter(|b| b.owns(&config.image)) else {
            return Ok(None);
        };

        if !config.extras.is_empty() {
            return Err(Error::Unsupported {
                gaps: vec!["packages in an image this crate does not build"],
            });
        }

        let tag = build::tag(bundle, &config.extras);
        let (slug, project) = self.registry().await?;
        let reference = build::reference(&slug, &project, bundle.name, &tag);
        let short = format!("{}:{tag}", bundle.name);

        if self.pushed(&slug, &project, bundle.name, &tag).await? {
            return Ok(Some(short));
        }

        tracing::info!(image = %reference, "building the image in a vercel sandbox");
        self.build(bundle, &reference).await?;
        tracing::info!(image = %reference, "the image is pushed");

        Ok(Some(short))
    }
}

impl Cloud {
    fn registry_login(&self) -> Result<String> {
        let team = self.team.as_deref().ok_or_else(|| Error::Unavailable {
            provider: "vercel".to_string(),
            detail: format!("the registry takes the team as its user: set {TEAM_ENV}"),
        })?;

        Ok(crate::cdp::base64_encode(
            format!("{team}:{}", self.token).as_bytes(),
        ))
    }

    async fn registry(&self) -> Result<(String, String)> {
        let project = self
            .json(self.request(
                Method::GET,
                &format!("/v9/projects/{}", escape(&self.project)),
            ))
            .await
            .ok()
            .and_then(|answer| build::name_of(&answer))
            .ok_or_else(|| Error::Unavailable {
                provider: "vercel".to_string(),
                detail: format!("the name of project {} could not be read", self.project),
            })?;

        if let Some(slug) = &self.team_slug {
            return Ok((slug.clone(), project));
        }

        let team = self.team.as_deref().unwrap_or_default();
        let slug = self
            .json(self.request(Method::GET, &format!("/v2/teams/{}", escape(team))))
            .await
            .ok()
            .and_then(|answer| build::slug_of(&answer))
            .ok_or_else(|| Error::Unavailable {
                provider: "vercel".to_string(),
                detail: format!(
                    "this token cannot read the team's slug, which the registry path                      needs: set {TEAM_SLUG_ENV}"
                ),
            })?;

        Ok((slug, project))
    }

    async fn pushed(&self, slug: &str, project: &str, repository: &str, tag: &str) -> Result<bool> {
        let response = self
            .send(
                self.http
                    .head(build::manifest_url(slug, project, repository, tag))
                    .header("Authorization", format!("Basic {}", self.registry_login()?))
                    .header(
                        "Accept",
                        "application/vnd.oci.image.index.v1+json, \
                         application/vnd.oci.image.manifest.v1+json, \
                         application/vnd.docker.distribution.manifest.v2+json",
                    ),
            )
            .await?;

        match response.status() {
            status if status.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(from_status(status, "the registry")),
        }
    }

    async fn build(&self, bundle: &crate::bundle::Bundle, reference: &str) -> Result<()> {
        let name = format!("computer-build-{}-{:x}", std::process::id(), now_ms());

        let answer = self
            .json(
                self.request(Method::POST, "/v3/sandboxes")
                    .header(CONTENT_TYPE, "application/json")
                    .body(build::builder(&self.project, &name, reference).to_string()),
            )
            .await?;
        let (builder, session) = wire::sandbox_from(&answer)?;

        let built = self.build_in(&session, bundle, reference).await;

        if let Err(error) = self.kill(&builder.id).await {
            tracing::warn!(sandbox = %builder.id, %error, "the builder sandbox was left to its deadline");
        }

        built
    }

    async fn build_in(
        &self,
        session: &str,
        bundle: &crate::bundle::Bundle,
        reference: &str,
    ) -> Result<()> {
        let upload = self
            .in_session(Method::POST, session, "/fs/write")
            .header(CONTENT_TYPE, "application/gzip")
            .header("x-cwd", "/")
            .body(tarred(&build::context(bundle))?);
        self.body(self.send(upload).await?).await?;

        let run = self
            .in_session(Method::POST, session, "/cmd")
            .header(CONTENT_TYPE, "application/json")
            .timeout(build::BUILD_WAIT)
            .body(build::command(&self.registry_login()?, reference).to_string());

        let result = wire::parse_stream(&self.body(self.send(run).await?).await?)?;
        if result.code != 0 {
            return Err(Error::Failed {
                code: result.code,
                stderr: build::tail(
                    &format!("{}{}", result.stdout_utf8(), result.stderr_utf8()),
                    40,
                ),
            });
        }

        Ok(())
    }
}

pub fn pair_from_env() -> Result<(remote::RemoteMachine, std::sync::Arc<remote::RemoteProfile>)> {
    Ok(remote::pair(
        std::sync::Arc::new(Cloud::from_env()?),
        std::sync::Arc::new(crate::X11Profile),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn test_every_status_says_what_the_caller_does_next() {
        assert!(!from_status(StatusCode::FORBIDDEN, "nope").needs_another_place());
        assert!(from_status(StatusCode::NOT_FOUND, "gone").needs_another_place());
        assert!(from_status(StatusCode::TOO_MANY_REQUESTS, "slow down").retryable());
        assert!(!from_status(StatusCode::BAD_REQUEST, "bad").retryable());
    }

    #[test]
    fn test_a_written_file_lands_at_its_path_under_the_root() {
        let packed =
            tarred(&[("/tmp/deep/one.bin".to_string(), b"\x00\xff".to_vec())]).expect("a tar");

        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(packed.as_slice()));
        let mut entries = archive.entries().expect("entries");
        let mut entry = entries.next().expect("one").expect("an entry");

        assert_eq!(
            entry.path().expect("a path").to_str(),
            Some("tmp/deep/one.bin"),
            "relative, because the archive is unpacked under x-cwd"
        );
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("read");
        assert_eq!(bytes, b"\x00\xff");
    }

    #[test]
    fn test_a_name_cannot_leave_its_path_segment() {
        assert_eq!(escape("desk-1"), "desk-1");
        assert_eq!(escape("a/b?c"), "a%2Fb%3Fc");
    }
}
