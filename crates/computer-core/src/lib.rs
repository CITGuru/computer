//! A computer in a box.
//!
//! ```no_run
//! # extern crate computer_core as computer;
//! use computer::{Button, Computer, Point};
//!
//! # async fn run() -> computer::Result<()> {
//! let box_ = Computer::launch().await?;
//!
//! println!("watch it at {}", box_.viewer_url().unwrap_or_default());
//! box_.open_url("https://example.com").await?;
//!
//! let png = box_.screenshot().await?;
//! box_.click(Point::new(640, 400), Button::Left).await?;
//! box_.type_text("hello from rust").await?;
//!
//! box_.shutdown().await?;
//! # Ok(()) }
//! ```

mod auth;
mod desktop;
mod error;
mod exec;
pub mod reach;
mod secret;

pub mod apps;
pub mod audit;
pub mod bundle;
pub mod cdp;
pub mod image;
pub mod machine;
pub mod microvm;
pub mod motion;

/// The most lines a search answers with. `grep -rn "the" /usr/share` is 96,912
/// on a plain box.
pub const MATCHES: usize = 200;
pub mod profile;
pub mod runtime;
pub mod sandboxes;
pub mod screens;
pub mod servers;
pub mod spec;
pub mod testing;

pub use audit::{Audit, audit};
pub use auth::{AUTH_ENV, Auth, CONTROL_SECRET_ENV, Credentials, VIEW_SECRET_ENV, VIEWER_USER};
pub use cdp::{
    BrowserGroup, BrowserStore, Carry, Changes, Cookie, Database, Devtools, Element, Link, Page,
    PageText, Reading, Scroll, SearchProvider, Session, Snapshot, Target,
};
pub use desktop::{
    Browser, BrowserEndpoint, Button, Clipboard, Control, Delta, Desktop, DesktopFactory,
    DesktopNeed, DesktopPresence, DesktopSupport, Display, DisplayServer, Held, Keys, Node,
    NodeQuery, Of, Point, Press, Rect, Selection, Shot, Typing, Viewer, ViewerKind, Viewers,
};
pub use error::{Error, Result};

/// Caps a capture's scale, so a mistyped percentage cannot exhaust the box's memory.
pub const MAGNIFY: u32 = 400;
pub use exec::ExecResult;
pub use image::{ScreenAction, ScreenPorts};
pub use machine::ScreenHost;
pub use machine::{DockerMachine, Machine, MachineHost, PortMap};
pub use microvm::MicroVm;
pub use profile::{
    AppRuntime, Arrange, BrowserRuntime, CommandBrowserRuntime, CommandScreen,
    CommandScreenRuntime, CommandWallpaperRuntime, ConfiguredProfile, DesktopContract, FORCE,
    GeometrySpec, ImageSource, Launch, PROFILE_ENV, PROFILE_LABEL, PortLayout, Profile,
    ProfileBuilder, Recording, SHARED, ScreenCommands, ScreenEnvironment, ScreenRuntime,
    UnsupportedAppRuntime, UnsupportedWallpaperRuntime, ViewerUrl, WallpaperRuntime,
    WaylandAppRuntime, WaylandEnvironment, WaylandWallpaperRuntime, Window, X11AppRuntime,
    X11Environment, X11WallpaperRuntime,
};
pub use reach::{Address, Bind, Reach, Scheme};
pub use runtime::{Config, ContainerCli, SystemDocker};
pub use screens::{ControlGate, DEFAULT_LEASE, ScreenLease, Screens};
pub use secret::Secret;
pub use servers::wayland::{WaylandDesktop, WaylandDriver, WaylandProfile};
pub use servers::x11::{X11Desktop, X11Driver, X11Profile};
pub use spec::Resolved;

