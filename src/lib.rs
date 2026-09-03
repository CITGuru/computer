//! A computer in a box.
//!
//! One call gives a program a real graphical desktop — an X server, a window
//! manager, a browser and a viewer — running on the machine it is already on,
//! and a small API to look at it and drive it. The desktop lives in a
//! container by default and in a microVM on request; nothing above
//! [`Machine`] knows the difference.
//!
//! ```no_run
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
//!
//! # What is in the box
//!
//! `images/desktop/` is compiled into this crate and built on first use, so
//! the only thing to install is a runtime to put it in: `docker`, `podman` or
//! `nerdctl`, which take the same arguments, or a sandbox vendor from
//! [`sandboxes`] for a machine
//! with a kernel of its own. The image is ours, so what it supports is a
//! constant, and `tests/image.rs` fails when the code and the image disagree.
//!
//! # What drives it
//!
//! A [`Profile`] is the image's contract: its ports, its commands, the
//! environment it reads, what it claims, and the driver it expects. A
//! [`DesktopFactory`] is how a screen is driven.
//!
//! Two pairs ship. [`X11Profile`] with [`X11Driver`] is the default — Xvfb,
//! fluxbox, x11vnc and `xdotool`. [`WaylandProfile`] with [`WaylandDriver`]
//! runs the same box on sway headless, wayvnc, `grim` and `wtype`, with no
//! `/dev/uinput`, so the box keeps the isolation it was started with.
//!
//! A third image is another `Profile`; a third display server is another
//! `DesktopFactory` with a [`Desktop`] under it. The screens, the takeover
//! gate, the leases and the descriptor are written against the traits. A
//! profile names its own driver, so a Wayland image cannot be left on the X11
//! default, where the commands go in and nothing moves.
//!
//! # What it will not do for you
//!
//! - A screenshot does not show the pointer. A capture of the screen does not
//!   include the cursor, so track its position yourself or read it back with
//!   [`Desktop::cursor`].
//! - Coordinates are device pixels against the frame you just captured. A
//!   click computed from a scaled or stale screenshot lands somewhere else,
//!   and neither the result nor the next frame says so.
//! - The screen has no password on it. Viewer ports are published on loopback
//!   only, and anyone who reaches a control port can drive the desktop.

mod auth;
mod desktop;
mod error;
mod exec;
pub mod reach;
mod secret;

pub mod audit;
pub mod bundle;
pub mod cdp;
pub mod image;
pub mod machine;
pub mod microvm;
pub mod profile;
pub mod runtime;
pub mod sandboxes;
pub mod screens;
pub mod servers;
pub mod testing;

pub use audit::{Audit, audit};
pub use auth::{AUTH_ENV, Auth, CONTROL_SECRET_ENV, Credentials, VIEW_SECRET_ENV, VIEWER_USER};
pub use cdp::{BrowserGroup, Devtools, Page, Target};
pub use desktop::{
    Browser, BrowserEndpoint, Button, Clipboard, Control, Delta, Desktop, DesktopFactory,
    DesktopNeed, DesktopPresence, DesktopSupport, Display, DisplayServer, Point, Selection, Viewer,
    ViewerKind, Viewers,
};
pub use error::{Error, Result};
pub use exec::ExecResult;
pub use image::{ScreenAction, ScreenPorts};
pub use machine::ScreenHost;
pub use machine::{DockerMachine, Machine, MachineHost, PortMap};
pub use microvm::MicroVm;
pub use profile::{
    BrowserRuntime, CommandBrowserRuntime, CommandScreen, CommandScreenRuntime,
    CommandWallpaperRuntime, ConfiguredProfile, DesktopContract, FORCE, GeometrySpec, ImageSource,
    PROFILE_ENV, PROFILE_LABEL, PortLayout, Profile, ProfileBuilder, SHARED, ScreenCommands,
    ScreenEnvironment, ScreenRuntime, UnsupportedWallpaperRuntime, WallpaperRuntime,
    WaylandEnvironment, WaylandWallpaperRuntime, X11Environment, X11WallpaperRuntime,
};
pub use reach::{Address, Bind, Reach, Scheme};
pub use runtime::{Config, ContainerCli, SystemDocker};
pub use screens::{ControlGate, DEFAULT_LEASE, ScreenLease, Screens};
pub use secret::Secret;
pub use servers::wayland::{WaylandDesktop, WaylandDriver, WaylandProfile};
pub use servers::x11::{X11Desktop, X11Driver, X11Profile};

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

/// Whoever holds a screen. Opaque here — this crate never learns what it is.
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

/// Where a box records when it should be taken away.
///
/// On the box as well as in a timer, so a box that outlives its process can
/// still be swept.
pub const EXPIRY_LABEL: &str = "computer.expires-at";

/// How long [`Builder::launch`] waits for the screen and the browser.
pub const READY_TIMEOUT: Duration = Duration::from_secs(90);

