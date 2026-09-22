use crate::config::ServerConfig;
use crate::error::{ApiError, ApiResult};
use crate::spec::profile_for;
use computer::sandboxes::remote::{self, RemoteApi};
use computer::{Builder, ContainerCli, DockerMachine, Machine, Profile, SystemDocker};
use computer_api::{
    Arch, Capabilities, DisplayServer, Environment, PlaceKind, Placement, PortReach, Resources,
    RuntimeState, RuntimeView, Source, Start,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const HOSTS: [&str; 3] = ["docker", "podman", "nerdctl"];

pub const OFFERED: &str = "COMPUTER_SERVER_RUNTIMES";

pub const SANDBOXES: &str = "COMPUTER_SERVER_SANDBOXES";

const E2B_LIFETIME_SECS: u64 = 60 * 60;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Tuning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

pub enum Place {
    Host {
        machine: Arc<dyn Machine>,
    },
    Remote {
        api: Arc<dyn RemoteApi>,
        fields: Value,
    },
}

impl Place {
    pub fn kind(&self) -> PlaceKind {
        match self {
            Self::Host { .. } => PlaceKind::Host,
            Self::Remote { .. } => PlaceKind::Remote,
        }
    }
}

pub struct Runtime {
    pub name: String,
    pub provider: String,
    pub source: Source,
    pub environment: Environment,
    pub place: Place,
    pub can: Capabilities,
    pub tuning: Tuning,
    pub state: RuntimeState,
}

impl Runtime {
    pub fn ready(&self) -> bool {
        self.state == RuntimeState::Ready
    }

    pub fn pair(&self, server: DisplayServer) -> (Arc<dyn Machine>, Arc<dyn Profile>) {
        let image = profile_for(server);

        match &self.place {
            Place::Host { machine } => (Arc::clone(machine), image),
            Place::Remote { api, .. } => {
                let (machine, profile) = remote::pair(Arc::clone(api), image);
                (Arc::new(machine), profile)
            }
        }
    }

    pub fn scanning(&self) -> Arc<dyn Machine> {
        self.pair(DisplayServer::default()).0
    }

    pub fn drive(&self, builder: Builder, server: DisplayServer) -> Builder {
        let (machine, profile) = self.pair(server);
        let mut builder = builder.machine(machine).profile(profile);

        if let Some(memory) = &self.tuning.memory {
            builder = builder.memory(memory.clone());
        }
        if let Some(cpus) = &self.tuning.cpus {
            builder = builder.cpus(cpus.clone());
        }
        if let Some(isolation) = &self.tuning.isolation {
            builder = builder.isolation(isolation.clone());
        }

        builder
    }

    pub fn check(&self, placement: &Placement) -> ApiResult<()> {
        if let RuntimeState::Unavailable { why } = &self.state {
            return Err(ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                computer_api::ErrorCode::Unavailable,
                format!("the runtime {} is not answering: {why}", self.name),
            ));
        }

        if let (Some(asked), Some(most)) =
            (placement.expires_after_secs, self.can.max_lifetime_secs)
            && asked > most
        {
            return Err(ApiError::bad_request(format!(
                "{} keeps a box for at most {most}s and this asks for {asked}s",
                self.name
            )));
        }

        if self.can.resources == Resources::AtImage
            && (placement.memory.is_some() || placement.cpus.is_some())
        {
            return Err(ApiError::bad_request(format!(
                "{} sets memory and cpus when its image is built, not when a box \
                 is created: leave them out and build an image that has them",
                self.name
            )));
        }

        Ok(())
    }

    pub fn fields(&self) -> Value {
        match &self.place {
            Place::Host { .. } => serde_json::to_value(&self.tuning).unwrap_or(Value::Null),
            Place::Remote { fields, .. } => fields.clone(),
        }
    }

    pub fn view(&self, boxes: u32) -> RuntimeView {
        RuntimeView {
            name: self.name.clone(),
            provider: self.provider.clone(),
            source: self.source,
            place: self.place.kind(),
            environment: self.environment.clone(),
            state: self.state.clone(),
            fields: self.fields(),
            secrets: Vec::new(),
            can: self.can.clone(),
            boxes,
        }
    }
}

