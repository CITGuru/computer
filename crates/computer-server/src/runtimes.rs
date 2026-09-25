use crate::config::ServerConfig;
use crate::error::{ApiError, ApiResult};
use crate::spec::profile_for;
use computer::sandboxes::remote::{self, RemoteApi};
use computer::{Builder, Engine, EngineMachine, Machine, Profile, SystemEngine};
use computer_api::{
    Arch, Capabilities, DisplayServer, Environment, PlaceKind, Placement, PortReach, Resources,
    RuntimeState, RuntimeView, Source, Start,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub const HOSTS: [&str; 5] = ["docker", "podman", "nerdctl", "smolvm", "microsandbox"];

pub const MICROVMS: [&str; 2] = ["smolvm", "microsandbox"];

pub const OFFERED: &str = "COMPUTER_SERVER_RUNTIMES";

pub const SANDBOXES: &str = "COMPUTER_SERVER_SANDBOXES";

pub const LIFETIME_SECS: u64 = 60 * 60;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builds_with: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_lifetime_secs: Option<u64>,
}

impl Tuning {
    pub fn host_only(&self) -> Option<&'static str> {
        match self {
            Self {
                memory: Some(_), ..
            } => Some("memory"),
            Self { cpus: Some(_), .. } => Some("cpus"),
            Self {
                isolation: Some(_), ..
            } => Some("isolation"),
            Self {
                context: Some(_), ..
            } => Some("context"),
            _ => None,
        }
    }
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
    pub secrets: Vec<String>,
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
            Place::Remote { api, fields } => {
                let (machine, profile) = remote::pair(Arc::clone(api), image);
                let machine = machine
                    .public_viewer(true)
                    .public_traffic(public_traffic(fields));
                (Arc::new(machine), profile)
            }
        }
    }

    pub fn scanning(&self) -> Arc<dyn Machine> {
        self.pair(DisplayServer::default()).0
    }

    pub fn lifetime(&self) -> u64 {
        self.tuning.lifetime_secs.unwrap_or(LIFETIME_SECS)
    }

    pub fn drive(&self, builder: Builder, spec: &computer_api::Spec) -> Builder {
        let (machine, profile) = self.pair(spec.desktop.server);
        let mut builder = builder
            .machine(machine)
            .profile(profile)
            .expires_after(std::time::Duration::from_secs(self.lifetime()));

        if self.can.reach == PortReach::VendorUrl && spec.policy.auth == computer_api::Auth::None {
            builder = builder.auth(computer::Auth::Token);
        }

        if let Some(image) = self.image_for(&spec.digest()) {
            builder = builder.image(image);
        }

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

    pub fn image_for(&self, digest: &str) -> Option<String> {
        let Place::Remote { fields, .. } = &self.place else {
            return None;
        };

        let said = |at: &Value| at.as_str().map(str::to_string);

        fields
            .get("images")
            .and_then(|images| images.get(digest))
            .and_then(said)
            .or_else(|| fields.get("image").and_then(said))
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
                "{} keeps a box for at most {most}s and this asks for {asked}s: \
                 raise max_lifetime on the runtime to ask for more",
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
        let mut said = serde_json::to_value(&self.tuning).unwrap_or(Value::Null);

        if let Place::Remote { fields, .. } = &self.place
            && let (Some(mine), Some(theirs)) = (said.as_object_mut(), fields.as_object())
        {
            for (key, value) in theirs {
                mine.insert(key.clone(), value.clone());
            }
        }

        said
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
            secrets: self.secrets.clone(),
            can: self.can.clone(),
            boxes,
        }
    }
}

#[derive(Default)]
pub struct Runtimes {
    held: RwLock<Held>,
}

#[derive(Default)]
struct Held {
    all: BTreeMap<String, Arc<Runtime>>,
    default: Option<String>,
}

