use super::{ImageSource, Profile};
use crate::image;
use crate::machine::MachineHost;
use crate::{
    Address, DesktopFactory, DesktopSupport, Error, ExecResult, PortLayout, Result, ScreenAction,
    ScreenId, Secret, Viewers,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How a profile turns a typed screen action into an image command.
///
/// This is the compatibility boundary for images that own their screen
/// implementation. A later native runtime can perform the action directly;
/// profiles that use scripts can share this command protocol meanwhile.
pub trait ScreenCommands: Send + Sync {
    fn command(&self, action: ScreenAction, screen: ScreenId, extra: &[String]) -> Vec<String>;
}

/// A screen command whose first arguments are fixed.
///
/// The built-in images use `computer-screen`. A custom image can use another
/// executable, or an interpreter plus a script path, without rewriting the
/// action and screen-number protocol in its profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandScreen {
    prefix: Vec<String>,
}

impl CommandScreen {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            prefix: vec![program.into()],
        }
    }

    pub fn with_args<I, S>(program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut prefix = vec![program.into()];
        prefix.extend(args.into_iter().map(Into::into));
        Self { prefix }
    }

    pub fn prefix(&self) -> &[String] {
        &self.prefix
    }
}

impl ScreenCommands for CommandScreen {
    fn command(&self, action: ScreenAction, screen: ScreenId, extra: &[String]) -> Vec<String> {
        let mut command = self.prefix.clone();
        command.push(action.verb().to_string());
        command.push(screen.0.to_string());
        command.extend_from_slice(extra);
        command
    }
}

/// Browser operations that are independent of screen lifecycle.
#[async_trait]
pub trait BrowserRuntime: Send + Sync {
    async fn open(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        url: &str,
    ) -> Result<()>;
}

/// Open pages through the command protocol supplied by a profile.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandBrowserRuntime;

#[async_trait]
impl BrowserRuntime for CommandBrowserRuntime {
    async fn open(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        url: &str,
    ) -> Result<()> {
        let result = host.exec(&profile.open_command(screen, url)).await?;
        match result.code {
            0 => Ok(()),
            code => Err(Error::Failed {
                code,
                stderr: result.stderr_utf8().trim().to_string(),
            }),
        }
    }
}

/// Change one running screen's wallpaper.
#[async_trait]
pub trait WallpaperRuntime: Send + Sync {
    async fn set(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        path: &Path,
    ) -> Result<()>;

    /// Whether this runtime can set one at all.
    ///
    /// Separate from [`WallpaperRuntime::set`] so a caller can be refused
    /// before it sends an image nothing will use.
    fn supported(&self) -> Result<()> {
        Ok(())
    }
}

/// A wallpaper setter implemented by a command inside the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandWallpaperRuntime {
    prefix: Vec<String>,
}

impl CommandWallpaperRuntime {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            prefix: vec![program.into()],
        }
    }

    pub fn with_args<I, S>(program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut prefix = vec![program.into()];
        prefix.extend(args.into_iter().map(Into::into));
        Self { prefix }
    }
}

#[async_trait]
impl WallpaperRuntime for CommandWallpaperRuntime {
    async fn set(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        path: &Path,
    ) -> Result<()> {
        let mut command = self.prefix.clone();
        command.push(path.display().to_string());
        let result = host
            .run_within(&command, &profile.screen_env(screen), host.timeout())
            .await?;

        CommandScreenRuntime::succeeded(result)
    }
}

/// The built-in X11 wallpaper setter.
#[derive(Debug, Clone, Copy, Default)]
pub struct X11WallpaperRuntime;

#[async_trait]
impl WallpaperRuntime for X11WallpaperRuntime {
    async fn set(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        path: &Path,
    ) -> Result<()> {
        CommandWallpaperRuntime::with_args("hsetroot", ["-fill"])
            .set(host, profile, screen, path)
            .await
    }
}

/// The built-in Wayland wallpaper setter.
#[derive(Debug, Clone, Copy, Default)]
pub struct WaylandWallpaperRuntime;

#[async_trait]
impl WallpaperRuntime for WaylandWallpaperRuntime {
    async fn set(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        path: &Path,
    ) -> Result<()> {
        let sockfile = format!("/tmp/computer/screen-{}.sway", screen.0);
        let socket = host
            .machine()
            .read_file(host.name(), Path::new(&sockfile))
            .await?;
        let socket = String::from_utf8(socket)
            .map_err(|_| Error::denied(format!("{sockfile} is not text")))?;
        let socket = socket.trim();
        if socket.is_empty() {
            return Err(Error::Gone(format!("no compositor on {screen}")));
        }

        let command = vec![
            "swaymsg".to_string(),
            "-s".to_string(),
            socket.to_string(),
            "output".to_string(),
            "HEADLESS-1".to_string(),
            "bg".to_string(),
            path.display().to_string(),
            "fill".to_string(),
        ];
        let result = host
            .run_within(&command, &profile.screen_env(screen), host.timeout())
            .await?;

        CommandScreenRuntime::succeeded(result)
    }
}

/// A profile that does not declare wallpaper support.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedWallpaperRuntime;

#[async_trait]
impl WallpaperRuntime for UnsupportedWallpaperRuntime {
    async fn set(
        &self,
        _host: &MachineHost,
        _profile: &dyn Profile,
        _screen: ScreenId,
        _path: &Path,
    ) -> Result<()> {
        self.supported()
    }

