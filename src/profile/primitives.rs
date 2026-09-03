use super::{ImageSource, Profile};
use crate::image;
use crate::machine::MachineHost;
use crate::{
    Address, DesktopFactory, DesktopSupport, Error, ExecResult, PortLayout, Result, ScreenAction,
    ScreenId, Secret, Viewers,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

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
        Err(Error::Unsupported {
            gaps: vec!["wallpaper"],
        })
    }
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
    screens: Option<Arc<dyn ScreenCommands>>,
    screen_runtime: Option<Arc<dyn ScreenRuntime>>,
    browser_runtime: Option<Arc<dyn BrowserRuntime>>,
    wallpaper_runtime: Option<Arc<dyn WallpaperRuntime>>,
    boot: Option<Vec<String>>,
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
                screens: None,
                screen_runtime: None,
                browser_runtime: None,
                wallpaper_runtime: None,
                boot: None,
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
        self.base.ports()
    }

    fn default_size(&self) -> (u32, u32) {
        self.base.default_size()
    }

    fn support_at(&self, width: u32, height: u32) -> DesktopSupport {
        self.base.support_at(width, height)
    }

    fn driver(&self) -> Arc<dyn DesktopFactory> {
        self.base.driver()
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
        self.base.launch_env(width, height)
    }

    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String> {
        self.base.screen_env(screen)
    }

    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        self.base.geometry_from(environment)
    }

    fn viewer_url(&self, at: &Address, ticket: Option<&Secret>) -> String {
        self.base.viewer_url(at, ticket)
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
