use super::api::Sandbox;
use crate::profile::{
    AppRuntime, BrowserRuntime, PortLayout, Profile, ScreenRuntime, WallpaperRuntime,
};
use crate::{BrowserEndpoint, DesktopFactory, DesktopSupport, ImageSource, ScreenAction, ScreenId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Shared by machine and profile: the profile is built before the vendor
/// assigns the ID its viewer URL needs.
#[derive(Debug, Default)]
pub struct Remote(Mutex<Option<Sandbox>>);

impl Remote {
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

pub const DEVTOOLS_SECRET_ENV: &str = "COMPUTER_DEVTOOLS_SECRET";

pub const DEVTOOLS_SECRET_HEADER: &str = "x-computer-devtools";

pub struct RemoteProfile {
    inner: Arc<dyn Profile>,
    remote: Arc<Remote>,
    devtools_secret: Option<crate::Secret>,
}

impl RemoteProfile {
    pub fn new(inner: Arc<dyn Profile>, remote: Arc<Remote>) -> Self {
        Self {
            inner,
            remote,
            devtools_secret: crate::Secret::generate().ok(),
        }
    }

    pub fn inner(&self) -> &Arc<dyn Profile> {
        &self.inner
    }
}

impl Profile for RemoteProfile {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn image(&self) -> ImageSource {
        self.inner.image()
    }

    fn ports(&self) -> PortLayout {
        self.inner.ports()
    }

    fn default_size(&self) -> (u32, u32) {
        self.inner.default_size()
    }

    fn support_at(&self, width: u32, height: u32) -> DesktopSupport {
        self.inner.support_at(width, height)
    }

    fn driver(&self) -> Arc<dyn DesktopFactory> {
        self.inner.driver()
    }

    fn screen_runtime(&self) -> Arc<dyn ScreenRuntime> {
        self.inner.screen_runtime()
    }

    fn browser_runtime(&self) -> Arc<dyn BrowserRuntime> {
        self.inner.browser_runtime()
    }

    fn wallpaper_runtime(&self) -> Arc<dyn WallpaperRuntime> {
        self.inner.wallpaper_runtime()
    }

    fn app_runtime(&self) -> Arc<dyn AppRuntime> {
        self.inner.app_runtime()
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
        let mut env = self.inner.launch_env(width, height);
        if let Some(secret) = &self.devtools_secret {
            env.insert(DEVTOOLS_SECRET_ENV.to_string(), secret.expose().to_string());
        }
        env
    }

    fn screen_env(&self, screen: ScreenId) -> BTreeMap<String, String> {
        self.inner.screen_env(screen)
    }

    fn geometry_from(&self, environment: &BTreeMap<String, String>) -> Option<(u32, u32)> {
        self.inner.geometry_from(environment)
    }

    fn devtools(&self, bridge: u16) -> Option<BrowserEndpoint> {
        let sandbox = self.remote.get()?;
        let base = sandbox.url(bridge)?;
        let socket = base
            .strip_prefix("https://")
            .map(|host| format!("wss://{host}"))
            .or_else(|| {
                base.strip_prefix("http://")
                    .map(|host| format!("ws://{host}"))
            })?;

        Some(BrowserEndpoint {
            http_url: base.to_string(),
            ws_url: format!("{socket}/devtools/browser"),
            headers: sandbox
                .headers
                .into_iter()
                .chain(self.devtools_secret.iter().map(|secret| {
                    (
                        DEVTOOLS_SECRET_HEADER.to_string(),
                        secret.expose().to_string(),
                    )
                }))
                .collect(),
        })
    }

    fn port_headers(&self) -> Vec<(String, String)> {
        self.remote
            .get()
            .map(|sandbox| sandbox.headers.into_iter().collect())
            .unwrap_or_default()
    }

    fn viewer_socket(&self, at: &crate::Address, ticket: Option<&crate::Secret>) -> String {
        let Some(host) = self.remote.get().and_then(|sandbox| {
            sandbox
                .url(at.port)
                .and_then(|base| base.strip_prefix("https://"))
                .map(str::to_string)
        }) else {
            return self.inner.viewer_socket(at, ticket);
        };

        let mut url = format!("wss://{host}/websockify");
        if let Some(ticket) = ticket {
            url.push_str("?token=");
            url.push_str(ticket.expose());
        }
        url
    }

    fn viewer_url(&self, at: &crate::Address, ticket: Option<&crate::Secret>) -> String {
        let Some(base) = self
            .remote
            .get()
            .and_then(|sandbox| sandbox.url(at.port).map(str::to_string))
        else {
            return self.inner.viewer_url(at, ticket);
        };

        let mut url = format!("{base}/vnc.html?autoconnect=1&resize=scale");
        if let Some(ticket) = ticket {
            url.push_str(&crate::profile::viewer_path(ticket));
        }
        url
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::X11Profile;

    fn profile() -> (Arc<Remote>, RemoteProfile) {
        let remote = Arc::new(Remote::new());
        let profile = RemoteProfile::new(Arc::new(X11Profile), Arc::clone(&remote));
        (remote, profile)
    }

    fn sandbox() -> Sandbox {
        Sandbox::new("i7q3").published_as([6080, 6081], |port, id| format!("{port}-{id}.x.dev"))
    }

    #[test]
    fn test_the_viewer_url_is_the_address_the_vendor_published() {
        let (remote, profile) = profile();
        remote.set(sandbox());

        assert_eq!(
            profile.viewer_url(&crate::Address::loopback(6080), None),
            "https://6080-i7q3.x.dev/vnc.html?autoconnect=1&resize=scale"
        );
    }

    #[test]
    fn test_the_control_port_gets_its_own_address() {
        let (remote, profile) = profile();
        remote.set(sandbox());

        assert!(
            profile
                .viewer_url(&crate::Address::loopback(6081), None)
                .starts_with("https://6081-i7q3."),
            "takeover is a second server, so it is a second address"
        );
    }

    #[test]
    fn test_a_port_the_vendor_never_published_falls_back() {
        let (remote, profile) = profile();
        remote.set(sandbox());

        assert!(
            profile
                .viewer_url(&crate::Address::loopback(6090), None)
                .starts_with("http://127.0.0.1:6090"),
            "inventing a host for an unpublished port would be a URL to nowhere"
        );
    }

    #[test]
    fn test_the_devtools_bridge_is_published() {
        let (_, profile) = profile();

        assert_eq!(profile.ports().devtools_bridge, Some(9223));
        assert!(
            profile.ports().to_publish().contains(&9223),
            "the vendor only hands back an address for what was asked for"
        );
    }

    #[test]
    fn test_the_descriptor_still_claims_devtools() {
        let (_, profile) = profile();
        let support = profile.support_at(1280, 800);
        let browser = support.browser.expect("chromium is still in the box");

        assert!(browser.cdp);
        assert!(browser.headed, "it still has a window on the screen");
    }

    #[test]
    fn test_devtools_is_reached_at_the_vendors_address_with_its_headers() {
        let (remote, profile) = profile();
        remote.set(
            Sandbox::new("i7q3")
                .published_as([9223], |port, id| format!("{port}-{id}.x.dev"))
                .with_header("gate", "hunter2"),
        );

        let endpoint = profile.devtools(9223).expect("an endpoint");
        assert_eq!(endpoint.http_url, "https://9223-i7q3.x.dev");
        assert_eq!(endpoint.ws_url, "wss://9223-i7q3.x.dev/devtools/browser");
        assert_eq!(
            endpoint.headers.first(),
            Some(&("gate".to_string(), "hunter2".to_string()))
        );
    }

    #[test]
    fn test_the_bridge_is_given_the_secret_devtools_carries() {
        let (remote, profile) = profile();
        remote.set(sandbox().published_as([9223], |port, id| format!("{port}-{id}.x.dev")));

        let env = profile.launch_env(1280, 800);
        let secret = env.get(DEVTOOLS_SECRET_ENV).expect("a secret in the box");
        let endpoint = profile.devtools(9223).expect("an endpoint");

        assert!(
            endpoint
                .headers
                .contains(&(DEVTOOLS_SECRET_HEADER.to_string(), secret.clone())),
            "the vendor's address answers anyone, so the bridge answers only this"
        );
        assert_ne!(
            RemoteProfile::new(Arc::new(X11Profile), Arc::new(Remote::new()))
                .launch_env(1280, 800)
                .get(DEVTOOLS_SECRET_ENV),
            Some(secret),
            "one box's secret opens no other box"
        );
    }

    #[test]
    fn test_devtools_has_no_endpoint_before_the_box_exists() {
        let (_, profile) = profile();

        assert!(
            profile.devtools(9223).is_none(),
            "127.0.0.1 is this machine, not the sandbox"
        );
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
        assert!(
            profile.app_runtime().supported().is_ok(),
            "an image's apps do not stop existing because the box moved"
        );
    }
}