    fn supported(&self) -> Result<()> {
        Err(Error::Unsupported {
            gaps: vec!["wallpaper"],
        })
    }
}

/// One window on a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Whatever the display server calls it. An X11 window id, or a sway
    /// container id.
    pub id: String,
    pub title: String,
}

/// Starting programs on a screen, and finding what they drew.
///
/// The whole reason this is a runtime rather than a command is readiness. A
/// mapped window is not a drawn one: GIMP maps a splash screen carrying its
/// own `WM_CLASS` about half a second before the program exists, and VS Code
/// maps its real window and then paints for another second. Both are
/// display-server questions, and neither is answerable in one command.
/// What to start, and what counts as it having started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The whole argv, flags included.
    pub command: Vec<String>,
    /// The window class to wait for.
    pub class: String,
    /// How long the window has to hold still before it counts as drawn.
    pub settle: Duration,
    /// How long to wait for that before giving up.
    pub within: Duration,
}

#[async_trait]
pub trait AppRuntime: Send + Sync {
    /// Start it, and return the window it drew.
    ///
    /// Returns only once a window of the class has held still for `settle`,
    /// so a caller's next click lands on the app rather than on its splash
    /// screen or its empty frame.
    async fn launch(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        launch: &Launch,
    ) -> Result<Window>;

    async fn windows(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
    ) -> Result<Vec<Window>>;

    async fn focus(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        window: &str,
    ) -> Result<()>;

    async fn close(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        window: &str,
    ) -> Result<()>;

    /// Whether this runtime can start anything at all.
    fn supported(&self) -> Result<()> {
        Ok(())
    }
}

/// A profile that does not declare app support.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedAppRuntime;

#[async_trait]
impl AppRuntime for UnsupportedAppRuntime {
    async fn launch(
        &self,
        _host: &MachineHost,
        _profile: &dyn Profile,
        _screen: ScreenId,
        _launch: &Launch,
    ) -> Result<Window> {
        Err(self.supported().unwrap_err())
    }

    async fn windows(
        &self,
        _host: &MachineHost,
        _profile: &dyn Profile,
        _screen: ScreenId,
    ) -> Result<Vec<Window>> {
        Err(self.supported().unwrap_err())
    }

    async fn focus(
        &self,
        _host: &MachineHost,
        _profile: &dyn Profile,
        _screen: ScreenId,
        _window: &str,
    ) -> Result<()> {
        self.supported()
    }

    async fn close(
        &self,
        _host: &MachineHost,
        _profile: &dyn Profile,
        _screen: ScreenId,
        _window: &str,
    ) -> Result<()> {
        self.supported()
    }

    fn supported(&self) -> Result<()> {
        Err(Error::Unsupported { gaps: vec!["apps"] })
    }
}

/// Apps on an X11 screen, through `xdotool`, `xprop` and ImageMagick.
#[derive(Debug, Clone, Copy, Default)]
pub struct X11AppRuntime;

impl X11AppRuntime {
    /// A visible, `_NET_WM_WINDOW_TYPE_NORMAL` window of this class, and the
    /// hash of what it is showing.
    ///
    /// The window type is what tells a program from its splash screen: GIMP's
    /// splash carries the same `WM_CLASS` as GIMP, and appears first.
    fn probe(class: &str) -> Vec<String> {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            format!(
                "for w in $(xdotool search --onlyvisible --class {class} 2>/dev/null); do \
                   xprop -id $w _NET_WM_WINDOW_TYPE 2>/dev/null | grep -q _NET_WM_WINDOW_TYPE_NORMAL || continue; \
                   echo \"$w $(import -window $w png:- 2>/dev/null | cksum | cut -d\" \" -f1) $(xdotool getwindowname $w 2>/dev/null)\"; \
                   exit 0; \
                 done"
            ),
        ]
    }
}

#[async_trait]
impl AppRuntime for X11AppRuntime {
    async fn launch(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        launch: &Launch,
    ) -> Result<Window> {
        let Launch {
            command,
            class,
            settle,
            within,
        } = launch;
        let (settle, within) = (*settle, *within);
        let env = profile.screen_env(screen);

        // Detached, and its output thrown away: a GUI program does not exit,
        // so a call that waited for it would never come back.
        let start = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!(
                "setsid {} >/dev/null 2>&1 </dev/null &",
                command
                    .iter()
                    .map(|word| shell_word(word))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ];
        CommandScreenRuntime::succeeded(host.run_within(&start, &env, host.timeout()).await?)?;

        let probe = Self::probe(class);
        let deadline = Instant::now() + within;
        let mut seen: Option<(String, String, String)> = None;
        let mut since = Instant::now();

        while Instant::now() < deadline {
            let result = host.run_within(&probe, &env, host.timeout()).await?;
            let line = result.stdout_utf8();
            let mut words = line.trim().splitn(3, ' ');

            match (words.next(), words.next()) {
                (Some(id), Some(hash)) if !id.is_empty() => {
                    let title = words.next().unwrap_or_default().to_string();
                    let now = (id.to_string(), hash.to_string(), title);

                    match seen.as_ref() {
                        // Unchanged: it is drawing nothing new, which is the
                        // only signal both a splash-screened program and an
                        // Electron one give at the same point.
                        Some((was_id, was_hash, _)) if was_id == &now.0 && was_hash == &now.1 => {
                            if since.elapsed() >= settle {
                                return Ok(Window {
                                    id: now.0,
                                    title: now.2,
                                });
                            }
                        }
                        _ => since = Instant::now(),
                    }

                    seen = Some(now);
                }
                _ => since = Instant::now(),
            }

            tokio::time::sleep(POLL).await;
        }

        Err(Error::Timeout {
            after: within,
            detail: format!(
                "{} started, but no window of class {class} settled",
                command.first().map(String::as_str).unwrap_or("the app")
            ),
        })
    }

