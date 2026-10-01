use super::wire::{self, Kind, Source};
use super::{API_URL, MOST_PORTS, REGISTRY};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::sandboxes::context::{self, dockerfile};
use crate::sandboxes::remote::{self, LABELS_PATH, NAME_KEY, RemoteApi, Sandbox, SandboxPlan};
use async_trait::async_trait;
use computer_types::{Arch, Capabilities, Environment, PortReach, Resources, Start};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const TOKEN_ENV: &str = "SMOL_CLOUD_TOKEN";
pub const URL_ENV: &str = "SMOL_CLOUD_URL";
pub const PACK_ENV: &str = "SMOL_CLOUD_PACK";

pub const TIMEOUT: Duration = Duration::from_secs(150);

pub const START_TIMEOUT: Duration = Duration::from_secs(600);

pub const COMMAND_TIMEOUT_SECS: u64 = 140;

pub const WRITE_CHUNK: usize = 512 * 1024;

pub struct Cloud {
    key: String,
    base: String,
    http: Client,
    pack: bool,
    namespace: Mutex<Option<String>>,
    sources: Mutex<BTreeMap<String, Source>>,
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
            http,
            pack: true,
            namespace: Mutex::new(None),
            sources: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn from_env() -> Result<Self> {
        let key = std::env::var(TOKEN_ENV)
            .ok()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| Error::Unavailable {
                provider: "smol".to_string(),
                detail: format!("{TOKEN_ENV} is not set"),
            })?;