impl Runtimes {
    fn read(&self) -> RwLockReadGuard<'_, Held> {
        self.held.read().unwrap_or_else(|held| held.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Held> {
        self.held.write().unwrap_or_else(|held| held.into_inner())
    }

    pub fn add(&self, runtime: Runtime) {
        self.write()
            .all
            .insert(runtime.name.clone(), Arc::new(runtime));
    }

    pub fn prefer(&self, name: &str) -> Result<(), String> {
        let mut held = self.write();

        if !held.all.contains_key(name) {
            let names = names(&held);
            return Err(format!(
                "the default runtime is {name} and this server has {names}"
            ));
        }

        held.default = Some(name.to_string());
        Ok(())
    }

    pub fn settle(&self) {
        let mut held = self.write();

        if held.default.is_some() {
            return;
        }

        held.default = held
            .all
            .values()
            .find(|runtime| runtime.name == "docker" && runtime.ready())
            .or_else(|| held.all.values().find(|runtime| runtime.ready()))
            .map(|runtime| runtime.name.clone());
    }

    pub fn get(&self, name: &str) -> Option<Arc<Runtime>> {
        self.read().all.get(name).cloned()
    }

    pub fn all(&self) -> Vec<Arc<Runtime>> {
        self.read().all.values().cloned().collect()
    }

    pub fn forget(&self, name: &str) -> Option<Arc<Runtime>> {
        let mut held = self.write();

        if held.default.as_deref() == Some(name) {
            held.default = None;
        }

        held.all.remove(name)
    }

    pub fn default_name(&self) -> Option<String> {
        self.read().default.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.read().all.is_empty()
    }

    pub fn names(&self) -> String {
        names(&self.read())
    }

    pub fn resolve(&self, asked: Option<&str>) -> ApiResult<Arc<Runtime>> {
        let held = self.read();

        let name = match asked {
            Some(name) => name.to_string(),
            None => held.default.clone().ok_or_else(|| {
                ApiError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    computer_api::ErrorCode::Unavailable,
                    "this server has no runtime to put a box on".to_string(),
                )
            })?,
        };

        held.all.get(&name).cloned().ok_or_else(|| {
            ApiError::bad_request(format!(
                "no runtime named {name} here: this server has {}",
                names(&held)
            ))
        })
    }
}

fn names(held: &Held) -> String {
    match held.all.is_empty() {
        true => "none".to_string(),
        false => held.all.keys().cloned().collect::<Vec<_>>().join(", "),
    }
}

pub async fn host(name: String, provider: String, source: Source, tuning: Tuning) -> Runtime {
    if MICROVMS.contains(&provider.as_str()) {
        return hypervisor(name, provider, source, tuning).await;
    }

    let cli: Arc<dyn Engine> = Arc::new(program(&provider, &tuning));
    let machine: Arc<dyn Machine> = Arc::new(EngineMachine::new(Arc::clone(&cli)));

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

    let mut can = container_can(arch);
    capped(&mut can, &tuning);

    Runtime {
        name,
        provider,
        secrets: Vec::new(),
        source,
        environment,
        place: Place::Host { machine },
        can,
        tuning,
        state,
    }
}

fn capped(can: &mut Capabilities, tuning: &Tuning) {
    can.max_lifetime_secs = tuning
        .max_lifetime_secs
        .or(can.max_lifetime_secs)
        .or(Some(LIFETIME_SECS));
}

pub fn remote(name: String, api: Arc<dyn RemoteApi>, tuning: Tuning) -> Runtime {
    let provider = api.vendor().to_string();
    let environment = api.environment();
    let mut can = api.can();
    capped(&mut can, &tuning);

    Runtime {
        name,
        provider,
        secrets: Vec::new(),
        source: Source::Environment,
        environment,
        place: Place::Remote {
            api,
            fields: Value::Object(Map::new()),
        },
        can,
        tuning,
        state: RuntimeState::Ready,
    }
}

