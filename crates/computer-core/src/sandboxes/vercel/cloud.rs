use super::{API_URL, build, oidc, wire};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::sandboxes::remote::{self, NAME_KEY, RemoteApi, Sandbox, SandboxPlan};
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

pub const STOP_WAIT: Duration = Duration::from_secs(120);

enum Credential {
    Token {
        token: String,
        team: Option<String>,
        project: String,
    },
    Oidc,
}

struct Access {
    token: String,
    team: Option<String>,
    project: String,
    named: Option<(String, String)>,
}

pub struct Cloud {
    credential: Credential,
    team_slug: Option<String>,
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
        Self::with(Credential::Token {
            token: token.into(),
            team: team.filter(|team| !team.is_empty()),
            project: project.into(),
        })
    }

    pub fn oidc() -> Result<Self> {
        Self::with(Credential::Oidc)
    }

    fn with(credential: Credential) -> Result<Self> {
        let http = Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| Error::transport(error.to_string(), false))?;

        Ok(Self {
            credential,
            team_slug: None,
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

        let cloud = match needed(TOKEN_ENV) {
            Ok(token) => Self::new(token, std::env::var(TEAM_ENV).ok(), needed(PROJECT_ENV)?)?,
            Err(_) if oidc::possible() => Self::oidc()?,
            Err(_) => {
                return Err(Error::Unavailable {
                    provider: "vercel".to_string(),
                    detail: format!(
                        "neither {TOKEN_ENV} nor {} is set, and this is not a Vercel function",
                        oidc::TOKEN_ENV
                    ),
                });
            }
        };

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

    fn access(&self) -> Result<Access> {
        match &self.credential {
            Credential::Token {
                token,
                team,
                project,
            } => Ok(Access {
                token: token.clone(),
                team: team.clone(),
                project: project.clone(),
                named: None,
            }),
            Credential::Oidc => oidc::current()
                .map(|oidc| Access {
                    named: oidc.team_slug.zip(oidc.project_name),
                    token: oidc.token,
                    team: Some(oidc.team),
                    project: oidc.project,
                })
                .ok_or_else(|| Error::Unavailable {
                    provider: "vercel".to_string(),
                    detail: format!(
                        "no OIDC token has arrived yet: Vercel sends one in {} on each request",
                        oidc::HEADER
                    ),
                }),
        }
    }

    fn project(&self) -> Result<String> {
        self.access().map(|access| access.project)
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let access = self.access()?;
        let request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&access.token);

        Ok(match &access.team {
            Some(team) => request.query(&[("teamId", team)]),
            None => request,
        })
    }

    fn named(&self, method: Method, name: &str) -> Result<RequestBuilder> {
        Ok(self
            .request(method, &format!("/v2/sandboxes/{}", escape(name)))?
            .query(&[("projectId", &self.project()?)]))
    }

    fn in_session(&self, method: Method, session: &str, path: &str) -> Result<RequestBuilder> {
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

    async fn snapshots(&self, name: &str) -> Result<Vec<String>> {
        let listing = self
            .json(
                self.request(Method::GET, "/v2/sandboxes/snapshots")?
                    .query(&[("project", self.project()?.as_str()), ("name", name)]),
            )
            .await?;

        Ok(wire::snapshot_ids(&listing))
    }

    async fn described(&self, name: &str) -> Result<Option<Value>> {
        let response = self.send(self.named(Method::GET, name)?).await?;

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

fn tarred(files: &[build::File]) -> Result<Vec<u8>> {
    let failed = |error: std::io::Error| Error::transport(format!("a tar: {error}"), false);
    let mtime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();

    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(gzip);
    for build::File { path, bytes, mode } in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(*mode);
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
            ports: Some(super::MOST_PORTS as u32),
            arch: vec![Arch::Amd64],
            stop: true,
            ..Capabilities::default()
        }
    }

    fn sweepable(&self) -> bool {
        true
    }

    async fn available(&self) -> Result<()> {
        let project = self.project()?;
        let response = self
            .send(
                self.request(Method::GET, "/v2/sandboxes")?
                    .query(&[("project", project.as_str()), ("limit", "1")]),
            )
            .await?;

        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::Unavailable {
                provider: "vercel".to_string(),
                detail: "the token was refused for this team and project".to_string(),
            }),
            status => Err(Error::Unavailable {
                provider: "vercel".to_string(),
                detail: status.to_string(),
            }),
        }
    }

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox> {
        let body = wire::new_sandbox(&self.project()?, plan)?.to_string();
        let deadline = Instant::now() + IMAGE_WAIT;

        let answer = loop {
            let response = self
                .send(
                    self.request(Method::POST, "/v3/sandboxes")?
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

            if wire::image_missing(&text) {
                return Err(Error::Unavailable {
                    provider: "vercel".to_string(),
                    detail: format!(
                        "{} is not in Vercel Container Registry for this project. Push a \
                         linux/amd64 build of the desktop image there, or name no image \
                         and it is built for you",
                        plan.image
                    ),
                });
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

        if wire::untagged(&plan.metadata)
            && let Err(error) = self.write_labels(&session, &plan.metadata).await
        {
            let _ = self.kill(&sandbox.id).await;
            return Err(error);
        }

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
        let snapshots = self.snapshots(id).await.unwrap_or_default();
        let response = self.send(self.named(Method::DELETE, id)?).await?;
        self.forget(id);

        match response.status() {
            status if status.is_success() || status == StatusCode::NOT_FOUND => {}
            status => return Err(from_status(status, "")),
        }

        for snapshot in snapshots {
            let deleted = self
                .send(self.request(
                    Method::DELETE,
                    &format!("/v2/sandboxes/snapshots/{}", escape(&snapshot)),
                )?)
                .await;
            if let Err(error) = deleted {
                tracing::warn!(sandbox = %id, %snapshot, %error, "a snapshot was not removed, and it is still paid for");
            }
        }
        Ok(())
    }

    fn persists(&self) -> bool {
        true
    }

    async fn stop(&self, id: &str) -> Result<()> {
        let session = self.session(id).await?;
        self.json(
            self.in_session(Method::POST, &session, "/stop")?
                .header(CONTENT_TYPE, "application/json")
                .body("{}"),
        )
        .await?;
        self.forget(id);

        let deadline = Instant::now() + STOP_WAIT;
        loop {
            let answer = self.json(self.named(Method::GET, id)?).await?;
            let status = answer
                .pointer("/sandbox/status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if status == "stopped" {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout {
                    after: STOP_WAIT,
                    detail: format!("{id} was still {status}"),
                });
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn start(&self, id: &str) -> Result<Sandbox> {
        let deadline = Instant::now() + STOP_WAIT;

        loop {
            let answer = self
                .json(self.named(Method::GET, id)?.query(&[("resume", "true")]))
                .await?;
            let status = answer
                .pointer("/session/status")
                .or_else(|| answer.pointer("/sandbox/status"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            if status == "running" {
                let (sandbox, session) = wire::sandbox_from(&answer)?;
                self.remember(id, session);
                return Ok(sandbox);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout {
                    after: STOP_WAIT,
                    detail: format!("{id} was still {status}"),
                });
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn keep_alive(&self, id: &str, ttl: Duration) -> Result<()> {
        let session = self.session(id).await?;
        let answer = self
            .json(self.in_session(Method::GET, &session, "")?)
            .await?;

        let Some(short) = wire::shortfall(&answer, ttl, now_ms()) else {
            return Ok(());
        };

        let request = self
            .in_session(Method::POST, &session, "/extend-timeout")?
            .header(CONTENT_TYPE, "application/json")
            .body(serde_json::json!({ "duration": short }).to_string());

        self.body(self.send(request).await?).await.map(|_| ())
    }

    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>> {
        let project = self.project()?;
        let mut found = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut request = self
                .request(Method::GET, "/v2/sandboxes")?
                .query(&[("project", project.as_str()), ("limit", "50")]);
            if let Some(cursor) = &cursor {
                request = request.query(&[("cursor", cursor)]);
            }

            let listing = self.json(request).await?;
            for listed in wire::listed(&listing) {
                if !listed.tags.contains_key(NAME_KEY) {
                    continue;
                }

                if let Some(value) = listed.tags.get(key) {
                    found.push((listed.name, value.clone()));
                    continue;
                }

                let Some(session) = &listed.session else {
                    continue;
                };
                match self.read_labels(session).await {
                    Ok(labels) => {
                        if let Some(value) = labels.get(key) {
                            found.push((listed.name, value.clone()));
                        }
                    }
                    Err(error) => {
                        tracing::warn!(sandbox = %listed.name, %error, "its labels could not be read")
                    }
                }
            }

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
            .in_session(Method::POST, &session, "/cmd")?
            .header(CONTENT_TYPE, "application/json")
            .body(wire::command(argv, env)?.to_string());

        let bytes = self.body(self.send(request).await?).await?;
        wire::parse_command(&bytes)
    }

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>> {
        let session = self.session(&sandbox.id).await?;
        let request = self
            .in_session(Method::POST, &session, "/fs/read")?
            .header(CONTENT_TYPE, "application/json")
            .body(serde_json::json!({ "path": path }).to_string());

        self.body(self.send(request).await?)
            .await
            .map_err(|error| match error {
                Error::Gone(detail) => Error::Failed {
                    code: 1,
                    stderr: detail,
                },
                other => other,
            })
    }

    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()> {
        let session = self.session(&sandbox.id).await?;
        let request = self
            .in_session(Method::POST, &session, "/fs/write")?
            .header(CONTENT_TYPE, "application/gzip")
            .header("x-cwd", "/")
            .body(tarred(&[build::File::new(path, bytes.to_vec())])?);

        self.body(self.send(request).await?).await.map(|_| ())
    }

    async fn ensure_image(&self, config: &Config) -> Result<Option<String>> {
        let context = match (&config.bundle, &config.image_dir) {
            (Some(bundle), _) if bundle.owns(&config.image) => {
                build::bundled(bundle, &config.extras)
            }
            (_, Some(directory)) => build::directory(directory, &config.image)?,
            _ => return Ok(None),
        };
        let context = build::with_extras(context, &config.extras);

        let (slug, project) = self.registry().await?;
        let reference = build::reference(&slug, &project, &context.repository, &context.tag);
        let short = format!("{}:{}", context.repository, context.tag);

        if self
            .pushed(&slug, &project, &context.repository, &context.tag)
            .await?
        {
            return Ok(Some(short));
        }

        tracing::info!(image = %reference, "building the image in a vercel sandbox");
        self.build(&context, &reference).await?;
        tracing::info!(image = %reference, "the image is pushed");

        Ok(Some(short))
    }
}

impl Cloud {
    async fn write_labels(&self, session: &str, labels: &BTreeMap<String, String>) -> Result<()> {
        let bytes = serde_json::to_vec(labels)
            .map_err(|error| Error::transport(format!("labels: {error}"), false))?;
        let request = self
            .in_session(Method::POST, session, "/fs/write")?
            .header(CONTENT_TYPE, "application/gzip")
            .header("x-cwd", "/")
            .body(tarred(&[build::File::new(super::LABELS_PATH, bytes)])?);

        self.body(self.send(request).await?).await.map(|_| ())
    }

    async fn read_labels(&self, session: &str) -> Result<BTreeMap<String, String>> {
        let response = self
            .send(
                self.in_session(Method::POST, session, "/fs/read")?
                    .header(CONTENT_TYPE, "application/json")
                    .body(serde_json::json!({ "path": super::LABELS_PATH }).to_string()),
            )
            .await?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(BTreeMap::new());
        }

        Ok(wire::labels(&self.body(response).await?))
    }

    fn registry_login(&self) -> Result<String> {
        let access = self.access()?;
        let team = access.team.ok_or_else(|| Error::Unavailable {
            provider: "vercel".to_string(),
            detail: format!("the registry takes the team as its user: set {TEAM_ENV}"),
        })?;

        Ok(crate::cdp::base64_encode(
            format!("{team}:{}", access.token).as_bytes(),
        ))
    }

    async fn registry(&self) -> Result<(String, String)> {
        let access = self.access()?;
        if let Some(named) = access.named {
            return Ok(named);
        }

        let project = self
            .json(self.request(
                Method::GET,
                &format!("/v9/projects/{}", escape(&access.project)),
            )?)
            .await
            .ok()
            .and_then(|answer| build::name_of(&answer))
            .ok_or_else(|| Error::Unavailable {
                provider: "vercel".to_string(),
                detail: format!("the name of project {} could not be read", access.project),
            })?;

        if let Some(slug) = &self.team_slug {
            return Ok((slug.clone(), project));
        }

        let team = access.team.unwrap_or_default();
        let slug = self
            .json(self.request(Method::GET, &format!("/v2/teams/{}", escape(&team)))?)
            .await
            .ok()
            .and_then(|answer| build::slug_of(&answer))
            .ok_or_else(|| Error::Unavailable {
                provider: "vercel".to_string(),
                detail: format!(
                    "this token cannot read the team's slug, which the registry path needs: set {TEAM_SLUG_ENV}"
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

    async fn build(&self, context: &build::Context, reference: &str) -> Result<()> {
        let name = format!("computer-build-{}-{:x}", std::process::id(), now_ms());

        let answer = self
            .json(
                self.request(Method::POST, "/v3/sandboxes")?
                    .header(CONTENT_TYPE, "application/json")
                    .body(build::builder(&self.project()?, &name, reference).to_string()),
            )
            .await?;
        let (builder, session) = wire::sandbox_from(&answer)?;

        let built = self.build_in(&session, context, reference).await;

        if let Err(error) = self.kill(&builder.id).await {
            tracing::warn!(sandbox = %builder.id, %error, "the builder sandbox was left to its deadline");
        }

        built
    }

    async fn build_in(
        &self,
        session: &str,
        context: &build::Context,
        reference: &str,
    ) -> Result<()> {
        let upload = self
            .in_session(Method::POST, session, "/fs/write")?
            .header(CONTENT_TYPE, "application/gzip")
            .header("x-cwd", "/")
            .body(tarred(&context.files)?);
        self.body(self.send(upload).await?).await?;

        let run = self
            .in_session(Method::POST, session, "/cmd")?
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
            tarred(&[build::File::new("/tmp/deep/one.bin", b"\x00\xff".to_vec())]).expect("a tar");

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
