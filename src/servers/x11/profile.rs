//! What the X11 image offers, and how to talk to it.

use super::X11Driver;
use crate::desktop::DesktopFactory;
use crate::image;
use crate::profile::{
    DesktopContract, ImageSource, PortLayout, Profile, ScreenEnvironment, WallpaperRuntime,
    X11Environment, X11WallpaperRuntime,
};
use crate::{DesktopSupport, ScreenAction, ScreenId};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

/// The image this crate carries: Xvfb, fluxbox, chromium and a noVNC viewer.
///
/// The default every box gets. Its numbers are [`crate::image`]'s constants.
#[derive(Debug, Clone, Copy, Default)]
pub struct X11Profile;

static CONTRACT: LazyLock<DesktopContract> = LazyLock::new(|| {
    DesktopContract::standard(
        image::PROFILE_NAME,
        ImageSource::Bundled(&crate::bundle::DESKTOP),
    )
});

impl X11Profile {
    pub fn contract() -> &'static DesktopContract {
        LazyLock::force(&CONTRACT)
    }
}

impl Profile for X11Profile {
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
        image::support_at(width, height)
    }

    fn driver(&self) -> Arc<dyn DesktopFactory> {
        Arc::new(X11Driver)
    }

    fn wallpaper_runtime(&self) -> Arc<dyn WallpaperRuntime> {
        Arc::new(X11WallpaperRuntime)
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

    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String> {
        X11Environment.environment(screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::error::Error;
    use crate::profile::{FORCE, SHARED};
    use crate::{DisplayServer, ScreenId};
    #[test]
    fn test_the_built_in_profile_is_the_image_this_crate_carries() {
        let profile = X11Profile;

        assert_eq!(
            profile.image(),
            ImageSource::Bundled(&crate::bundle::DESKTOP)
        );
        assert_eq!(
            profile.support_at(1280, 800).display.map(|d| d.server),
            Some(DisplayServer::X11)
        );
        assert_eq!(profile.driver().display_server(), DisplayServer::X11);
    }

    #[test]
    fn test_each_screen_takes_two_ports_from_the_base() {
        let layout = X11Profile.ports();
        let first = layout.screen(ScreenId(0)).expect("screen 0");
        let second = layout.screen(ScreenId(1)).expect("screen 1");

        assert_eq!(
            first.display_number, 1,
            ":0 is a real console on a real host"
        );
        assert_eq!((first.view, first.control), (6080, 6081));
        assert_eq!((second.view, second.control), (6082, 6083));
    }

    #[test]
    fn test_no_two_screens_share_a_port() {
        let ports = X11Profile.ports().viewer_ports();
        let mut unique = ports.clone();
        unique.dedup();

        assert_eq!(ports, unique);
        assert_eq!(ports.len(), (image::MAX_SCREENS * 2) as usize);
    }

    #[test]
    fn test_a_screen_past_the_limit_is_refused_rather_than_computed() {
        let layout = X11Profile.ports();
        let error = layout
            .screen(ScreenId(layout.max_screens))
            .expect_err("beyond the limit");

        assert!(matches!(error, Error::ScreenUnavailable { .. }));
    }

    #[test]
    fn test_the_devtools_bridge_is_published_and_chromiums_own_port_is_not() {
        let layout = X11Profile.ports();
        let publish = layout.to_publish();

        assert!(publish.contains(&image::DEVTOOLS_BRIDGE_PORT));
        assert!(
            !publish.contains(&image::DEVTOOLS_PORT),
            "chromium holds that one on loopback inside the box, so a host \
             port forwarded to it answers with nothing"
        );
    }

    #[test]
    fn test_the_command_helpers_all_build_on_the_one_method() {
        let profile = X11Profile;

        assert_eq!(
            profile.start_command(ScreenId(3)),
            vec!["computer-screen", "start", "3"],
            "the script does the +1, so only one place knows the offset"
        );
        assert_eq!(
            profile.control_command(ScreenId(0), "token-1", false),
            vec!["computer-screen", "control", "0", "token-1"]
        );
        assert_eq!(
            profile.control_command(ScreenId(0), "token-1", true).last(),
            Some(&SHARED.to_string()),
            "a shared session records no token, or the guard locks out the \
             owner it was sharing with"
        );
        assert_eq!(
            profile.release_command(ScreenId(0), FORCE),
            vec!["computer-screen", "release", "0", "--force"]
        );
        assert_eq!(
            profile.open_command(ScreenId(1), "https://example.com"),
            vec!["computer-screen", "open", "1", "https://example.com"]
        );
    }

    #[test]
    fn test_the_size_written_at_launch_is_the_size_read_back() {
        let profile = X11Profile;
        let environment = profile.launch_env(1920, 1080);

        assert_eq!(
            profile.geometry_from(&environment),
            Some((1920, 1080)),
            "a box picked up by another process has nothing but its \
             environment to say what size it came up at"
        );
    }

    #[test]
    fn test_screen_zero_runs_against_display_one() {
        let environment = X11Profile.screen_env(ScreenId(0));

        assert_eq!(environment.get("DISPLAY").map(String::as_str), Some(":1"));
        assert_eq!(
            X11Profile.screen_env(ScreenId(7)).get("DISPLAY"),
            Some(&":8".to_string()),
            ":0 is a real console on a real host, and never handed out"
        );
    }

    #[test]
    fn test_a_half_written_environment_is_none_rather_than_a_default() {
        let environment = BTreeMap::from([(image::WIDTH_ENV.to_string(), "1920".to_string())]);

        assert_eq!(
            X11Profile.geometry_from(&environment),
            None,
            "a guessed height is a coordinate space the caller never chose"
        );
    }
}
