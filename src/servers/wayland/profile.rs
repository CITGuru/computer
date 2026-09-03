//! What the Wayland image offers, and how to talk to it.

use super::WaylandDriver;
use crate::bundle;
use crate::desktop::DesktopFactory;
use crate::image;
use crate::profile::{
    DesktopContract, ImageSource, PortLayout, Profile, ScreenEnvironment, WallpaperRuntime,
    WaylandEnvironment, WaylandWallpaperRuntime,
};
use crate::{DesktopSupport, Display, DisplayServer, ScreenAction, ScreenId};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

/// The Wayland image this crate carries: sway headless, chromium and wayvnc.
///
/// Its ports, verbs and screen numbering are the X11 image's, so swapping
/// the image changes what is in the box and not how it is reached.
#[derive(Debug, Clone, Copy, Default)]
pub struct WaylandProfile;

static CONTRACT: LazyLock<DesktopContract> = LazyLock::new(|| {
    DesktopContract::standard(
        bundle::WAYLAND_IMAGE_NAME,
        ImageSource::Bundled(&bundle::WAYLAND),
    )
});

impl WaylandProfile {
    pub fn contract() -> &'static DesktopContract {
        LazyLock::force(&CONTRACT)
    }
}

impl Profile for WaylandProfile {
    fn name(&self) -> &str {
        Self::contract().name()
    }

    fn image(&self) -> ImageSource {
        Self::contract().image()
    }

    fn ports(&self) -> PortLayout {
        Self::contract().ports()
    }

    fn default_size(&self) -> (u32, u32) {
        Self::contract().default_size()
    }

    fn support_at(&self, width: u32, height: u32) -> DesktopSupport {
        DesktopSupport {
            display: Some(Display {
                width,
                height,
                server: DisplayServer::Wayland,
            }),
            ..image::support_at(width, height)
        }
    }

    fn driver(&self) -> Arc<dyn DesktopFactory> {
        Arc::new(WaylandDriver)
    }

    fn wallpaper_runtime(&self) -> Arc<dyn WallpaperRuntime> {
        Arc::new(WaylandWallpaperRuntime)
    }

    fn screen_command(
        &self,
        action: ScreenAction,
        screen: ScreenId,
        extra: &[String],
    ) -> Vec<String> {
        Self::contract().screen_command(action, screen, extra)
    }

    fn boot_command(&self) -> Vec<String> {
        Self::contract().boot_command()
    }

    fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String> {
        Self::contract().launch_env(width, height)
    }

    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        Self::contract().geometry_from(environment)
    }

    /// A compositor is reached through its socket and the directory that
    /// holds it, where an X server is reached through a display number.
    ///
    /// The directory changes per screen and the socket name does not. A
    /// Wayland socket is a file, so its name is only unique within one
    /// directory; a screen number carried into the name is an X11 habit that
    /// points every screen after the first at nothing.
    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String> {
        WaylandEnvironment.environment(screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DisplayServer;
    #[test]
    fn test_the_wayland_profile_reports_the_server_it_runs() {
        assert_eq!(
            WaylandProfile
                .support_at(1280, 800)
                .display
                .map(|d| d.server),
            Some(DisplayServer::Wayland)
        );
        assert_eq!(
            WaylandProfile.driver().display_server(),
            DisplayServer::Wayland,
            "a profile that named the other driver would be a box whose \
             commands go in and move nothing"
        );
    }
}
