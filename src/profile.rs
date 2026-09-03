//! What a box image offers, and how to talk to it.
//!
//! Every port, command name and environment variable an image uses is a
//! decision that image made. A [`Profile`] holds those decisions, so a second
//! image is a second profile rather than an edit to the code that drives one.

pub mod primitives;

use crate::bundle::{Bundle, Extras};
use crate::desktop::DesktopFactory;
use crate::error::{Error, Result};
use crate::{DesktopSupport, ScreenAction, ScreenId, ScreenPorts};
use std::collections::BTreeMap;
use std::sync::Arc;

pub use primitives::{
    BrowserRuntime, CommandBrowserRuntime, CommandScreen, CommandScreenRuntime,
    CommandWallpaperRuntime, ConfiguredProfile, DesktopContract, GeometrySpec, ProfileBuilder,
    ScreenCommands, ScreenEnvironment, ScreenRuntime, UnsupportedWallpaperRuntime,
    WallpaperRuntime, WaylandEnvironment, WaylandWallpaperRuntime, X11Environment,
    X11WallpaperRuntime,
};

/// Where an image declares which contract it implements.
///
/// A label, so it can be read before the image is started.
pub const PROFILE_LABEL: &str = "computer.profile";

/// Where a box records which contract it speaks.
///
/// On the box, so a process attaching later can read it.
pub const PROFILE_ENV: &str = "COMPUTER_PROFILE";

/// The websocket path a ticketed viewer connects on.
///
/// The token rides in the socket path's own query rather than the page's: the
/// page is static and the credential is read where the socket opens. The inner
/// `?` and `=` are percent-encoded, because both this crate's `embed.html` and
/// noVNC's `vnc.html` put the value through `decodeURIComponent` before using
/// it as a path.
pub fn viewer_path(ticket: &crate::Secret) -> String {
    format!("&path=websockify%3Ftoken%3D{}", ticket.expose())
}

/// The profile a box says it speaks, where this crate ships one by that name.
///
/// `None` for one a caller wrote: see [`crate::Computer::attach_using`].
pub fn builtin(name: &str) -> Option<Arc<dyn Profile>> {
    match name {
        _ if name == crate::X11Profile.name() => Some(Arc::new(crate::X11Profile)),
        _ if name == crate::WaylandProfile.name() => Some(Arc::new(crate::WaylandProfile)),
        _ => None,
    }
}

/// The word that ends a takeover whoever started it.
///
/// A box refuses a release carrying the wrong token. This is the way past
/// that, for a caller that has decided the person is finished.
pub const FORCE: &str = "--force";

/// A shared session, where the owner keeps driving beside the person.
pub const SHARED: &str = "shared";

/// Where an image comes from.
///
/// Asked rather than guessed from the tag, so a caller who names their own
/// image `computer-desktop:mine` is not handed ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// Built from a set of bytes this crate carries, under a tag that follows
    /// them.
    ///
    /// The only kind that takes extra packages: they are installed by the build.
    Bundled(&'static Bundle),
    /// Fetched under this name, and never overwritten by ours.
    Registry(String),
}

impl ImageSource {
    /// The tag this resolves to, with these packages asked for.
    ///
    /// Refuses where packages were asked of an image nothing here builds.
    pub fn tag(&self, extras: &Extras) -> Result<String> {
        match self {
            Self::Bundled(bundle) => Ok(bundle.tag_with(extras)),
            Self::Registry(name) if extras.is_empty() => Ok(name.clone()),
            Self::Registry(_) => Err(Error::Unsupported {
                gaps: vec!["packages in an image this crate does not build"],
            }),
        }
    }

    /// The bytes to build, where there are any.
    pub fn bundle(&self) -> Option<&'static Bundle> {
        match self {
            Self::Bundled(bundle) => Some(bundle),
            Self::Registry(_) => None,
        }
    }
}

/// Where an image's screens listen.
///
/// A value rather than three methods, so the arithmetic that turns bases
/// into one screen's ports stays in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortLayout {
    /// The first websocket viewer port. Screens take two each — a read-only
    /// one and a control one.
    pub view_base: u16,
    /// The first VNC port, behind the websocket bridge.
    pub vnc_base: u16,
    /// Where the browser listens for DevTools inside the box.
    ///
    /// `None` where the image has no browser to debug.
    pub devtools: Option<u16>,
    /// The bridge in front of it, which is the one worth publishing.
    ///
    /// Chromium binds its debugging port to loopback whatever it is told, so a
    /// host port forwarded onto that one reaches nothing.
    pub devtools_bridge: Option<u16>,
    pub max_screens: u32,
}

impl PortLayout {
    /// The ports for one screen, or a refusal.
    ///
    /// A screen past the limit is refused rather than computed onto a port
    /// something else holds.
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

