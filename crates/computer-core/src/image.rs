use crate::{Browser, Control, DesktopSupport, Display, DisplayServer, Viewer, ViewerKind};

pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 800;

/// Eight screens is roughly a core and 2 GB; past that a screen starves rather than queues.
pub const MAX_SCREENS: u32 = 8;

pub const VIEW_PORT_BASE: u16 = 6080;
pub const VNC_PORT_BASE: u16 = 5900;

/// Loopback only, whatever `--remote-debugging-address` says.
pub const DEVTOOLS_PORT: u16 = 9222;

pub const DEVTOOLS_BRIDGE_PORT: u16 = 9223;

pub const PROFILE_NAME: &str = "computer-desktop";

pub const DESKTOP_COMMAND: &str = "computer-desktop";
pub const SCREEN_COMMAND: &str = "computer-screen";
pub const BROWSER_COMMAND: &str = "computer-browser";

pub const WIDTH_ENV: &str = "COMPUTER_SCREEN_WIDTH";
pub const HEIGHT_ENV: &str = "COMPUTER_SCREEN_HEIGHT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenPorts {
    /// Never `:0`, which is a real console on a real host.
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
    Control,
    Release,
    Viewers,
    Open,
    Record,
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
            Self::Record => "record",
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