    async fn windows(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
    ) -> Result<Vec<Window>> {
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"for w in $(xdotool search --onlyvisible --name . 2>/dev/null); do echo "$w $(xdotool getwindowname $w 2>/dev/null)"; done"#
                .to_string(),
        ];
        let result = host
            .run_within(&argv, &profile.screen_env(screen), host.timeout())
            .await?;

        Ok(result
            .stdout_utf8()
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(id, title)| Window {
                id: id.to_string(),
                title: title.to_string(),
            })
            .collect())
    }

    async fn focus(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        window: &str,
    ) -> Result<()> {
        let argv = vec![
            "xdotool".to_string(),
            "windowactivate".to_string(),
            window.to_string(),
        ];

        CommandScreenRuntime::succeeded(
            host.run_within(&argv, &profile.screen_env(screen), host.timeout())
                .await?,
        )
    }

    async fn close(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        window: &str,
    ) -> Result<()> {
        let argv = vec![
            "xdotool".to_string(),
            "windowclose".to_string(),
            window.to_string(),
        ];

        CommandScreenRuntime::succeeded(
            host.run_within(&argv, &profile.screen_env(screen), host.timeout())
                .await?,
        )
    }
}

/// How often a launch asks whether the window has stopped moving.
const POLL: Duration = Duration::from_millis(100);

/// One word, safe inside the shell a launch builds.
///
/// A launch takes an argv and has to hand it to `setsid` through `sh -c`,
/// because nothing else detaches. Quoting here is what keeps an argument an
/// argument.
fn shell_word(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// The lifecycle of a screen, independent of how an image implements it.
///
/// Built-in and existing custom images use [`CommandScreenRuntime`]. A guest
/// agent or another native implementation can implement these operations
/// without changing `Computer`, `Screen` or `Takeover`.
#[async_trait]
pub trait ScreenRuntime: Send + Sync {
    async fn start(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
    ) -> Result<()>;

    async fn stop(&self, host: &MachineHost, profile: &dyn Profile, screen: ScreenId)
    -> Result<()>;

    async fn viewers(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
    ) -> Result<Viewers>;

    async fn control(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        token: &str,
        shared: bool,
    ) -> Result<()>;

    async fn release(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        token: &str,
    ) -> Result<()>;

    async fn reclaim(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
    ) -> Result<()>;
}

/// Run the command protocol supplied by a profile.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandScreenRuntime;

impl CommandScreenRuntime {
    async fn run(&self, host: &MachineHost, command: Vec<String>) -> Result<ExecResult> {
        host.exec(&command).await
    }

    fn succeeded(result: ExecResult) -> Result<()> {
        match result.code {
            0 => Ok(()),
            code => Err(Error::Failed {
                code,
                stderr: result.stderr_utf8().trim().to_string(),
            }),
        }
    }

    fn released(result: ExecResult) -> Result<()> {
        match result.code {
            0 => Ok(()),
            3 => Err(Error::denied(
                "this takeover was replaced; the screen belongs to whoever took it",
            )),
            code => Err(Error::Failed {
                code,
                stderr: result.stderr_utf8().trim().to_string(),
            }),
        }
    }
}

#[async_trait]
impl ScreenRuntime for CommandScreenRuntime {
    async fn start(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
    ) -> Result<()> {
        Self::succeeded(self.run(host, profile.start_command(screen)).await?)
    }

    async fn stop(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
    ) -> Result<()> {
        Self::succeeded(self.run(host, profile.stop_command(screen)).await?)
    }

    async fn viewers(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
    ) -> Result<Viewers> {
        let result = self.run(host, profile.viewers_command(screen)).await?;
        if result.code != 0 {
            return Err(Error::Failed {
                code: result.code,
                stderr: result.stderr_utf8().trim().to_string(),
            });
        }

        Viewers::parse(&result.stdout_utf8())
            .ok_or_else(|| Error::denied("the viewer count could not be read"))
    }

    async fn control(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        token: &str,
        shared: bool,
    ) -> Result<()> {
        Self::succeeded(
            self.run(host, profile.control_command(screen, token, shared))
                .await?,
        )
    }

    async fn release(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
        token: &str,
    ) -> Result<()> {
        Self::released(
            self.run(host, profile.release_command(screen, token))
                .await?,
        )
    }

    async fn reclaim(
        &self,
        host: &MachineHost,
        profile: &dyn Profile,
        screen: ScreenId,
    ) -> Result<()> {
        Self::succeeded(self.run(host, profile.reclaim_command(screen)).await?)
    }
}

/// How a profile records and recovers the size of a desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeometrySpec {
    width_env: String,
    height_env: String,
    default: (u32, u32),
}

