use super::{API_URL, wire};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::sandboxes::context::{self, dockerfile};
use crate::sandboxes::e2b::wire::{escape, multipart};
use crate::sandboxes::remote::{self, RemoteApi, Sandbox, SandboxPlan};
use async_trait::async_trait;
use holm_types::{Arch, Capabilities, Environment, PortReach, Resources, Start};
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const KEY_ENV: &str = "DAYTONA_API_KEY";
pub const URL_ENV: &str = "DAYTONA_API_URL";
pub const TARGET_ENV: &str = "DAYTONA_TARGET";

pub const TIMEOUT: Duration = Duration::from_secs(150);

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(140);

pub const START_WAIT: Duration = Duration::from_secs(20 * 60);

pub const SIGNED_FOR: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Cloud {
    key: String,
    base: String,
    target: Option<String>,
    http: Client,
    toolboxes: Mutex<BTreeMap<String, String>>,
    builds: Mutex<BTreeMap<String, String>>,
}

impl Cloud {
    pub fn new(key: impl Into<String>) -> Result<Self> {
        let http = Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| Error::transport(error.to_string(), false))?;

        Ok(Self {
            key: key.into(),
            base: API_URL.to_string(),
            target: None,
            http,
            toolboxes: Mutex::new(BTreeMap::new()),
            builds: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn from_env() -> Result<Self> {
        let key = std::env::var(KEY_ENV)
            .ok()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| Error::Unavailable {
                provider: "daytona".to_string(),
                detail: format!("{KEY_ENV} is not set"),
            })?;

        let mut cloud = Self::new(key)?;
        if let Ok(base) = std::env::var(URL_ENV) {
            cloud = cloud.at(base);
        }
        if let Ok(target) = std::env::var(TARGET_ENV) {
            cloud = cloud.target(target);
        }
        Ok(cloud)
    }

    pub fn at(mut self, base: impl Into<String>) -> Self {
        self.base = base.into().trim_end_matches('/').to_string();
        self
    }

    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into()).filter(|target| !target.is_empty());
        self
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.key)
    }

    fn toolbox(&self, method: Method, id: &str, path: &str) -> Result<RequestBuilder> {
        let base = self
            .toolboxes
            .lock()
            .ok()
            .and_then(|toolboxes| toolboxes.get(id).cloned())
            .ok_or_else(|| Error::Gone(id.to_string()))?;

        Ok(self
            .http
            .request(method, format!("{base}{path}"))
            .bearer_auth(&self.key))
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
        wire::parse(&self.body(self.send(request).await?).await?)
    }

    fn remember(&self, sandbox: &Value) -> Result<String> {
        let id = wire::id_of(sandbox)?;
        let toolbox = wire::toolbox_of(sandbox)
            .ok_or_else(|| Error::transport(format!("{id} came back with no toolbox"), false))?;

        if let Ok(mut toolboxes) = self.toolboxes.lock() {
            toolboxes.insert(id.clone(), toolbox);
        }
        Ok(id)
    }

    async fn described(&self, id_or_name: &str) -> Result<Option<Value>> {
        let response = self
            .send(self.request(Method::GET, &format!("/sandbox/{}", escape(id_or_name))))
            .await?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let sandbox = wire::parse(&self.body(response).await?)?;
        Ok(wire::LIVE
            .contains(&wire::state_of(&sandbox).as_str())
            .then_some(sandbox))
    }

    async fn started(&self, id: &str) -> Result<Value> {
        let deadline = Instant::now() + START_WAIT;

        loop {
            let sandbox = self
                .json(self.request(Method::GET, &format!("/sandbox/{id}")))
                .await?;
            let state = wire::state_of(&sandbox);

            if state == "started" {
                return Ok(sandbox);
            }
            if wire::FAILED.contains(&state.as_str()) {
                return Err(Error::Failed {
                    code: 1,
                    stderr: format!("{id} is {state}: {}", wire::why_of(&sandbox)),
                });
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout {
                    after: START_WAIT,
                    detail: format!("{id} was still {state}"),
                });
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn signed(&self, id: &str, ports: &[u16]) -> Result<Sandbox> {
        let mut sandbox = Sandbox::new(id);

        for port in ports {
            let answer = self
                .json(
                    self.request(
                        Method::GET,
                        &format!("/sandbox/{id}/ports/{port}/signed-preview-url"),
                    )
                    .query(&[("expiresInSeconds", SIGNED_FOR.as_secs())]),
                )
                .await?;

            let url = wire::url_of(&answer)
                .ok_or_else(|| Error::transport(format!("no URL for port {port}"), false))?;
            sandbox
                .endpoints
                .insert(*port, url.trim_end_matches('/').to_string());
        }

        Ok(sandbox)
    }

    async fn launched(&self, id: &str, publish: &[u16]) -> Result<Sandbox> {
        let sandbox = self.started(id).await?;
        self.remember(&sandbox)?;
        self.signed(id, publish).await
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
            Error::denied(format!("daytona refused the key: {detail}"))
        }
        StatusCode::NOT_FOUND | StatusCode::GONE => Error::Gone(detail),
        StatusCode::TOO_MANY_REQUESTS
        | StatusCode::BAD_GATEWAY
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT => Error::transport(format!("{status}: {detail}"), true),
        _ => Error::transport(format!("{status}: {detail}"), false),
    }
}

