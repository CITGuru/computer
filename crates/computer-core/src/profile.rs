pub mod primitives;

use crate::bundle::{Bundle, Extras};
use crate::desktop::DesktopFactory;
use crate::error::{Error, Result};
use crate::{DesktopSupport, ScreenAction, ScreenId, ScreenPorts};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use primitives::{
    AppRuntime, Arrange, BrowserRuntime, CommandBrowserRuntime, CommandScreen,
    CommandScreenRuntime, CommandWallpaperRuntime, ConfiguredProfile, DesktopContract,
    GeometrySpec, Launch, ProfileBuilder, ScreenCommands, ScreenEnvironment, ScreenRuntime,
    UnsupportedAppRuntime, UnsupportedWallpaperRuntime, ViewerUrl, WallpaperRuntime,
    WaylandAppRuntime, WaylandEnvironment, WaylandWallpaperRuntime, Window, X11AppRuntime,
    X11Environment, X11WallpaperRuntime,
};

pub const PROFILE_LABEL: &str = "computer.profile";

pub const PROFILE_ENV: &str = "COMPUTER_PROFILE";

/// `?` and `=` are percent-encoded because both viewer pages `decodeURIComponent` the value.
pub fn viewer_path(ticket: &crate::Secret) -> String {
    format!("&path=websockify%3Ftoken%3D{}", ticket.expose())
}

pub fn builtin(name: &str) -> Option<Arc<dyn Profile>> {
    match name {
        _ if name == crate::X11Profile.name() => Some(Arc::new(crate::X11Profile)),
        _ if name == crate::WaylandProfile.name() => Some(Arc::new(crate::WaylandProfile)),
        _ => None,
    }
}

pub const FORCE: &str = "--force";

pub const SHARED: &str = "shared";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    Bundled(&'static Bundle),
    Registry(String),
    Directory(PathBuf),
}

impl ImageSource {
    pub fn tag(&self, extras: &Extras) -> Result<String> {
        match self {
            Self::Bundled(bundle) => Ok(bundle.tag_with(extras)),
            Self::Registry(name) if extras.is_empty() => Ok(name.clone()),
            Self::Registry(_) => Err(Error::Unsupported {
                gaps: vec!["packages in an image this crate does not build"],
            }),
            Self::Directory(directory) => {
                crate::bundle::directory_image(directory, extras).map(|(_, tag)| tag)
            }
        }
    }