impl GeometrySpec {
    pub fn new(
        width_env: impl Into<String>,
        height_env: impl Into<String>,
        default: (u32, u32),
    ) -> Self {
        Self {
            width_env: width_env.into(),
            height_env: height_env.into(),
            default,
        }
    }

    pub fn default_size(&self) -> (u32, u32) {
        self.default
    }

    pub fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String> {
        BTreeMap::from([
            (self.width_env.clone(), width.to_string()),
            (self.height_env.clone(), height.to_string()),
        ])
    }

    pub fn from_env(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        let width = environment.get(&self.width_env)?.parse().ok()?;
        let height = environment.get(&self.height_env)?.parse().ok()?;
        Some((width, height))
    }
}

/// The parts of the bundled desktop contract that do not depend on a display
/// server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopContract {
    name: String,
    image: ImageSource,
    ports: PortLayout,
    screens: CommandScreen,
    boot: Vec<String>,
    geometry: GeometrySpec,
}

impl DesktopContract {
    pub fn new(
        name: impl Into<String>,
        image: ImageSource,
        ports: PortLayout,
        screens: CommandScreen,
        boot: impl IntoIterator<Item = impl Into<String>>,
        geometry: GeometrySpec,
    ) -> Self {
        Self {
            name: name.into(),
            image,
            ports,
            screens,
            boot: boot.into_iter().map(Into::into).collect(),
            geometry,
        }
    }

    /// The shared contract implemented by the bundled X11 and Wayland images.
    pub fn standard(name: impl Into<String>, image: ImageSource) -> Self {
        Self::new(
            name,
            image,
            PortLayout {
                view_base: image::VIEW_PORT_BASE,
                vnc_base: image::VNC_PORT_BASE,
                devtools: Some(image::DEVTOOLS_PORT),
                devtools_bridge: Some(image::DEVTOOLS_BRIDGE_PORT),
                max_screens: image::MAX_SCREENS,
            },
            CommandScreen::new(image::SCREEN_COMMAND),
            [image::DESKTOP_COMMAND, "--once"],
            GeometrySpec::new(
                image::WIDTH_ENV,
                image::HEIGHT_ENV,
                (image::WIDTH, image::HEIGHT),
            ),
        )
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn image(&self) -> ImageSource {
        self.image.clone()
    }

    pub fn ports(&self) -> PortLayout {
        self.ports
    }

    pub fn default_size(&self) -> (u32, u32) {
        self.geometry.default_size()
    }

    pub fn screen_command(
        &self,
        action: ScreenAction,
        screen: ScreenId,
        extra: &[String],
    ) -> Vec<String> {
        self.screens.command(action, screen, extra)
    }

    pub fn boot_command(&self) -> Vec<String> {
        self.boot.clone()
    }

    pub fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String> {
        self.geometry.launch_env(width, height)
    }

    pub fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        self.geometry.from_env(environment)
    }
}

/// How driver commands reach one screen's display server.
pub trait ScreenEnvironment: Send + Sync {
    fn environment(&self, screen: ScreenId) -> BTreeMap<String, String>;
}

/// Where a person watches a screen.
///
/// The shipped images serve noVNC's `vnc.html`; an image with a viewer of its
/// own hands back a different address for the same port.
pub trait ViewerUrl: Send + Sync {
    fn url(&self, at: &Address, ticket: Option<&Secret>) -> String;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct X11Environment;

impl ScreenEnvironment for X11Environment {
    fn environment(&self, screen: ScreenId) -> BTreeMap<String, String> {
        BTreeMap::from([(
            "DISPLAY".to_string(),
            crate::servers::x11::display_for(screen),
        )])
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WaylandEnvironment;

impl ScreenEnvironment for WaylandEnvironment {
    fn environment(&self, screen: ScreenId) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "WAYLAND_DISPLAY".to_string(),
                crate::servers::wayland::DISPLAY_NAME.to_string(),
            ),
            (
                "XDG_RUNTIME_DIR".to_string(),
                crate::servers::wayland::runtime_dir(screen),
            ),
        ])
    }
}

/// A profile derived from another profile with explicit overrides.
///
/// Ports, display behavior, capabilities, environment and viewer URLs stay on
/// the base profile unless a separate profile is needed. This keeps a custom
/// image from copying the X11 or Wayland contract only to change its image or
/// command names.
#[derive(Clone)]
pub struct ConfiguredProfile {
    base: Arc<dyn Profile>,
    name: Option<String>,
    image: Option<ImageSource>,
    driver: Option<Arc<dyn DesktopFactory>>,
    screens: Option<Arc<dyn ScreenCommands>>,
    screen_runtime: Option<Arc<dyn ScreenRuntime>>,
    browser_runtime: Option<Arc<dyn BrowserRuntime>>,
    wallpaper_runtime: Option<Arc<dyn WallpaperRuntime>>,
    boot: Option<Vec<String>>,
    ports: Option<PortLayout>,
    geometry: Option<GeometrySpec>,
    screen_environment: Option<Arc<dyn ScreenEnvironment>>,
    support: Option<DesktopSupport>,
    viewer: Option<Arc<dyn ViewerUrl>>,
}

impl ConfiguredProfile {
    pub fn base(&self) -> &Arc<dyn Profile> {
        &self.base
    }
}

