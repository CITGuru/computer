//! The desktop image this crate carries, and what it claims.
//!
//! The constants here describe `images/desktop/`: its resolution, its screen
//! limit, the ports each screen uses, and the port Chromium debugs on. A
//! caller compares them against what it needs before opening anything.
//!
//! `tests/image.rs` reads the image as text and fails when the two disagree.

use crate::{Browser, Control, DesktopSupport, Display, DisplayServer, Viewer, ViewerKind};

pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 800;

/// Eight stacks of X server, window manager, browser and viewer is roughly a
/// core and 2 GB. Past that a screen starves rather than queues, which is a
/// worse failure than being refused a ninth.
pub const MAX_SCREENS: u32 = 8;

/// The first viewer port. Screens take two each — see [`ScreenPorts`].
pub const VIEW_PORT_BASE: u16 = 6080;
/// The first VNC port, behind the websocket bridge.
pub const VNC_PORT_BASE: u16 = 5900;

/// Where chromium listens for DevTools, inside the box.
///
/// Loopback only, whatever `--remote-debugging-address` says. Probe this
/// port from inside; publish [`DEVTOOLS_BRIDGE_PORT`].
pub const DEVTOOLS_PORT: u16 = 9222;

/// The bridge in front of it, which is what a client out here connects to.
///
/// A host port forwarded onto [`DEVTOOLS_PORT`] reaches nothing and answers
/// with an empty reply.
pub const DEVTOOLS_BRIDGE_PORT: u16 = 9223;

/// What this image is called in a message.
pub const PROFILE_NAME: &str = "computer-desktop";

/// The command names the image installs on the path.
pub const DESKTOP_COMMAND: &str = "computer-desktop";
pub const SCREEN_COMMAND: &str = "computer-screen";
pub const BROWSER_COMMAND: &str = "computer-browser";

/// The environment variables the image reads for its geometry.
pub const WIDTH_ENV: &str = "COMPUTER_SCREEN_WIDTH";
pub const HEIGHT_ENV: &str = "COMPUTER_SCREEN_HEIGHT";

/// One screen's ports, as [`crate::PortLayout`] computes them.
///
/// The view-only and control servers are separate endpoints, so a takeover
/// is started on request rather than switched on a live connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenPorts {
    /// `:1` for screen 0. Never `:0` — that is a real console on a real host.
    pub display_number: u32,
    pub view: u16,
    pub control: u16,
    pub view_vnc: u16,
    pub control_vnc: u16,
}

impl ScreenPorts {
    pub fn display(&self) -> String {
        format!(":{}", self.display_number)
    }
}

/// What this image can show.
///
/// A constant, because the image is ours.
pub fn support() -> DesktopSupport {
    DesktopSupport {
        display: Some(Display {
            width: WIDTH,
            height: HEIGHT,
            server: DisplayServer::X11,
        }),
        input: true,
        browser: Some(Browser {
            name: "chromium".to_string(),
            version: None,
            cdp: true,
            headed: true,
        }),
        clipboard: true,
        viewer: Some(Viewer {
            kind: ViewerKind::Vnc,
            takeover: true,
            control: Control::Owner,
        }),
        max_screens: MAX_SCREENS,
    }
}

/// The same claim, at a resolution the caller chose.
///
/// The image takes its geometry from the environment, so a box started with
/// different values supports a different size.
pub fn support_at(width: u32, height: u32) -> DesktopSupport {
    DesktopSupport {
        display: Some(Display {
            width,
            height,
            server: DisplayServer::X11,
        }),
        ..support()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenAction {
    Start,
    Stop,
    /// Open the input-accepting viewer, so a person can take the screen.
    Control,
    /// Close it again. The read-only viewer is untouched.
    Release,
    /// Count who is actually connected.
    Viewers,
    /// Point that screen's browser at a URL. Takes one the others do not.
    Open,
}

impl ScreenAction {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Control => "control",
            Self::Release => "release",
            Self::Viewers => "viewers",
            Self::Open => "open",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DesktopNeed;

    #[test]
    fn test_the_image_meets_a_desktop_need_and_a_browser_need() {
        assert!(DesktopNeed::desktop().unsupported_by(&support()).is_empty());
        assert!(DesktopNeed::browser().unsupported_by(&support()).is_empty());
    }

    #[test]
    fn test_the_image_refuses_a_screen_larger_than_it_has() {
        let need = DesktopNeed::desktop().at_least(1920, 1080);
        assert_eq!(need.unsupported_by(&support()), vec!["display size"]);
    }

    #[test]
    fn test_a_bigger_box_reports_the_size_it_was_started_at() {
        let need = DesktopNeed::desktop().at_least(1920, 1080);
        assert!(
            need.unsupported_by(&support_at(1920, 1080)).is_empty(),
            "the descriptor follows the environment the box was given"
        );
    }
}