async fn hypervisor(name: String, provider: String, source: Source, tuning: Tuning) -> Runtime {
    let (api, loader, said): (
        Arc<dyn computer::microvm::MicroVmApi>,
        Arc<dyn computer::microvm::ImageLoader>,
        String,
    ) = match provider.as_str() {
        "microsandbox" => {
            use computer::sandboxes::microsandbox::msb::Msb;

            let msb = Arc::new(match &tuning.program {
                Some(at) => Msb::new(at.clone()),
                None => Msb::found(),
            });
            let said = msb.program().to_string();

            (Arc::clone(&msb) as _, msb as _, said)
        }
        _ => {
            use computer::sandboxes::smolvm::SmolVm;

            let smolvm = Arc::new(match &tuning.program {
                Some(at) => SmolVm::new(at.clone()),
                None => SmolVm::found(),
            });
            let said = smolvm.program().to_string();

            (Arc::clone(&smolvm) as _, smolvm as _, said)
        }
    };
    let mut can = api.can();

    let builder: Arc<dyn Engine> = Arc::new(SystemEngine::new(
        tuning
            .builds_with
            .clone()
            .unwrap_or_else(|| "docker".to_string()),
    ));
    let machine: Arc<dyn Machine> = Arc::new(
        computer::MicroVm::new(api)
            .named(provider.clone())
            .building_with(builder, loader),
    );

    let state = match machine.preflight().await {
        Ok(()) => RuntimeState::Ready,
        Err(error) => RuntimeState::Unavailable {
            why: error.to_string(),
        },
    };

    let mut info = Map::new();
    info.insert("engine".to_string(), Value::String(provider.clone()));
    info.insert("program".to_string(), Value::String(said));
    info.insert(
        "builds_with".to_string(),
        Value::String(
            tuning
                .builds_with
                .clone()
                .unwrap_or_else(|| "docker".to_string()),
        ),
    );
    info.insert(
        "hypervisor".to_string(),
        Value::String("libkrun".to_string()),
    );

    capped(&mut can, &tuning);

    Runtime {
        name,
        provider,
        secrets: Vec::new(),
        source,
        environment: Environment::MicroVm(Value::Object(info)),
        place: Place::Host { machine },
        can,
        tuning,
        state,
    }
}

fn program(provider: &str, tuning: &Tuning) -> SystemEngine {
    let mut before = Vec::new();

    if let Some(context) = &tuning.context {
        before.push("--context".to_string());
        before.push(context.clone());
    }

    SystemEngine::new(provider).before(before)
}

async fn probe(cli: &dyn Engine, provider: &str, tuning: &Tuning) -> (Environment, Vec<Arch>) {
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
    vendors: &dyn Vendors,
) -> Result<Runtimes, String> {
    let plan = plan(config, hosts, sandboxes)?;
    let runtimes = Runtimes::default();

    for (name, (provider, tuning, source)) in plan.hosts {
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
                told_of(&runtime);
                runtimes.add(runtime);
            }
        }
    }

    for (name, tuning) in plan.remotes {
        if runtimes.get(&name).is_some() {
            return Err(format!(
                "{SANDBOXES} names {name} and a host runtime of that name is already here"
            ));
        }

        if let Some(api) = vendors.in_environment(&name) {
            let runtime = remote(name, api, tuning);
            told_of(&runtime);
            runtimes.add(runtime);
        }
    }

    match &config.default {
        Some(name) => runtimes.prefer(name)?,
        None => runtimes.settle(),
    }

    Ok(runtimes)
}

fn told_of(runtime: &Runtime) {
    tracing::info!(
        runtime = %runtime.name,
        provider = %runtime.provider,
        environment = ?runtime.environment,
        lifetime = runtime.lifetime(),
        most = ?runtime.can.max_lifetime_secs,
        "a runtime to put boxes on"
    );
}

#[derive(Debug, Default)]
struct Plan {
    hosts: BTreeMap<String, (String, Tuning, Source)>,
    remotes: BTreeMap<String, Tuning>,
}

