use super::proto::{self, CONTROL, ROUTER};
use super::{APP_NAME, SERVER_URL, wire};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::sandboxes::context::{self, dockerfile};
use crate::sandboxes::remote::{self, RemoteApi, Sandbox, SandboxPlan};
use async_trait::async_trait;
use holm_types::{Arch, Capabilities, Environment, PortReach, Resources, Start};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tonic::codegen::http::uri::PathAndQuery;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Code, Status};

pub const TOKEN_ID_ENV: &str = "MODAL_TOKEN_ID";
pub const TOKEN_SECRET_ENV: &str = "MODAL_TOKEN_SECRET";
pub const ENVIRONMENT_ENV: &str = "MODAL_ENVIRONMENT";
pub const SERVER_URL_ENV: &str = "MODAL_SERVER_URL";
pub const APP_ENV: &str = "MODAL_APP";

pub const MOST_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

pub const WAIT_CALL: Duration = Duration::from_secs(60);

pub const MESSAGE_LIMIT: usize = 100 * 1024 * 1024;

pub const STDIN_CHUNK: usize = 256 * 1024;

const CLIENT_TYPE: &str = "9";
const CLIENT_VERSION: &str = "1.0.0";
const FALLBACK_BUILDER: &str = "2024.10";

#[derive(Clone)]
struct Task {
    task_id: String,
    router: Channel,
    url: String,
    jwt: String,
    env: BTreeMap<String, String>,
}

pub struct Cloud {
    token_id: String,
    token_secret: String,
    environment: String,
    app_name: String,
    host: String,
    control: Channel,
    auth: Mutex<Option<(String, u64)>>,
    app_id: Mutex<Option<String>>,
    builds: Mutex<BTreeMap<String, Vec<String>>>,
    images: Mutex<BTreeMap<String, String>>,
    tasks: Mutex<BTreeMap<String, Task>>,
}

impl Cloud {
    pub fn new(token_id: impl Into<String>, token_secret: impl Into<String>) -> Result<Self> {
        Self::at(token_id, token_secret, SERVER_URL)
    }