fn missing_is_a_failure(read: Result<Vec<u8>>) -> Result<Vec<u8>> {
    read.map_err(|error| match error {
        Error::Gone(detail) => Error::Failed {
            code: 1,
            stderr: detail,
        },
        other => other,
    })
}

fn boundary() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();

    format!("holm{:x}{:x}", nanos, NEXT.fetch_add(1, Ordering::Relaxed))
}

#[async_trait]
impl RemoteApi for Cloud {
    fn vendor(&self) -> &str {
        "daytona"
    }

    fn environment(&self) -> Environment {
        Environment::Container(serde_json::json!({ "vendor": "daytona" }))
    }

    fn can(&self) -> Capabilities {
        Capabilities {
            start: Start::AfterEveryStart,
            reach: PortReach::VendorUrl,
            resources: Resources::AtCreate,
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
                self.request(Method::GET, "/sandbox")
                    .query(&[("limit", "1")]),
            )
            .await?;

        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::Unavailable {
                provider: "daytona".to_string(),
                detail: format!("{KEY_ENV} was refused"),
            }),
            status => Err(Error::Unavailable {
                provider: "daytona".to_string(),
                detail: status.to_string(),
            }),
        }
    }

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox> {
        let built = self
            .builds
            .lock()
            .ok()
            .and_then(|builds| builds.get(&plan.image).cloned());
        let source = match &built {
            Some(content) => wire::Source::Dockerfile(content),
            None => wire::Source::Snapshot(&plan.image),
        };

        let mut body = wire::new_sandbox(plan, source)?;
        if let Some(target) = &self.target {
            body["target"] = serde_json::json!(target);
        }

        let answer = self
            .json(
                self.request(Method::POST, "/sandbox")
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.to_string()),
            )
            .await?;
        let id = wire::id_of(&answer)?;

        match self.launched(&id, &plan.publish).await {
            Ok(sandbox) => Ok(sandbox),
            Err(error) => {
                let _ = self.kill(&id).await;
                Err(error)
            }
        }
    }

    async fn find(&self, name: &str) -> Result<Option<Sandbox>> {
        let Some(sandbox) = self.described(name).await? else {
            return Ok(None);
        };
        let id = self.remember(&sandbox)?;

        let standard = crate::Profile::ports(&crate::X11Profile).to_publish();
        self.signed(&id, &standard).await.map(Some)
    }

    async fn kill(&self, id: &str) -> Result<()> {
        let response = self
            .send(self.request(Method::DELETE, &format!("/sandbox/{}", escape(id))))
            .await?;

        if let Ok(mut toolboxes) = self.toolboxes.lock() {
            toolboxes.remove(id);
        }

        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Ok(()),
            status => Err(from_status(status, "")),
        }
    }

    async fn keep_alive(&self, id: &str, _ttl: Duration) -> Result<()> {
        let request = self.request(Method::POST, &format!("/sandbox/{id}/last-activity"));
        self.body(self.send(request).await?).await.map(|_| ())
    }

    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>> {
        let mut found = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut request = self
                .request(Method::GET, "/sandbox")
                .query(&[("limit", "100")]);
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
        let request = self
            .toolbox(Method::POST, &sandbox.id, "/process/execute")?
            .header(CONTENT_TYPE, "application/json")
            .body(wire::command(argv, env, COMMAND_TIMEOUT)?.to_string());

        wire::parse_command(&self.json(request).await?)
    }

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>> {
        let request = self.toolbox(
            Method::GET,
            &sandbox.id,
            &format!("/files/download?path={}", escape(path)),
        )?;

        missing_is_a_failure(self.body(self.send(request).await?).await)
    }

    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()> {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());
        let boundary = boundary();

        let request = self
            .toolbox(
                Method::POST,
                &sandbox.id,
                &format!("/files/upload-v2?path={}", escape(path)),
            )?
            .header(
                CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(multipart(&name, bytes, &boundary));

        self.body(self.send(request).await?).await.map(|_| ())
    }

    async fn ensure_image(&self, config: &Config) -> Result<Option<String>> {
        let files = match (&config.bundle, &config.image_dir) {
            (Some(bundle), _) if bundle.owns(&config.image) => context::bundled(bundle),
            (_, Some(directory)) => context::directory(directory)?,
            _ => return Ok(None),
        };

        let original = files
            .iter()
            .find(|file| file.path == "Dockerfile")
            .ok_or_else(|| Error::denied(format!("{} has no Dockerfile", config.image)))?;
        let inlined = dockerfile::inline(
            &dockerfile::with_extras(&String::from_utf8_lossy(&original.bytes), &config.extras),
            &files,
        )?;

        if let Ok(mut builds) = self.builds.lock() {
            builds.insert(config.image.clone(), inlined);
        }
        Ok(Some(config.image.clone()))
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
    use crate::bundle;

    #[test]
    fn test_every_status_says_what_the_caller_does_next() {
        assert!(!from_status(StatusCode::FORBIDDEN, "nope").needs_another_place());
        assert!(from_status(StatusCode::NOT_FOUND, "gone").needs_another_place());
        assert!(from_status(StatusCode::TOO_MANY_REQUESTS, "slow down").retryable());
        assert!(!from_status(StatusCode::BAD_REQUEST, "bad").retryable());
    }

    #[tokio::test]
    async fn test_the_bundle_becomes_a_dockerfile_daytona_builds_with_no_context() {
        let cloud = Cloud::new("key").expect("a client");
        let config = Config {
            image: bundle::DESKTOP.tag(),
            bundle: Some(bundle::DESKTOP),
            ..Config::default()
        };

        let image = cloud
            .ensure_image(&config)
            .await
            .expect("inlined")
            .expect("an image");

        let built = cloud
            .builds
            .lock()
            .expect("held")
            .get(&image)
            .cloned()
            .expect("a Dockerfile");
        assert!(built.starts_with("FROM "));
        assert!(
            !built
                .lines()
                .any(|line| line.starts_with("COPY ") && !line.contains("--from")),
            "no file is left for a build context that daytona does not have"
        );
    }

    #[tokio::test]
    async fn test_a_named_image_is_a_snapshot_left_as_it_is() {
        let cloud = Cloud::new("key").expect("a client");
        let config = Config {
            image: "my-snapshot".to_string(),
            bundle: None,
            ..Config::default()
        };

        assert_eq!(cloud.ensure_image(&config).await.expect("asked"), None);
    }

    #[test]
    fn test_a_missing_file_is_not_a_missing_box() {
        let read = missing_is_a_failure(Err(Error::Gone("no such file".to_string())));

        assert!(
            matches!(read, Err(Error::Failed { .. })),
            "Gone would tell the caller the box is gone"
        );
    }

    #[test]
    fn test_a_boundary_is_not_reused() {
        assert_ne!(boundary(), boundary());
    }
}