fn plan(config: &ServerConfig, hosts: &[String], sandboxes: &[String]) -> Result<Plan, String> {
    let mut plan = Plan::default();

    for provider in hosts {
        match HOSTS.contains(&provider.as_str()) {
            true => {
                plan.hosts.insert(
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

    for name in sandboxes {
        plan.remotes.insert(name.clone(), Tuning::default());
    }

    for (name, entry) in &config.runtimes {
        if entry.enabled == Some(false) {
            plan.hosts.remove(name);
            plan.remotes.remove(name);
            continue;
        }

        if entry.provider.is_some() {
            let provider = entry.host(name)?;
            let tuning = entry.tuning(name, &provider)?;
            plan.hosts
                .insert(name.clone(), (provider, tuning, Source::File));
            continue;
        }

        if let Some(held) = plan.hosts.get_mut(name) {
            held.1 = entry.tuning(name, &held.0)?;
            held.2 = Source::File;
            continue;
        }

        if let Some(held) = plan.remotes.get_mut(name) {
            let tuning = entry.tuning(name, name)?;

            if let Some(field) = tuning.host_only() {
                return Err(format!(
                    "runtimes.{name}.{field} is a host engine's field and {name} is a vendor"
                ));
            }

            *held = tuning;
            continue;
        }

        return Err(format!(
            "runtimes.{name} tunes a runtime this server does not offer; \
             name its provider to add one"
        ));
    }

    Ok(plan)
}

pub const KEY_FIELD: &str = "api_key";

pub fn nameable(name: &str) -> Result<(), String> {
    let plain = name
        .chars()
        .all(|one| one.is_ascii_alphanumeric() || matches!(one, '-' | '_' | '.'));
    let leads = name
        .chars()
        .next()
        .is_some_and(|one| one.is_ascii_alphanumeric());

    match (name.is_empty(), plain && leads && name.len() <= 40) {
        (true, _) => Err("a runtime needs a name, such as cloud".to_string()),
        (false, true) => Ok(()),
        (false, false) => Err(format!(
            "{name:?} is not a runtime name: a letter or digit first, then letters, \
             digits, - _ and ., 40 at most"
        )),
    }
}

pub fn public(fields: &Value) -> Result<(), String> {
    let Some(endpoint) = fields.get("endpoint").and_then(Value::as_str) else {
        return Ok(());
    };

    let rest = endpoint
        .strip_prefix("https://")
        .ok_or_else(|| format!("{endpoint} is not https, and a key would go over it"))?;

    let host = rest
        .split('/')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();

    if host.is_empty() {
        return Err(format!("{endpoint} names no host"));
    }

    let private = host == "localhost"
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| match address {
                std::net::IpAddr::V4(four) => {
                    four.is_loopback() || four.is_private() || four.is_link_local()
                }
                std::net::IpAddr::V6(six) => six.is_loopback() || six.is_unique_local(),
            });

    match private {
        true => Err(format!(
            "{endpoint} is not reachable from outside this network, and a runtime added \
             over the API must be: name it in the configuration file instead"
        )),
        false => Ok(()),
    }
}

pub trait Vendors: Send + Sync {
    fn build(
        &self,
        provider: &str,
        fields: &Value,
        key: Option<&computer::Secret>,
    ) -> Result<Arc<dyn RemoteApi>, String>;

    fn in_environment(&self, provider: &str) -> Option<Arc<dyn RemoteApi>>;
}

pub struct Builtin;

impl Vendors for Builtin {
    fn build(
        &self,
        provider: &str,
        fields: &Value,
        key: Option<&computer::Secret>,
    ) -> Result<Arc<dyn RemoteApi>, String> {
        #[cfg(feature = "e2b")]
        if provider == "e2b" {
            use computer::sandboxes::e2b::{E2bVendor, cloud::Cloud};

            let key = key.ok_or_else(|| "e2b takes an api_key".to_string())?;
            let cloud = match fields.get("endpoint").and_then(Value::as_str) {
                Some(endpoint) => Cloud::at(key.expose(), endpoint.trim_start_matches("https://")),
                None => Cloud::new(key.expose()),
            }
            .map_err(|error| error.to_string())?;

            return Ok(Arc::new(E2bVendor::new(Arc::new(cloud))));
        }

        let _ = (fields, key);
        Err(format!(
            "{provider} is not a vendor this server was built with"
        ))
    }

    fn in_environment(&self, provider: &str) -> Option<Arc<dyn RemoteApi>> {
        #[cfg(feature = "e2b")]
        if provider == "e2b" {
            use computer::sandboxes::e2b::{E2bVendor, cloud::Cloud};

            return match Cloud::from_env() {
                Ok(cloud) => Some(Arc::new(E2bVendor::new(Arc::new(cloud)))),
                Err(error) => {
                    tracing::warn!(vendor = %provider, %error, "this vendor cannot be reached");
                    None
                }
            };
        }

        tracing::warn!(
            vendor = %provider,
            "this vendor was named and is not built into this server"
        );
        None
    }
}

pub async fn from_store(
    runtimes: &Runtimes,
    store: &dyn computer_storage::Store,
    keeper: &crate::secrets::Keeper,
    vendors: &dyn Vendors,
) -> usize {
    let records = match store.list_runtimes().await {
        Ok(records) => records,
        Err(why) => {
            tracing::warn!(%why, "the runtimes this server stored could not be read");
            return 0;
        }
    };

    let mut added = 0;

    for record in records {
        if runtimes.get(&record.name).is_some() {
            tracing::warn!(
                runtime = %record.name,
                "a runtime of this name is configured here, so the stored one is not offered"
            );
            continue;
        }

        match stored(&record, keeper, vendors) {
            Ok(runtime) => {
                told_of(&runtime);
                runtimes.add(runtime);
                added += 1;
            }
            Err(why) => {
                tracing::warn!(runtime = %record.name, %why, "a stored runtime is not usable");
                runtimes.add(unavailable(&record, why));
            }
        }
    }

    added
}

pub fn stored(
    record: &computer_storage::RuntimeRecord,
    keeper: &crate::secrets::Keeper,
    vendors: &dyn Vendors,
) -> Result<Runtime, String> {
    let key = match record.secrets.get(KEY_FIELD) {
        Some(sealed) => Some(keeper.open(
            &crate::secrets::Whose::new(&record.name, &record.provider, KEY_FIELD),
            sealed,
        )?),
        None => None,
    };

    let api = vendors.build(&record.provider, &record.fields, key.as_ref())?;
    let environment = api.environment();
    let mut can = api.can();
    let tuning = lives(&record.fields);
    capped(&mut can, &tuning);

    Ok(Runtime {
        name: record.name.clone(),
        provider: record.provider.clone(),
        secrets: record.secrets.keys().cloned().collect(),
        source: Source::Store,
        environment,
        place: Place::Remote {
            api,
            fields: record.fields.clone(),
        },
        can,
        tuning,
        state: RuntimeState::Ready,
    })
}

fn public_traffic(fields: &Value) -> bool {
    match fields.get("public_traffic") {
        Some(Value::Bool(public)) => *public,
        Some(Value::String(said)) => said.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn lives(fields: &Value) -> Tuning {
    Tuning {
        lifetime_secs: fields.get("lifetime_secs").and_then(Value::as_u64),
        max_lifetime_secs: fields.get("max_lifetime_secs").and_then(Value::as_u64),
        ..Tuning::default()
    }
}

fn unavailable(record: &computer_storage::RuntimeRecord, why: String) -> Runtime {
    let absent = Absent(record.provider.clone());
    let environment = absent.environment();
    let can = absent.can();

    Runtime {
        name: record.name.clone(),
        provider: record.provider.clone(),
        secrets: record.secrets.keys().cloned().collect(),
        source: Source::Store,
        environment,
        place: Place::Remote {
            api: Arc::new(absent),
            fields: record.fields.clone(),
        },
        can,
        tuning: lives(&record.fields),
        state: RuntimeState::Unavailable { why },
    }
}

struct Absent(String);

#[async_trait::async_trait]
impl RemoteApi for Absent {
    fn vendor(&self) -> &str {
        &self.0
    }

    async fn available(&self) -> computer::Result<()> {
        Err(computer::Error::Unavailable {
            provider: self.0.clone(),
            detail: "this runtime is not usable on this server".to_string(),
        })
    }

    async fn create(
        &self,
        _plan: &computer::sandboxes::remote::SandboxPlan,
    ) -> computer::Result<computer::sandboxes::remote::Sandbox> {
        self.available().await?;
        unreachable!()
    }

    async fn find(
        &self,
        _name: &str,
    ) -> computer::Result<Option<computer::sandboxes::remote::Sandbox>> {
        Ok(None)
    }

    async fn kill(&self, _id: &str) -> computer::Result<()> {
        self.available().await
    }

    async fn exec(
        &self,
        _sandbox: &computer::sandboxes::remote::Sandbox,
        _argv: &[String],
        _env: &BTreeMap<String, String>,
    ) -> computer::Result<computer::ExecResult> {
        self.available().await?;
        unreachable!()
    }

    async fn read(
        &self,
        _sandbox: &computer::sandboxes::remote::Sandbox,
        _path: &str,
    ) -> computer::Result<Vec<u8>> {
        self.available().await?;
        unreachable!()
    }

    async fn write(
        &self,
        _sandbox: &computer::sandboxes::remote::Sandbox,
        _path: &str,
        _bytes: &[u8],
    ) -> computer::Result<()> {
        self.available().await
    }
}

pub fn engine(name: &str, machine: Arc<dyn Machine>) -> Runtime {
    let mut can = container_can(Vec::new());
    capped(&mut can, &Tuning::default());

    Runtime {
        name: name.to_string(),
        provider: name.to_string(),
        secrets: Vec::new(),
        source: Source::Found,
        environment: Environment::Container(Value::Object(Map::new())),
        place: Place::Host { machine },
        can,
        tuning: Tuning::default(),
        state: RuntimeState::Ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use computer::EngineMachine;
    use computer::testing::{ScriptedEngine, ScriptedRemote};

    fn file(text: &str) -> ServerConfig {
        ServerConfig::parse(text).expect("a file this server reads")
    }

    fn hosts() -> Vec<String> {
        HOSTS.iter().map(|host| host.to_string()).collect()
    }

    fn docker() -> Runtime {
        engine(
            "docker",
            Arc::new(EngineMachine::new(Arc::new(ScriptedEngine::new()))),
        )
    }

    fn cloud() -> Runtime {
        let api = Arc::new(ScriptedRemote::new().keeping(Capabilities {
            resources: Resources::AtImage,
            max_lifetime_secs: Some(LIFETIME_SECS),
            ..Absent("scripted".to_string()).can()
        }));
        let environment = api.environment();
        let can = api.can();

        Runtime {
            name: "cloud".to_string(),
            provider: "e2b".to_string(),
            secrets: Vec::new(),
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

    #[tokio::test]
    async fn test_public_traffic_is_a_field_the_runtime_is_given() {
        for (fields, public) in [
            (serde_json::json!({}), false),
            (serde_json::json!({ "public_traffic": "true" }), true),
            (serde_json::json!({ "public_traffic": true }), true),
            (serde_json::json!({ "public_traffic": "false" }), false),
        ] {
            let api = Arc::new(ScriptedRemote::new());
            let runtime = Runtime {
                place: Place::Remote {
                    api: Arc::clone(&api) as Arc<dyn RemoteApi>,
                    fields: fields.clone(),
                },
                ..cloud()
            };

            let (machine, _) = runtime.pair(DisplayServer::default());
            machine
                .start(
                    "box",
                    &computer::Config {
                        boot: vec!["true".to_string()],
                        ..computer::Config::default()
                    },
                )
                .await
                .expect("started");

            assert_eq!(
                api.plans().pop().expect("one plan").public,
                public,
                "{fields}"
            );
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
        let runtimes = Runtimes::default();
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
        let runtimes = Runtimes::default();
        runtimes.add(engine(
            "podman",
            Arc::new(EngineMachine::new(Arc::new(ScriptedEngine::new()))),
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
        let runtimes = Runtimes::default();
        runtimes.add(docker());

        assert!(runtimes.prefer("cloud").is_err());
    }

    #[test]
    fn test_a_cap_comes_from_the_file_then_the_vendor_then_this_server() {
        let vendor = Capabilities {
            max_lifetime_secs: Some(12 * 60 * 60),
            ..Capabilities::default()
        };

        let mut said = vendor.clone();
        capped(&mut said, &Tuning::default());
        assert_eq!(
            said.max_lifetime_secs,
            Some(12 * 60 * 60),
            "a vendor that states its own limit keeps it"
        );

        let mut told = vendor.clone();
        capped(
            &mut told,
            &Tuning {
                max_lifetime_secs: Some(24 * 60 * 60),
                ..Tuning::default()
            },
        );
        assert_eq!(
            told.max_lifetime_secs,
            Some(24 * 60 * 60),
            "and the operator overrides the vendor"
        );

        let mut quiet = Capabilities::default();
        capped(&mut quiet, &Tuning::default());
        assert_eq!(
            quiet.max_lifetime_secs,
            Some(LIFETIME_SECS),
            "a runtime that says nothing takes this server's hour"
        );
    }

    #[test]
    fn test_a_box_lives_an_hour_unless_the_runtime_says_otherwise() {
        assert_eq!(docker().lifetime(), LIFETIME_SECS);
        assert_eq!(cloud().lifetime(), LIFETIME_SECS);

        let mut longer = docker();
        longer.tuning.lifetime_secs = Some(8 * 60 * 60);

        assert_eq!(longer.lifetime(), 8 * 60 * 60);
    }

    #[test]
    fn test_a_life_longer_than_the_runtime_keeps_a_box_is_refused_before_anything_starts() {
        let placement = Placement {
            expires_after_secs: Some(60 * 60 * 24),
            ..Placement::default()
        };

        for runtime in [docker(), cloud()] {
            let Err(error) = runtime.check(&placement) else {
                panic!("{} took a box it would take away first", runtime.name);
            };
            assert!(
                error.body.message.contains(&LIFETIME_SECS.to_string())
                    && error.body.message.contains("86400"),
                "the refusal carries both numbers: {}",
                error.body.message
            );
            assert!(
                error.body.message.contains("max_lifetime"),
                "and says what to raise: {}",
                error.body.message
            );
        }
    }

    #[test]
    fn test_a_runtime_told_to_keep_a_box_longer_takes_the_longer_life() {
        let placement = Placement {
            expires_after_secs: Some(60 * 60 * 24),
            ..Placement::default()
        };

        let mut day = cloud();
        day.can.max_lifetime_secs = Some(60 * 60 * 24);

        assert!(
            day.check(&placement).is_ok(),
            "an account that keeps a box for a day is the operator's to state, not ours to guess"
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
        let plan = plan(&file("[runtimes.docker]\nmemory = \"4g\"\n"), &hosts(), &[])
            .expect("a file this server takes");

        let (provider, tuning, source) = plan.hosts.get("docker").expect("docker is still offered");
        assert_eq!(provider, "docker");
        assert_eq!(tuning.memory.as_deref(), Some("4g"));
        assert_eq!(*source, Source::File);
    }

    #[test]
    fn test_a_file_table_with_a_provider_offers_the_same_engine_twice() {
        let plan = plan(
            &file("[runtimes.hardened]\nprovider = \"docker\"\nisolation = \"runsc\"\n"),
            &hosts(),
            &[],
        )
        .expect("a file this server takes");

        assert!(
            plan.hosts.contains_key("docker"),
            "the plain engine is still here"
        );
        let (provider, tuning, _) = plan
            .hosts
            .get("hardened")
            .expect("and so is the hardened one");
        assert_eq!(provider, "docker");
        assert_eq!(tuning.isolation.as_deref(), Some("runsc"));
    }

    #[test]
    fn test_an_engine_the_file_turns_off_is_not_offered() {
        let plan = plan(&file("[runtimes.podman]\nenabled = false\n"), &hosts(), &[])
            .expect("a file this server takes");

        assert!(!plan.hosts.contains_key("podman"));
        assert!(plan.hosts.contains_key("docker"));
    }

    #[test]
    fn test_a_file_that_tunes_nothing_is_an_error_rather_than_a_silent_nothing() {
        let why = plan(
            &file("[runtimes.nowhere]\nmemory = \"4g\"\n"),
            &hosts(),
            &[],
        )
        .expect_err("a table naming no provider and tuning nothing");

        assert!(why.contains("nowhere"), "{why}");
    }

    #[test]
    fn test_a_file_adds_host_runtimes_only() {
        let why = plan(
            &file("[runtimes.cloud]\nprovider = \"e2b\"\n"),
            &hosts(),
            &[],
        )
        .expect_err("a vendor needs a key, which a file in git must not hold");

        assert!(
            why.contains("docker"),
            "the refusal says what a file may name: {why}"
        );
    }

    #[test]
    fn test_a_field_of_another_engine_is_refused_by_name() {
        let why = plan(
            &file("[runtimes.podman]\ncontext = \"gpu-1\"\n"),
            &hosts(),
            &[],
        )
        .expect_err("a docker context means nothing to podman");

        assert!(why.contains("runtimes.podman.context"), "{why}");

        let why = plan(&file("[runtimes.docker]\nmemroy = \"4g\"\n"), &hosts(), &[])
            .expect_err("a misspelled field silently doing nothing is worse");

        assert!(why.contains("memroy"), "{why}");
    }

    #[test]
    fn test_only_the_engines_this_server_drives_are_offered() {
        let plan = plan(
            &ServerConfig::default(),
            &["docker".to_string(), "tart".to_string()],
            &[],
        )
        .expect("a list with one name this server cannot drive");

        assert_eq!(plan.hosts.len(), 1);
        assert!(plan.hosts.contains_key("docker"));
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

        let mut tuned = cloud();
        tuned.tuning.max_lifetime_secs = Some(24 * 60 * 60);

        assert_eq!(
            tuned.view(0).fields["max_lifetime_secs"],
            24 * 60 * 60,
            "what an operator told us about a vendor is reported back"
        );
        assert!(view.secrets.is_empty());
        assert_eq!(view.boxes, 2);
        assert_eq!(view.place, PlaceKind::Host);
    }
}

#[cfg(test)]
mod lives {
    use super::*;
    use crate::config::ServerConfig;

    fn file(text: &str) -> ServerConfig {
        ServerConfig::parse(text).expect("a file this server reads")
    }

    #[test]
    fn test_the_file_raises_the_cap_on_a_vendor_it_did_not_add() {
        let plan = plan(
            &file("[runtimes.e2b]\nmax_lifetime = \"24h\"\nlifetime = \"2h\"\n"),
            &[],
            &["e2b".to_string()],
        )
        .expect("a file tuning a vendor the environment named");

        let tuning = plan.remotes.get("e2b").expect("e2b is still offered");
        assert_eq!(tuning.max_lifetime_secs, Some(24 * 60 * 60));
        assert_eq!(tuning.lifetime_secs, Some(2 * 60 * 60));
    }

    #[test]
    fn test_a_vendor_the_file_turns_off_is_not_offered() {
        let plan = plan(
            &file("[runtimes.e2b]\nenabled = false\n"),
            &[],
            &["e2b".to_string()],
        )
        .expect("a file this server takes");

        assert!(plan.remotes.is_empty());
    }

    #[test]
    fn test_an_engine_s_field_is_refused_on_a_vendor() {
        let why = plan(
            &file("[runtimes.e2b]\nmemory = \"4g\"\n"),
            &[],
            &["e2b".to_string()],
        )
        .expect_err("a vendor sets memory when its image is built");

        assert!(why.contains("memory"), "{why}");
    }
}