#[derive(Default)]
pub struct Runtimes {
    all: BTreeMap<String, Arc<Runtime>>,
    default: Option<String>,
}

impl Runtimes {
    pub fn add(&mut self, runtime: Runtime) {
        self.all.insert(runtime.name.clone(), Arc::new(runtime));
    }

    pub fn prefer(&mut self, name: &str) -> Result<(), String> {
        if !self.all.contains_key(name) {
            return Err(format!(
                "the default runtime is {name} and this server has {}",
                self.names()
            ));
        }

        self.default = Some(name.to_string());
        Ok(())
    }

    pub fn settle(&mut self) {
        if self.default.is_some() {
            return;
        }

        self.default = self
            .all
            .values()
            .find(|runtime| runtime.name == "docker" && runtime.ready())
            .or_else(|| self.all.values().find(|runtime| runtime.ready()))
            .map(|runtime| runtime.name.clone());
    }

    pub fn get(&self, name: &str) -> Option<Arc<Runtime>> {
        self.all.get(name).cloned()
    }

    pub fn all(&self) -> Vec<Arc<Runtime>> {
        self.all.values().cloned().collect()
    }

    pub fn default_name(&self) -> Option<&str> {
        self.default.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn names(&self) -> String {
        match self.all.is_empty() {
            true => "none".to_string(),
            false => self.all.keys().cloned().collect::<Vec<_>>().join(", "),
        }
    }

    pub fn resolve(&self, asked: Option<&str>) -> ApiResult<Arc<Runtime>> {
        let name = match asked {
            Some(name) => name,
            None => self.default.as_deref().ok_or_else(|| {
                ApiError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    computer_api::ErrorCode::Unavailable,
                    "this server has no runtime to put a box on".to_string(),
                )
            })?,
        };

        self.get(name).ok_or_else(|| {
            ApiError::bad_request(format!(
                "no runtime named {name} here: this server has {}",
                self.names()
            ))
        })
    }
}

pub async fn host(name: String, provider: String, source: Source, tuning: Tuning) -> Runtime {
    let cli: Arc<dyn ContainerCli> = Arc::new(program(&provider, &tuning));
    let machine: Arc<dyn Machine> = Arc::new(DockerMachine::new(Arc::clone(&cli)));

    let state = match machine.preflight().await {
        Ok(()) => RuntimeState::Ready,
        Err(error) => RuntimeState::Unavailable {
            why: error.to_string(),
        },
    };

    let (environment, arch) = match state {
        RuntimeState::Ready => probe(cli.as_ref(), &provider, &tuning).await,
        _ => (Environment::default(), Vec::new()),
    };

    Runtime {
        name,
        provider,
        source,
        environment,
        place: Place::Host { machine },
        can: container_can(arch),
        tuning,
        state,
    }
}

pub fn remote(name: String, api: Arc<dyn RemoteApi>) -> Runtime {
    let provider = api.vendor().to_string();
    let (environment, can) = vendor_can(&provider);

    Runtime {
        name,
        provider,
        source: Source::Environment,
        environment,
        place: Place::Remote {
            api,
            fields: Value::Object(Map::new()),
        },
        can,
        tuning: Tuning::default(),
        state: RuntimeState::Ready,
    }
}

fn program(provider: &str, tuning: &Tuning) -> SystemDocker {
    let mut before = Vec::new();

    if let Some(context) = &tuning.context {
        before.push("--context".to_string());
        before.push(context.clone());
    }

    SystemDocker::new(provider).before(before)
}

async fn probe(
    cli: &dyn ContainerCli,
    provider: &str,
    tuning: &Tuning,
) -> (Environment, Vec<Arch>) {
    let format = match provider {
        "podman" => {
            "{{.Version.Version}}\t{{.Host.OCIRuntime.Name}}\t{{.Store.GraphDriverName}}\t{{.Host.Arch}}"
        }
        _ => "{{.ServerVersion}}\t{{.DefaultRuntime}}\t{{.Driver}}\t{{.Architecture}}",
    };

    let said = cli
        .run(&[
            "info".to_string(),
            "--format".to_string(),
            format.to_string(),
        ])
        .await
        .ok()
        .filter(|said| said.ok())
        .map(|said| said.stdout_utf8())
        .unwrap_or_default();

    told(provider, &said, tuning)
}