    pub fn at(
        token_id: impl Into<String>,
        token_secret: impl Into<String>,
        server: &str,
    ) -> Result<Self> {
        let (control, host) = channel(server)?;

        Ok(Self {
            token_id: token_id.into(),
            token_secret: token_secret.into(),
            environment: String::new(),
            app_name: APP_NAME.to_string(),
            host,
            control,
            auth: Mutex::new(None),
            app_id: Mutex::new(None),
            builds: Mutex::new(BTreeMap::new()),
            images: Mutex::new(BTreeMap::new()),
            tasks: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn from_env() -> Result<Self> {
        let needed = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| Error::Unavailable {
                    provider: "modal".to_string(),
                    detail: format!("{name} is not set"),
                })
        };

        let server = std::env::var(SERVER_URL_ENV).unwrap_or_else(|_| SERVER_URL.to_string());
        let mut cloud = Self::at(needed(TOKEN_ID_ENV)?, needed(TOKEN_SECRET_ENV)?, &server)?;
        if let Ok(environment) = std::env::var(ENVIRONMENT_ENV) {
            cloud = cloud.environment(environment);
        }
        if let Ok(app) = std::env::var(APP_ENV) {
            cloud = cloud.app(app);
        }
        Ok(cloud)
    }

    pub fn environment(mut self, environment: impl Into<String>) -> Self {
        self.environment = environment.into();
        self
    }

    pub fn app(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !name.is_empty() {
            self.app_name = name;
        }
        self
    }

    fn base_headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("x-modal-client-type", CLIENT_TYPE.to_string()),
            ("x-modal-client-version", CLIENT_VERSION.to_string()),
            (
                "x-modal-libmodal-version",
                format!("holm-rs/{}", env!("CARGO_PKG_VERSION")),
            ),
            ("x-modal-token-id", self.token_id.clone()),
            ("x-modal-token-secret", self.token_secret.clone()),
            ("x-modal-host", self.host.clone()),
        ]
    }

    async fn headers(&self) -> Result<Vec<(&'static str, String)>> {
        let mut headers = self.base_headers();
        headers.push(("x-modal-auth-token", self.auth_token().await?));
        Ok(headers)
    }

    async fn auth_token(&self) -> Result<String> {
        let now = now_secs();
        if let Some((token, expires)) = self.auth.lock().ok().and_then(|held| held.clone())
            && now + 300 < expires
        {
            return Ok(token);
        }

        let answer: proto::AuthTokenGetResponse = unary(
            &self.control,
            &format!("{CONTROL}AuthTokenGet"),
            proto::Empty {},
            &self.base_headers(),
            None,
        )
        .await?;

        if answer.token.is_empty() {
            return Err(Error::denied("modal answered with an empty auth token"));
        }

        let expires = wire::jwt_expiry(&answer.token).unwrap_or(now + 20 * 60);
        if let Ok(mut held) = self.auth.lock() {
            *held = Some((answer.token.clone(), expires));
        }
        Ok(answer.token)
    }

    async fn control<Req, Resp>(&self, method: &str, request: Req) -> Result<Resp>
    where
        Req: prost::Message + Send + Sync + 'static,
        Resp: prost::Message + Default + Send + Sync + 'static,
    {
        unary(
            &self.control,
            &format!("{CONTROL}{method}"),
            request,
            &self.headers().await?,
            None,
        )
        .await
    }

    async fn app_id(&self) -> Result<String> {
        if let Some(id) = self.app_id.lock().ok().and_then(|held| held.clone()) {
            return Ok(id);
        }

        let answer: proto::AppGetOrCreateResponse = self
            .control(
                "AppGetOrCreate",
                proto::AppGetOrCreateRequest {
                    app_name: self.app_name.clone(),
                    environment_name: self.environment.clone(),
                    object_creation_type: proto::CREATE_IF_MISSING,
                },
            )
            .await?;

        if let Ok(mut held) = self.app_id.lock() {
            *held = Some(answer.app_id.clone());
        }
        Ok(answer.app_id)
    }

    async fn builder_version(&self) -> String {
        let answer: Result<proto::EnvironmentGetOrCreateResponse> = self
            .control(
                "EnvironmentGetOrCreate",
                proto::EnvironmentGetOrCreateRequest {
                    deployment_name: self.environment.clone(),
                    object_creation_type: 0,
                },
            )
            .await;

        answer
            .ok()
            .and_then(|answer| answer.metadata)
            .and_then(|metadata| metadata.settings)
            .map(|settings| settings.image_builder_version)
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| FALLBACK_BUILDER.to_string())
    }

    async fn image_id(&self, image: &str) -> Result<String> {
        if wire::is_image_id(image) {
            return Ok(image.to_string());
        }
        if let Some(id) = self
            .images
            .lock()
            .ok()
            .and_then(|images| images.get(image).cloned())
        {
            return Ok(id);
        }

        let commands = self
            .builds
            .lock()
            .ok()
            .and_then(|builds| builds.get(image).cloned())
            .unwrap_or_else(|| wire::from_registry(image));

        let answer: proto::ImageGetOrCreateResponse = self
            .control(
                "ImageGetOrCreate",
                proto::ImageGetOrCreateRequest {
                    image: Some(proto::Image {
                        dockerfile_commands: commands,
                    }),
                    app_id: self.app_id().await?,
                    builder_version: self.builder_version().await,
                },
            )
            .await?;

        let result = match answer.result {
            Some(result) if result.status != proto::STATUS_UNSPECIFIED => result,
            _ => self.joined(&answer.image_id).await?,
        };

        if result.status != proto::STATUS_SUCCESS {
            return Err(Error::Failed {
                code: result.exitcode,
                stderr: format!("the image {image} did not build: {}", result.exception),
            });
        }

        if let Ok(mut images) = self.images.lock() {
            images.insert(image.to_string(), answer.image_id.clone());
        }
        Ok(answer.image_id)
    }

    async fn joined(&self, image_id: &str) -> Result<proto::GenericResult> {
        let mut last_entry_id = String::new();
        let mut logs = String::new();

        loop {
            let mut stream = streaming::<_, proto::ImageJoinStreamingResponse>(
                &self.control,
                &format!("{CONTROL}ImageJoinStreaming"),
                proto::ImageJoinStreamingRequest {
                    image_id: image_id.to_string(),
                    timeout: 55.0,
                    last_entry_id: last_entry_id.clone(),
                },
                &self.base_headers(),
            )
            .await?;

            while let Some(message) = stream.message().await.map_err(from_status)? {
                for entry in &message.task_logs {
                    logs.push_str(&entry.data);
                }
                if !message.entry_id.is_empty() {
                    last_entry_id = message.entry_id.clone();
                }
                if let Some(result) = message.result
                    && result.status != proto::STATUS_UNSPECIFIED
                {
                    if result.status != proto::STATUS_SUCCESS {
                        tracing::warn!(image = %image_id, logs = %tail(&logs, 40), "the image build failed");
                    }
                    return Ok(result);
                }
            }
        }
    }

    fn remember(&self, sandbox_id: &str, task: Task) {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.insert(sandbox_id.to_string(), task);
        }
    }

    async fn task(&self, sandbox_id: &str) -> Result<Task> {
        if let Some(task) = self
            .tasks
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(sandbox_id).cloned())
        {
            return Ok(task);
        }

        let answer: proto::SandboxGetTaskIdResponse = self
            .control(
                "SandboxGetTaskIdV2",
                proto::SandboxGetTaskIdRequest {
                    sandbox_id: sandbox_id.to_string(),
                    timeout: Some(10.0),
                },
            )
            .await?;
        let task_id = answer
            .task_id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Error::Gone(sandbox_id.to_string()))?;

        let task = self.routed(sandbox_id, task_id, BTreeMap::new()).await?;
        self.remember(sandbox_id, task.clone());
        Ok(task)
    }

    async fn routed(
        &self,
        sandbox_id: &str,
        task_id: String,
        env: BTreeMap<String, String>,
    ) -> Result<Task> {
        let access: proto::SandboxGetCommandRouterAccessResponse = self
            .control(
                "SandboxGetCommandRouterAccess",
                proto::SandboxId {
                    sandbox_id: sandbox_id.to_string(),
                },
            )
            .await?;

        Ok(Task {
            task_id,
            router: channel(&access.url)?.0,
            url: access.url,
            jwt: access.jwt,
            env,
        })
    }

    async fn refreshed(&self, sandbox_id: &str, stale: &Task) -> Result<Task> {
        let mut task = self
            .routed(sandbox_id, stale.task_id.clone(), stale.env.clone())
            .await?;
        if task.url == stale.url {
            task.router = stale.router.clone();
        }
        self.remember(sandbox_id, task.clone());
        Ok(task)
    }

    async fn routed_call<Req, Resp>(
        &self,
        sandbox_id: &str,
        task: &mut Task,
        method: &str,
        request: Req,
        timeout: Option<Duration>,
    ) -> Result<Resp>
    where
        Req: prost::Message + Clone + Send + Sync + 'static,
        Resp: prost::Message + Default + Send + Sync + 'static,
    {
        let path = format!("{ROUTER}{method}");
        match unary(
            &task.router,
            &path,
            request.clone(),
            &bearer(&task.jwt),
            timeout,
        )
        .await
        {
            Err(Error::Denied { .. }) => {
                *task = self.refreshed(sandbox_id, task).await?;
                unary(&task.router, &path, request, &bearer(&task.jwt), timeout).await
            }
            other => other,
        }
    }

    async fn run(
        &self,
        sandbox_id: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
        stdin: Option<&[u8]>,
    ) -> Result<ExecResult> {
        if argv.is_empty() {
            return Err(Error::denied("an empty command has nothing to run"));
        }

        let mut task = self.task(sandbox_id).await?;
        let task_id = task.task_id.clone();
        let exec_id = wire::exec_id()?;
        let mut merged = task.env.clone();
        merged.extend(env.clone());

        let _: proto::Empty = self
            .routed_call(
                sandbox_id,
                &mut task,
                "TaskExecStart",
                proto::TaskExecStartRequest {
                    task_id: task_id.clone(),
                    exec_id: exec_id.clone(),
                    command_args: argv.to_vec(),
                    stdout_config: proto::PIPE,
                    stderr_config: proto::PIPE,
                    timeout_secs: None,
                    env: merged.into_iter().collect(),
                },
                None,
            )
            .await?;

        if let Some(bytes) = stdin {
            self.feed(sandbox_id, &mut task, &exec_id, bytes).await?;
        }

        let code = self.waited(sandbox_id, &mut task, &exec_id).await?;

        Ok(ExecResult {
            code,
            stdout: self.drained(&task, &exec_id, proto::STDOUT).await?,
            stderr: self.drained(&task, &exec_id, proto::STDERR).await?,
            timed_out: false,
        })
    }

    async fn feed(
        &self,
        sandbox_id: &str,
        task: &mut Task,
        exec_id: &str,
        bytes: &[u8],
    ) -> Result<()> {
        let task_id = task.task_id.clone();
        let mut offset = 0u64;
        let chunks = bytes.chunks(STDIN_CHUNK).map(Some).chain([None]);

        for chunk in chunks {
            let data = chunk.map(<[u8]>::to_vec).unwrap_or_default();
            let length = data.len() as u64;

            let _: proto::Empty = self
                .routed_call(
                    sandbox_id,
                    task,
                    "TaskExecStdinWrite",
                    proto::TaskExecStdinWriteRequest {
                        task_id: task_id.clone(),
                        exec_id: exec_id.to_string(),
                        offset,
                        data,
                        eof: chunk.is_none(),
                    },
                    None,
                )
                .await?;
            offset += length;
        }

        Ok(())
    }

    async fn waited(&self, sandbox_id: &str, task: &mut Task, exec_id: &str) -> Result<i32> {
        let task_id = task.task_id.clone();
        loop {
            let answer: Result<proto::TaskExecWaitResponse> = self
                .routed_call(
                    sandbox_id,
                    task,
                    "TaskExecWait",
                    proto::TaskExec {
                        task_id: task_id.clone(),
                        exec_id: exec_id.to_string(),
                    },
                    Some(WAIT_CALL),
                )
                .await;

            match answer {
                Ok(answer) => {
                    if let Some(code) = wire::exit_code(answer.code, answer.signal) {
                        return Ok(code);
                    }
                }
                Err(Error::Transport {
                    retryable: true, ..
                }) => {}
                Err(error) => return Err(error),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn drained(&self, task: &Task, exec_id: &str, descriptor: i32) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut stream = streaming::<_, proto::TaskExecStdioReadResponse>(
            &task.router,
            &format!("{ROUTER}TaskExecStdioRead"),
            proto::TaskExecStdioReadRequest {
                task_id: task.task_id.clone(),
                exec_id: exec_id.to_string(),
                offset: 0,
                file_descriptor: descriptor,
            },
            &bearer(&task.jwt),
        )
        .await?;

        while let Some(message) = stream.message().await.map_err(from_status)? {
            bytes.extend(message.data);
        }
        Ok(bytes)
    }

    async fn tunnels(
        &self,
        sandbox_id: &str,
        have: Vec<proto::TunnelData>,
        want: usize,
    ) -> Result<Sandbox> {
        let tunnels = match have.len() >= want {
            true => have,
            false => {
                let answer: proto::SandboxGetTunnelsResponse = self
                    .control(
                        "SandboxGetTunnelsV2",
                        proto::SandboxGetTunnelsRequest {
                            sandbox_id: sandbox_id.to_string(),
                            timeout: 30.0,
                        },
                    )
                    .await?;
                if answer
                    .result
                    .is_some_and(|result| result.status == proto::STATUS_TIMEOUT)
                {
                    return Err(Error::Timeout {
                        after: Duration::from_secs(30),
                        detail: format!("{sandbox_id} had no tunnels yet"),
                    });
                }
                answer.tunnels
            }
        };

        let mut sandbox = Sandbox::new(sandbox_id);
        for tunnel in tunnels {
            if let Ok(port) = u16::try_from(tunnel.container_port) {
                sandbox
                    .endpoints
                    .insert(port, wire::tunnel_url(&tunnel.host, tunnel.port));
            }
        }
        Ok(sandbox)
    }

    async fn launched(
        &self,
        answer: proto::SandboxCreateV2Response,
        plan: &SandboxPlan,
    ) -> Result<Sandbox> {
        let access = answer
            .command_router_access
            .ok_or_else(|| Error::transport("a sandbox with no command router", false))?;

        self.remember(
            &answer.sandbox_id,
            Task {
                task_id: answer.task_id,
                router: channel(&access.url)?.0,
                url: access.url,
                jwt: access.jwt,
                env: plan.env.clone(),
            },
        );

        self.tunnels(&answer.sandbox_id, answer.tunnels, plan.publish.len())
            .await
    }
}

fn channel(url: &str) -> Result<(Channel, String)> {
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(['/', ':'])
        .next()
        .unwrap_or_default()
        .to_string();

    let mut endpoint = Endpoint::from_shared(url.to_string())
        .map_err(|error| Error::denied(format!("{url}: {error}")))?
        .http2_keep_alive_interval(Duration::from_secs(30))
        .keep_alive_timeout(Duration::from_secs(10))
        .keep_alive_while_idle(true);

    if url.starts_with("https://") {
        endpoint = endpoint
            .tls_config(
                ClientTlsConfig::new()
                    .with_webpki_roots()
                    .domain_name(&host),
            )
            .map_err(|error| Error::transport(format!("{url}: {error}"), false))?;
    }

    Ok((endpoint.connect_lazy(), host))
}

fn bearer(jwt: &str) -> Vec<(&'static str, String)> {
    vec![("authorization", format!("Bearer {jwt}"))]
}

fn request<Req>(message: Req, headers: &[(&'static str, String)]) -> Result<tonic::Request<Req>> {
    let mut request = tonic::Request::new(message);
    for (key, value) in headers {
        let value = MetadataValue::try_from(value.as_str())
            .map_err(|_| Error::denied(format!("a {key} that cannot be a header")))?;
        request.metadata_mut().insert(*key, value);
    }
    Ok(request)
}

fn path(method: &str) -> Result<PathAndQuery> {
    PathAndQuery::try_from(method).map_err(|error| Error::denied(format!("{method}: {error}")))
}

async fn unary<Req, Resp>(
    channel: &Channel,
    method: &str,
    message: Req,
    headers: &[(&'static str, String)],
    timeout: Option<Duration>,
) -> Result<Resp>
where
    Req: prost::Message + Send + Sync + 'static,
    Resp: prost::Message + Default + Send + Sync + 'static,
{
    let mut grpc = tonic::client::Grpc::new(channel.clone())
        .max_decoding_message_size(MESSAGE_LIMIT)
        .max_encoding_message_size(MESSAGE_LIMIT);
    grpc.ready()
        .await
        .map_err(|error| Error::transport(chain(&error), true))?;

    let mut request = request(message, headers)?;
    if let Some(timeout) = timeout {
        request.set_timeout(timeout);
    }

    grpc.unary(
        request,
        path(method)?,
        tonic_prost::ProstCodec::<Req, Resp>::default(),
    )
    .await
    .map(tonic::Response::into_inner)
    .map_err(from_status)
}

async fn streaming<Req, Resp>(
    channel: &Channel,
    method: &str,
    message: Req,
    headers: &[(&'static str, String)],
) -> Result<tonic::Streaming<Resp>>
where
    Req: prost::Message + Send + Sync + 'static,
    Resp: prost::Message + Default + Send + Sync + 'static,
{
    let mut grpc = tonic::client::Grpc::new(channel.clone())
        .max_decoding_message_size(MESSAGE_LIMIT)
        .max_encoding_message_size(MESSAGE_LIMIT);
    grpc.ready()
        .await
        .map_err(|error| Error::transport(chain(&error), true))?;

    grpc.server_streaming(
        request(message, headers)?,
        path(method)?,
        tonic_prost::ProstCodec::<Req, Resp>::default(),
    )
    .await
    .map(tonic::Response::into_inner)
    .map_err(from_status)
}

fn from_status(status: Status) -> Error {
    let detail = format!("{:?}: {}", status.code(), status.message());

    match status.code() {
        Code::Unauthenticated | Code::PermissionDenied => {
            Error::denied(format!("modal refused the token: {detail}"))
        }
        Code::NotFound => Error::Gone(detail),
        Code::Unavailable
        | Code::DeadlineExceeded
        | Code::ResourceExhausted
        | Code::Cancelled
        | Code::Internal
        | Code::Unknown => Error::transport(detail, true),
        _ => Error::transport(detail, false),
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

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[async_trait]
impl RemoteApi for Cloud {
    fn vendor(&self) -> &str {
        "modal"
    }

    fn environment(&self) -> Environment {
        Environment::Container(serde_json::json!({ "runtime": "gvisor" }))
    }

    fn can(&self) -> Capabilities {
        Capabilities {
            start: Start::AfterEveryStart,
            reach: PortReach::VendorUrl,
            resources: Resources::AtCreate,
            max_lifetime_secs: Some(MOST_LIFETIME.as_secs()),
            arch: vec![Arch::Amd64],
            ..Capabilities::default()
        }
    }

    fn sweepable(&self) -> bool {
        true
    }

    async fn available(&self) -> Result<()> {
        self.auth_token()
            .await
            .map(|_| ())
            .map_err(|error| Error::Unavailable {
                provider: "modal".to_string(),
                detail: error.to_string(),
            })
    }

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox> {
        let image_id = self.image_id(&plan.image).await?;

        let definition = proto::Sandbox {
            entrypoint_args: wire::idle_entrypoint(),
            image_id,
            resources: Some(proto::Resources {
                memory_mb: wire::memory_mib(plan.memory.as_deref())?,
                milli_cpu: wire::milli_cpu(plan.cpus.as_deref())?,
                gpu_config: Some(proto::GpuConfig {}),
            }),
            timeout_secs: wire::seconds(MOST_LIFETIME),
            open_ports: Some(proto::PortSpecs {
                ports: plan
                    .publish
                    .iter()
                    .map(|port| proto::PortSpec {
                        port: u32::from(*port),
                        unencrypted: false,
                    })
                    .collect(),
            }),
            network_access: Some(proto::NetworkAccess {
                network_access_type: match plan.network {
                    true => proto::NETWORK_OPEN,
                    false => proto::NETWORK_BLOCKED,
                },
            }),
            name: Some(plan.name.clone()),
            idle_timeout_secs: Some(wire::seconds(plan.ttl)),
        };

        let answer: proto::SandboxCreateV2Response = self
            .control(
                "SandboxCreateV2",
                proto::SandboxCreateV2Request {
                    app_id: self.app_id().await?,
                    definition: Some(definition),
                    ephemeral_secrets: (!plan.env.is_empty()).then(|| proto::StringMap {
                        contents: plan.env.clone().into_iter().collect(),
                    }),
                    tags: plan
                        .metadata
                        .iter()
                        .map(|(key, value)| proto::SandboxTag {
                            tag_name: key.clone(),
                            tag_value: value.clone(),
                        })
                        .collect(),
                },
            )
            .await?;

        let sandbox_id = answer.sandbox_id.clone();
        match self.launched(answer, plan).await {
            Ok(sandbox) => Ok(sandbox),
            Err(error) => {
                let _ = self.kill(&sandbox_id).await;
                Err(error)
            }
        }
    }

    async fn find(&self, name: &str) -> Result<Option<Sandbox>> {
        let found: Result<proto::SandboxId> = self
            .control(
                "SandboxGetFromNameV2",
                proto::SandboxGetFromNameRequest {
                    sandbox_name: name.to_string(),
                    environment_name: self.environment.clone(),
                    app_name: self.app_name.clone(),
                },
            )
            .await;

        let sandbox_id = match found {
            Ok(found) => found.sandbox_id,
            Err(Error::Gone(_)) => return Ok(None),
            Err(error) => return Err(error),
        };

        match self.task(&sandbox_id).await {
            Ok(_) => {}
            Err(Error::Gone(_)) => return Ok(None),
            Err(error) => return Err(error),
        }

        let standard = crate::Profile::ports(&crate::X11Profile).to_publish();
        self.tunnels(&sandbox_id, Vec::new(), standard.len())
            .await
            .map(Some)
    }

    async fn kill(&self, id: &str) -> Result<()> {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.remove(id);
        }

        let answer: Result<proto::Empty> = self
            .control(
                "SandboxTerminateV2",
                proto::SandboxId {
                    sandbox_id: id.to_string(),
                },
            )
            .await;

        match answer {
            Ok(_) | Err(Error::Gone(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>> {
        let app_id = self.app_id().await?;
        let mut found = Vec::new();
        let mut before = 0.0;

        loop {
            let page: proto::SandboxListResponse = self
                .control(
                    "SandboxListV2",
                    proto::SandboxListRequest {
                        app_id: app_id.clone(),
                        before_timestamp: before,
                        environment_name: String::new(),
                        include_finished: false,
                        tags: Vec::new(),
                    },
                )
                .await?;

            let Some(last) = page.sandboxes.last() else {
                return Ok(found);
            };
            before = last.created_at;

            for sandbox in page.sandboxes {
                let tags = match sandbox.tags.is_empty() {
                    false => sandbox.tags,
                    true => {
                        let answer: proto::SandboxTagsGetResponse = self
                            .control(
                                "SandboxTagsGetV2",
                                proto::SandboxId {
                                    sandbox_id: sandbox.id.clone(),
                                },
                            )
                            .await?;
                        answer.tags
                    }
                };

                if let Some(tag) = tags.into_iter().find(|tag| tag.tag_name == key) {
                    found.push((sandbox.id.clone(), tag.tag_value));
                }
            }
        }
    }

    async fn exec(
        &self,
        sandbox: &Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        self.run(&sandbox.id, argv, env, None).await
    }

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>> {
        let result = self
            .run(
                &sandbox.id,
                &["cat".to_string(), "--".to_string(), path.to_string()],
                &BTreeMap::new(),
                None,
            )
            .await?;

        match result.code {
            0 => Ok(result.stdout),
            code => Err(Error::Failed {
                code,
                stderr: result.stderr_utf8().trim().to_string(),
            }),
        }
    }

    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()> {
        let argv = [
            "sh".to_string(),
            "-c".to_string(),
            r#"cat > "$1""#.to_string(),
            "sh".to_string(),
            path.to_string(),
        ];
        let result = self
            .run(&sandbox.id, &argv, &BTreeMap::new(), Some(bytes))
            .await?;

        match result.code {
            0 => Ok(()),
            code => Err(Error::Failed {
                code,
                stderr: result.stderr_utf8().trim().to_string(),
            }),
        }
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
            builds.insert(config.image.clone(), wire::dockerfile_commands(&inlined));
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
        assert!(!from_status(Status::unauthenticated("no")).needs_another_place());
        assert!(from_status(Status::not_found("gone")).needs_another_place());
        assert!(from_status(Status::unavailable("busy")).retryable());
        assert!(!from_status(Status::invalid_argument("bad")).retryable());
    }

    #[test]
    fn test_the_router_gets_only_its_bearer() {
        assert_eq!(
            bearer("jwt"),
            [("authorization", "Bearer jwt".to_string())],
            "the control plane's token headers do not go to a worker"
        );
    }

    #[tokio::test]
    async fn test_the_bundle_becomes_dockerfile_lines_with_no_context() {
        let cloud = Cloud::new("id", "secret").expect("a client");
        let config = Config {
            image: crate::bundle::DESKTOP.tag(),
            bundle: Some(crate::bundle::DESKTOP),
            ..Config::default()
        };

        let image = cloud
            .ensure_image(&config)
            .await
            .expect("inlined")
            .expect("an image");

        let commands = cloud
            .builds
            .lock()
            .expect("held")
            .get(&image)
            .cloned()
            .expect("lines");
        assert!(commands[0].starts_with("FROM "));
        assert!(
            !commands
                .iter()
                .any(|line| line.starts_with("COPY ") && !line.contains("--from")),
            "modal refuses a COPY from a context it was not sent"
        );
    }
}