impl std::fmt::Debug for ConfiguredProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfiguredProfile")
            .field("base", &self.base.name())
            .field("name", &self.name)
            .field("image", &self.image)
            .field("driver", &self.driver.as_ref().map(|_| "custom"))
            .field("screens", &self.screens.as_ref().map(|_| "custom"))
            .field(
                "screen_runtime",
                &self.screen_runtime.as_ref().map(|_| "custom"),
            )
            .field(
                "browser_runtime",
                &self.browser_runtime.as_ref().map(|_| "custom"),
            )
            .field(
                "wallpaper_runtime",
                &self.wallpaper_runtime.as_ref().map(|_| "custom"),
            )
            .field("boot", &self.boot)
            .field("ports", &self.ports)
            .field("geometry", &self.geometry)
            .field(
                "screen_environment",
                &self.screen_environment.as_ref().map(|_| "custom"),
            )
            .field("support", &self.support)
            .field("viewer", &self.viewer.as_ref().map(|_| "custom"))
            .finish()
    }
}

/// Derive a custom profile from a tested base contract.
pub struct ProfileBuilder {
    profile: ConfiguredProfile,
}

impl ProfileBuilder {
    pub fn new<P>(base: P) -> Self
    where
        P: Profile + 'static,
    {
        Self::from_arc(Arc::new(base))
    }

    pub fn from_arc(base: Arc<dyn Profile>) -> Self {
        Self {
            profile: ConfiguredProfile {
                base,
                name: None,
                image: None,
                driver: None,
                screens: None,
                screen_runtime: None,
                browser_runtime: None,
                wallpaper_runtime: None,
                boot: None,
                ports: None,
                geometry: None,
                screen_environment: None,
                support: None,
                viewer: None,
            },
        }
    }

    /// Change the contract name.
    ///
    /// A built image must carry the same value in `computer.profile`.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.profile.name = Some(name.into());
        self
    }

    pub fn image(mut self, image: ImageSource) -> Self {
        self.profile.image = Some(image);
        self
    }

    /// Build this profile's image from a local Docker build context.
    ///
    /// The directory needs a `Dockerfile` carrying this profile's name in its
    /// `computer.profile` label. Carried here rather than on
    /// [`crate::Builder`] so a launch cannot pair a custom image with the
    /// wrong contract by omission.
    pub fn image_dir(self, directory: impl Into<PathBuf>) -> Self {
        self.image(ImageSource::Directory(directory.into()))
    }

    /// The driver this image expects.
    ///
    /// A base contract names one, and an image that keeps the contract but
    /// speaks to a different display server needs its own.
    pub fn driver<D>(mut self, driver: D) -> Self
    where
        D: DesktopFactory + 'static,
    {
        self.profile.driver = Some(Arc::new(driver));
        self
    }

    pub fn screen_commands<S>(mut self, screens: S) -> Self
    where
        S: ScreenCommands + 'static,
    {
        self.profile.screens = Some(Arc::new(screens));
        self
    }

    pub fn screen_runtime<R>(mut self, runtime: R) -> Self
    where
        R: ScreenRuntime + 'static,
    {
        self.profile.screen_runtime = Some(Arc::new(runtime));
        self
    }

    pub fn browser_runtime<R>(mut self, runtime: R) -> Self
    where
        R: BrowserRuntime + 'static,
    {
        self.profile.browser_runtime = Some(Arc::new(runtime));
        self
    }

    pub fn wallpaper_runtime<R>(mut self, runtime: R) -> Self
    where
        R: WallpaperRuntime + 'static,
    {
        self.profile.wallpaper_runtime = Some(Arc::new(runtime));
        self
    }

    pub fn boot_command<I, S>(mut self, command: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.profile.boot = Some(command.into_iter().map(Into::into).collect());
        self
    }

    /// Where this image's screens listen.
    ///
    /// The port arithmetic comes with it, so a screen's ports and the ones the
    /// runtime publishes cannot disagree.
    pub fn ports(mut self, ports: PortLayout) -> Self {
        self.profile.ports = Some(ports);
        self
    }

    /// How a desktop's size is asked for and read back.
    ///
    /// One spec rather than three methods: the default size, the environment a
    /// launch carries and the geometry read off a running box have to agree,
    /// and separately overridable versions of them would not have to.
    pub fn geometry(mut self, geometry: GeometrySpec) -> Self {
        self.profile.geometry = Some(geometry);
        self
    }

    /// The variables one screen's commands run with.
    pub fn screen_environment<E>(mut self, environment: E) -> Self
    where
        E: ScreenEnvironment + 'static,
    {
        self.profile.screen_environment = Some(Arc::new(environment));
        self
    }

    /// What this image can do.
    ///
    /// The display is filled in per request from the size asked for, so a
    /// caller states the capabilities and not the geometry twice.
    pub fn support(mut self, support: DesktopSupport) -> Self {
        self.profile.support = Some(support);
        self
    }

    /// Where a person watches a screen.
    pub fn viewer_url<V>(mut self, viewer: V) -> Self
    where
        V: ViewerUrl + 'static,
    {
        self.profile.viewer = Some(Arc::new(viewer));
        self
    }

    pub fn build(self) -> ConfiguredProfile {
        self.profile
    }
}

impl Profile for ConfiguredProfile {
    fn name(&self) -> &str {
        self.name.as_deref().unwrap_or_else(|| self.base.name())
    }

    fn image(&self) -> ImageSource {
        self.image.clone().unwrap_or_else(|| self.base.image())
    }