        let mut cloud = Self::new(key)?;
        if let Ok(base) = std::env::var(URL_ENV) {
            cloud = cloud.at(base);
        }
        if std::env::var(PACK_ENV).is_ok_and(|pack| matches!(pack.as_str(), "0" | "false" | "no")) {
            cloud = cloud.packing(false);
        }
        Ok(cloud)
    }

    pub fn at(mut self, base: impl Into<String>) -> Self {
        self.base = base.into().trim_end_matches('/').to_string();
        self
    }

    pub fn packing(mut self, pack: bool) -> Self {
        self.pack = pack;
        self
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.key)
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

    async fn namespace(&self) -> Result<String> {
        if let Some(namespace) = self.namespace.lock().ok().and_then(|held| held.clone()) {
            return Ok(namespace);
        }

        let me = self.json(self.request(Method::GET, "/v1/me")).await?;
        let namespace = me
            .get("registryNamespace")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::transport("an account with no registry namespace", false))?;

        if let Ok(mut held) = self.namespace.lock() {
            *held = Some(namespace.clone());
        }
        Ok(namespace)
    }

    async fn registry(
        &self,
        url: &str,
        reference: &wire::Reference,
        method: Method,
    ) -> Result<Response> {
        let first = self
            .send(
                self.http
                    .request(method.clone(), url)
                    .header(ACCEPT, wire::MANIFEST_TYPES),
            )
            .await?;
        if first.status() != StatusCode::UNAUTHORIZED {
            return Ok(first);
        }

        let challenge = first
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .and_then(wire::challenge)
            .ok_or_else(|| {
                Error::denied(format!(
                    "{} asked for a login it did not name",
                    reference.registry
                ))
            })?;

        let scope = format!("repository:{}:pull", reference.repository);
        let mut token = self
            .http
            .get(&challenge.realm)
            .query(&[("scope", scope.as_str())]);
        if let Some(service) = &challenge.service {
            token = token.query(&[("service", service.as_str())]);
        }
        if reference.is_smol() {
            token = token.basic_auth("token", Some(&self.key));
        }

        let answer = self.json(token).await?;
        let bearer = answer
            .get("token")
            .or_else(|| answer.get("access_token"))
            .and_then(Value::as_str)
            .ok_or_else(|| Error::denied(format!("{} gave no token", reference.registry)))?;

        self.send(
            self.http
                .request(method, url)
                .header(ACCEPT, wire::MANIFEST_TYPES)
                .header(AUTHORIZATION, format!("Bearer {bearer}")),
        )
        .await
    }

    async fn digest(&self, image: &str) -> Result<Option<String>> {
        let reference = wire::reference(image);
        let response = self
            .registry(&reference.manifest_url(), &reference, Method::HEAD)
            .await?;

        match response.status() {
            status if status.is_success() => Ok(response
                .headers()
                .get("docker-content-digest")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
                .or(Some(String::new()))),
            StatusCode::NOT_FOUND => Ok(None),
            status => Err(from_status(status, &reference.registry)),
        }
    }

    async fn manifest(&self, reference: &wire::Reference) -> Result<Value> {
        let response = self
            .registry(&reference.manifest_url(), reference, Method::GET)
            .await?;
        let bytes = self.body(response).await?;
        serde_json::from_slice(&bytes).map_err(|error| {
            Error::transport(format!("a manifest that is not JSON: {error}"), false)
        })
    }

    async fn settings(&self, image: &str) -> Result<(BTreeMap<String, String>, Option<String>)> {
        let reference = wire::reference(image);
        let mut manifest = self.manifest(&reference).await?;
        if let Some(amd64) = wire::amd64_of(&manifest) {
            manifest = self.manifest(&reference.at(&amd64)).await?;
        }

        let config = wire::config_digest(&manifest)
            .ok_or_else(|| Error::transport(format!("{image} has no config"), false))?;
        let response = self
            .registry(&reference.blob_url(&config), &reference, Method::GET)
            .await?;
        let bytes = self.body(response).await?;
        let config: Value = serde_json::from_slice(&bytes).map_err(|error| {
            Error::transport(format!("an image config that is not JSON: {error}"), false)
        })?;

        Ok(wire::image_settings(&config))
    }

    fn login_dir(&self) -> Result<PathBuf> {
        let dir = std::env::temp_dir().join(format!("computer-smol-login-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .map_err(|error| Error::transport(error.to_string(), false))?;

        let host = host_docker_config();
        let mut config = host
            .as_ref()
            .and_then(|host| std::fs::read(host.join("config.json")).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(fields) = config.as_object_mut() {
            fields.remove("credsStore");
            fields.remove("credHelpers");
        }

        #[cfg(unix)]
        if let Some(host) = &host {
            for shared in ["cli-plugins", "contexts", "buildx"] {
                if host.join(shared).exists() {
                    let _ = std::os::unix::fs::symlink(host.join(shared), dir.join(shared));
                }
            }
        }

        let auth = crate::cdp::base64_encode(format!("token:{}", self.key).as_bytes());
        config["auths"] = serde_json::json!({ REGISTRY: { "auth": auth } });
        let file = dir.join("config.json");
        std::fs::write(&file, config.to_string())
            .map_err(|error| Error::transport(error.to_string(), false))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        }
        Ok(dir)
    }

    async fn build(&self, files: &[context::File], image: &str) -> Result<()> {
        let dir = std::env::temp_dir().join(format!("computer-smol-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for file in files {
            let path = dir.join(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| Error::transport(error.to_string(), false))?;
            }
            std::fs::write(&path, &file.bytes)
                .map_err(|error| Error::transport(error.to_string(), false))?;
        }

        let login = self.login_dir()?;
        let built = run(
            "docker",
            &[
                "buildx",
                "build",
                "--platform",
                "linux/amd64",
                "--push",
                "-t",
                image,
                &dir.display().to_string(),
            ],
            &login,
        )
        .await;

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&login);
        built
    }

    async fn packed(&self, image: &str) -> Result<Source> {
        let digest = self
            .digest(image)
            .await?
            .filter(|digest| !digest.is_empty())
            .ok_or_else(|| Error::Gone(format!("{image} is not in its registry")))?;
        let namespace = self.namespace().await?;
        let pack = wire::in_namespace(&namespace, wire::PACK_REPOSITORY, &wire::pack_tag(&digest));

        if self.digest(&pack).await?.is_none() {
            tracing::info!(image, pack = %pack, "packing the image for smol");
            let login = self.login_dir()?;
            let out = std::env::temp_dir()
                .join(format!("computer-smol-pack-{}", wire::pack_tag(&digest)));
            let out_text = out.display().to_string();

            let made = async {
                run(
                    "smolvm",
                    &[
                        "pack",
                        "create",
                        "--image",
                        image,
                        "--oci-platform",
                        "linux/amd64",
                        "--entrypoint",
                        "sleep infinity",
                        "--no-sign",
                        "-o",
                        &out_text,
                    ],
                    &login,
                )
                .await?;
                run(
                    "smolvm",
                    &[
                        "pack",
                        "push",
                        "--file",
                        &format!("{out_text}.smolmachine"),
                        &pack,
                    ],
                    &login,
                )
                .await
            }
            .await;

            let _ = std::fs::remove_file(&out);
            let _ = std::fs::remove_file(format!("{out_text}.smolmachine"));
            let _ = std::fs::remove_dir_all(&login);
            made?;
        }

        let (env, workdir) = self.settings(image).await?;
        Ok(Source {
            kind: Kind::Smolmachine,
            reference: pack,
            env,
            workdir,
        })
    }

    async fn exec_in(
        &self,
        id: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
        stdin: Option<&str>,
    ) -> Result<ExecResult> {
        let request = self
            .request(Method::POST, &format!("/v1/machines/{id}/exec"))
            .query(&[("output", "b64")])
            .header(CONTENT_TYPE, "application/json")
            .body(wire::command(argv, env, stdin, COMMAND_TIMEOUT_SECS)?.to_string());

        wire::parse_command(&self.json(request).await?)
    }

    async fn write_labels(&self, id: &str, labels: &BTreeMap<String, String>) -> Result<()> {
        let bytes = serde_json::to_vec(labels)
            .map_err(|error| Error::transport(format!("labels: {error}"), false))?;
        self.write_bytes(id, LABELS_PATH, &bytes).await
    }

    async fn read_labels(&self, id: &str) -> Result<BTreeMap<String, String>> {
        let read = self
            .exec_in(
                id,
                &["cat".to_string(), LABELS_PATH.to_string()],
                &BTreeMap::new(),
                None,
            )
            .await?;
        Ok(match read.code {
            0 => serde_json::from_slice(&read.stdout).unwrap_or_default(),
            _ => BTreeMap::new(),
        })
    }

    async fn write_bytes(&self, id: &str, path: &str, bytes: &[u8]) -> Result<()> {
        let mut chunks: Vec<&[u8]> = bytes.chunks(WRITE_CHUNK).collect();
        if chunks.is_empty() {
            chunks.push(&[]);
        }

        for (at, chunk) in chunks.into_iter().enumerate() {
            let redirect = match at {
                0 => r#"base64 -d > "$1""#,
                _ => r#"base64 -d >> "$1""#,
            };
            let argv = [
                "sh".to_string(),
                "-c".to_string(),
                redirect.to_string(),
                "sh".to_string(),
                path.to_string(),
            ];
            let written = self
                .exec_in(
                    id,
                    &argv,
                    &BTreeMap::new(),
                    Some(&crate::cdp::base64_encode(chunk)),
                )
                .await?;
            if written.code != 0 {
                return Err(Error::Failed {
                    code: written.code,
                    stderr: written.stderr_utf8().trim().to_string(),
                });
            }
        }
        Ok(())
    }

    fn reached(&self, id: &str, ports: impl IntoIterator<Item = u16>) -> Sandbox {
        let mut sandbox =
            Sandbox::new(id).with_header(AUTHORIZATION.as_str(), format!("Bearer {}", self.key));
        for port in ports {
            sandbox.endpoints.insert(
                port,
                format!("{}/v1/machines/{id}/connect/{port}", self.base),
            );
        }
        sandbox
    }

    async fn launched(&self, id: &str, plan: &SandboxPlan, ports: Vec<u16>) -> Result<Sandbox> {
        let start = self
            .request(Method::POST, &format!("/v1/machines/{id}/start"))
            .timeout(START_TIMEOUT);
        self.body(self.send(start).await?).await?;

        self.write_labels(id, &plan.metadata).await?;
        Ok(self.reached(id, ports))
    }
}

