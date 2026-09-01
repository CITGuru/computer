//! What a macOS box offers, and how to talk to it.

use super::QuartzDriver;
use crate::desktop::DesktopFactory;
use crate::image;
use crate::profile::{ImageSource, PortLayout, Profile};
use crate::{
    Browser, Control, DesktopSupport, Display, DisplayServer, ScreenAction, ScreenId, Viewer,
    ViewerKind,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What this crate calls the macOS contract.
pub const PROFILE_NAME: &str = "computer-macos";

/// The prepared guest a box is cloned from when the caller names none.
pub const DEFAULT_IMAGE: &str = "ghcr.io/cirruslabs/macos-sequoia-base:latest";

/// The browser a box drives.
pub const BROWSER_APP: &str = "Google Chrome";

/// A macOS desktop, in a guest on Apple hardware.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacProfile;

/// A command that fails saying what a macOS box does not have.
fn refuse(gap: &str) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("echo 'a macOS box has no {gap}' >&2; exit 1"),
    ]
}

impl Profile for MacProfile {
    fn name(&self) -> &str {
        PROFILE_NAME
    }

    fn image(&self) -> ImageSource {
        ImageSource::Registry(DEFAULT_IMAGE.to_string())
    }

    /// One screen, and otherwise the numbers every other box uses.
    fn ports(&self) -> PortLayout {
        PortLayout {
            view_base: image::VIEW_PORT_BASE,
            vnc_base: image::VNC_PORT_BASE,
            devtools: Some(image::DEVTOOLS_PORT),
            devtools_bridge: Some(image::DEVTOOLS_BRIDGE_PORT),
            max_screens: 1,
        }
    }

    /// The browser's bridge, and nothing else that lives in the guest.
    fn publish(&self) -> Vec<u16> {
        self.ports().devtools_bridge.into_iter().collect()
    }

    fn default_size(&self) -> (u32, u32) {
        (image::WIDTH, image::HEIGHT)
    }

    fn support_at(&self, width: u32, height: u32) -> DesktopSupport {
        DesktopSupport {
            display: Some(Display {
                width,
                height,
                server: DisplayServer::Quartz,
            }),
            input: true,
            browser: Some(Browser {
                name: BROWSER_APP.to_string(),
                version: None,
                cdp: true,
                headed: true,
            }),
            clipboard: true,
            viewer: Some(Viewer {
                kind: ViewerKind::Vnc,
                // No takeover: Tart's server has one mode and it accepts
                // input, so the only safe way to publish it is through a
                // filter that drops input — and a filtered port cannot then
                // be handed to anybody to drive.
                takeover: false,
                control: Control::Owner,
            }),
            max_screens: 1,
        }
    }

    fn driver(&self) -> Arc<dyn DesktopFactory> {
        Arc::new(QuartzDriver)
    }

    fn screen_command(
        &self,
        action: ScreenAction,
        _screen: ScreenId,
        extra: &[String],
    ) -> Vec<String> {
        match action {
            // The session is logged in before the box answers at all, so the
            // one screen is already up by the time anything can ask for it.
            ScreenAction::Start => vec!["true".to_string()],
            ScreenAction::Open => {
                let mut args = vec![
                    "open".to_string(),
                    "-a".to_string(),
                    BROWSER_APP.to_string(),
                ];
                args.extend_from_slice(extra);
                args
            }
            ScreenAction::Stop => refuse("second screen to stop, only the session it boots into"),
            ScreenAction::Viewers => refuse("viewer inside it to count"),
            ScreenAction::Control | ScreenAction::Release => {
                refuse("input gate: its viewer is a host-side server with no view-only mode")
            }
        }
    }

    /// Nothing: the guest boots into its own session.
    fn boot_command(&self) -> Vec<String> {
        Vec::new()
    }