fn told(provider: &str, said: &str, tuning: &Tuning) -> (Environment, Vec<Arch>) {
    let mut fields = said.trim().split('\t').map(|field| {
        let field = field.trim();
        match field.is_empty() || field == "<no value>" {
            true => "",
            false => field,
        }
    });

    let version = fields.next().unwrap_or_default();
    let default_runtime = fields.next().unwrap_or_default();
    let storage = fields.next().unwrap_or_default();
    let arch = fields.next().unwrap_or_default();

    let mut info = Map::new();
    let engine = match version.is_empty() {
        true => provider.to_string(),
        false => format!("{provider} {version}"),
    };
    info.insert("engine".to_string(), Value::String(engine));
    if !default_runtime.is_empty() {
        info.insert(
            "default_runtime".to_string(),
            Value::String(default_runtime.to_string()),
        );
    }
    if !storage.is_empty() {
        info.insert("storage".to_string(), Value::String(storage.to_string()));
    }

    let on = tuning.isolation.as_deref().unwrap_or(default_runtime);
    let environment = match on.starts_with("kata") {
        true => Environment::MicroVm(Value::Object(info)),
        false => Environment::Container(Value::Object(info)),
    };

    (environment, arches(arch))
}

fn arches(arch: &str) -> Vec<Arch> {
    match arch {
        "x86_64" | "amd64" => vec![Arch::Amd64],
        "aarch64" | "arm64" => vec![Arch::Arm64],
        _ => Vec::new(),
    }
}

fn container_can(arch: Vec<Arch>) -> Capabilities {
    Capabilities {
        start: Start::Entrypoint,
        reach: PortReach::HostPort,
        pause: true,
        stop: true,
        fork: false,
        volumes: true,
        resources: Resources::AtCreate,
        max_lifetime_secs: None,
        ports: None,
        arch,
    }
}

fn vendor_can(provider: &str) -> (Environment, Capabilities) {
    match provider {
        "e2b" => (
            Environment::MicroVm(serde_json::json!({ "hypervisor": "firecracker" })),
            Capabilities {
                start: Start::Snapshot,
                reach: PortReach::VendorUrl,
                pause: true,
                stop: false,
                fork: true,
                volumes: true,
                resources: Resources::AtImage,
                max_lifetime_secs: Some(E2B_LIFETIME_SECS),
                ports: None,
                arch: Vec::new(),
            },
        ),
        _ => (
            Environment::default(),
            Capabilities {
                start: Start::Entrypoint,
                reach: PortReach::VendorUrl,
                pause: false,
                stop: false,
                fork: false,
                volumes: false,
                resources: Resources::AtCreate,
                max_lifetime_secs: None,
                ports: None,
                arch: Vec::new(),
            },
        ),
    }
}

pub fn offered() -> Vec<String> {
    let named = named(std::env::var(OFFERED).ok().as_deref());

    match named.is_empty() {
        true => HOSTS.iter().map(|host| host.to_string()).collect(),
        false => named,
    }
}

pub fn sandboxes() -> Vec<String> {
    named(std::env::var(SANDBOXES).ok().as_deref())
}