fn host_docker_config() -> Option<PathBuf> {
    std::env::var_os("DOCKER_CONFIG")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".docker")))
        .filter(|dir| dir.is_dir())
}

async fn run(program: &str, args: &[&str], login: &Path) -> Result<()> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .env("DOCKER_CONFIG", login)
        .output()
        .await
        .map_err(|error| Error::Unavailable {
            provider: "smol".to_string(),
            detail: format!("{program} could not run: {error}"),
        })?;

    match output.status.success() {
        true => Ok(()),
        false => Err(Error::Failed {
            code: output.status.code().unwrap_or(1),
            stderr: format!(
                "{program} {}: {}",
                args.first().copied().unwrap_or_default(),
                tail(&String::from_utf8_lossy(&output.stderr), 20)
            ),
        }),
    }
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
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
            Error::denied(format!("smol refused the key: {detail}"))
        }
        StatusCode::NOT_FOUND => Error::Gone(detail),
        StatusCode::TOO_MANY_REQUESTS
        | StatusCode::BAD_GATEWAY
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT => Error::transport(format!("{status}: {detail}"), true),
        _ => Error::transport(format!("{status}: {detail}"), false),
    }
}

#[async_trait]
impl RemoteApi for Cloud {
    fn vendor(&self) -> &str {
        "smol"
    }