    /// The size, recorded where a process attaching later can read it.
    fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String> {
        crate::X11Profile.launch_env(width, height)
    }

    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        crate::X11Profile.geometry_from(environment)
    }

    /// Nothing. A macOS screen is not selected by a variable.
    fn screen_env(&self, _screen: ScreenId) -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    /// A VNC address, not a noVNC page.
    fn viewer_url(&self, at: &crate::Address, _ticket: Option<&crate::Secret>) -> String {
        format!("vnc://{}", at.authority())
    }

    /// `nc`, not `/dev/tcp`.
    fn port_probe_command(&self, port: u16) -> Vec<String> {
        vec![
            "nc".to_string(),
            "-z".to_string(),
            "127.0.0.1".to_string(),
            port.to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::FORCE;

    #[test]
    fn test_the_profile_names_the_driver_that_can_actually_move_it() {
        assert_eq!(
            MacProfile.support_at(1280, 800).display.map(|d| d.server),
            Some(DisplayServer::Quartz)
        );
        assert_eq!(
            MacProfile.driver().display_server(),
            DisplayServer::Quartz,
            "a profile naming another driver is a box whose commands go in \
             and move nothing"
        );
    }

    #[test]
    fn test_a_macos_box_has_one_screen_and_says_so_before_it_starts() {
        let layout = MacProfile.ports();

        assert_eq!(layout.max_screens, 1);
        assert!(
            layout.screen(ScreenId(1)).is_err(),
            "macOS has one GUI session per boot, so a second screen is not a \
             port away"
        );
        assert_eq!(MacProfile.support_at(1280, 800).max_screens, 1);
    }

    #[test]
    fn test_no_guest_port_is_carried_for_a_viewer_that_is_not_in_the_guest() {
        let published = MacProfile.publish();

        assert_eq!(
            published,
            vec![image::DEVTOOLS_BRIDGE_PORT],
            "the viewer is a host-side server, so there is no port inside the \
             box to forward: the machine records it directly"
        );
    }

    #[test]
    fn test_the_machine_records_the_viewer_where_this_profile_looks_for_it() {
        assert_eq!(
            crate::mac::VIEWER_PORT,
            MacProfile.ports().view_base,
            "the machine keys the viewer under one number and Screen reads it \
             back under another: a mismatch is a URL that is always None"
        );
    }

    #[test]
    fn test_a_person_is_given_an_address_a_vnc_client_understands() {
        let at = crate::Address {
            scheme: crate::Scheme::Http,
            host: "127.0.0.1".to_string(),
            port: 6080,
        };

        assert_eq!(
            MacProfile.viewer_url(&at, None),
            "vnc://127.0.0.1:6080",
            "there is no noVNC page in this box to serve /vnc.html"
        );
    }

    #[test]
    fn test_a_viewer_that_cannot_be_driven_does_not_claim_it_can() {
        let viewer = MacProfile.support_at(1280, 800).viewer.expect("a viewer");

        assert!(
            !viewer.takeover,
            "the only publishable form of Tart's server is one with the input \
             filtered out, and a filtered port cannot be handed to anybody"
        );
    }

    #[test]
    fn test_what_the_platform_lacks_refuses_instead_of_succeeding_quietly() {
        for command in [
            MacProfile.viewers_command(ScreenId(0)),
            MacProfile.stop_command(ScreenId(0)),
            MacProfile.control_command(ScreenId(0), "token", false),
            MacProfile.release_command(ScreenId(0), FORCE),
        ] {
            assert!(
                command.last().is_some_and(|part| part.contains("exit 1")),
                "a command that quietly succeeded would report a screen \
                 stopped, or nobody watching, when neither was asked: \
                 {command:?}"
            );
        }
    }

    #[test]
    fn test_a_url_is_opened_in_the_browser_that_has_devtools() {
        let command = MacProfile.open_command(ScreenId(0), "https://example.com");

        assert_eq!(
            command,
            ["open", "-a", BROWSER_APP, "https://example.com"],
            "the default browser is whatever the image was left with, and the \
             box promises a DevTools endpoint Safari does not have"
        );
    }

    #[test]
    fn test_a_screen_is_not_selected_by_a_variable() {
        assert!(
            MacProfile.screen_env(ScreenId(0)).is_empty(),
            "there is one session and the agent already lands in it"
        );
    }

    #[test]
    fn test_the_size_written_at_launch_is_the_size_read_back() {
        let environment = MacProfile.launch_env(1920, 1080);
        assert_eq!(MacProfile.geometry_from(&environment), Some((1920, 1080)));
    }

    #[test]
    fn test_a_port_is_probed_with_something_the_base_system_ships() {
        let probe = MacProfile.port_probe_command(9222);

        assert_eq!(probe[0], "nc");
        assert!(probe.contains(&"9222".to_string()));
        assert_ne!(
            probe,
            crate::X11Profile.port_probe_command(9222),
            "macOS keeps bash 3.2 for a licence rather than for use"
        );
    }

    #[test]
    fn test_nothing_is_ever_built_for_a_macos_box() {
        assert!(
            matches!(MacProfile.image(), ImageSource::Registry(_)),
            "a macOS guest is installed once out of band and pushed; there is \
             no building one on first use"
        );
        assert!(MacProfile.boot_command().is_empty());
    }
}