    pub fn bundle(&self) -> Option<&'static Bundle> {
        match self {
            Self::Bundled(bundle) => Some(bundle),
            Self::Registry(_) | Self::Directory(_) => None,
        }
    }

    pub fn directory(&self) -> Option<&Path> {
        match self {
            Self::Directory(directory) => Some(directory),
            Self::Bundled(_) | Self::Registry(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortLayout {
    pub view_base: u16,
    pub vnc_base: u16,
    pub devtools: Option<u16>,
    /// Chromium binds DevTools to loopback, so only the bridge is worth publishing.
    pub devtools_bridge: Option<u16>,
    pub max_screens: u32,
}

impl PortLayout {
    pub fn screen(&self, screen: ScreenId) -> Result<ScreenPorts> {
        if screen.0 >= self.max_screens {
            return Err(Error::ScreenUnavailable {
                screen: Some(screen),
                held_by: None,
            });
        }

        let offset = (screen.0 * 2) as u16;
        Ok(ScreenPorts {
            display_number: screen.0 + 1,
            view: self.view_base + offset,
            control: self.view_base + offset + 1,
            view_vnc: self.vnc_base + offset,
            control_vnc: self.vnc_base + offset + 1,
        })
    }

    pub fn viewer_ports(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = (0..self.max_screens)
            .filter_map(|screen| self.screen(ScreenId(screen)).ok())
            .flat_map(|slot| [slot.view, slot.control])
            .collect();
        ports.sort_unstable();
        ports
    }

    pub fn to_publish(&self) -> Vec<u16> {
        let mut ports = self.viewer_ports();
        ports.extend(self.devtools_bridge);
        ports
    }
}

pub trait Profile: Send + Sync {
    fn name(&self) -> &str;

    fn image(&self) -> ImageSource;

    fn ports(&self) -> PortLayout;

    fn default_size(&self) -> (u32, u32);

    fn support_at(&self, width: u32, height: u32) -> DesktopSupport;

    fn driver(&self) -> Arc<dyn DesktopFactory>;

    fn screen_runtime(&self) -> Arc<dyn ScreenRuntime> {
        Arc::new(CommandScreenRuntime)
    }

    fn browser_runtime(&self) -> Arc<dyn BrowserRuntime> {
        Arc::new(CommandBrowserRuntime)
    }

    fn wallpaper_runtime(&self) -> Arc<dyn WallpaperRuntime> {
        Arc::new(UnsupportedWallpaperRuntime)
    }

    fn app_runtime(&self) -> Arc<dyn AppRuntime> {
        Arc::new(UnsupportedAppRuntime)
    }

    fn screen_command(
        &self,
        action: ScreenAction,
        screen: ScreenId,
        extra: &[String],
    ) -> Vec<String>;

    /// Bring the whole box up once and exit, for a place with no entrypoint.
    fn boot_command(&self) -> Vec<String>;

    fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String>;

    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String>;

    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)>;

    fn viewer_url(&self, at: &crate::Address, ticket: Option<&crate::Secret>) -> String {
        let mut url = format!(
            "{}://{}/vnc.html?autoconnect=1&resize=scale",
            at.scheme.as_str(),
            at.authority()
        );
        if let Some(ticket) = ticket {
            url.push_str(&viewer_path(ticket));
        }
        url
    }

    fn start_command(&self, screen: ScreenId) -> Vec<String> {
        self.screen_command(ScreenAction::Start, screen, &[])
    }

    fn stop_command(&self, screen: ScreenId) -> Vec<String> {
        self.screen_command(ScreenAction::Stop, screen, &[])
    }

    fn viewers_command(&self, screen: ScreenId) -> Vec<String> {
        self.screen_command(ScreenAction::Viewers, screen, &[])
    }

    fn control_command(&self, screen: ScreenId, token: &str, shared: bool) -> Vec<String> {
        let mut extra = vec![token.to_string()];
        if shared {
            extra.push(SHARED.to_string());
        }
        self.screen_command(ScreenAction::Control, screen, &extra)
    }

    fn release_command(&self, screen: ScreenId, token: &str) -> Vec<String> {
        self.screen_command(ScreenAction::Release, screen, &[token.to_string()])
    }

    fn reclaim_command(&self, screen: ScreenId) -> Vec<String> {
        self.release_command(screen, FORCE)
    }

    fn open_command(&self, screen: ScreenId, url: &str) -> Vec<String> {
        self.screen_command(ScreenAction::Open, screen, &[url.to_string()])
    }

    fn record_command(&self, screen: ScreenId, what: Recording, fps: Option<u32>) -> Vec<String> {
        let mut extra = vec![what.verb().to_string()];
        if let Some(fps) = fps {
            extra.push(fps.to_string());
        }
        self.screen_command(ScreenAction::Record, screen, &extra)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recording {
    Start,
    Stop,
    Status,
}

impl Recording {
    fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Status => "status",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{WaylandProfile, X11Profile};

    #[test]
    fn test_the_two_built_in_images_are_never_one_tag() {
        assert_ne!(
            X11Profile.image(),
            WaylandProfile.image(),
            "one tag for two images hands a caller whichever was built first, \
             and every command after that goes to the wrong display server"
        );
        assert_ne!(
            X11Profile.image().tag(&Extras::none()).ok(),
            WaylandProfile.image().tag(&Extras::none()).ok()
        );
    }

    #[test]
    fn test_a_recording_is_asked_for_by_verb() {
        assert_eq!(
            X11Profile.record_command(ScreenId(0), Recording::Start, Some(15)),
            vec!["computer-screen", "record", "0", "start", "15"]
        );
        assert_eq!(
            X11Profile.record_command(ScreenId(2), Recording::Stop, None),
            vec!["computer-screen", "record", "2", "stop"],
            "a rate belongs to the recording that is starting, not the one ending"
        );
        assert_eq!(
            X11Profile.record_command(ScreenId(0), Recording::Status, None),
            vec!["computer-screen", "record", "0", "status"]
        );
    }

    #[test]
    fn test_both_images_are_reached_the_same_way() {
        assert_eq!(X11Profile.ports(), WaylandProfile.ports());
        assert_eq!(
            X11Profile.start_command(ScreenId(3)),
            WaylandProfile.start_command(ScreenId(3))
        );
        assert_eq!(
            X11Profile.launch_env(1920, 1080),
            WaylandProfile.launch_env(1920, 1080)
        );
    }

    #[test]
    fn test_a_screen_is_reached_through_whatever_its_server_uses() {
        let x11 = X11Profile.screen_env(ScreenId(1));
        let wayland = WaylandProfile.screen_env(ScreenId(1));

        assert_eq!(x11.get("DISPLAY").map(String::as_str), Some(":2"));
        assert_eq!(
            wayland.get("WAYLAND_DISPLAY").map(String::as_str),
            Some("wayland-1"),
            "the socket name is per directory, so it is the directory that \
             changes per screen"
        );
        assert_eq!(
            wayland.get("XDG_RUNTIME_DIR").map(String::as_str),
            Some("/tmp/computer/run-2")
        );
        assert_eq!(
            wayland.get("DISPLAY").map(String::as_str),
            Some(":0"),
            "for an X11 program under Xwayland, which sway puts on :0. The \
             driver's own commands read the socket above and not this, so it \
             steers nothing that belongs to the compositor"
        );
        assert!(
            wayland.contains_key("XDG_RUNTIME_DIR"),
            "a Wayland socket lives in one, and two compositors sharing a \
             directory each claim wayland-1"
        );
    }

    #[test]
    fn test_packages_are_refused_against_an_image_nothing_here_builds() {
        let wanted = Extras::with(["fonts-noto-cjk"]);

        assert_eq!(
            ImageSource::Bundled(&crate::bundle::DESKTOP)
                .tag(&wanted)
                .ok(),
            Some(crate::bundle::DESKTOP.tag_with(&wanted))
        );
        assert!(
            ImageSource::Registry("mine:1".to_string())
                .tag(&wanted)
                .is_err(),
            "there is no build to install them in, and handing back the plain \
             image hides that until the box is running"
        );
        assert_eq!(
            ImageSource::Registry("mine:1".to_string())
                .tag(&Extras::none())
                .ok(),
            Some("mine:1".to_string())
        );
    }

    #[test]
    fn test_a_registry_image_is_never_the_bundled_one_under_another_name() {
        let theirs = ImageSource::Registry("computer-desktop:mine".to_string());

        assert!(
            theirs.bundle().is_none(),
            "deciding by the shape of a tag builds our image under their name"
        );
    }
}