    fn environment(&self) -> Environment {
        Environment::MicroVm(serde_json::json!({ "hypervisor": "libkrun" }))
    }

    fn can(&self) -> Capabilities {
        Capabilities {
            start: Start::AfterEveryStart,
            reach: PortReach::VendorUrl,
            resources: Resources::AtCreate,
            ports: Some(MOST_PORTS as u32),
            arch: vec![Arch::Amd64],
            ..Capabilities::default()
        }
    }

    fn sweepable(&self) -> bool {
        true
    }

    async fn available(&self) -> Result<()> {
        self.namespace()
            .await
            .map(|_| ())
            .map_err(|error| Error::Unavailable {
                provider: "smol".to_string(),
                detail: error.to_string(),
            })
    }

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox> {
        let source = self
            .sources
            .lock()
            .ok()
            .and_then(|sources| sources.get(&plan.image).cloned())
            .unwrap_or_else(|| Source::image(&plan.image));
        let ports = wire::published(plan)?;

        let answer = self
            .json(
                self.request(Method::POST, "/v1/machines")
                    .header(CONTENT_TYPE, "application/json")
                    .body(wire::new_machine(plan, &source, &ports)?.to_string()),
            )
            .await?;
        let id = wire::id_of(&answer)?;

        match self.launched(&id, plan, ports).await {
            Ok(sandbox) => Ok(sandbox),
            Err(error) => {
                let _ = self.kill(&id).await;
                Err(error)
            }
        }
    }

    async fn find(&self, name: &str) -> Result<Option<Sandbox>> {
        let listing = self.json(self.request(Method::GET, "/v1/machines")).await?;
        let Some(machine) = wire::named(&listing, name) else {
            return Ok(None);
        };
        Ok(Some(
            self.reached(&wire::id_of(machine)?, wire::ports_of(machine)),
        ))
    }

