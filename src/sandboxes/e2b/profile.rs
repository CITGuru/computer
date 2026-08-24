//! The same image, reached over the internet instead of over loopback.
//!
//! Two claims stop being true when the box moves off this host, and a profile
//! is where an image's claims live. The viewer is not on `127.0.0.1` any more,
//! and DevTools is not reachable at all.

use super::api::Sandbox;
use crate::profile::{PortLayout, Profile};
use crate::{DesktopFactory, DesktopSupport, ImageSource, ScreenAction, ScreenId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Where the sandbox turned out to be.
///
/// A container's ports are known once `docker port` answers, and a profile can
/// be built before either. A sandbox's host contains an ID the control plane
/// assigns, so it is not known until the sandbox exists — and the profile that
/// formats the viewer URL is built before that. The machine and the profile
/// hold one of these between them.
#[derive(Debug, Default)]
pub struct Reachable(Mutex<Option<Sandbox>>);

impl Reachable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, sandbox: Sandbox) {
        if let Ok(mut held) = self.0.lock() {
            *held = Some(sandbox);
        }
    }

    pub fn get(&self) -> Option<Sandbox> {
        self.0.lock().ok().and_then(|held| held.clone())
    }

    pub fn clear(&self) {
        if let Ok(mut held) = self.0.lock() {
            *held = None;
        }
    }
}

/// An image's contract, with the two claims that do not survive the move.
///
/// Everything else delegates, so a caller who wrote their own profile keeps it
/// — this wraps one rather than replacing it.
pub struct E2bProfile {
    inner: Arc<dyn Profile>,
    reachable: Arc<Reachable>,
}

impl E2bProfile {
    pub fn new(inner: Arc<dyn Profile>, reachable: Arc<Reachable>) -> Self {
        Self { inner, reachable }
    }

    pub fn inner(&self) -> &Arc<dyn Profile> {
        &self.inner
    }
}

impl Profile for E2bProfile {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn image(&self) -> ImageSource {
        self.inner.image()
    }

    /// The image's ports, minus the DevTools bridge.
    ///
    /// **Withdrawn rather than published to nowhere.** An E2B endpoint is
    /// `wss` on a public host and [`crate::cdp`] connects with a plain
    /// `TcpStream`, so neither the URL nor the transport survives. Dropping it
    /// here takes it out of what is published, makes
    /// [`crate::Computer::devtools`] answer `None`, and is why `support_at`
    /// below stops claiming it.
    fn ports(&self) -> PortLayout {
        PortLayout {
            devtools_bridge: None,
            ..self.inner.ports()
        }
    }

    fn default_size(&self) -> (u32, u32) {
        self.inner.default_size()
    }

    fn support_at(&self, width: u32, height: u32) -> DesktopSupport {
        let mut support = self.inner.support_at(width, height);

        if let Some(browser) = support.browser.as_mut() {
            // The browser is still there and still headed; nothing out here
            // can reach its debugger. `audit` skips the check rather than
            // failing it, which is the difference between a claim withdrawn
            // and a claim broken.
            browser.cdp = false;
        }
        support
    }

    fn driver(&self) -> Arc<dyn DesktopFactory> {
        self.inner.driver()
    }

    fn screen_command(
        &self,
        action: ScreenAction,
        screen: ScreenId,
        extra: &[String],
    ) -> Vec<String> {
        self.inner.screen_command(action, screen, extra)
    }

    fn boot_command(&self) -> Vec<String> {
        self.inner.boot_command()
    }

    fn launch_env(&self, width: u32, height: u32) -> BTreeMap<String, String> {
        self.inner.launch_env(width, height)
    }

    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String> {
        self.inner.screen_env(screen)
    }

    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        self.inner.geometry_from(environment)
    }

    /// The sandbox's own host, which is a subdomain and not a host port.
    ///
    /// Falls back to the inner profile before a sandbox exists. Nothing calls
    /// this that early — a screen has no URL until the box it is on was
    /// started — and answering with loopback beats formatting a host out of an
    /// ID nobody has yet.
    fn viewer_url(&self, port: u16) -> String {
        match self.reachable.get() {
            Some(sandbox) => format!("{}/vnc.html?autoconnect=1&resize=scale", sandbox.url(port)),
            None => self.inner.viewer_url(port),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::X11Profile;

    fn profile() -> (Arc<Reachable>, E2bProfile) {
        let reachable = Arc::new(Reachable::new());
        let profile = E2bProfile::new(Arc::new(X11Profile), Arc::clone(&reachable));
        (reachable, profile)
    }

    #[test]
    fn test_the_viewer_url_is_the_sandbox_host() {
        let (reachable, profile) = profile();
        reachable.set(Sandbox::new("i7q3"));

        assert_eq!(
            profile.viewer_url(6080),
            "https://6080-i7q3.e2b.app/vnc.html?autoconnect=1&resize=scale"
        );
    }

    #[test]
    fn test_the_control_port_gets_its_own_host() {
        let (reachable, profile) = profile();
        reachable.set(Sandbox::new("i7q3"));

        assert!(
            profile.viewer_url(6081).starts_with("https://6081-i7q3."),
            "takeover is a second server, so it is a second host"
        );
    }

    #[test]
    fn test_the_devtools_bridge_is_not_published() {
        let (_, profile) = profile();

        assert!(profile.ports().devtools_bridge.is_none());
        assert!(
            !profile.ports().to_publish().contains(&9223),
            "a port nothing out here can reach is worse published than absent"
        );
    }

    #[test]
    fn test_the_descriptor_stops_claiming_devtools() {
        let (_, profile) = profile();
        let support = profile.support_at(1280, 800);
        let browser = support.browser.expect("chromium is still in the box");

        assert!(!browser.cdp);
        assert!(browser.headed, "it still has a window on the screen");
    }

    #[test]
    fn test_everything_else_is_the_image_it_wraps() {
        let (_, profile) = profile();
        let inner = X11Profile;

        assert_eq!(profile.name(), inner.name());
        assert_eq!(profile.default_size(), inner.default_size());
        assert_eq!(profile.boot_command(), inner.boot_command());
        assert_eq!(
            profile.screen_env(ScreenId(1)),
            inner.screen_env(ScreenId(1))
        );
        assert_eq!(profile.ports().max_screens, inner.ports().max_screens);
    }
}