fn named(listed: Option<&str>) -> Vec<String> {
    listed
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

pub async fn discover(
    config: &ServerConfig,
    hosts: &[String],
    sandboxes: &[String],
) -> Result<Runtimes, String> {
    let wanted = wanted(config, hosts)?;

    let mut runtimes = Runtimes::default();

    for (name, (provider, tuning, source)) in wanted {
        let runtime = host(name, provider, source, tuning).await;

        match (&runtime.state, source) {
            (RuntimeState::Unavailable { why }, Source::Found) => {
                tracing::debug!(runtime = %runtime.name, %why, "no engine of this name here")
            }
            (RuntimeState::Unavailable { why }, _) => {
                tracing::warn!(runtime = %runtime.name, %why, "a configured runtime is not answering");
                runtimes.add(runtime);
            }
            _ => {
                tracing::info!(
                    runtime = %runtime.name,
                    provider = %runtime.provider,
                    environment = ?runtime.environment,
                    "a runtime to put boxes on"
                );
                runtimes.add(runtime);
            }
        }
    }

    for name in sandboxes {
        if runtimes.get(name).is_some() {
            return Err(format!(
                "{SANDBOXES} names {name} and a host runtime of that name is already here"
            ));
        }

        if let Some(api) = vendor(name) {
            runtimes.add(remote(name.clone(), api));
        }
    }

    match &config.default {
        Some(name) => runtimes.prefer(name)?,
        None => runtimes.settle(),
    }

    Ok(runtimes)
}

type Wanted = BTreeMap<String, (String, Tuning, Source)>;

fn wanted(config: &ServerConfig, hosts: &[String]) -> Result<Wanted, String> {
    let mut wanted = Wanted::new();

    for provider in hosts {
        match HOSTS.contains(&provider.as_str()) {
            true => {
                wanted.insert(
                    provider.clone(),
                    (provider.clone(), Tuning::default(), Source::Found),
                );
            }
            false => tracing::warn!(
                provider = %provider,
                "{OFFERED} names an engine this server cannot drive"
            ),
        }
    }

    for (name, entry) in &config.runtimes {
        if entry.enabled == Some(false) {
            wanted.remove(name);
            continue;
        }

        let provider = entry.host(name)?;

        match entry.provider.is_some() {
            true => {
                let tuning = entry.tuning(name, &provider)?;
                wanted.insert(name.clone(), (provider, tuning, Source::File));
            }
            false => {
                let Some(held) = wanted.get_mut(name) else {
                    return Err(format!(
                        "runtimes.{name} tunes a runtime this server does not offer; \
                         name its provider to add one"
                    ));
                };
                held.1 = entry.tuning(name, &held.0)?;
                held.2 = Source::File;
            }
        }
    }

    Ok(wanted)
}

pub fn vendor(name: &str) -> Option<Arc<dyn RemoteApi>> {
    #[cfg(feature = "e2b")]
    if name == "e2b" {
        use computer::sandboxes::e2b::{E2bVendor, cloud::Cloud};

        return match Cloud::from_env() {
            Ok(cloud) => Some(Arc::new(E2bVendor::new(Arc::new(cloud)))),
            Err(error) => {
                tracing::warn!(vendor = %name, %error, "this vendor was named and cannot be reached");
                None
            }
        };
    }

    tracing::warn!(
        vendor = %name,
        "this vendor was named and is not built into this server"
    );
    None
}

pub fn engine(name: &str, machine: Arc<dyn Machine>) -> Runtime {
    Runtime {
        name: name.to_string(),
        provider: name.to_string(),
        source: Source::Found,
        environment: Environment::Container(Value::Object(Map::new())),
        place: Place::Host { machine },
        can: container_can(Vec::new()),
        tuning: Tuning::default(),
        state: RuntimeState::Ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use computer::DockerMachine;
    use computer::testing::{ScriptedCli, ScriptedRemote};

    fn file(text: &str) -> ServerConfig {
        ServerConfig::parse(text).expect("a file this server reads")
    }

    fn hosts() -> Vec<String> {
        HOSTS.iter().map(|host| host.to_string()).collect()
    }

    fn docker() -> Runtime {
        engine(
            "docker",
            Arc::new(DockerMachine::new(Arc::new(ScriptedCli::new()))),
        )
    }

    fn cloud() -> Runtime {
        let (environment, can) = vendor_can("e2b");

        Runtime {
            name: "cloud".to_string(),
            provider: "e2b".to_string(),
            source: Source::Environment,
            environment,
            place: Place::Remote {
                api: Arc::new(ScriptedRemote::new()),
                fields: Value::Object(Map::new()),
            },
            can,
            tuning: Tuning::default(),
            state: RuntimeState::Ready,
        }
    }

    #[test]
    fn test_an_engine_says_what_it_runs_and_what_it_runs_on() {
        let (environment, arch) = told(
            "docker",
            "27.3.1\trunc\toverlay2\tx86_64",
            &Tuning::default(),
        );

        let Environment::Container(info) = &environment else {
            panic!("runc is a container runtime: {environment:?}");
        };
        assert_eq!(info["engine"], "docker 27.3.1");
        assert_eq!(info["default_runtime"], "runc");
        assert_eq!(info["storage"], "overlay2");
        assert_eq!(arch, vec![Arch::Amd64]);
    }

    #[test]
    fn test_an_engine_on_kata_runs_micro_vms_and_one_on_gvisor_runs_containers() {
        let (kata, _) = told(
            "docker",
            "27.3.1\tkata-runtime\toverlay2\t",
            &Tuning::default(),
        );
        let (gvisor, _) = told("docker", "27.3.1\trunsc\toverlay2\t", &Tuning::default());

        assert!(
            matches!(kata, Environment::MicroVm(_)),
            "Kata boots a kernel per box, so the same engine is not a container runtime here"
        );
        assert!(matches!(gvisor, Environment::Container(_)));
    }

    #[test]
    fn test_an_engine_offered_on_another_oci_runtime_is_described_by_that_one() {
        let tuning = Tuning {
            isolation: Some("kata".to_string()),
            ..Tuning::default()
        };
        let (environment, _) = told("docker", "27.3.1\trunc\toverlay2\t", &tuning);

        assert!(
            matches!(environment, Environment::MicroVm(_)),
            "the boxes go on kata whatever the engine's own default is"
        );
    }

    #[test]
    fn test_an_engine_that_answers_nothing_is_still_named() {
        let (environment, arch) = told("podman", "", &Tuning::default());

        assert_eq!(environment.info()["engine"], "podman");
        assert!(arch.is_empty(), "nothing said is not a guess at the arch");
    }

    #[test]
    fn test_a_runtime_nothing_offers_is_refused_with_what_is_here() {
        let mut runtimes = Runtimes::default();
        runtimes.add(docker());
        runtimes.settle();

        let Err(error) = runtimes.resolve(Some("/usr/bin/id")) else {
            panic!("a placement naming a program was accepted as a runtime");
        };

        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            error.body.message.contains("docker"),
            "the refusal says what can be asked for: {}",
            error.body.message
        );
    }

    #[test]
    fn test_a_box_that_names_no_runtime_lands_on_the_default() {
        let mut runtimes = Runtimes::default();
        runtimes.add(engine(
            "podman",
            Arc::new(DockerMachine::new(Arc::new(ScriptedCli::new()))),
        ));
        runtimes.add(docker());
        runtimes.settle();

        assert_eq!(
            runtimes.resolve(None).expect("a default").name,
            "docker",
            "docker is the default where it is offered, whatever sorts first"
        );

        runtimes.prefer("podman").expect("a name it has");
        assert_eq!(runtimes.resolve(None).expect("a default").name, "podman");
    }

    #[test]
    fn test_a_server_with_no_runtime_says_so_rather_than_names_one() {
        let Err(error) = Runtimes::default().resolve(None) else {
            panic!("a box was placed on a server that has nowhere to put one");
        };

        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_a_default_the_file_names_and_this_server_does_not_have_is_an_error() {
        let mut runtimes = Runtimes::default();
        runtimes.add(docker());

        assert!(runtimes.prefer("cloud").is_err());
    }

    #[test]
    fn test_a_life_longer_than_the_vendor_keeps_a_box_is_refused_before_anything_starts() {
        let placement = Placement {
            expires_after_secs: Some(60 * 60 * 24),
            ..Placement::default()
        };

        let Err(error) = cloud().check(&placement) else {
            panic!("a box was accepted that the vendor would take away first");
        };
        assert!(
            error.body.message.contains(&E2B_LIFETIME_SECS.to_string()),
            "the refusal carries both numbers: {}",
            error.body.message
        );
        assert!(
            docker().check(&placement).is_ok(),
            "an engine keeps it as long as asked"
        );
    }

    #[test]
    fn test_memory_asked_of_a_runtime_that_bakes_it_into_its_image_is_refused() {
        let placement = Placement {
            memory: Some("4g".to_string()),
            ..Placement::default()
        };

        assert!(cloud().check(&placement).is_err());
        assert!(docker().check(&placement).is_ok());
    }

    #[test]
    fn test_a_runtime_that_is_not_answering_takes_no_box() {
        let mut down = docker();
        down.state = RuntimeState::Unavailable {
            why: "the daemon is not running".to_string(),
        };

        let error = down
            .check(&Placement::default())
            .expect_err("a box cannot go on an engine that is not there");

        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_a_file_table_with_no_provider_tunes_the_engine_that_was_found() {
        let wanted = wanted(&file("[runtimes.docker]\nmemory = \"4g\"\n"), &hosts())
            .expect("a file this server takes");

        let (provider, tuning, source) = wanted.get("docker").expect("docker is still offered");
        assert_eq!(provider, "docker");
        assert_eq!(tuning.memory.as_deref(), Some("4g"));
        assert_eq!(*source, Source::File);
    }

    #[test]
    fn test_a_file_table_with_a_provider_offers_the_same_engine_twice() {
        let wanted = wanted(
            &file("[runtimes.hardened]\nprovider = \"docker\"\nisolation = \"runsc\"\n"),
            &hosts(),
        )
        .expect("a file this server takes");

        assert!(
            wanted.contains_key("docker"),
            "the plain engine is still here"
        );
        let (provider, tuning, _) = wanted.get("hardened").expect("and so is the hardened one");
        assert_eq!(provider, "docker");
        assert_eq!(tuning.isolation.as_deref(), Some("runsc"));
    }

    #[test]
    fn test_an_engine_the_file_turns_off_is_not_offered() {
        let wanted = wanted(&file("[runtimes.podman]\nenabled = false\n"), &hosts())
            .expect("a file this server takes");

        assert!(!wanted.contains_key("podman"));
        assert!(wanted.contains_key("docker"));
    }

    #[test]
    fn test_a_file_that_tunes_nothing_is_an_error_rather_than_a_silent_nothing() {
        let why = wanted(&file("[runtimes.nowhere]\nmemory = \"4g\"\n"), &hosts())
            .expect_err("a table naming no provider and tuning nothing");

        assert!(why.contains("nowhere"), "{why}");
    }

    #[test]
    fn test_a_file_names_host_runtimes_only() {
        let why = wanted(&file("[runtimes.cloud]\nprovider = \"e2b\"\n"), &hosts())
            .expect_err("a vendor needs a key, which a file in git must not hold");

        assert!(
            why.contains("docker"),
            "the refusal says what a file may name: {why}"
        );
    }

    #[test]
    fn test_a_field_of_another_engine_is_refused_by_name() {
        let why = wanted(&file("[runtimes.podman]\ncontext = \"gpu-1\"\n"), &hosts())
            .expect_err("a docker context means nothing to podman");

        assert!(why.contains("runtimes.podman.context"), "{why}");

        let why = wanted(&file("[runtimes.docker]\nmemroy = \"4g\"\n"), &hosts())
            .expect_err("a misspelled field silently doing nothing is worse");

        assert!(why.contains("memroy"), "{why}");
    }

    #[test]
    fn test_only_the_engines_this_server_drives_are_offered() {
        let wanted = wanted(
            &ServerConfig::default(),
            &["docker".to_string(), "tart".to_string()],
        )
        .expect("a list with one name this server cannot drive");

        assert_eq!(wanted.len(), 1);
        assert!(wanted.contains_key("docker"));
    }

    #[test]
    fn test_what_configured_a_runtime_is_reported_without_a_secret() {
        let mut tuned = docker();
        tuned.tuning = Tuning {
            memory: Some("4g".to_string()),
            context: Some("gpu-1".to_string()),
            ..Tuning::default()
        };

        let view = tuned.view(2);

        assert_eq!(view.fields["memory"], "4g");
        assert_eq!(view.fields["context"], "gpu-1");
        assert!(
            view.fields.get("cpus").is_none(),
            "a field nothing set is not reported"
        );
        assert!(view.secrets.is_empty());
        assert_eq!(view.boxes, 2);
        assert_eq!(view.place, PlaceKind::Host);
    }
}