/// How a box is opened.
pub struct Builder {
    config: Config,
    machine: Option<Arc<dyn Machine>>,
    profile: Arc<dyn Profile>,
    /// Unset means the profile's own, which is the pairing that cannot be
    /// wrong. Set is a caller overriding it on purpose.
    driver: Option<Arc<dyn DesktopFactory>>,
    /// An image the caller named, which is never one this crate builds.
    image: Option<String>,
    /// A caller-owned Docker build context.
    image_dir: Option<PathBuf>,
    /// Unset takes the profile's own, because 1280x800 is the built-in
    /// image's number and not every image's.
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
    /// The image to run.
    ///
    /// Fetched rather than built. An image named here takes no packages from
    /// [`Builder::packages`]: it is somebody else's image, already built.
    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.image = Some(image.into());
        self.image_dir = None;
        self
    }

    /// Build and run an image from a local Docker build context.
    ///
    /// The directory needs a `Dockerfile` and has to implement the active
    /// [`Profile`]. Its tag follows the files, the packages and the
    /// architecture, so an edit builds a new image.
    pub fn image_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.image = None;
        self.image_dir = Some(directory.into());
        self
    }

    /// Which image contract the box speaks: its ports, its commands, and what it
    /// claims.
    ///
    /// A profile also names the driver it expects, which [`Builder::driver`]
    /// overrides.
    pub fn profile(mut self, profile: Arc<dyn Profile>) -> Self {
        self.profile = profile;
        self
    }

    /// Install extra packages into the image.
    ///
    /// A different list is a different image, so the first launch with a new one
    /// builds it.
    pub fn packages(mut self, packages: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.config.extras = bundle::Extras::with(packages);
        self
    }

    /// Give the box a dock along the bottom.
    ///
    /// Opt-in: a dock costs about sixty pixels of every screenshot, which a
    /// box driven by a program would rather keep. See [`bundle::Extras::dock`].
    pub fn dock(self) -> Self {
        let wanted = bundle::Extras::dock();
        self.packages(wanted.packages)
    }

    /// Fonts for Chinese, Japanese, Korean and emoji, which the base image
    /// cannot draw. About 100 MB, which is why they are opt-in.
    pub fn wide_fonts(self) -> Self {
        let wanted = bundle::Extras::wide_fonts();
        self.packages(wanted.packages)
    }

    /// `docker`, `podman` or `nerdctl`.
    pub fn runtime(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    /// Reach the container runtime through something else entirely — a remote
    /// host, a recording, a test double.
    pub fn cli(mut self, cli: Arc<dyn ContainerCli>) -> Self {
        self.cli = Some(cli);
        self
    }

    /// Put the box somewhere other than a container.
    ///
    /// [`MicroVm`] is one. Anything that can run a
    /// command against a display can be another.
    pub fn machine(mut self, machine: Arc<dyn Machine>) -> Self {
        self.machine = Some(machine);
        self
    }

    /// Drive the box through a display server other than the one its profile
    /// names.
    ///
    /// Unset, the box takes [`Profile::driver`].
    pub fn driver(mut self, driver: Arc<dyn DesktopFactory>) -> Self {
        self.driver = Some(driver);
        self
    }

    /// Name the container, instead of one derived from this process.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The size of every screen in this box.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Off gives the box no network at all.
    pub fn network(mut self, on: bool) -> Self {
        self.config.network = on;
        self
    }

    /// Map the viewer and DevTools ports onto loopback, so a person can watch.
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

    /// A memory ceiling, in the runtime's own notation — `"2g"`.
    pub fn memory(mut self, limit: impl Into<String>) -> Self {
        self.config.memory = Some(limit.into());
        self
    }

    pub fn cpus(mut self, cpus: impl Into<String>) -> Self {
        self.config.cpus = Some(cpus.into());
        self
    }

    pub fn shm_size(mut self, size: impl Into<String>) -> Self {
        self.config.shm_size = Some(size.into());
        self
    }

    /// Build or pull the image when the runtime does not have it.
    ///
    /// On by default. The first launch on a machine therefore takes minutes.
    pub fn ensure_image(mut self, ensure: bool) -> Self {
        self.ensure_image = ensure;
        self
    }

    /// Wait for the screen and the browser before returning.
    ///
    /// `None` returns as soon as the box is up, before anything is drawn.
    pub fn wait_for_ready(mut self, within: Option<Duration>) -> Self {
        self.wait = within;
        self
    }

    /// Take the box away after this long, whatever else happens.
    ///
    /// Kept by a task here and written on the box as [`EXPIRY_LABEL`], so
    /// [`sweep_expired`] can find it later.
    pub fn expires_after(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Take the box away once nothing has been asked of it for this long.
    ///
    /// Idleness is measured through this handle: every command, capture and file
    /// copy counts. Work arriving another way needs [`Computer::touch`].
    pub fn expires_when_idle(mut self, idle: Duration) -> Self {
        self.idle = Some(idle);
        self
    }

    /// Leave the box running when the handle is dropped.
    pub fn keep_on_drop(mut self, keep: bool) -> Self {
        self.keep = keep;
        self
    }

    /// Which addresses the published ports answer on.
    ///
    /// Anything but [`Bind::Loopback`] is refused for now: the viewer has no
    /// gate on it yet, so a routable bind would be an unlocked desktop on the
    /// network. See `docs/viewer-auth.md`.
    pub fn publish_on(mut self, bind: Bind) -> Self {
        self.config.bind = bind;
        self
    }

    /// What the viewer asks of whoever connects.
    ///
    /// [`Auth::Open`] is the default and is what a box on loopback has always
    /// been. Anything published beyond loopback needs one of the other two —
    /// see `docs/viewer-auth.md` for which fits a deployment.
    pub fn auth(mut self, auth: Auth) -> Self {
        self.config.auth = auth;
        self
    }

    /// Credentials that have to outlive this process.
    ///
    /// Unset, a pair is minted at launch and lives as long as the handle. A
    /// second program attaching to the same box hands out working URLs only if
    /// it was given the same values.
    pub fn credentials(mut self, credentials: Credentials) -> Self {
        self.config.credentials = Some(credentials);
        self
    }

    /// The host to put in a viewer URL.
    ///
    /// A box published on every interface is reached at a name this crate has
    /// never been told, so a URL built from the bind would be wrong. Unset
    /// keeps the loopback address.
    pub fn advertise(mut self, host: impl Into<String>) -> Self {
        self.config.advertise = Some(host.into());
        self
    }

    /// The arguments this would run, without running anything.
    ///
    /// Carries no credential: the gate's secrets are minted at launch, and a
    /// preview that printed them would put a desktop in whatever logged it.
    pub fn preview(&self) -> Result<Vec<String>> {
        Ok(runtime::run_args(
            self.name.as_deref().unwrap_or("computer-preview"),
            &self.config()?,
        ))
    }

    /// Where the image is fetched from, or built.
    fn source(&self) -> ImageSource {
        match &self.image {
            Some(name) => ImageSource::Registry(name.clone()),
            None => self.profile.image(),
        }
    }

    /// What this box would be started with, resolved.
    ///
    /// The profile's decisions become plain data here, so a [`Machine`] starts a
    /// box from values rather than from a contract.
    pub fn config(&self) -> Result<Config> {
        let mut config = self.config.clone();

        let (width, height) = self.size.unwrap_or_else(|| self.profile.default_size());
        config.width = width;
        config.height = height;
        if let Some(directory) = &self.image_dir {
            let (directory, image) = bundle::directory_image(directory, &config.extras)?;
            config.bundle = None;
            config.image_dir = Some(directory);
            config.image = image;
        } else {
            let source = self.source();
            config.bundle = source.bundle().copied();
            config.image_dir = None;
            config.image = source.tag(&config.extras)?;
        }
        config.boot = self.profile.boot_command();
        config.publish = if self.publish {
            self.profile.ports().to_publish()
        } else {
            Vec::new()
        };

        // The profile's first, so a caller who set a variable by hand keeps
        // it, even where that disagrees with `size()`.
        let mut env = self.profile.launch_env(width, height);
        // Which contract this box speaks, written where a process that never
        // saw this builder can read it back.
        env.insert(
            profile::PROFILE_ENV.to_string(),
            self.profile.name().to_string(),
        );
        env.extend(config.env.clone());
        config.env = env;

        Ok(config)
    }

    /// Open the box.
    pub async fn launch(self) -> Result<Computer> {
        let machine: Arc<dyn Machine> = match (&self.machine, &self.cli) {
            (Some(machine), _) => Arc::clone(machine),
            (None, Some(cli)) => Arc::new(DockerMachine::new(Arc::clone(cli))),
            (None, None) => Arc::new(DockerMachine::new(Arc::new(SystemDocker::new(
                self.program.clone(),
            )))),
        };

        // Asked first, so "the runtime is not there" arrives as itself
        // rather than as a box that would not start.
        machine.preflight().await?;

        let mut config = self.config()?;

        // Asked of the `Machine` rather than the bind, because a bind is a
        // container idea: E2B publishes a hostname per port and would walk
        // past a rule phrased as "loopback or not".
        let routable = !config.publish.is_empty() && machine.reach(&config).needs_a_secret();

        if routable && !config.auth.is_gated() {
            return Err(Error::denied(
                "this box publishes beyond loopback with an open viewer: the \
                 view and control ports would accept anyone who reaches them, \
                 and the control port drives the desktop. Choose \
                 Auth::Password or Auth::Token.",
            ));
        }

        // CDP has no authentication and cannot be given one — Chromium binds
        // its debugging port to loopback whatever it is told, and a forward
        // cannot add a check to a WebSocket upgrade. Withdrawn rather than
        // published where the gate cannot follow: reach it through a tunnel,
        // or from inside the box.
        if routable && let Some(bridge) = self.profile.ports().devtools_bridge {
            config.publish.retain(|port| *port != bridge);
        }

        // Minted here rather than in `config`, so `preview` stays a thing that
        // can be printed: a secret exists once a box is going to.
        if config.auth.is_gated() {
            let credentials = match config.credentials.take() {
                Some(supplied) => supplied,
                None => Credentials::generate()?,
            };
            // Carried as container environment, where every `screen.sh`
            // invocation reads it — a screen opened long after launch has to
            // be reachable with the credential the first one was.
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

        // Checked before the box is started. A mismatched pairing builds and
        // starts, and only shows up as a display that never came up.
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
            // On the box as well as in this process, so a box that outlives
            // its program can still be swept.
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
            // A claim withdrawn rather than one broken, so `audit` skips the
            // check instead of failing it.
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
            // Detached: the box has to go even if nobody polls the handle
            // again.
            let doomed = Arc::clone(&machine);
            let condemned = name.clone();
            tokio::spawn(async move {
                tokio::time::sleep(ttl).await;
                reap(doomed, condemned, "its life ran out").await;
            });
        }

        if let Some(idle) = self.idle {
            // Woken on the same period it is watching for, so a box goes
            // within one interval of the last thing asked of it rather than
            // being polled every second for hours.
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
                // The box's own output explains a screen that never came up,
                // and dropping `computer` below takes it away.
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

/// The image's claim, corrected for the driver actually in use.
fn driven_by(mut support: DesktopSupport, driver: &dyn DesktopFactory) -> DesktopSupport {
    if let Some(display) = support.display.as_mut() {
        display.server = driver.display_server();
    }
    support
}

/// How many times a reaper asks before it gives up.
///
/// A runtime that is restarting answers nothing for a few seconds, and a box
/// abandoned because the first ask landed in that window is the whole failure
/// this exists to prevent.
const REAP_ATTEMPTS: u32 = 5;

/// How long between those asks.
const REAP_PAUSE: Duration = Duration::from_secs(10);

/// Take a box away, and say so if it will not go.
///
/// **A detached task has no caller to return to.** Both reapers run long after
/// the handle that made them stopped being polled, so a discarded error here
/// is a box that keeps its processor and its memory with nothing anywhere
/// recording why — which is exactly what a deadline was set to prevent.
async fn reap(machine: Arc<dyn Machine>, name: String, because: &'static str) {
    for attempt in 1..=REAP_ATTEMPTS {
        match machine.stop(&name).await {
            Ok(()) => {
                tracing::info!(box_ = %name, reason = because, "box removed");
                return;
            }
            // Already gone: something else took it, which is the outcome asked
            // for rather than a failure to report.
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
}

impl ProfileRuntimes {
    fn from_profile(profile: &dyn Profile) -> Self {
        Self {
            screen: profile.screen_runtime(),
            browser: profile.browser_runtime(),
            wallpaper: profile.wallpaper_runtime(),
        }
    }
}

/// A running box: a place with a desktop in it.
///
/// Which kind of place is a [`Machine`].
pub struct Computer {
    machine: Arc<dyn Machine>,
    /// Both held for the life of the box: a screen started on demand has to get
    /// the same image contract and the same driver as the one it opened with.
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
    /// A box with everything at its default.
    pub async fn launch() -> Result<Self> {
        Self::builder().launch().await
    }

    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Pick up a box that is already running, with its windows, its browser
    /// profile and its files.
    ///
    /// Never removed when the handle is dropped: this process did not create it.
    pub async fn attach(name: impl Into<String>) -> Result<Self> {
        Self::attach_to(Arc::new(DockerMachine::default()), name).await
    }

    /// Pick up a box on any machine.
    ///
    /// The profile is the one the box names, where this crate ships one by that
    /// name. See [`Computer::attach_using`] for any other.
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

    /// Pick up a box that speaks a contract this crate does not ship.
    ///
    /// A box records the name of its profile, not the profile itself, so the
    /// caller supplies it again.
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

    /// The half both attach paths share, with the environment already read.
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
        let computer = Self::assemble(
            Arc::new(MachineHost::new(machine, profile, name)),
            driver,
            support,
            mapped,
            None,
        );

        // A person may already have this screen, and the gate is per
        // process, so the box is asked rather than assumed.
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

    /// Built around a [`MachineHost`] rather than its parts, because the host
    /// is what knows where the box is advertised, and a second one made here
    /// would answer differently.
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

    /// When this box will be taken away, if it was given a life.
    pub fn expires_at(&self) -> Option<SystemTime> {
        self.expires_at
    }

    /// How long the box has had nothing asked of it through this handle.
    pub fn idle_for(&self) -> Duration {
        self.host.idle_for()
    }

    /// Count this moment as activity, for work that does not go through this
    /// handle.
    pub fn touch(&self) {
        self.host.touch();
    }

    /// Whether its time has run out. The removal starts here rather than ends.
    pub fn expired(&self) -> bool {
        self.expires_at
            .map(|at| SystemTime::now() >= at)
            .unwrap_or(false)
    }

    /// The container's name, which is also how [`Computer::attach`] finds it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What this box's viewers ask, and what opens them.
    ///
    /// The way to learn the password under [`Auth::Password`], where no URL
    /// carries it by design. `None` where the gate is open.
    pub fn credentials(&self) -> Option<&Credentials> {
        self.host.gate().1
    }

    /// What this box can show. A constant for the image it was started from.
    pub fn support(&self) -> &DesktopSupport {
        &self.support
    }

    /// Screen 0, which the image starts for itself.
    pub fn primary(&self) -> &Screen {
        &self.primary
    }

    /// Who holds which screen. Only meaningful where several callers share one
    /// box; a single caller can ignore it entirely.
    pub fn leases(&self) -> &Screens {
        &self.screen_registry
    }

    /// Start another screen and hand back a driver for it, under a lease.
    ///
    /// Held for this process, so a screen somebody else holds is refused. See
    /// [`Computer::claim`] to hold one under a name of your own.
    pub async fn screen(&self, screen: ScreenId) -> Result<LeasedScreen> {
        self.take(screen, &process_holder(), 0).await
    }

    /// Start a screen with no lease at all, for one caller with one box.
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

    /// Take a screen under a lease, held under a name of your own.
    ///
    /// A returning holder is given the screen it already had, and the lease is
    /// returned when the handle drops.
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
                // The screen never started, so the lease would block whoever
                // asks next until it ran out.
                let _ = self.screen_registry.release(&lease);
                Err(error)
            }
        }
    }

    /// Take one particular screen, whoever holds it.
    ///
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

    /// Stop a screen's whole stack.
    pub async fn close_screen(&self, screen: ScreenId) -> Result<()> {
        self.runtimes
            .screen
            .stop(self.host.as_ref(), self.profile.as_ref(), screen)
            .await
    }

    /// Whether the screen and the browser answer now.
    ///
    /// The driver asks its own display server; the browser is asked for its
    /// port.
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

    /// Wait until there is something on the screen to look at.
    ///
    /// Both halves: the display answers long before the browser has a window.
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

    /// Where the browser answers *inside* the box, which is how [`Computer::probe`]
    /// knows it came up.
    ///
    /// Not gated on `cdp`, which says whether a caller out here can reach the
    /// debugger — a different question, and one a box on somebody else's host
    /// answers no to while its browser is perfectly up. Gating this on it made
    /// such a box never report ready.
    fn devtools_port_in_box(&self) -> Option<u16> {
        self.support.browser.as_ref()?;
        self.profile.ports().devtools
    }

    /// The machine this box runs on, for anything this API does not cover.
    pub fn machine(&self) -> &Arc<dyn Machine> {
        &self.machine
    }

    /// The image contract this box speaks.
    pub fn profile(&self) -> &Arc<dyn Profile> {
        &self.profile
    }

    /// Where to watch screen 0 in a browser, read-only.
    pub fn viewer_url(&self) -> Option<String> {
        self.primary.viewer_url()
    }

    /// Chromium's DevTools, as reached from this machine.
    ///
    /// Screen 0's browser: the debugging port is one port, and the first
    /// browser to start holds it. `None` where no ports were published.
    pub fn devtools(&self) -> Option<BrowserEndpoint> {
        let bridge = self.profile.ports().devtools_bridge?;
        let port = self.mapped.get(&bridge)?;
        Some(BrowserEndpoint {
            http_url: format!("http://127.0.0.1:{port}"),
            ws_url: format!("ws://127.0.0.1:{port}/devtools/browser"),
        })
    }

    /// Every port mapping the runtime made, as the port inside the box to the
    /// port out here.
    pub fn ports(&self) -> &PortMap {
        &self.mapped
    }

    /// Open a URL in screen 0's browser.
    pub async fn open_url(&self, url: &str) -> Result<()> {
        self.primary.open_url(url).await
    }

    /// Replace screen 0's wallpaper with these image bytes.
    pub async fn set_wallpaper(&self, image: &[u8]) -> Result<()> {
        self.primary.set_wallpaper(image).await
    }

    /// The browser, driven through the DevTools protocol rather than through the
    /// screen.
    ///
    /// Works on a box with no display. `None` where no ports were published.
    pub fn browser(&self) -> Option<Devtools> {
        self.devtools()
            .as_ref()
            .and_then(|endpoint| Devtools::from_endpoint(endpoint).ok())
    }

    /// Hand screen 0 to a person, and hold the input back until they are done.
    pub async fn hand_over(&self) -> Result<Takeover> {
        self.primary.hand_over().await
    }

    /// Let a person click while this keeps driving. See [`Screen::share`].
    pub async fn share(&self) -> Result<Takeover> {
        self.primary.share().await
    }

    /// Whether somebody is driving screen 0, asked of the box itself.
    pub async fn person_driving(&self) -> bool {
        self.primary.person_driving().await
    }

    /// How many people are watching screen 0, and how many are on the input.
    pub async fn viewers(&self) -> Result<Viewers> {
        self.primary.viewers().await
    }

    /// What is on screen 0's clipboard.
    pub async fn clipboard(&self) -> Result<String> {
        self.primary.clipboard().await
    }

    /// Put text on screen 0's clipboard, ready to paste with `ctrl+v`.
    pub async fn set_clipboard(&self, text: &str) -> Result<()> {
        self.primary.set_clipboard(text).await
    }

    /// What is on one of screen 0's selections.
    pub async fn selection(&self, selection: Selection) -> Result<String> {
        self.primary.selection(selection).await
    }

    /// Screen 0's selection as one of the types its owner offers.
    pub async fn clipboard_bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>> {
        self.primary.clipboard_bytes(selection, target).await
    }

    /// The types screen 0's selection can be read as.
    pub async fn clipboard_targets(&self, selection: Selection) -> Result<Vec<String>> {
        self.primary.clipboard_targets(selection).await
    }

    /// Record screen 0 as video, into a file inside the box.
    pub async fn record(&self, duration: Duration, path: &str) -> Result<()> {
        self.primary.record(duration, path).await
    }

    /// Put bytes on screen 0's selection, offered as this type.
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

    /// Put text on one of screen 0's selections.
    pub async fn set_selection(&self, selection: Selection, text: &str) -> Result<()> {
        self.primary.set_selection(selection, text).await
    }

    /// Wait until the person on screen 0 has closed their tab.
    pub async fn wait_until_free(&self, within: Duration) -> Result<Viewers> {
        self.primary.wait_until_free(within).await
    }

    /// End a takeover on screen 0 — including one started by a process that
    /// has since exited.
    pub async fn reclaim(&self) -> Result<()> {
        self.primary.reclaim().await
    }

    /// Run a command in the box, with no display attached.
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

    /// Run a command against one screen's display.
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

    /// Put bytes in the box.
    pub async fn write_file(&self, path: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
        self.machine
            .write_file(&self.name, path.as_ref(), bytes)
            .await
    }

    /// Take bytes out of the box.
    pub async fn read_file(&self, path: impl AsRef<Path>) -> Result<Vec<u8>> {
        self.machine.read_file(&self.name, path.as_ref()).await
    }

    /// A whole file in, without holding it in memory where the runtime can
    /// move it directly.
    pub async fn upload(&self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
        self.machine
            .upload(&self.name, from.as_ref(), to.as_ref())
            .await
    }

    /// A whole file out, the same way.
    pub async fn download(&self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
        self.machine
            .download(&self.name, from.as_ref(), to.as_ref())
            .await
    }

    /// Run a command, and give up on it after `within`.
    ///
    /// [`Computer::exec`] uses [`machine::DEFAULT_TIMEOUT`].
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

    /// What the box itself has said, which is where a screen that never came
    /// up explains itself.
    pub async fn logs(&self) -> Result<String> {
        self.machine.logs(&self.name).await
    }

    /// Take the box down.
    ///
    /// Dropping the handle does this too, without reporting whether it worked.
    pub async fn shutdown(mut self) -> Result<()> {
        let outcome = self.machine.stop(&self.name).await;

        // Cleared whatever the runtime answered, so the drop below does not
        // remove it a second time.
        self.cleanup = None;
        outcome
    }

    // Screen 0, without reaching through `primary()`.

    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        self.primary.screenshot().await
    }

    pub async fn move_to(&self, at: impl Into<Point>) -> Result<()> {
        self.primary.move_to(at).await
    }

    pub async fn click(&self, at: impl Into<Point>, button: Button) -> Result<()> {
        self.primary.click(at, button).await
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

    pub async fn type_text(&self, text: &str) -> Result<()> {
        self.primary.type_text(text).await
    }

    pub async fn key(&self, chord: &str) -> Result<()> {
        self.primary.key(chord).await
    }

    pub async fn scroll(&self, at: impl Into<Point>, by: Delta) -> Result<()> {
        self.primary.scroll(at, by).await
    }

    pub async fn cursor(&self) -> Result<Point> {
        self.primary.cursor().await
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

        // Spawned and not waited on: a drop cannot await, and the container
        // runtime outlives this process.
        //
        // Whether it *finished* is unknowable from here — but whether it
        // started is not, and a command that never started is a box nobody
        // will ever take away.
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

    async fn move_to(&self, at: Point) -> Result<()> {
        Desktop::move_to(&self.primary, at).await
    }

    async fn click(&self, at: Point, button: Button) -> Result<()> {
        Desktop::click(&self.primary, at, button).await
    }

    async fn double_click(&self, at: Point, button: Button) -> Result<()> {
        Desktop::double_click(&self.primary, at, button).await
    }

    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()> {
        Desktop::drag(&self.primary, from, to, button).await
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        Desktop::type_text(&self.primary, text).await
    }

    async fn key(&self, chord: &str) -> Result<()> {
        Desktop::key(&self.primary, chord).await
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

/// One screen of a box.
pub struct Screen {
    /// Behind the trait, not the X11 type: which display server is under a
    /// screen is the image's business, and nothing above here should have to
    /// be edited to add a second one.
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
            // Screen 0 always has ports; a screen past the limit is refused by
            // `Computer::screen` before it reaches here.
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

    /// The display this screen is on — `:1` for screen 0.
    pub fn display(&self) -> String {
        self.ports.display()
    }

    pub fn ports(&self) -> ScreenPorts {
        self.ports
    }

    /// The driver, for code that wants a `&dyn Desktop`.
    pub fn desktop(&self) -> &dyn Desktop {
        self.driver.as_ref()
    }

    /// Whether the owner may act, and where a takeover is recorded.
    pub fn control(&self) -> &Arc<ControlGate> {
        self.driver.control()
    }

    /// Watch this screen in a browser, read-only.
    ///
    /// `None` where no ports were published.
    pub fn viewer_url(&self) -> Option<String> {
        self.mapped.get(&self.ports.view).map(|port| {
            self.profile
                .viewer_url(&self.host.address(*port), self.host.view_ticket())
        })
    }

    /// The input-accepting viewer, which exists only while somebody has been
    /// handed the screen. See [`Screen::hand_over`].
    pub fn control_url(&self) -> Option<String> {
        self.mapped.get(&self.ports.control).map(|port| {
            self.profile
                .viewer_url(&self.host.address(*port), self.host.control_ticket())
        })
    }

    /// Open a URL in this screen's browser.
    ///
    /// A new tab, raised in front of the last one, so coordinates from an
    /// earlier screenshot now belong to a page that is no longer on screen.
    pub async fn open_url(&self, url: &str) -> Result<()> {
        self.runtimes
            .browser
            .open(self.host.as_ref(), self.profile.as_ref(), self.id, url)
            .await
    }

    /// Replace this screen's wallpaper with these image bytes.
    ///
    /// The display stack detects the image format from its contents.
    pub async fn set_wallpaper(&self, image: &[u8]) -> Result<()> {
        if image.is_empty() {
            return Err(Error::denied("a wallpaper cannot be empty"));
        }

        // The file stays: swaybg reads it after swaymsg has already returned,
        // and reads it again whenever sway restarts the background. One path
        // per screen, overwritten, so it cannot grow.
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

    /// Give the screen to a person, and stop sending input until they give it
    /// back.
    ///
    /// A second server on a second port, so a viewer already connected to the
    /// read-only stream is not handed the keyboard with it.
    pub async fn hand_over(&self) -> Result<Takeover> {
        self.open_control(true).await
    }

    /// Let a person click while the owner keeps driving.
    ///
    /// The same server with the gate left open. Both sides send to one display,
    /// which merges them, so their input can interleave mid-gesture.
    pub async fn share(&self) -> Result<Takeover> {
        self.open_control(false).await
    }

    /// How many people are watching, and how many are on the input.
    ///
    /// Counted from live connections: both servers keep listening either way.
    pub async fn viewers(&self) -> Result<Viewers> {
        self.runtimes
            .screen
            .viewers(self.host.as_ref(), self.profile.as_ref(), self.id)
            .await
    }

    /// Wait until nobody is on the input any more.
    ///
    /// Read the screen again afterwards. It may look nothing like it did.
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
        // Minted before the server opens, so the box records who opened it.
        //
        // From the CSPRNG, not the clock: this token is what the input guard
        // refuses on, so anyone who can work one out can drive a screen during
        // somebody else's takeover. The screen number stays for the person
        // reading the file; the entropy is the rest.
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

    /// Whether somebody has been handed this screen.
    ///
    /// Asked of the box, which holds the token, rather than of this process,
    /// which holds only its own gate.
    pub async fn person_driving(&self) -> bool {
        servers::x11::port_listening(self.host.as_ref(), self.id, self.ports.control).await
    }

    /// End a takeover this process did not start.
    ///
    /// Closes the input-accepting server and reopens the gate. Nothing reaches
    /// this on a timeout or a retry: the person is still holding a keyboard.
    pub async fn reclaim(&self) -> Result<()> {
        // Forced: the takeover this ends is one the process never started,
        // so it holds no token to prove anything with.
        self.runtimes
            .screen
            .reclaim(self.host.as_ref(), self.profile.as_ref(), self.id)
            .await?;

        self.control().reclaim();
        Ok(())
    }

    /// The screen's own idea of its size, read from the X server rather than
    /// from what the box was asked for.
    pub async fn geometry(&self) -> Result<(u32, u32)> {
        self.driver.geometry().await
    }

    /// The clipboard, or the refusal a box without one gives.
    fn clipboard_port(&self) -> Result<&dyn Clipboard> {
        self.driver.as_clipboard().ok_or(Error::Unsupported {
            gaps: vec!["clipboard"],
        })
    }

    /// What is on this screen's clipboard.
    ///
    /// Each screen has its own selections.
    pub async fn clipboard(&self) -> Result<String> {
        self.selection(Selection::Clipboard).await
    }

    /// What is on one of this screen's selections.
    ///
    /// `CLIPBOARD` is copy and paste. `PRIMARY` is what dragging the mouse over
    /// text fills, which a middle click pastes.
    pub async fn selection(&self, selection: Selection) -> Result<String> {
        self.clipboard_port()?.text(selection).await
    }

    /// Put text on this screen's clipboard, ready to paste.
    ///
    /// Staged as a file in the box, because a command line is no place for a
    /// document.
    pub async fn set_clipboard(&self, text: &str) -> Result<()> {
        self.set_selection(Selection::Clipboard, text).await
    }

    /// Where this screen's sound card listens, for a client inside the box.
    ///
    /// PulseAudio writes its socket under the caller's runtime directory, which
    /// an exec does not share. Everything reaches it through this path.
    pub fn audio_socket(&self) -> String {
        // One daemon for the box: PulseAudio is a singleton per user, so a
        // second one for a second screen refuses to start.
        "/tmp/computer/pulse.socket".to_string()
    }

    /// What a recorder listens to on this screen.
    ///
    /// The sink's monitor. `default` finds no source on a box whose only card is
    /// a sink that goes nowhere.
    pub fn audio_source(&self) -> String {
        format!("screen{}.monitor", self.ports.display_number)
    }

    /// Record this screen as video, into a file inside the box.
    ///
    /// Needs `ffmpeg` from [`bundle::Extras::video`], and takes sound from a
    /// card where [`bundle::Extras::audio`] put one. The call runs for the whole
    /// duration.
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

        // Sound only where there is a card: `ffmpeg` fails on an input that
        // is not there, and loses the video with it.
        let sound = self
            .host
            .exec(&["sh".into(), "-c".into(), "command -v pactl".into()])
            .await?;

        if sound.ok() {
            argv.push("-f".to_string());
            argv.push("pulse".to_string());
            argv.push("-i".to_string());
            argv.push(self.audio_source());

            // Through `env` rather than a shell, which would be one more
            // thing to quote correctly.
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

        // Longer than the recording, because the recording is the wait.
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

    /// The selection as one of the types its owner offers, such as `image/png`.
    /// [`Screen::clipboard_targets`] lists them.
    pub async fn clipboard_bytes(&self, selection: Selection, target: &str) -> Result<Vec<u8>> {
        self.clipboard_port()?.bytes(selection, target).await
    }

    /// The types this selection can be read as.
    pub async fn clipboard_targets(&self, selection: Selection) -> Result<Vec<String>> {
        self.clipboard_port()?.targets(selection).await
    }

    /// Put bytes on a selection, offered as this type.
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

    /// Put text on one of this screen's selections.
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

    pub async fn move_to(&self, at: impl Into<Point>) -> Result<()> {
        self.driver.move_to(at.into()).await
    }

    pub async fn click(&self, at: impl Into<Point>, button: Button) -> Result<()> {
        self.driver.click(at.into(), button).await
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

    pub async fn type_text(&self, text: &str) -> Result<()> {
        self.driver.type_text(text).await
    }

    pub async fn key(&self, chord: &str) -> Result<()> {
        self.driver.key(chord).await
    }

    pub async fn scroll(&self, at: impl Into<Point>, by: Delta) -> Result<()> {
        self.driver.scroll(at.into(), by).await
    }

    pub async fn cursor(&self) -> Result<Point> {
        self.driver.cursor().await
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

    async fn move_to(&self, at: Point) -> Result<()> {
        self.driver.move_to(at).await
    }

    async fn click(&self, at: Point, button: Button) -> Result<()> {
        self.driver.click(at, button).await
    }

    async fn double_click(&self, at: Point, button: Button) -> Result<()> {
        self.driver.double_click(at, button).await
    }

    async fn drag(&self, from: Point, to: Point, button: Button) -> Result<()> {
        self.driver.drag(from, to, button).await
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        self.driver.type_text(text).await
    }

    async fn key(&self, chord: &str) -> Result<()> {
        self.driver.key(chord).await
    }

    async fn scroll(&self, at: Point, by: Delta) -> Result<()> {
        self.driver.scroll(at, by).await
    }

    async fn cursor(&self) -> Result<Point> {
        self.driver.cursor().await
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

/// A screen held under a lease.
///
/// Dereferences to the [`Screen`]. The lease is returned on drop, and a
/// stale release is refused.
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

    /// Give the screen back now, and say whether the release was accepted.
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
        // Refused where somebody newer holds it: a slow holder's release must
        // not tear down the screen its replacement is working on.
        let _ = self.leases.release(&self.lease);
    }
}

/// A person is driving. The owner may look, and may not touch.
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
    /// Where the person connects. `None` when ports were not published.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn screen(&self) -> ScreenId {
        self.screen
    }

    /// Whether the owner's input is being held back, or both are driving.
    pub fn exclusive(&self) -> bool {
        self.exclusive
    }

    /// Take the screen back, and close the input-accepting viewer.
    ///
    /// Refused where somebody else has been handed the screen since.
    pub async fn end(self) -> Result<()> {
        // Checked here first, because this process may know it was replaced …
        if !self.control.hand_back(&self.token) {
            return Err(Error::denied(
                "this takeover is no longer the one running; somebody else has the screen",
            ));
        }

        // … and checked again in the box, because a replacement started by
        // another process is one this gate never heard about.
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

/// Remove every box on this machine whose deadline has passed.
///
/// Reads [`EXPIRY_LABEL`] from the runtime, so it finds boxes this process
/// never opened. A runtime that cannot list by label answers `Unsupported`.
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
            // A label nobody here wrote. Left alone: this sweeps what it
            // understands and nothing else.
            continue;
        };

        if seconds >= at {
            machine.stop(&name).await?;
            swept.push(name);
        }
    }

    Ok(swept)
}

/// Who holds a screen when the caller did not say.
///
/// One holder per process, so two handles in one program share their
/// screens and two programs do not.
fn process_holder() -> HolderId {
    HolderId::new(format!("process-{}", std::process::id()))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0)
}

/// One more than the last caller got.
///
/// A coarse clock puts two calls in the same tick, and two boxes asking for
/// one name is a launch that fails on a conflict nobody caused.
fn tick() -> u64 {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A name no other box on this machine holds.
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