    fn ports(&self) -> PortLayout {
        self.ports.unwrap_or_else(|| self.base.ports())
    }

    fn default_size(&self) -> (u32, u32) {
        match &self.geometry {
            Some(geometry) => geometry.default_size(),
            None => self.base.default_size(),
        }
    }

    fn support_at(&self, width: u32, height: u32) -> DesktopSupport {
        let Some(support) = &self.support else {
            return self.base.support_at(width, height);
        };

        // The size is the request's, not the template's: a caller states what
        // the image can do once, and every size it is asked for reuses it.
        DesktopSupport {
            display: support.display.map(|display| crate::Display {
                width,
                height,
                ..display
            }),
            ..support.clone()
        }
    }

    fn driver(&self) -> Arc<dyn DesktopFactory> {
        self.driver.clone().unwrap_or_else(|| self.base.driver())
    }

    fn screen_runtime(&self) -> Arc<dyn ScreenRuntime> {
        self.screen_runtime
            .clone()
            .unwrap_or_else(|| self.base.screen_runtime())
    }

    fn browser_runtime(&self) -> Arc<dyn BrowserRuntime> {
        self.browser_runtime
            .clone()
            .unwrap_or_else(|| self.base.browser_runtime())
    }

    fn wallpaper_runtime(&self) -> Arc<dyn WallpaperRuntime> {
        self.wallpaper_runtime
            .clone()
            .unwrap_or_else(|| self.base.wallpaper_runtime())
    }

    fn screen_command(
        &self,
        action: ScreenAction,
        screen: ScreenId,
        extra: &[String],
    ) -> Vec<String> {
        match &self.screens {
            Some(screens) => screens.command(action, screen, extra),
            None => self.base.screen_command(action, screen, extra),
        }
    }

    fn boot_command(&self) -> Vec<String> {
        self.boot
            .clone()
            .unwrap_or_else(|| self.base.boot_command())
    }

    fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String> {
        match &self.geometry {
            Some(geometry) => geometry.launch_env(width, height),
            None => self.base.launch_env(width, height),
        }
    }

    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String> {
        match &self.screen_environment {
            Some(environment) => environment.environment(screen),
            None => self.base.screen_env(screen),
        }
    }

    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        match &self.geometry {
            Some(geometry) => geometry.from_env(environment),
            None => self.base.geometry_from(environment),
        }
    }

    fn viewer_url(&self, at: &Address, ticket: Option<&Secret>) -> String {
        match &self.viewer {
            Some(viewer) => viewer.url(at, ticket),
            None => self.base.viewer_url(at, ticket),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ScriptedCli, ScriptedProfile};
    use crate::{DisplayServer, WaylandProfile, X11Profile};
    use std::sync::Mutex;

    #[derive(Clone)]
    struct RecordingRuntime {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl RecordingRuntime {
        fn record(&self, action: &'static str) {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(action);
            }
        }
    }

    #[async_trait]
    impl ScreenRuntime for RecordingRuntime {
        async fn start(
            &self,
            _host: &MachineHost,
            _profile: &dyn Profile,
            _screen: ScreenId,
        ) -> Result<()> {
            self.record("start");
            Ok(())
        }

        async fn stop(
            &self,
            _host: &MachineHost,
            _profile: &dyn Profile,
            _screen: ScreenId,
        ) -> Result<()> {
            self.record("stop");
            Ok(())
        }

        async fn viewers(
            &self,
            _host: &MachineHost,
            _profile: &dyn Profile,
            _screen: ScreenId,
        ) -> Result<Viewers> {
            self.record("viewers");
            Ok(Viewers::default())
        }

        async fn control(
            &self,
            _host: &MachineHost,
            _profile: &dyn Profile,
            _screen: ScreenId,
            _token: &str,
            _shared: bool,
        ) -> Result<()> {
            self.record("control");
            Ok(())
        }

        async fn release(
            &self,
            _host: &MachineHost,
            _profile: &dyn Profile,
            _screen: ScreenId,
            _token: &str,
        ) -> Result<()> {
            self.record("release");
            Ok(())
        }

        async fn reclaim(
            &self,
            _host: &MachineHost,
            _profile: &dyn Profile,
            _screen: ScreenId,
        ) -> Result<()> {
            self.record("reclaim");
            Ok(())
        }
    }

    #[derive(Clone)]
    struct RecordingBrowserRuntime {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl BrowserRuntime for RecordingBrowserRuntime {
        async fn open(
            &self,
            _host: &MachineHost,
            _profile: &dyn Profile,
            _screen: ScreenId,
            _url: &str,
        ) -> Result<()> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push("open");
            }
            Ok(())
        }
    }

    #[derive(Clone)]
    struct RecordingWallpaperRuntime {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl WallpaperRuntime for RecordingWallpaperRuntime {
        async fn set(
            &self,
            _host: &MachineHost,
            _profile: &dyn Profile,
            _screen: ScreenId,
            _path: &Path,
        ) -> Result<()> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push("wallpaper");
            }
            Ok(())
        }
    }

    fn host(cli: Arc<ScriptedCli>) -> MachineHost {
        let machine: Arc<dyn crate::Machine> = Arc::new(crate::DockerMachine::new(
            cli as Arc<dyn crate::ContainerCli>,
        ));
        MachineHost::new(machine, Arc::new(ScriptedProfile), "box")
    }

    #[test]
    fn test_a_command_screen_keeps_the_shared_protocol_shape() {
        let screens = CommandScreen::with_args("python3", ["/opt/custom/screen.py"]);

        assert_eq!(
            screens.command(
                ScreenAction::Control,
                ScreenId(2),
                &["token".to_string(), "shared".to_string()]
            ),
            vec![
                "python3",
                "/opt/custom/screen.py",
                "control",
                "2",
                "token",
                "shared"
            ]
        );
    }

    #[tokio::test]
    async fn test_the_command_runtime_uses_the_profiles_protocol() {
        let cli = Arc::new(ScriptedCli::new().saying("watching=2 driving=1"));
        let host = host(Arc::clone(&cli));

        let viewers = CommandScreenRuntime
            .viewers(&host, &ScriptedProfile, ScreenId(2))
            .await
            .expect("viewer count");

        assert_eq!((viewers.watching, viewers.driving), (2, 1));
        assert!(
            cli.last().is_some_and(|command| command.ends_with(&[
                "scripted-screen".to_string(),
                "viewers".to_string(),
                "2".to_string(),
            ])),
            "the adapter must use the custom profile rather than a built-in command"
        );
    }

    #[tokio::test]
    async fn test_the_browser_runtime_uses_the_profiles_protocol() {
        let cli = Arc::new(ScriptedCli::new());
        let host = host(Arc::clone(&cli));

        CommandBrowserRuntime
            .open(&host, &ScriptedProfile, ScreenId(2), "https://example.com")
            .await
            .expect("a page");

        assert!(
            cli.last().is_some_and(|command| command.ends_with(&[
                "scripted-screen".to_string(),
                "open".to_string(),
                "2".to_string(),
                "https://example.com".to_string(),
            ])),
            "the adapter must use the custom profile rather than a built-in command"
        );
    }

    #[tokio::test]
    async fn test_a_command_wallpaper_runtime_supports_custom_images() {
        let cli = Arc::new(ScriptedCli::new());
        let host = host(Arc::clone(&cli));

        CommandWallpaperRuntime::new("custom-wallpaper")
            .set(
                &host,
                &ScriptedProfile,
                ScreenId(2),
                Path::new("/tmp/custom.image"),
            )
            .await
            .expect("a wallpaper");

        assert!(
            cli.last().is_some_and(|command| command.ends_with(&[
                "custom-wallpaper".to_string(),
                "/tmp/custom.image".to_string(),
            ])),
            "the custom image's setter must receive the guest path"
        );
    }

    #[tokio::test]
    async fn test_the_command_runtime_preserves_a_stale_release() {
        let cli = Arc::new(ScriptedCli::new().replying(ExecResult {
            code: 3,
            ..ExecResult::default()
        }));
        let host = host(cli);

        let error = CommandScreenRuntime
            .release(&host, &ScriptedProfile, ScreenId(0), "old-token")
            .await
            .expect_err("the token was replaced");

        assert!(matches!(error, Error::Denied { .. }));
    }

    #[tokio::test]
    async fn test_a_profile_runtime_receives_screen_lifecycle_calls() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let profile = ProfileBuilder::new(ScriptedProfile)
            .screen_runtime(RecordingRuntime {
                calls: Arc::clone(&calls),
            })
            .browser_runtime(RecordingBrowserRuntime {
                calls: Arc::clone(&calls),
            })
            .wallpaper_runtime(RecordingWallpaperRuntime {
                calls: Arc::clone(&calls),
            })
            .build();
        let cli = Arc::new(ScriptedCli::new());
        let computer = crate::Computer::builder()
            .cli(cli as Arc<dyn crate::ContainerCli>)
            .profile(Arc::new(profile))
            .wait_for_ready(None)
            .keep_on_drop(true)
            .launch()
            .await
            .expect("a box");

        let screen = computer
            .screen_unfenced(ScreenId(1))
            .await
            .expect("a screen");
        screen
            .open_url("https://example.com")
            .await
            .expect("a page");
        screen.set_wallpaper(b"image").await.expect("a wallpaper");
        computer
            .close_screen(ScreenId(1))
            .await
            .expect("screen stopped");

        assert_eq!(
            calls.lock().map(|calls| calls.clone()).unwrap_or_default(),
            ["start", "open", "wallpaper", "stop"]
        );
    }

    #[test]
    fn test_geometry_is_written_and_read_by_one_spec() {
        let geometry = GeometrySpec::new("WIDTH", "HEIGHT", (1280, 800));
        let environment = geometry.launch_env(1920, 1080);

        assert_eq!(geometry.default_size(), (1280, 800));
        assert_eq!(geometry.from_env(&environment), Some((1920, 1080)));
        assert_eq!(
            geometry.from_env(&BTreeMap::from([("WIDTH".to_string(), "1920".to_string())])),
            None,
            "half a size is not a coordinate space"
        );
    }

    #[test]
    fn test_the_builtin_contract_keeps_shared_values_together() {
        let contract = DesktopContract::standard(
            "custom",
            ImageSource::Registry("example/custom:1".to_string()),
        );

        assert_eq!(contract.name(), "custom");
        assert_eq!(contract.default_size(), (image::WIDTH, image::HEIGHT));
        assert_eq!(contract.ports().max_screens, image::MAX_SCREENS);
        assert_eq!(
            contract.screen_command(ScreenAction::Start, ScreenId(3), &[]),
            vec!["computer-screen", "start", "3"]
        );
        assert_eq!(contract.boot_command(), vec!["computer-desktop", "--once"]);
    }

    #[test]
    fn test_a_profile_override_keeps_the_base_contract() {
        let profile = ProfileBuilder::new(WaylandProfile)
            .name("custom-wayland")
            .image(ImageSource::Registry("example/custom:1".to_string()))
            .screen_commands(CommandScreen::new("custom-screen"))
            .boot_command(["custom-desktop", "--once"])
            .build();

        assert_eq!(profile.name(), "custom-wayland");
        assert_eq!(
            profile.image(),
            ImageSource::Registry("example/custom:1".to_string())
        );
        assert_eq!(
            profile.start_command(ScreenId(1)),
            vec!["custom-screen", "start", "1"]
        );
        assert_eq!(profile.boot_command(), vec!["custom-desktop", "--once"]);
        assert_eq!(profile.ports(), WaylandProfile.ports());
        assert_eq!(profile.driver().display_server(), DisplayServer::Wayland);
        assert_eq!(
            profile.launch_env(1920, 1080),
            WaylandProfile.launch_env(1920, 1080)
        );
    }

    #[test]
    fn test_a_profile_carries_the_build_context_it_was_given() {
        let profile = ProfileBuilder::new(X11Profile)
            .image_dir("images/ubuntu")
            .build();

        assert_eq!(
            profile.image().directory(),
            Some(Path::new("images/ubuntu")),
            "a derived profile has to name the image that implements it"
        );
        assert!(profile.image().bundle().is_none());
    }

    #[test]
    fn test_a_derived_profile_can_name_its_own_driver() {
        let profile = ProfileBuilder::new(X11Profile)
            .driver(crate::WaylandDriver)
            .build();

        // An image that keeps a contract but speaks to another display server
        // needs its own driver, or it has to write the whole trait out.
        assert_eq!(profile.driver().display_server(), DisplayServer::Wayland);
        assert_eq!(X11Profile.driver().display_server(), DisplayServer::X11);
    }

    #[test]
    fn test_a_derived_profile_can_name_its_own_ports() {
        let ports = PortLayout {
            view_base: 7000,
            vnc_base: 7100,
            devtools: None,
            devtools_bridge: None,
            max_screens: 2,
        };
        let profile = ProfileBuilder::new(X11Profile).ports(ports).build();

        assert_eq!(profile.ports(), ports);
        assert_ne!(profile.ports(), X11Profile.ports());
    }

    #[test]
    fn test_one_spec_governs_every_way_a_size_is_carried() {
        let profile = ProfileBuilder::new(X11Profile)
            .geometry(GeometrySpec::new("W", "H", (640, 480)))
            .build();

        assert_eq!(profile.default_size(), (640, 480));
        let launch = profile.launch_env(1024, 768);
        assert_eq!(launch.get("W").map(String::as_str), Some("1024"));
        assert_eq!(launch.get("H").map(String::as_str), Some("768"));
        assert_eq!(profile.geometry_from(&launch), Some((1024, 768)));

        // The base reads other names, so a spec that governed only one of the
        // three would let a launch and a read-back disagree.
        assert_eq!(X11Profile.geometry_from(&launch), None);
    }

    #[test]
    fn test_a_derived_profile_can_name_its_own_screen_environment() {
        let profile = ProfileBuilder::new(X11Profile)
            .screen_environment(WaylandEnvironment)
            .build();

        assert_eq!(
            profile.screen_env(ScreenId(1)),
            WaylandEnvironment.environment(ScreenId(1))
        );
    }

    #[test]
    fn test_stated_support_takes_the_size_it_is_asked_for() {
        let mut support = X11Profile.support_at(0, 0);
        support.browser = None;
        support.max_screens = 3;

        let profile = ProfileBuilder::new(X11Profile).support(support).build();
        let at = profile.support_at(1920, 1080);

        assert!(at.browser.is_none());
        assert_eq!(at.max_screens, 3);
        assert_eq!(
            at.display.map(|display| (display.width, display.height)),
            Some((1920, 1080)),
            "the size is the request's, not the template's"
        );
    }

    #[test]
    fn test_a_derived_profile_can_serve_its_own_viewer_page() {
        struct OwnPage;
        impl ViewerUrl for OwnPage {
            fn url(&self, at: &Address, _ticket: Option<&Secret>) -> String {
                format!("https://{}/watch", at.authority())
            }
        }

        let profile = ProfileBuilder::new(X11Profile).viewer_url(OwnPage).build();
        let at = Address {
            scheme: crate::Scheme::Http,
            host: "box.example".to_string(),
            port: 6080,
        };

        assert_eq!(
            profile.viewer_url(&at, None),
            "https://box.example:6080/watch"
        );
        assert!(X11Profile.viewer_url(&at, None).contains("vnc.html"));
    }

    #[test]
    fn test_an_unmodified_profile_is_only_a_wrapper() {
        let profile = ProfileBuilder::new(X11Profile).build();

        assert_eq!(profile.name(), X11Profile.name());
        assert_eq!(profile.image(), X11Profile.image());
        assert_eq!(profile.ports(), X11Profile.ports());
        assert_eq!(
            profile.open_command(ScreenId(0), "https://example.com"),
            X11Profile.open_command(ScreenId(0), "https://example.com")
        );
    }
}
