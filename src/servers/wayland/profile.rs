//! What the Wayland image offers, and how to talk to it.

use super::{DISPLAY_NAME, WaylandDriver, runtime_dir};
use crate::bundle;
use crate::desktop::DesktopFactory;
use crate::profile::{ImageSource, PortLayout, Profile};
use crate::{DesktopSupport, Display, DisplayServer, ScreenAction, ScreenId};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The Wayland image this crate carries: sway headless, chromium and wayvnc.
///
/// Its ports, verbs and screen numbering are the X11 image's, so swapping
/// the image changes what is in the box and not how it is reached.
#[derive(Debug, Clone, Copy, Default)]
pub struct WaylandProfile;

impl Profile for WaylandProfile {
    fn name(&self) -> &str {
        bundle::WAYLAND_IMAGE_NAME
    }

    fn image(&self) -> ImageSource {
        ImageSource::Bundled(&bundle::WAYLAND)
    }

    fn ports(&self) -> PortLayout {
        crate::X11Profile.ports()
    }

    fn default_size(&self) -> (u32, u32) {
        crate::X11Profile.default_size()
    }

    fn support_at(&self, width: u32, height: u32) -> DesktopSupport {
        DesktopSupport {
            display: Some(Display {
                width,
                height,
                server: DisplayServer::Wayland,
            }),
            ..crate::X11Profile.support_at(width, height)
        }
    }

    fn driver(&self) -> Arc<dyn DesktopFactory> {
        Arc::new(WaylandDriver)
    }

    fn screen_command(
        &self,
        action: ScreenAction,
        screen: ScreenId,
        extra: &[String],
    ) -> Vec<String> {
        crate::X11Profile.screen_command(action, screen, extra)
    }

    fn boot_command(&self) -> Vec<String> {
        crate::X11Profile.boot_command()
    }

    fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String> {
        crate::X11Profile.launch_env(width, height)
    }

    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        crate::X11Profile.geometry_from(environment)
    }

    /// A compositor is reached through its socket and the directory that
    /// holds it, where an X server is reached through a display number.
    ///
    /// The directory changes per screen and the socket name does not. A
    /// Wayland socket is a file, so its name is only unique within one
    /// directory; a screen number carried into the name is an X11 habit that
    /// points every screen after the first at nothing.
    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("WAYLAND_DISPLAY".to_string(), DISPLAY_NAME.to_string()),
            ("XDG_RUNTIME_DIR".to_string(), runtime_dir(screen)),
        ])
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