/// Aliased, not glob-imported: several of its names clash with this crate's own types.
pub use computer_types as types;
pub use computer_types::{DirEntry, Match, Motion, Placement, Search, Spec};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Screens are numbered from zero. Screen *N* is display `:N+1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScreenId(pub u32);

impl std::fmt::Display for ScreenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "screen {}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HolderId(String);

impl HolderId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HolderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Written on the box too, so a box that outlives its process can still be swept.
pub const EXPIRY_LABEL: &str = "computer.expires-at";

pub const READY_TIMEOUT: Duration = Duration::from_secs(90);

pub struct Builder {
    config: Config,
    machine: Option<Arc<dyn Machine>>,
    profile: Arc<dyn Profile>,
    driver: Option<Arc<dyn DesktopFactory>>,
    image: Option<String>,
    image_dir: Option<PathBuf>,
    size: Option<(u32, u32)>,
    publish: bool,
    cli: Option<Arc<dyn ContainerCli>>,
    program: String,
    name: Option<String>,
    ensure_image: bool,
    wait: Option<Duration>,
    keep: bool,
    ttl: Option<Duration>,
    idle: Option<Duration>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            config: Config::default(),
            machine: None,
            profile: Arc::new(X11Profile),
            driver: None,
            image: None,
            image_dir: None,
            size: None,
            publish: true,
            cli: None,
            program: "docker".to_string(),
            name: None,
            ensure_image: true,
            wait: Some(READY_TIMEOUT),
            keep: false,
            ttl: None,
            idle: None,
        }
    }
}

impl Builder {
    /// Pulled, never built, so [`Builder::packages`] is refused.
    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.image = Some(image.into());
        self.image_dir = None;
        self
    }

    pub fn image_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.image = None;
        self.image_dir = Some(directory.into());
        self
    }

    pub fn profile(mut self, profile: Arc<dyn Profile>) -> Self {
        self.profile = profile;
        self
    }

    pub fn packages(mut self, packages: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.config.extras = bundle::Extras::with(packages);
        self
    }

    /// Replaces the launchers; set it after the packages, which reset them.
    pub fn launchers(mut self, launchers: impl IntoIterator<Item = bundle::Launcher>) -> Self {
        self.config.extras = self.config.extras.clone().with_launchers(launchers);
        self
    }

    pub fn packages_from(
        mut self,
        packages: impl IntoIterator<Item = impl Into<String>>,
        sources: impl IntoIterator<Item = bundle::AptSource>,
    ) -> Self {
        self.config.extras = bundle::Extras::from_sources(packages, sources);
        self
    }

    pub fn dock(self) -> Self {
        let wanted = bundle::Extras::dock();
        self.packages(wanted.packages)
    }

    pub fn wide_fonts(self) -> Self {
        let wanted = bundle::Extras::wide_fonts();
        self.packages(wanted.packages)
    }

    /// Launch-time only: an app joins the tree only if the bus exists before its first window.
    pub fn accessibility(self) -> Self {
        let wanted = bundle::Extras::accessibility();
        self.packages(wanted.packages)
    }

    pub fn video(self) -> Self {
        let wanted = bundle::Extras::video();
        self.packages(wanted.packages)
    }

    pub fn runtime(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    pub fn cli(mut self, cli: Arc<dyn ContainerCli>) -> Self {
        self.cli = Some(cli);
        self
    }

    pub fn machine(mut self, machine: Arc<dyn Machine>) -> Self {
        self.machine = Some(machine);
        self
    }

    pub fn driver(mut self, driver: Arc<dyn DesktopFactory>) -> Self {
        self.driver = Some(driver);
        self
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width, height));
        self
    }

    pub fn network(mut self, on: bool) -> Self {
        self.config.network = on;
        self
    }

    pub fn publish_ports(mut self, publish: bool) -> Self {
        self.publish = publish;
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.env.insert(key.into(), value.into());
        self
    }

    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.labels.insert(key.into(), value.into());
        self
    }

    pub fn memory(mut self, limit: impl Into<String>) -> Self {
        self.config.memory = Some(limit.into());
        self
    }

    pub fn cpus(mut self, cpus: impl Into<String>) -> Self {
        self.config.cpus = Some(cpus.into());
        self
    }

    pub fn isolation(mut self, isolation: impl Into<String>) -> Self {
        self.config.isolation = Some(isolation.into());
        self
    }

    /// One box per volume at a time: two browsers sharing a profile directory corrupt it.
    /// A cookie set just before removal may be lost: Chromium flushes on its own schedule.
    pub fn profiles(mut self, volume: impl Into<String>) -> Self {
        self.config.profiles = Some(volume.into());
        self
    }

    pub fn shm_size(mut self, size: impl Into<String>) -> Self {
        self.config.shm_size = Some(size.into());
        self
    }

    pub fn ensure_image(mut self, ensure: bool) -> Self {
        self.ensure_image = ensure;
        self
    }

    pub fn wait_for_ready(mut self, within: Option<Duration>) -> Self {
        self.wait = within;
        self
    }

    pub fn expires_after(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Only activity through this handle counts; other work needs [`Computer::touch`].
    pub fn expires_when_idle(mut self, idle: Duration) -> Self {
        self.idle = Some(idle);
        self
    }

    pub fn keep_on_drop(mut self, keep: bool) -> Self {
        self.keep = keep;
        self
    }

    pub fn publish_on(mut self, bind: Bind) -> Self {
        self.config.bind = bind;
        self
    }

    pub fn auth(mut self, auth: Auth) -> Self {
        self.config.auth = auth;
        self
    }

    pub fn credentials(mut self, credentials: Credentials) -> Self {
        self.config.credentials = Some(credentials);
        self
    }

    pub fn advertise(mut self, host: impl Into<String>) -> Self {
        self.config.advertise = Some(host.into());
        self
    }

    /// Carries no credential: secrets are minted at launch, so this is safe to log.
    pub fn preview(&self) -> Result<Vec<String>> {
        Ok(runtime::run_args(
            self.name.as_deref().unwrap_or("computer-preview"),
            &self.config()?,
        ))
    }

    fn source(&self) -> ImageSource {
        match &self.image {
            Some(name) => ImageSource::Registry(name.clone()),
            None => self.profile.image(),
        }
    }

    pub fn config(&self) -> Result<Config> {
        let mut config = self.config.clone();

        let (width, height) = self.size.unwrap_or_else(|| self.profile.default_size());
        config.width = width;
        config.height = height;
        let source = self.source();
        match self.image_dir.as_deref().or_else(|| source.directory()) {
            Some(directory) => {
                let (directory, image) = bundle::directory_image(directory, &config.extras)?;
                config.bundle = None;
                config.image_dir = Some(directory);
                config.image = image;
            }
            None => {
                config.bundle = source.bundle().copied();
                config.image_dir = None;
                config.image = source.tag(&config.extras)?;
            }
        }
        config.boot = self.profile.boot_command();
        config.publish = if self.publish {
            self.profile.ports().to_publish()
        } else {
            Vec::new()
        };

        // The profile's first, so a variable the caller set by hand wins.
        let mut env = self.profile.launch_env(width, height);
        env.insert(
            profile::PROFILE_ENV.to_string(),
            self.profile.name().to_string(),
        );
        env.extend(config.env.clone());
        config.env = env;

        Ok(config)
    }

    pub async fn launch(self) -> Result<Computer> {
        let machine: Arc<dyn Machine> = match (&self.machine, &self.cli) {
            (Some(machine), _) => Arc::clone(machine),
            (None, Some(cli)) => Arc::new(DockerMachine::new(Arc::clone(cli))),
            (None, None) => Arc::new(DockerMachine::new(Arc::new(SystemDocker::new(
                self.program.clone(),
            )))),
        };

        machine.preflight().await?;

        let mut config = self.config()?;

        // Asked of the machine, not the bind: E2B publishes a hostname per port.
        let routable = !config.publish.is_empty() && machine.reach(&config).needs_a_secret();

        if routable && !config.auth.is_gated() {
            return Err(Error::denied(
                "this box publishes beyond loopback with an open viewer: the \
                 view and control ports would accept anyone who reaches them, \
                 and the control port drives the desktop. Choose \
                 Auth::Password or Auth::Token.",
            ));
        }

        // CDP has no authentication, and a forward cannot add one to a WebSocket upgrade.
        if routable && let Some(bridge) = self.profile.ports().devtools_bridge {
            config.publish.retain(|port| *port != bridge);
        }

        if config.auth.is_gated() {
            let credentials = match config.credentials.take() {
                Some(supplied) => supplied,
                None => Credentials::generate()?,
            };
            // In the container environment, so a screen opened later takes the same credential.
            config
                .env
                .insert(auth::AUTH_ENV.to_string(), config.auth.as_str().to_string());
            config.env.insert(
                auth::VIEW_SECRET_ENV.to_string(),
                credentials.view.expose().to_string(),
            );
            config.env.insert(
                auth::CONTROL_SECRET_ENV.to_string(),
                credentials.control.expose().to_string(),
            );
            config.credentials = Some(credentials);
        }

        if self.ensure_image {
            machine.ensure_image(&config).await?;
        }

        if let Some(declared) = machine.image_contract(&config.image).await
            && declared != self.profile.name()
        {
            return Err(Error::denied(format!(
                "{} implements the {declared} contract and this box is driven \
                 by {}: the commands would go in and the screen would not move",
                config.image,
                self.profile.name()
            )));
        }

        let name = self.name.clone().unwrap_or_else(unique_name);
        let expires_at = self.ttl.map(|ttl| SystemTime::now() + ttl);
        if let Some(at) = expires_at {
            config.labels.insert(
                EXPIRY_LABEL.to_string(),
                at.duration_since(UNIX_EPOCH)
                    .map(|since| since.as_secs())
                    .unwrap_or(0)
                    .to_string(),
            );
        }

        let mapped = machine.start(&name, &config).await?;
        tracing::info!(
            box_ = %name,
            image = %config.image,
            profile = %self.profile.name(),
            runtime = %machine.runtime(),
            "box opened"
        );

        let cleanup = (!self.keep)
            .then(|| machine.reaper(&name))
            .flatten()
            .map(|(program, args)| Cleanup { program, args });

        let driver = self.driver.clone().unwrap_or_else(|| self.profile.driver());

        let host = MachineHost::new(
            Arc::clone(&machine),
            Arc::clone(&self.profile),
            name.clone(),
        )
        .advertised_at(
            Scheme::Http,
            config
                .advertise
                .clone()
                .unwrap_or_else(|| config.bind.publish_prefix()),
        )
        .gated_by(config.auth, config.credentials.clone());

        let mut support = driven_by(
            self.profile.support_at(config.width, config.height),
            driver.as_ref(),
        );
        if routable && let Some(browser) = support.browser.as_mut() {
            // Withdrawn rather than broken, so `audit` skips the check instead of failing it.
            browser.cdp = false;
        }

        let mut computer = Computer::assemble(
            Arc::new(host),
            Arc::clone(&driver),
            support,
            mapped,
            cleanup,
        );
        computer.expires_at = expires_at;

        if let Some(ttl) = self.ttl {
            let doomed = Arc::clone(&machine);
            let condemned = name.clone();
            tokio::spawn(async move {
                tokio::time::sleep(ttl).await;
                reap(doomed, condemned, "its life ran out").await;
            });
        }

        if let Some(idle) = self.idle {
            let doomed = Arc::clone(&machine);
            let condemned = name.clone();
            let active_at = computer.host.active_at();

            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(idle).await;

                    let last = active_at.load(std::sync::atomic::Ordering::Relaxed);
                    let quiet = Duration::from_nanos(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|since| since.as_nanos() as u64)
                            .unwrap_or(0)
                            .saturating_sub(last),
                    );

                    if quiet >= idle {
                        reap(doomed, condemned, "it went idle").await;
                        return;
                    }
                }
            });
        }

        if let Some(within) = self.wait {
            if let Err(error) = computer.wait_until_ready(within).await {
                return Err(match (error, computer.logs().await) {
                    (Error::Timeout { after, detail }, Ok(logs)) if !logs.trim().is_empty() => {
                        Error::Timeout {
                            after,
                            detail: format!("{detail}; the box said: {}", logs.trim()),
                        }
                    }
                    (error, _) => error,
                });
            }
        }

        Ok(computer)
    }
}

fn driven_by(mut support: DesktopSupport, driver: &dyn DesktopFactory) -> DesktopSupport {
    if let Some(display) = support.display.as_mut() {
        display.server = driver.display_server();
    }
    support
}

/// A restarting runtime answers nothing for a few seconds.
const REAP_ATTEMPTS: u32 = 5;

const REAP_PAUSE: Duration = Duration::from_secs(10);

async fn reap(machine: Arc<dyn Machine>, name: String, because: &'static str) {
    for attempt in 1..=REAP_ATTEMPTS {
        match machine.remove(&name).await {
            Ok(()) => {
                tracing::info!(box_ = %name, reason = because, "box removed");
                return;
            }
            Err(Error::Gone(_)) => return,
            Err(error) => {
                tracing::warn!(
                    box_ = %name,
                    reason = because,
                    attempt,
                    of = REAP_ATTEMPTS,
                    %error,
                    "the box would not go away"
                );
            }
        }

        if attempt < REAP_ATTEMPTS {
            tokio::time::sleep(REAP_PAUSE).await;
        }
    }

    tracing::error!(
        box_ = %name,
        reason = because,
        "the box is still running after every attempt to remove it, and \
         nothing else here is watching it: it holds its processor and its \
         memory until somebody takes it away"
    );
}

struct Cleanup {
    program: String,
    args: Vec<String>,
}

#[derive(Clone)]
struct ProfileRuntimes {
    screen: Arc<dyn ScreenRuntime>,
    browser: Arc<dyn BrowserRuntime>,
    wallpaper: Arc<dyn WallpaperRuntime>,
    app: Arc<dyn AppRuntime>,
}

impl ProfileRuntimes {
    fn from_profile(profile: &dyn Profile) -> Self {
        Self {
            screen: profile.screen_runtime(),
            browser: profile.browser_runtime(),
            wallpaper: profile.wallpaper_runtime(),
            app: profile.app_runtime(),
        }
    }
}

pub struct Computer {
    machine: Arc<dyn Machine>,
    profile: Arc<dyn Profile>,
    runtimes: ProfileRuntimes,
    driver: Arc<dyn DesktopFactory>,
    host: Arc<MachineHost>,
    name: String,
    support: DesktopSupport,
    mapped: PortMap,
    screen_registry: Arc<Screens>,
    primary: Screen,
    cleanup: Option<Cleanup>,
    expires_at: Option<SystemTime>,
}

impl Computer {
    pub async fn launch() -> Result<Self> {
        Self::builder().launch().await
    }

    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Never removed on drop: this process did not create it.
    pub async fn attach(name: impl Into<String>) -> Result<Self> {
        Self::attach_to(Arc::new(DockerMachine::default()), name).await
    }

    pub async fn attach_to(machine: Arc<dyn Machine>, name: impl Into<String>) -> Result<Self> {
        let name = name.into();

        if !machine.running(&name).await? {
            return Err(Error::Gone(name));
        }

        let environment = machine.env(&name).await;
        let profile = environment
            .get(profile::PROFILE_ENV)
            .and_then(|said| profile::builtin(said))
            .unwrap_or_else(|| Arc::new(X11Profile));

        Self::pick_up(machine, name, profile, None, environment).await
    }

    pub async fn attach_using(
        machine: Arc<dyn Machine>,
        name: impl Into<String>,
        profile: Arc<dyn Profile>,
        driver: Option<Arc<dyn DesktopFactory>>,
    ) -> Result<Self> {
        let name = name.into();

        if !machine.running(&name).await? {
            return Err(Error::Gone(name));
        }

        let environment = machine.env(&name).await;
        Self::pick_up(machine, name, profile, driver, environment).await
    }

    /// Only [`Computer::start`] and [`Computer::shutdown`] work on the handle this answers.
    pub async fn attach_stopped(
        machine: Arc<dyn Machine>,
        name: impl Into<String>,
        profile: Arc<dyn Profile>,
        driver: Option<Arc<dyn DesktopFactory>>,
    ) -> Result<Self> {
        let name = name.into();
        let environment = machine.env(&name).await;

        Self::pick_up(machine, name, profile, driver, environment).await
    }

    async fn pick_up(
        machine: Arc<dyn Machine>,
        name: String,
        profile: Arc<dyn Profile>,
        driver: Option<Arc<dyn DesktopFactory>>,
        environment: BTreeMap<String, String>,
    ) -> Result<Self> {
        let driver = driver.unwrap_or_else(|| profile.driver());
        let (width, height) = profile
            .geometry_from(&environment)
            .unwrap_or_else(|| profile.default_size());
        let support = driven_by(profile.support_at(width, height), driver.as_ref());

        let mapped = machine.ports(&name).await;
        let (auth, credentials) = auth::from_environment(&environment);
        let host = MachineHost::new(machine, profile, name).gated_by(auth, credentials);
        let computer = Self::assemble(Arc::new(host), driver, support, mapped, None);

        // The gate is per process, so a takeover already running is asked of the box.
        if computer.person_driving().await {
            computer
                .primary
                .control()
                .hand_over("a takeover already running in the box", SystemTime::now());
        }

        Ok(computer)
    }

    pub async fn attach_with(cli: Arc<dyn ContainerCli>, name: impl Into<String>) -> Result<Self> {
        Self::attach_to(Arc::new(DockerMachine::new(cli)), name).await
    }

    fn assemble(
        host: Arc<MachineHost>,
        driver: Arc<dyn DesktopFactory>,
        support: DesktopSupport,
        mapped: PortMap,
        cleanup: Option<Cleanup>,
    ) -> Self {
        let machine = Arc::clone(host.machine());
        let profile = Arc::clone(host.profile());
        let runtimes = ProfileRuntimes::from_profile(profile.as_ref());
        let name = host.name().to_string();

        let primary = Screen::new(
            Arc::clone(&profile),
            runtimes.clone(),
            driver.as_ref(),
            Arc::clone(&host),
            ScreenId(0),
            mapped.clone(),
        );

        Self {
            screen_registry: Arc::new(Screens::new(support.max_screens)),
            machine,
            profile,
            runtimes,
            driver,
            host,
            name,
            support,
            mapped,
            primary,
            cleanup,
            expires_at: None,
        }
    }

    pub fn runtime(&self) -> &str {
        self.machine.runtime()
    }

    pub async fn pause(&self) -> Result<()> {
        self.touch();
        self.machine.pause(&self.name).await
    }

    pub async fn resume(&self) -> Result<()> {
        self.touch();
        self.machine.resume(&self.name).await
    }

    /// `false` on a runtime that cannot freeze one.
    pub async fn paused(&self) -> Result<bool> {
        self.machine.paused(&self.name).await
    }

    pub async fn stop(&self) -> Result<()> {
        self.touch();
        self.machine.halt(&self.name).await
    }

    /// A new handle, which does not own the box: the runtime picks new host ports on every start.
    pub async fn start(&self, within: Duration) -> Result<Self> {
        self.machine.wake(&self.name).await?;

        let environment = self.machine.env(&self.name).await;
        let woken = Self::pick_up(
            Arc::clone(&self.machine),
            self.name.clone(),
            Arc::clone(&self.profile),
            Some(Arc::clone(&self.driver)),
            environment,
        )
        .await?;

        woken.wait_until_ready(within).await?;

        Ok(woken)
    }

    pub async fn stopped(&self) -> Result<bool> {
        Ok(!self.machine.running(&self.name).await?)
    }

    pub fn expires_at(&self) -> Option<SystemTime> {
        self.expires_at
    }

    pub fn idle_for(&self) -> Duration {
        self.host.idle_for()
    }

    pub fn touch(&self) {
        self.host.touch();
    }

    pub fn expired(&self) -> bool {
        self.expires_at
            .map(|at| SystemTime::now() >= at)
            .unwrap_or(false)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn credentials(&self) -> Option<&Credentials> {
        self.host.gate().1
    }

    pub fn support(&self) -> &DesktopSupport {
        &self.support
    }

    pub fn primary(&self) -> &Screen {
        &self.primary
    }

    pub fn leases(&self) -> &Screens {
        &self.screen_registry
    }

    pub async fn screen(&self, screen: ScreenId) -> Result<LeasedScreen> {
        self.take(screen, &process_holder(), 0).await
    }

    pub async fn screen_unfenced(&self, screen: ScreenId) -> Result<Screen> {
        if screen.0 >= self.support.max_screens {
            return Err(Error::ScreenUnavailable {
                screen: Some(screen),
                held_by: None,
            });
        }

        self.runtimes
            .screen
            .start(self.host.as_ref(), self.profile.as_ref(), screen)
            .await?;

        Ok(Screen::new(
            Arc::clone(&self.profile),
            self.runtimes.clone(),
            self.driver.as_ref(),
            Arc::clone(&self.host),
            screen,
            self.mapped.clone(),
        ))
    }

    pub async fn claim(&self, holder: &HolderId, fence: u64) -> Result<LeasedScreen> {
        let lease = self
            .screen_registry
            .claim(holder, fence, SystemTime::now(), DEFAULT_LEASE)?;

        match self.screen_unfenced(lease.screen).await {
            Ok(screen) => Ok(LeasedScreen {
                screen,
                lease,
                leases: Arc::clone(&self.screen_registry),
            }),
            Err(error) => {
                let _ = self.screen_registry.release(&lease);
                Err(error)
            }
        }
    }

    /// Succeeds when the fence is higher than the held one.
    pub async fn take(
        &self,
        screen: ScreenId,
        holder: &HolderId,
        fence: u64,
    ) -> Result<LeasedScreen> {
        let lease =
            self.screen_registry
                .take(screen, holder, fence, SystemTime::now(), DEFAULT_LEASE)?;

        match self.screen_unfenced(screen).await {
            Ok(screen) => Ok(LeasedScreen {
                screen,
                lease,
                leases: Arc::clone(&self.screen_registry),
            }),
            Err(error) => {
                let _ = self.screen_registry.release(&lease);
                Err(error)
            }
        }
    }

    pub async fn close_screen(&self, screen: ScreenId) -> Result<()> {
        self.runtimes
            .screen
            .stop(self.host.as_ref(), self.profile.as_ref(), screen)
            .await
    }

    pub async fn probe(&self) -> DesktopPresence {
        let alive = Desktop::alive(&self.primary).await;

        let browser = match self.devtools_port_in_box() {
            None => false,
            Some(port) => {
                servers::x11::port_listening(self.host.as_ref(), self.primary.id(), port).await
            }
        };

        DesktopPresence {
            display: alive.is_ok(),
            browser,
            detail: alive.err().map(|error| error.to_string()),
        }
    }

    pub async fn wait_until_ready(&self, within: Duration) -> Result<DesktopPresence> {
        let deadline = SystemTime::now() + within;

        loop {
            let last = self.probe().await;
            if last.ready() {
                return Ok(last);
            }
            if SystemTime::now() >= deadline {
                return Err(Error::Timeout {
                    after: within,
                    detail: format!("display={} browser={}", last.display, last.browser),
                });
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Not gated on `cdp`, which is reachability from here, not whether the browser is up.
    fn devtools_port_in_box(&self) -> Option<u16> {
        self.support.browser.as_ref()?;
        self.profile.ports().devtools
    }

    pub fn machine(&self) -> &Arc<dyn Machine> {
        &self.machine
    }

    pub fn profile(&self) -> &Arc<dyn Profile> {
        &self.profile
    }

    pub fn viewer_url(&self) -> Option<String> {
        self.primary.viewer_url()
    }

    pub fn devtools(&self) -> Option<BrowserEndpoint> {
        let bridge = self.profile.ports().devtools_bridge?;
        let port = self.mapped.get(&bridge)?;
        Some(BrowserEndpoint {
            http_url: format!("http://127.0.0.1:{port}"),
            ws_url: format!("ws://127.0.0.1:{port}/devtools/browser"),
        })
    }

    pub fn ports(&self) -> &PortMap {
        &self.mapped
    }

    pub async fn open_url(&self, url: &str) -> Result<()> {
        self.primary.open_url(url).await
    }

    pub async fn set_wallpaper(&self, image: &[u8]) -> Result<()> {
        self.primary.set_wallpaper(image).await
    }

    pub fn browser(&self) -> Option<Devtools> {
        self.devtools()
            .as_ref()
            .and_then(|endpoint| Devtools::from_endpoint(endpoint).ok())
    }

    pub async fn hand_over(&self) -> Result<Takeover> {
        self.primary.hand_over().await
    }

    pub async fn share(&self) -> Result<Takeover> {
        self.primary.share().await
    }

    pub async fn person_driving(&self) -> bool {
        self.primary.person_driving().await
    }

    pub async fn viewers(&self) -> Result<Viewers> {
        self.primary.viewers().await
    }

    pub async fn start_recording(&self, fps: Option<u32>) -> Result<String> {
        self.primary.start_recording(fps).await
    }

    pub async fn stop_recording(&self) -> Result<String> {
        self.primary.stop_recording().await
    }

    pub async fn recording(&self) -> Result<Option<String>> {
        self.primary.recording().await
    }

    pub async fn clipboard(&self) -> Result<String> {
        self.primary.clipboard().await
    }

    pub async fn set_clipboard(&self, text: &str) -> Result<()> {
        self.primary.set_clipboard(text).await
    }

    pub async fn selection(&self, selection: Selection) -> Result<String> {
        self.primary.selection(selection).await
    }

    pub async fn clipboard_bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>> {
        self.primary.clipboard_bytes(selection, target).await
    }

    pub async fn clipboard_targets(&self, selection: Selection) -> Result<Vec<String>> {
        self.primary.clipboard_targets(selection).await
    }

    pub async fn record(&self, duration: Duration, path: &str) -> Result<()> {
        self.primary.record(duration, path).await
    }

    pub async fn set_clipboard_bytes(
        &self,
        selection: Selection,
        target: &str,
        bytes: &[u8],
    ) -> Result<()> {
        self.primary
            .set_clipboard_bytes(selection, target, bytes)
            .await
    }

    pub async fn set_selection(&self, selection: Selection, text: &str) -> Result<()> {
        self.primary.set_selection(selection, text).await
    }

    pub async fn wait_until_free(&self, within: Duration) -> Result<Viewers> {
        self.primary.wait_until_free(within).await
    }

    pub async fn reclaim(&self) -> Result<()> {
        self.primary.reclaim().await
    }

    pub async fn exec<I, S>(&self, argv: I) -> Result<ExecResult>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let argv: Vec<String> = argv.into_iter().map(Into::into).collect();
        if argv.is_empty() {
            return Err(Error::denied("an empty command has nothing to run"));
        }
        self.host.exec(&argv).await
    }

    pub async fn exec_on<I, S>(&self, screen: ScreenId, argv: I) -> Result<ExecResult>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let argv: Vec<String> = argv.into_iter().map(Into::into).collect();
        if argv.is_empty() {
            return Err(Error::denied("an empty command has nothing to run"));
        }
        self.host.run(&argv, screen).await
    }

    pub async fn write_file(&self, path: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
        self.touch();
        self.machine
            .write_file(&self.name, path.as_ref(), bytes)
            .await
    }

    pub async fn read_file(&self, path: impl AsRef<Path>) -> Result<Vec<u8>> {
        self.touch();
        self.machine.read_file(&self.name, path.as_ref()).await
    }

    /// One directory, not a walk.
    pub async fn list_dir(&self, path: impl AsRef<Path>) -> Result<Vec<DirEntry>> {
        self.touch();

        let at = path.as_ref().display().to_string();
        let result = self
            .exec(&[
                "find".into(),
                at.clone(),
                "-maxdepth".into(),
                "1".into(),
                "-mindepth".into(),
                "1".into(),
                "-printf".into(),
                // Tab separated, name last: a name may hold anything but a tab
                // and a newline, and neither of the first two fields can.
                "%y\t%s\t%f\n".into(),
            ])
            .await?;

        if !result.ok() {
            return Err(Error::invalid(format!(
                "{at}: {}",
                result.stderr_utf8().trim()
            )));
        }

        Ok(result
            .stdout_utf8()
            .lines()
            .filter_map(DirEntry::parse)
            .collect())
    }

    /// Capped in the box, so the bytes never cross the wire.
    pub async fn grep(&self, search: &Search) -> Result<Vec<Match>> {
        self.touch();

        let limit = search.limit.unwrap_or(MATCHES).clamp(1, MATCHES);
        let mut argv = vec![
            "grep".to_string(),
            "-rn".to_string(),
            // A binary that happens to hold the pattern is not a line anyone
            // can read, and one line of it can be megabytes.
            "--binary-files=without-match".to_string(),
        ];

        if search.ignore_case {
            argv.push("-i".to_string());
        }
        if let Some(include) = &search.include {
            argv.push(format!("--include={include}"));
        }
        argv.push("-e".to_string());
        argv.push(search.pattern.clone());
        argv.push(search.path.clone());

        // One past the cap, so a caller can tell a result that filled it from
        // one that happened to land on it.
        let result = self.exec_shell(&argv, limit + 1).await?;

        Ok(result.lines().filter_map(Match::parse).collect())
    }

    /// `*.log`, or a pattern with a slash matched against the whole path.
    pub async fn glob(
        &self,
        pattern: &str,
        path: &str,
        limit: Option<usize>,
    ) -> Result<Vec<String>> {
        self.touch();

        let limit = limit.unwrap_or(MATCHES).clamp(1, MATCHES);
        let argv = vec![
            "find".to_string(),
            path.to_string(),
            match pattern.contains('/') {
                true => "-path".to_string(),
                false => "-name".to_string(),
            },
            pattern.to_string(),
        ];

        let result = self.exec_shell(&argv, limit + 1).await?;

        Ok(result
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Through a shell for the pipe: cutting out here would mean carrying
    /// every line out of the box first, which is what the cap avoids.
    async fn exec_shell(&self, argv: &[String], limit: usize) -> Result<String> {
        let quoted = argv
            .iter()
            .map(|one| format!("'{}'", one.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ");

        let result = self
            .exec(&[
                "sh".to_string(),
                "-c".to_string(),
                format!("{quoted} 2>/dev/null | head -n {limit}"),
            ])
            .await?;

        Ok(result.stdout_utf8())
    }

    pub async fn upload(&self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
        self.touch();
        self.machine
            .upload(&self.name, from.as_ref(), to.as_ref())
            .await
    }

    pub async fn download(&self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
        self.touch();
        self.machine
            .download(&self.name, from.as_ref(), to.as_ref())
            .await
    }

    pub async fn exec_within<I, S>(&self, argv: I, within: Duration) -> Result<ExecResult>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let argv: Vec<String> = argv.into_iter().map(Into::into).collect();
        if argv.is_empty() {
            return Err(Error::denied("an empty command has nothing to run"));
        }
        self.host.run_within(&argv, &BTreeMap::new(), within).await
    }

    pub async fn logs(&self) -> Result<String> {
        self.machine.logs(&self.name).await
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let outcome = self.machine.remove(&self.name).await;

        // Cleared whatever the runtime answered, so the drop does not remove it again.
        self.cleanup = None;
        outcome
    }

    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        self.primary.screenshot().await
    }

    pub async fn capture(&self, shot: &Shot) -> Result<Vec<u8>> {
        self.primary.capture(shot).await
    }

    pub async fn move_to(&self, at: impl Into<Point>) -> Result<()> {
        self.primary.move_to(at).await
    }

    pub async fn click(&self, at: impl Into<Point>, button: Button) -> Result<()> {
        self.primary.click(at, button).await
    }

    pub async fn click_with(
        &self,
        at: impl Into<Point>,
        button: Button,
        held: &[Held],
    ) -> Result<()> {
        self.primary.click_with(at, button, held).await
    }

    pub async fn double_click(&self, at: impl Into<Point>, button: Button) -> Result<()> {
        self.primary.double_click(at, button).await
    }

    pub async fn drag(
        &self,
        from: impl Into<Point>,
        to: impl Into<Point>,
        button: Button,
    ) -> Result<()> {
        self.primary.drag(from, to, button).await
    }

    pub async fn drag_with(
        &self,
        from: impl Into<Point>,
        to: impl Into<Point>,
        button: Button,
        held: &[Held],
    ) -> Result<()> {
        self.primary.drag_with(from, to, button, held).await
    }

    pub async fn wait_until_still(&self, settle: Duration, within: Duration) -> Result<()> {
        self.primary.wait_until_still(settle, within).await
    }

    pub fn type_text(&self, text: impl Into<String>) -> Typing<'_> {
        Typing::new(&self.primary, text)
    }

    pub fn press(&self, keys: impl Keys) -> Press<'_> {
        Press::new(&self.primary, keys)
    }

    pub async fn scroll(&self, at: impl Into<Point>, by: Delta) -> Result<()> {
        self.primary.scroll(at, by).await
    }

    pub async fn cursor(&self) -> Result<Point> {
        self.primary.cursor().await
    }

    pub async fn find_cursor(&self) -> Result<Point> {
        self.primary.find_cursor().await
    }

    pub async fn nodes(&self, app: Option<&str>, depth: Option<u32>) -> Result<Vec<Node>> {
        self.primary.nodes(app, depth).await
    }

    pub async fn find_nodes(&self, query: &NodeQuery, limit: Option<usize>) -> Result<Vec<Node>> {
        self.primary.find_nodes(query, limit).await
    }

    pub async fn focus_node(&self, query: &NodeQuery) -> Result<Node> {
        self.primary.focus_node(query).await
    }

    pub async fn invoke_node(&self, query: &NodeQuery, action: Option<&str>) -> Result<Node> {
        self.primary.invoke_node(query, action).await
    }

    pub async fn set_node(&self, query: &NodeQuery, value: &str) -> Result<Node> {
        self.primary.set_node(query, value).await
    }
}

impl std::fmt::Debug for Computer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Computer")
            .field("name", &self.name)
            .field("image", &self.profile.name())
            .field("screens", &self.support.max_screens)
            .field("viewer", &self.viewer_url())
            .finish()
    }
}

impl Drop for Computer {
    fn drop(&mut self) {
        let Some(cleanup) = self.cleanup.take() else {
            return;
        };

        if let Err(error) = std::process::Command::new(&cleanup.program)
            .args(&cleanup.args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            tracing::warn!(
                box_ = %self.name,
                program = %cleanup.program,
                %error,
                "the box could not be handed to the runtime for removal, and \
                 will hold its memory until something else takes it away"
            );
        }
    }
}

#[async_trait]
impl Desktop for Computer {
    async fn screenshot(&self) -> Result<Vec<u8>> {
        Desktop::screenshot(&self.primary).await
    }

    async fn capture(&self, area: Option<Rect>, scale: Option<u32>) -> Result<Vec<u8>> {
        Desktop::capture(&self.primary, area, scale).await
    }

    async fn move_to(&self, at: Point) -> Result<()> {
        Desktop::move_to(&self.primary, at).await
    }

    async fn click(&self, at: Point, button: Button) -> Result<()> {
        Desktop::click(&self.primary, at, button).await
    }

    async fn click_with(&self, at: Point, button: Button, held: &[Held]) -> Result<()> {
        Desktop::click_with(&self.primary, at, button, held).await
    }

    async fn double_click(&self, at: Point, button: Button) -> Result<()> {
        Desktop::double_click(&self.primary, at, button).await
    }

    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()> {
        Desktop::drag(&self.primary, from, to, button).await
    }

    async fn drag_with(&self, from: Point, to: Point, button: Button, held: &[Held]) -> Result<()> {
        Desktop::drag_with(&self.primary, from, to, button, held).await
    }

    async fn move_along(&self, steps: &[motion::Step]) -> Result<()> {
        Desktop::move_along(&self.primary, steps).await
    }

    async fn drag_along(
        &self,
        from: Point,
        steps: &[motion::Step],
        button: Button,
        held: &[Held],
    ) -> Result<()> {
        Desktop::drag_along(&self.primary, from, steps, button, held).await
    }

    async fn button_down(&self, at: Option<Point>, button: Button) -> Result<()> {
        Desktop::button_down(&self.primary, at, button).await
    }

    async fn button_up(&self, at: Option<Point>, button: Button) -> Result<()> {
        Desktop::button_up(&self.primary, at, button).await
    }

    async fn let_go(&self, button: Button) -> Result<()> {
        Desktop::let_go(&self.primary, button).await
    }

    async fn key_down(&self, key: &str) -> Result<()> {
        Desktop::key_down(&self.primary, key).await
    }

    async fn key_up(&self, key: &str) -> Result<()> {
        Desktop::key_up(&self.primary, key).await
    }

    async fn let_key_go(&self, key: &str) -> Result<()> {
        Desktop::let_key_go(&self.primary, key).await
    }

    async fn let_keys_go(&self) -> Result<()> {
        Desktop::let_keys_go(&self.primary).await
    }

    async fn wait_until_still(&self, settle: Duration, within: Duration) -> Result<()> {
        Desktop::wait_until_still(&self.primary, settle, within).await
    }

    async fn type_text(&self, text: &str, delay: Option<Duration>) -> Result<()> {
        Desktop::type_text(&self.primary, text, delay).await
    }

    async fn press(&self, chords: &[String], held: &[Held]) -> Result<()> {
        Desktop::press(&self.primary, chords, held).await
    }

    async fn scroll(&self, at: Point, by: Delta) -> Result<()> {
        Desktop::scroll(&self.primary, at, by).await
    }

    async fn cursor(&self) -> Result<Point> {
        Desktop::cursor(&self.primary).await
    }

    async fn geometry(&self) -> Result<(u32, u32)> {
        Desktop::geometry(&self.primary).await
    }

    async fn alive(&self) -> Result<()> {
        Desktop::alive(&self.primary).await
    }

    fn control(&self) -> &Arc<ControlGate> {
        self.primary.control()
    }

    fn as_clipboard(&self) -> Option<&dyn Clipboard> {
        Desktop::as_clipboard(&self.primary)
    }
}

pub struct Screen {
    driver: Arc<dyn Desktop>,
    profile: Arc<dyn Profile>,
    runtimes: ProfileRuntimes,
    host: Arc<MachineHost>,
    id: ScreenId,
    ports: ScreenPorts,
    mapped: PortMap,
}

impl Screen {
    fn new(
        profile: Arc<dyn Profile>,
        runtimes: ProfileRuntimes,
        driver: &dyn DesktopFactory,
        host: Arc<MachineHost>,
        id: ScreenId,
        mapped: PortMap,
    ) -> Self {
        Self {
            driver: driver.open(Arc::clone(&host), id),
            // A screen past the limit is refused by `Computer::screen` before it reaches here.
            ports: profile.ports().screen(id).unwrap_or(ScreenPorts {
                display_number: id.0 + 1,
                view: 0,
                control: 0,
                view_vnc: 0,
                control_vnc: 0,
            }),
            profile,
            runtimes,
            host,
            id,
            mapped,
        }
    }

    pub fn id(&self) -> ScreenId {
        self.id
    }

    pub fn display(&self) -> String {
        self.ports.display()
    }

    pub fn ports(&self) -> ScreenPorts {
        self.ports
    }

    pub fn desktop(&self) -> &dyn Desktop {
        self.driver.as_ref()
    }

    pub fn control(&self) -> &Arc<ControlGate> {
        self.driver.control()
    }

    pub fn viewer_url(&self) -> Option<String> {
        self.mapped.get(&self.ports.view).map(|port| {
            self.profile
                .viewer_url(&self.host.address(*port), self.host.view_ticket())
        })
    }

    pub fn control_url(&self) -> Option<String> {
        self.mapped.get(&self.ports.control).map(|port| {
            self.profile
                .viewer_url(&self.host.address(*port), self.host.control_ticket())
        })
    }

    pub fn viewer_socket(&self) -> Option<String> {
        self.mapped.get(&self.ports.view).map(|port| {
            self.profile
                .viewer_socket(&self.host.address(*port), self.host.view_ticket())
        })
    }

    /// Answers while the port is mapped; the server behind it runs only during a takeover.
    pub fn control_socket(&self) -> Option<String> {
        self.mapped.get(&self.ports.control).map(|port| {
            self.profile
                .viewer_socket(&self.host.address(*port), self.host.control_ticket())
        })
    }

    pub async fn open_url(&self, url: &str) -> Result<()> {
        self.runtimes
            .browser
            .open(self.host.as_ref(), self.profile.as_ref(), self.id, url)
            .await
    }

    pub async fn set_wallpaper(&self, image: &[u8]) -> Result<()> {
        if image.is_empty() {
            return Err(Error::denied("a wallpaper cannot be empty"));
        }

        self.runtimes.wallpaper.supported()?;

        // Kept: swaybg reads it after swaymsg returns, and again whenever sway restarts it.
        let path = PathBuf::from(format!("/tmp/computer/wallpaper-{}.image", self.id.0));
        self.host.touch();
        self.host
            .machine()
            .write_file(self.host.name(), &path, image)
            .await?;
        self.runtimes
            .wallpaper
            .set(self.host.as_ref(), self.profile.as_ref(), self.id, &path)
            .await
    }

    pub async fn launch(&self, launch: &Launch) -> Result<Window> {
        if launch.command.is_empty() {
            return Err(Error::invalid("an app with no command cannot be started"));
        }

        self.runtimes.app.supported()?;
        self.runtimes
            .app
            .launch(self.host.as_ref(), self.profile.as_ref(), self.id, launch)
            .await
    }

    pub async fn windows(&self) -> Result<Vec<Window>> {
        self.runtimes
            .app
            .windows(self.host.as_ref(), self.profile.as_ref(), self.id)
            .await
    }

    pub async fn focus(&self, window: &str) -> Result<()> {
        self.runtimes
            .app
            .focus(self.host.as_ref(), self.profile.as_ref(), self.id, window)
            .await
    }

    pub async fn close_window(&self, window: &str) -> Result<()> {
        self.runtimes
            .app
            .close(self.host.as_ref(), self.profile.as_ref(), self.id, window)
            .await
    }

    pub async fn arrange(&self, window: &str, how: Arrange) -> Result<Window> {
        self.runtimes.app.supported()?;
        self.runtimes
            .app
            .arrange(
                self.host.as_ref(),
                self.profile.as_ref(),
                self.id,
                window,
                how,
            )
            .await
    }

    pub async fn active_window(&self) -> Result<Option<Window>> {
        self.runtimes.app.supported()?;
        self.runtimes
            .app
            .active(self.host.as_ref(), self.profile.as_ref(), self.id)
            .await
    }

    pub async fn wait_for_window(&self, class: &str, within: Duration) -> Result<Window> {
        self.runtimes.app.supported()?;
        self.runtimes
            .app
            .wait_for_window(
                self.host.as_ref(),
                self.profile.as_ref(),
                self.id,
                class,
                Duration::from_millis(apps::SETTLE_MS),
                within,
            )
            .await
    }

    pub async fn hand_over(&self) -> Result<Takeover> {
        self.open_control(true).await
    }

    pub async fn share(&self) -> Result<Takeover> {
        self.open_control(false).await
    }

    pub async fn viewers(&self) -> Result<Viewers> {
        self.runtimes
            .screen
            .viewers(self.host.as_ref(), self.profile.as_ref(), self.id)
            .await
    }

    pub async fn start_recording(&self, fps: Option<u32>) -> Result<String> {
        self.recorder(Recording::Start, fps)
            .await?
            .ok_or_else(|| Error::denied("the box did not say where it was recording"))
    }

    pub async fn stop_recording(&self) -> Result<String> {
        self.recorder(Recording::Stop, None)
            .await?
            .ok_or_else(|| Error::denied("the box did not say what it had recorded"))
    }

    pub async fn recording(&self) -> Result<Option<String>> {
        self.recorder(Recording::Status, None).await
    }

    async fn recorder(&self, what: Recording, fps: Option<u32>) -> Result<Option<String>> {
        self.runtimes
            .screen
            .record(
                self.host.as_ref(),
                self.profile.as_ref(),
                self.id,
                what,
                fps,
            )
            .await
    }

    pub async fn wait_until_free(&self, within: Duration) -> Result<Viewers> {
        let deadline = SystemTime::now() + within;

        loop {
            let viewers = self.viewers().await?;
            if !viewers.person_present() {
                return Ok(viewers);
            }
            if SystemTime::now() >= deadline {
                return Err(Error::Timeout {
                    after: within,
                    detail: format!("{} still driving", viewers.driving),
                });
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn open_control(&self, exclusive: bool) -> Result<Takeover> {
        // From the CSPRNG: whoever can guess this token can drive the takeover.
        let token = format!("takeover-{}-{}", self.id.0, Secret::generate()?.expose());

        self.runtimes
            .screen
            .control(
                self.host.as_ref(),
                self.profile.as_ref(),
                self.id,
                &token,
                !exclusive,
            )
            .await?;

        if exclusive {
            self.control().hand_over(token.clone(), SystemTime::now());
        }

        Ok(Takeover {
            host: Arc::clone(&self.host),
            profile: Arc::clone(&self.profile),
            screen_runtime: Arc::clone(&self.runtimes.screen),
            control: Arc::clone(self.control()),
            screen: self.id,
            url: self.control_url(),
            token,
            exclusive,
        })
    }

    pub async fn person_driving(&self) -> bool {
        servers::x11::port_listening(self.host.as_ref(), self.id, self.ports.control).await
    }

    pub async fn reclaim(&self) -> Result<()> {
        self.runtimes
            .screen
            .reclaim(self.host.as_ref(), self.profile.as_ref(), self.id)
            .await?;

        self.control().reclaim();
        Ok(())
    }

    pub async fn geometry(&self) -> Result<(u32, u32)> {
        self.driver.geometry().await
    }

    fn clipboard_port(&self) -> Result<&dyn Clipboard> {
        self.driver.as_clipboard().ok_or(Error::Unsupported {
            gaps: vec!["clipboard"],
        })
    }

    pub async fn clipboard(&self) -> Result<String> {
        self.selection(Selection::Clipboard).await
    }

    pub async fn selection(&self, selection: Selection) -> Result<String> {
        self.clipboard_port()?.text(selection).await
    }

    pub async fn set_clipboard(&self, text: &str) -> Result<()> {
        self.set_selection(Selection::Clipboard, text).await
    }

    pub fn audio_socket(&self) -> String {
        // PulseAudio is a singleton per user, so every screen shares one daemon.
        "/tmp/computer/pulse.socket".to_string()
    }

    /// The sink's monitor: `default` finds no source when the only card is a null sink.
    pub fn audio_source(&self) -> String {
        format!("screen{}.monitor", self.ports.display_number)
    }

    pub async fn record(&self, duration: Duration, path: &str) -> Result<()> {
        let recorder = self
            .host
            .exec(&["sh".into(), "-c".into(), "command -v ffmpeg".into()])
            .await?;

        if !recorder.ok() {
            return Err(Error::Unsupported {
                gaps: vec!["video recording"],
            });
        }

        let (width, height) = self.geometry().await?;
        let seconds = duration.as_secs().max(1);

        let mut argv: Vec<String> = [
            "ffmpeg",
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-f",
            "x11grab",
            "-framerate",
            "15",
            "-video_size",
        ]
        .iter()
        .map(|part| (*part).to_string())
        .collect();

        argv.push(format!("{width}x{height}"));
        argv.push("-i".to_string());
        argv.push(self.display());

        // ffmpeg fails on a missing input, and loses the video with it.
        let sound = self
            .host
            .exec(&["sh".into(), "-c".into(), "command -v pactl".into()])
            .await?;

        if sound.ok() {
            argv.push("-f".to_string());
            argv.push("pulse".to_string());
            argv.push("-i".to_string());
            argv.push(self.audio_source());

            let mut with_sound = vec![
                "env".to_string(),
                format!("PULSE_SERVER=unix:{}", self.audio_socket()),
            ];
            with_sound.extend(argv);
            argv = with_sound;
        }

        argv.push("-t".to_string());
        argv.push(seconds.to_string());
        argv.push(path.to_string());

        let result = self
            .host
            .run_within(&argv, &BTreeMap::new(), duration + Duration::from_secs(30))
            .await?;

        match result.ok() {
            true => Ok(()),
            false => Err(Error::Failed {
                code: result.code,
                stderr: result.stderr_utf8().trim().to_string(),
            }),
        }
    }

    pub async fn clipboard_bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>> {
        self.clipboard_port()?.bytes(selection, target).await
    }

    pub async fn clipboard_targets(&self, selection: Selection) -> Result<Vec<String>> {
        self.clipboard_port()?.targets(selection).await
    }

    pub async fn set_clipboard_bytes(
        &self,
        selection: Selection,
        target: &str,
        bytes: &[u8],
    ) -> Result<()> {
        let path = format!("/tmp/computer/{}-{}.bytes", selection.name(), self.id.0);
        let port = self.clipboard_port()?;

        self.host
            .machine()
            .write_file(self.host.name(), Path::new(&path), bytes)
            .await?;
        port.set_bytes_from(selection, target, &path).await
    }

    pub async fn set_selection(&self, selection: Selection, text: &str) -> Result<()> {
        let path = format!("/tmp/computer/{}-{}", selection.name(), self.id.0);
        let port = self.clipboard_port()?;

        self.host
            .machine()
            .write_file(self.host.name(), Path::new(&path), text.as_bytes())
            .await?;
        port.set_from(selection, &path).await
    }

    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        self.driver.screenshot().await
    }

    pub async fn capture(&self, shot: &Shot) -> Result<Vec<u8>> {
        if let Some(percent) = shot.scale {
            if percent == 0 || percent > MAGNIFY {
                return Err(Error::invalid(format!(
                    "a scale is a percentage of full size, between 1 and {MAGNIFY}: {percent}"
                )));
            }
        }

        let area = match &shot.of {
            Of::Screen => None,
            Of::Region(area) => Some(*area),
            Of::Window(id) => Some(self.window_rect(id).await?),
        };

        if let Some(area) = area {
            if area.width == 0 || area.height == 0 {
                return Err(Error::invalid(
                    "a region with no width or height is nothing",
                ));
            }
        }

        match shot.pointer {
            true => self.driver.capture_pointing(area, shot.scale).await,
            false => self.driver.capture(area, shot.scale).await,
        }
    }

    async fn window_rect(&self, id: &str) -> Result<Rect> {
        self.windows()
            .await?
            .into_iter()
            .find(|window| window.id == id)
            .map(|window| Rect::new(window.at, window.width, window.height))
            .ok_or_else(|| Error::invalid(format!("there is no window {id} on this screen")))
    }

    pub async fn move_to(&self, at: impl Into<Point>) -> Result<()> {
        self.driver.move_to(at.into()).await
    }

    pub async fn click(&self, at: impl Into<Point>, button: Button) -> Result<()> {
        self.driver.click(at.into(), button).await
    }

    pub async fn click_with(
        &self,
        at: impl Into<Point>,
        button: Button,
        held: &[Held],
    ) -> Result<()> {
        self.driver.click_with(at.into(), button, held).await
    }

    pub async fn double_click(&self, at: impl Into<Point>, button: Button) -> Result<()> {
        self.driver.double_click(at.into(), button).await
    }

    pub async fn drag(
        &self,
        from: impl Into<Point>,
        to: impl Into<Point>,
        button: Button,
    ) -> Result<()> {
        self.driver.drag(from.into(), to.into(), button).await
    }

    pub async fn drag_with(
        &self,
        from: impl Into<Point>,
        to: impl Into<Point>,
        button: Button,
        held: &[Held],
    ) -> Result<()> {
        self.driver
            .drag_with(from.into(), to.into(), button, held)
            .await
    }

    pub async fn wait_until_still(&self, settle: Duration, within: Duration) -> Result<()> {
        self.driver.wait_until_still(settle, within).await
    }

    pub fn type_text(&self, text: impl Into<String>) -> Typing<'_> {
        Typing::new(self.driver.as_ref(), text)
    }

    pub fn press(&self, keys: impl Keys) -> Press<'_> {
        Press::new(self.driver.as_ref(), keys)
    }

    pub async fn scroll(&self, at: impl Into<Point>, by: Delta) -> Result<()> {
        self.driver.scroll(at.into(), by).await
    }

    pub async fn cursor(&self) -> Result<Point> {
        self.driver.cursor().await
    }

    pub async fn find_cursor(&self) -> Result<Point> {
        self.driver.find_cursor().await
    }

    pub async fn nodes(&self, app: Option<&str>, depth: Option<u32>) -> Result<Vec<Node>> {
        self.driver.nodes(app, depth).await
    }

    pub async fn find_nodes(&self, query: &NodeQuery, limit: Option<usize>) -> Result<Vec<Node>> {
        self.driver.find_nodes(query, limit).await
    }

    pub async fn focus_node(&self, query: &NodeQuery) -> Result<Node> {
        self.driver.focus_node(query).await
    }

    pub async fn invoke_node(&self, query: &NodeQuery, action: Option<&str>) -> Result<Node> {
        self.driver.invoke_node(query, action).await
    }

    pub async fn set_node(&self, query: &NodeQuery, value: &str) -> Result<Node> {
        self.driver.set_node(query, value).await
    }
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen")
            .field("id", &self.id.0)
            .field("display", &self.display())
            .field("viewer", &self.viewer_url())
            .finish()
    }
}

#[async_trait]
impl Desktop for Screen {
    async fn screenshot(&self) -> Result<Vec<u8>> {
        self.driver.screenshot().await
    }

    async fn capture(&self, area: Option<Rect>, scale: Option<u32>) -> Result<Vec<u8>> {
        self.driver.capture(area, scale).await
    }

    async fn move_to(&self, at: Point) -> Result<()> {
        self.driver.move_to(at).await
    }

    async fn click(&self, at: Point, button: Button) -> Result<()> {
        self.driver.click(at, button).await
    }

    async fn click_with(&self, at: Point, button: Button, held: &[Held]) -> Result<()> {
        self.driver.click_with(at, button, held).await
    }

    async fn double_click(&self, at: Point, button: Button) -> Result<()> {
        self.driver.double_click(at, button).await
    }

    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()> {
        self.driver.drag(from, to, button).await
    }

    async fn drag_with(&self, from: Point, to: Point, button: Button, held: &[Held]) -> Result<()> {
        self.driver.drag_with(from, to, button, held).await
    }

    async fn move_along(&self, steps: &[motion::Step]) -> Result<()> {
        self.driver.move_along(steps).await
    }

    async fn drag_along(
        &self,
        from: Point,
        steps: &[motion::Step],
        button: Button,
        held: &[Held],
    ) -> Result<()> {
        self.driver.drag_along(from, steps, button, held).await
    }

    async fn button_down(&self, at: Option<Point>, button: Button) -> Result<()> {
        self.driver.button_down(at, button).await
    }

    async fn button_up(&self, at: Option<Point>, button: Button) -> Result<()> {
        self.driver.button_up(at, button).await
    }

    async fn let_go(&self, button: Button) -> Result<()> {
        self.driver.let_go(button).await
    }

    async fn key_down(&self, key: &str) -> Result<()> {
        self.driver.key_down(key).await
    }

    async fn key_up(&self, key: &str) -> Result<()> {
        self.driver.key_up(key).await
    }

    async fn let_key_go(&self, key: &str) -> Result<()> {
        self.driver.let_key_go(key).await
    }

    async fn let_keys_go(&self) -> Result<()> {
        self.driver.let_keys_go().await
    }

    async fn wait_until_still(&self, settle: Duration, within: Duration) -> Result<()> {
        self.driver.wait_until_still(settle, within).await
    }

    async fn type_text(&self, text: &str, delay: Option<Duration>) -> Result<()> {
        self.driver.type_text(text, delay).await
    }

    async fn press(&self, chords: &[String], held: &[Held]) -> Result<()> {
        self.driver.press(chords, held).await
    }

    async fn scroll(&self, at: Point, by: Delta) -> Result<()> {
        self.driver.scroll(at, by).await
    }

    async fn cursor(&self) -> Result<Point> {
        self.driver.cursor().await
    }

    /// Forwarded, or the trait default would run instead of the driver's own.
    async fn find_cursor(&self) -> Result<Point> {
        self.driver.find_cursor().await
    }

    async fn geometry(&self) -> Result<(u32, u32)> {
        self.driver.geometry().await
    }

    async fn alive(&self) -> Result<()> {
        self.driver.alive().await
    }

    fn control(&self) -> &Arc<ControlGate> {
        self.driver.control()
    }

    fn as_clipboard(&self) -> Option<&dyn Clipboard> {
        self.driver.as_clipboard()
    }
}

pub struct LeasedScreen {
    screen: Screen,
    lease: ScreenLease,
    leases: Arc<Screens>,
}

impl LeasedScreen {
    pub fn lease(&self) -> &ScreenLease {
        &self.lease
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn release(self) -> Result<()> {
        self.leases.release(&self.lease)
    }
}

impl std::ops::Deref for LeasedScreen {
    type Target = Screen;

    fn deref(&self) -> &Self::Target {
        &self.screen
    }
}

impl Drop for LeasedScreen {
    fn drop(&mut self) {
        let _ = self.leases.release(&self.lease);
    }
}

pub struct Takeover {
    host: Arc<MachineHost>,
    profile: Arc<dyn Profile>,
    screen_runtime: Arc<dyn ScreenRuntime>,
    control: Arc<ControlGate>,
    screen: ScreenId,
    url: Option<String>,
    token: String,
    exclusive: bool,
}

impl std::fmt::Debug for Takeover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Takeover")
            .field("screen", &self.screen.0)
            .field("url", &self.url)
            .field("exclusive", &self.exclusive)
            .finish_non_exhaustive()
    }
}

impl Takeover {
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn screen(&self) -> ScreenId {
        self.screen
    }

    pub fn exclusive(&self) -> bool {
        self.exclusive
    }

    pub async fn end(self) -> Result<()> {
        if !self.control.hand_back(&self.token) {
            return Err(Error::denied(
                "this takeover is no longer the one running; somebody else has the screen",
            ));
        }

        // Checked again in the box: a takeover another process started never reached this gate.
        self.screen_runtime
            .release(
                self.host.as_ref(),
                self.profile.as_ref(),
                self.screen,
                &self.token,
            )
            .await
    }
}

pub async fn sweep_expired(machine: &dyn Machine, now: SystemTime) -> Result<Vec<String>> {
    if !machine.sweepable() {
        return Err(Error::Unsupported {
            gaps: vec!["listing boxes by label"],
        });
    }

    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);

    let mut swept = Vec::new();
    for (name, deadline) in machine.labelled(EXPIRY_LABEL).await? {
        let Ok(at) = deadline.parse::<u64>() else {
            continue;
        };

        if seconds >= at {
            machine.remove(&name).await?;
            swept.push(name);
        }
    }

    Ok(swept)
}

fn process_holder() -> HolderId {
    HolderId::new(format!("process-{}", std::process::id()))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0)
}

/// Names also need a counter: a coarse clock gives two calls the same tick.
fn tick() -> u64 {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn unique_name() -> String {
    format!(
        "computer-{}-{:x}-{}",
        std::process::id(),
        nanos() as u32,
        tick()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_two_boxes_opened_at_once_do_not_share_a_name() {
        assert_ne!(
            unique_name(),
            unique_name(),
            "a clock coarser than the gap between two calls gives both the \
             same name, and the second launch fails on a conflict nobody caused"
        );
    }

    #[test]
    fn test_a_thousand_names_in_a_row_are_all_different() {
        let names: std::collections::HashSet<String> = (0..1_000).map(|_| unique_name()).collect();
        assert_eq!(names.len(), 1_000);
    }

    #[test]
    fn test_a_name_says_which_process_opened_it() {
        assert!(unique_name().starts_with(&format!("computer-{}-", std::process::id())));
    }

    #[test]
    fn test_packages_against_a_borrowed_image_are_refused_not_dropped() {
        let refused = Computer::builder()
            .image("someone-else/desktop:1")
            .packages(["fonts-noto-cjk"])
            .preview();

        assert!(
            matches!(refused, Err(Error::Unsupported { .. })),
            "there is no build to install them in, and running the plain image \
             hides that until a page renders as empty boxes"
        );
    }

    #[test]
    fn test_an_image_the_caller_named_is_the_one_that_runs() {
        let args = Computer::builder()
            .image("someone-else/desktop:1")
            .name("preview-box")
            .preview()
            .expect("no packages asked for");

        assert!(
            args.contains(&"someone-else/desktop:1".to_string()),
            "the tag a caller gave is not a suggestion"
        );
    }

    #[test]
    fn test_profiles_are_mounted_where_the_image_keeps_them() {
        let config = Computer::builder()
            .profiles("chinasa-work")
            .config()
            .expect("a config");
        let args = runtime::run_args("box", &config);

        let at = args
            .iter()
            .position(|arg| arg == "--volume")
            .expect("the volume is passed");
        assert_eq!(
            args[at + 1],
            format!("chinasa-work:{}", runtime::PROFILES),
            "a volume mounted anywhere else is a box that saves nothing"
        );
    }

    #[test]
    fn test_a_box_given_no_volume_mounts_nothing() {
        let config = Computer::builder().config().expect("a config");
        let args = runtime::run_args("box", &config);

        assert!(
            !args.iter().any(|arg| arg == "--volume"),
            "the default box keeps nothing between runs"
        );
    }

    #[test]
    fn test_a_local_image_directory_replaces_the_profile_bundle() {
        let asked = Path::new(env!("CARGO_MANIFEST_DIR")).join("images/ubuntu");
        let expected = std::fs::canonicalize(&asked).expect("the Ubuntu image directory");
        let config = Computer::builder()
            .image("ignored.example/desktop:1")
            .image_dir(asked)
            .config()
            .expect("the local image");

        assert_eq!(config.image_dir.as_deref(), Some(expected.as_path()));
        assert!(config.bundle.is_none());
        assert!(config.image.starts_with("computer-local:"));
    }

    #[test]
    fn test_a_profile_can_carry_its_own_build_context() {
        let asked = Path::new(env!("CARGO_MANIFEST_DIR")).join("images/ubuntu");
        let expected = std::fs::canonicalize(&asked).expect("the Ubuntu image directory");
        let profile = ProfileBuilder::new(X11Profile).image_dir(asked).build();

        let config = Computer::builder()
            .profile(Arc::new(profile))
            .config()
            .expect("the profile's own image");

        assert_eq!(config.image_dir.as_deref(), Some(expected.as_path()));
        assert!(config.bundle.is_none());
        assert!(config.image.starts_with("computer-local:"));
    }

    #[test]
    fn test_a_directory_on_the_builder_replaces_the_profiles_own() {
        let profile = ProfileBuilder::new(X11Profile)
            .image_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("images/wayland"))
            .build();
        let asked = Path::new(env!("CARGO_MANIFEST_DIR")).join("images/ubuntu");
        let expected = std::fs::canonicalize(&asked).expect("the Ubuntu image directory");

        let config = Computer::builder()
            .profile(Arc::new(profile))
            .image_dir(asked)
            .config()
            .expect("the builder's image");

        assert_eq!(config.image_dir.as_deref(), Some(expected.as_path()));
    }

    #[test]
    fn test_the_last_image_source_call_wins() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("images/ubuntu");
        let config = Computer::builder()
            .image_dir(directory)
            .image("someone-else/desktop:1")
            .config()
            .expect("the registry image");

        assert_eq!(config.image, "someone-else/desktop:1");
        assert!(config.image_dir.is_none());
    }

    #[test]
    fn test_a_builder_previews_what_it_would_run_without_running_it() {
        let args = Computer::builder()
            .size(1920, 1080)
            .network(false)
            .name("preview-box")
            .preview()
            .expect("an image this crate builds");

        assert!(args.contains(&"COMPUTER_SCREEN_WIDTH=1920".to_string()));
        assert!(args.contains(&"none".to_string()));
        assert!(args.contains(&"preview-box".to_string()));
    }
}