    /// Every viewer port the image serves, which is what its `EXPOSE` line
    /// must carry.
    pub fn viewer_ports(&self) -> Vec<u16> {
        let mut ports: Vec<u16> = (0..self.max_screens)
            .filter_map(|screen| self.screen(ScreenId(screen)).ok())
            .flat_map(|slot| [slot.view, slot.control])
            .collect();
        ports.sort_unstable();
        ports
    }

    /// Everything the runtime is asked to publish when the box starts.
    pub fn to_publish(&self) -> Vec<u16> {
        let mut ports = self.viewer_ports();
        ports.extend(self.devtools_bridge);
        ports
    }
}

/// One image's contract: its ports, its commands, and what it claims.
///
/// Held for the life of a box: a screen started on demand needs the same
/// answers the first one got.
pub trait Profile: Send + Sync {
    /// What to call this image in a message.
    fn name(&self) -> &str;

    fn image(&self) -> ImageSource;

    fn ports(&self) -> PortLayout;

    /// The size the image comes up at when nothing says otherwise.
    ///
    /// The fallback for a box whose environment carries no size.
    fn default_size(&self) -> (u32, u32);

    /// What the image can show, at the size it was started.
    fn support_at(&self, width: u32, height: u32) -> DesktopSupport;

    /// The driver this image expects.
    ///
    /// A default rather than a rule: [`crate::Builder::driver`] overrides it.
    fn driver(&self) -> Arc<dyn DesktopFactory>;

    /// How screen lifecycle operations are performed.
    ///
    /// The default keeps the command protocol implemented by existing images.
    fn screen_runtime(&self) -> Arc<dyn ScreenRuntime> {
        Arc::new(CommandScreenRuntime)
    }

    /// How browser operations are performed.
    ///
    /// The default keeps the command protocol implemented by existing images.
    fn browser_runtime(&self) -> Arc<dyn BrowserRuntime> {
        Arc::new(CommandBrowserRuntime)
    }

    /// How a running screen's wallpaper is changed.
    fn wallpaper_runtime(&self) -> Arc<dyn WallpaperRuntime> {
        Arc::new(UnsupportedWallpaperRuntime)
    }

    /// Perform `action` on one screen, with whatever extra words it takes.
    ///
    /// One command method, so an image with a script of the same shape
    /// overrides nothing else.
    fn screen_command(
        &self,
        action: ScreenAction,
        screen: ScreenId,
        extra: &[String],
    ) -> Vec<String>;

    /// Bring the whole box up once and exit, for a place with no entrypoint.
    fn boot_command(&self) -> Vec<String>;

    /// The environment the box is started with.
    fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String>;

    /// The environment one screen's commands run with.
    ///
    /// `DISPLAY=:N+1` for X11; a socket and a runtime directory for Wayland.
    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String>;

    /// The geometry read back out of it.
    ///
    /// The other half of [`Profile::launch_env`]: a box picked up by another
    /// process has nothing else to say what size it came up at.
    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)>;

    /// Where a person watches a screen.
    ///
    /// Given the whole address rather than a port: the host a box is reached at
    /// is not something this crate can derive once the box is off loopback, so
    /// it is carried in from whoever does know.
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

    /// Start a screen's whole stack.
    fn start_command(&self, screen: ScreenId) -> Vec<String> {
        self.screen_command(ScreenAction::Start, screen, &[])
    }

    /// Take it away again.
    fn stop_command(&self, screen: ScreenId) -> Vec<String> {
        self.screen_command(ScreenAction::Stop, screen, &[])
    }

    /// Count who is actually connected.
    fn viewers_command(&self, screen: ScreenId) -> Vec<String> {
        self.screen_command(ScreenAction::Viewers, screen, &[])
    }

    /// Open the input-accepting viewer, and record who opened it.
    ///
    /// `shared` decides whether the box records a token at all, since the token
    /// file is what the image's input guard refuses on.
    fn control_command(&self, screen: ScreenId, token: &str, shared: bool) -> Vec<String> {
        let mut extra = vec![token.to_string()];
        if shared {
            extra.push(SHARED.to_string());
        }
        self.screen_command(ScreenAction::Control, screen, &extra)
    }

    /// Close it again, proving this is the takeover that is running.
    fn release_command(&self, screen: ScreenId, token: &str) -> Vec<String> {
        self.screen_command(ScreenAction::Release, screen, &[token.to_string()])
    }

    /// End a takeover the caller did not start.
    fn reclaim_command(&self, screen: ScreenId) -> Vec<String> {
        self.release_command(screen, FORCE)
    }

    /// Open a URL in that screen's browser.
    fn open_command(&self, screen: ScreenId, url: &str) -> Vec<String> {
        self.screen_command(ScreenAction::Open, screen, &[url.to_string()])
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
    fn test_both_images_are_reached_the_same_way() {
        // Ports, verbs and geometry are this crate's convention rather than
        // X11's, so swapping the image swaps what is in the box and nothing
        // about how it is reached.
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
        assert!(
            !wayland.contains_key("DISPLAY"),
            "an X11 variable against a compositor is a command that goes in \
             and moves nothing"
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