    async fn kill(&self, id: &str) -> Result<()> {
        let response = self
            .send(self.request(Method::DELETE, &format!("/v1/machines/{id}")))
            .await?;

        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Ok(()),
            status => Err(from_status(status, "")),
        }
    }

    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>> {
        let listing = self.json(self.request(Method::GET, "/v1/machines")).await?;
        let mut found = Vec::new();

        for id in wire::started(&listing) {
            match self.read_labels(&id).await {
                Ok(labels) => {
                    if labels.contains_key(NAME_KEY)
                        && let Some(value) = labels.get(key)
                    {
                        found.push((id, value.clone()));
                    }
                }
                Err(error) => tracing::warn!(machine = %id, %error, "its labels could not be read"),
            }
        }
        Ok(found)
    }

    async fn exec(
        &self,
        sandbox: &Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        self.exec_in(&sandbox.id, argv, env, None).await
    }

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>> {
        let read = self
            .exec_in(
                &sandbox.id,
                &["cat".to_string(), "--".to_string(), path.to_string()],
                &BTreeMap::new(),
                None,
            )
            .await?;

        match read.code {
            0 => Ok(read.stdout),
            code => Err(Error::Failed {
                code,
                stderr: read.stderr_utf8().trim().to_string(),
            }),
        }
    }

    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()> {
        self.write_bytes(&sandbox.id, path, bytes).await
    }

    async fn ensure_image(&self, config: &Config) -> Result<Option<String>> {
        if let Some(held) = self
            .sources
            .lock()
            .ok()
            .filter(|sources| sources.contains_key(&config.image))
            .map(|_| config.image.clone())
        {
            return Ok(Some(held));
        }

        let files = match (&config.bundle, &config.image_dir) {
            (Some(bundle), _) if bundle.owns(&config.image) => Some((
                bundle.name.to_string(),
                format!("{}-x86_64", bundle.fingerprint(&config.extras)),
                context::bundled(bundle),
            )),
            (_, Some(directory)) => {
                let (repository, rest) = config
                    .image
                    .split_once(':')
                    .ok_or_else(|| Error::denied(format!("{} has no tag", config.image)))?;
                let hash = rest.rsplit_once('-').map_or(rest, |(hash, _)| hash);
                Some((
                    repository.to_string(),
                    format!("{hash}-x86_64"),
                    context::directory(directory)?,
                ))
            }
            _ => None,
        };

        let image = match files {
            None => config.image.clone(),
            Some((repository, tag, files)) => {
                let image = wire::in_namespace(&self.namespace().await?, &repository, &tag);
                if self.digest(&image).await?.is_none() {
                    let files: Vec<context::File> = files
                        .into_iter()
                        .map(|file| match file.path == "Dockerfile" {
                            true => context::File {
                                bytes: dockerfile::with_extras(
                                    &String::from_utf8_lossy(&file.bytes),
                                    &config.extras,
                                )
                                .into_bytes(),
                                ..file
                            },
                            false => file,
                        })
                        .collect();
                    tracing::info!(image = %image, "building the image for smol with docker on this host");
                    self.build(&files, &image).await?;
                }
                image
            }
        };

        let source = match self.pack {
            false => Source::image(&image),
            true => match self.packed(&image).await {
                Ok(source) => source,
                Err(error) => {
                    tracing::warn!(image = %image, %error, "no pack, so the machine starts from the image and runs its CMD");
                    Source::image(&image)
                }
            },
        };

        if let Ok(mut sources) = self.sources.lock() {
            sources.insert(config.image.clone(), source);
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

    #[test]
    fn test_every_status_says_what_the_caller_does_next() {
        assert!(!from_status(StatusCode::FORBIDDEN, "nope").needs_another_place());
        assert!(from_status(StatusCode::NOT_FOUND, "gone").needs_another_place());
        assert!(from_status(StatusCode::BAD_GATEWAY, "busy").retryable());
        assert!(!from_status(StatusCode::BAD_REQUEST, "bad").retryable());
    }

    #[test]
    fn test_a_port_is_reached_through_connect_with_the_key() {
        let sandbox = Cloud::new("smk_x")
            .expect("a client")
            .reached("mach-1", [6080, 9223]);

        assert_eq!(
            sandbox.url(9223),
            Some("https://api.smolmachines.com/v1/machines/mach-1/connect/9223")
        );
        assert_eq!(
            sandbox.headers.get("authorization").map(String::as_str),
            Some("Bearer smk_x")
        );
        assert!(
            !format!("{sandbox:?}").contains("smk_x"),
            "the key stays out of logs"
        );
    }

    #[test]
    fn test_packing_can_be_turned_off() {
        let cloud = Cloud::new("k").expect("a client").packing(false);
        assert!(!cloud.pack);
    }
}
