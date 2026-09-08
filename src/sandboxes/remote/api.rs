//! What a machine needs from a sandbox vendor, and nothing more.
//!
//! The seam sits here rather than on [`crate::microvm::MicroVmApi`] because a
//! hypervisor forwards host-to-guest port pairs and these vendors forward
//! none: each publishes an address of its own per port, and every port field
//! on a [`Plan`](crate::microvm::Plan) would be dead.

use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::runtime::Config;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::time::Duration;

/// The metadata key carrying the name this crate gave a box.
///
/// A vendor assigns the ID, so the name has to live somewhere a listing can be
/// filtered by, or a sweep reports IDs nothing recognises.
pub const NAME_KEY: &str = "computer.name";

/// What a sandbox gets when the caller names no deadline.
///
/// Long enough for an image pull and a desktop session, which the vendors' own
/// defaults are not.
pub const DEFAULT_TTL: Duration = Duration::from_secs(5 * 60);

/// One sandbox, as the control plane described it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Sandbox {
    pub id: String,

    /// Where each published port answers from out here, as a base URL.
    ///
    /// A map rather than a pattern, because Modal hands back a tunnel URL that
    /// no pattern produces. Empty means nothing out here reaches the box.
    pub endpoints: BTreeMap<u16, String>,

    /// What a later data-plane call has to prove, carried back untouched.
    ///
    /// A vendor needing more than one keeps the rest inside its own
    /// implementation, keyed by [`Sandbox::id`].
    pub token: Option<String>,
}

/// Redacted by hand rather than derived: this ends up in a `tracing` field on
/// the way past, and a token in a log is a sandbox anyone can drive.
impl std::fmt::Debug for Sandbox {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Sandbox")
            .field("id", &self.id)
            .field("endpoints", &self.endpoints)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Sandbox {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    /// Publish every port at `host`, formatted from the port and the ID.
    ///
    /// The `6080-<id>.<domain>` shape E2B and Daytona use. A vendor that names
    /// its own URLs fills [`Sandbox::endpoints`] directly.
    pub fn published_as(
        mut self,
        ports: impl IntoIterator<Item = u16>,
        host: impl Fn(u16, &str) -> String,
    ) -> Self {
        for port in ports {
            let url = format!("https://{}", host(port, &self.id));
            self.endpoints.insert(port, url);
        }
        self
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn url(&self, port: u16) -> Option<&str> {
        self.endpoints.get(&port).map(String::as_str)
    }
}

/// What to create, decided before anything exists.
///
/// A plain value, so a vendor's request body is a pure function of it and
/// testable with no account anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPlan {
    /// The name this crate knows the box by, which travels as metadata.
    pub name: String,

    /// A template ID, a snapshot, or an image reference. Which of those it is
    /// belongs to the vendor.
    pub image: String,

    /// The ports the box serves, so [`RemoteApi::create`] can answer with the
    /// address of each.
    pub publish: Vec<u16>,

    pub env: BTreeMap<String, String>,
    pub metadata: BTreeMap<String, String>,

    /// Off means a desktop with no way out, where the vendor can say so.
    pub network: bool,

    pub ttl: Duration,
}

impl Default for SandboxPlan {
    fn default() -> Self {
        Self {
            name: String::new(),
            image: String::new(),
            publish: Vec::new(),
            env: BTreeMap::new(),
            metadata: BTreeMap::new(),
            network: true,
            ttl: DEFAULT_TTL,
        }
    }
}

/// One sandbox vendor, as everything above it needs to see one.
///
/// The methods with defaults are the ones a vendor may not have: a deadline to
/// push out, a listing to sweep, a way to kill a sandbox from a signal
/// handler. Leaving one is an answer, not a gap.
#[async_trait]
pub trait RemoteApi: Send + Sync {
    /// What to call this vendor in an error message.
    fn vendor(&self) -> &str;

    /// Whether the control plane answers, and the credential is accepted.
    ///
    /// Asked before anything is created, so a vendor that is down reports
    /// itself rather than a box that would not start.
    async fn available(&self) -> Result<()>;

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox>;

    /// The sandbox carrying this name, with its endpoints and its token.
    ///
    /// A listing alone is not enough where it reports no credentials: nothing
    /// found that way could be driven.
    async fn find(&self, name: &str) -> Result<Option<Sandbox>>;

    async fn kill(&self, id: &str) -> Result<()>;

    async fn exec(
        &self,
        sandbox: &Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult>;

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>>;

    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()>;

    /// Push the deadline out to `ttl` from now.
    ///
    /// Called while work is arriving, so it costs a round trip per
    /// half-lifetime and no more.
    async fn keep_alive(&self, _id: &str, _ttl: Duration) -> Result<()> {
        Ok(())
    }

    /// What the box itself has said, where a screen that never came up
    /// explains itself.
    async fn logs(&self, _id: &str) -> Result<String> {
        Ok(String::new())
    }

    /// Sandbox ID to the value it carries under this key.
    async fn carrying(&self, _key: &str) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    /// Whether this vendor can be asked what it holds.
    ///
    /// A sweep over a vendor that lists nothing reports every box as gone.
    fn sweepable(&self) -> bool {
        false
    }

    /// Make sure the image exists, or refuse it with the way across.
    ///
    /// The default refuses a container image this crate built: the failure
    /// otherwise arrives later as an unknown image, which sends the caller
    /// looking for a typo. A vendor that pulls an OCI reference overrides this
    /// with `Ok(())`.
    async fn ensure_image(&self, config: &Config) -> Result<()> {
        let Some(bundle) = config.bundle.as_ref().filter(|b| b.owns(&config.image)) else {
            return Ok(());
        };

        Err(Error::Unavailable {
            runtime: self.vendor().to_string(),
            detail: format!(
                "{} is a container image and {} runs its own. Write the {} \
                 build context out with Bundle::materialize, build it there \
                 with /usr/local/bin/computer-desktop as the start command, \
                 and pass what {} calls the result to Builder::image",
                config.image,
                self.vendor(),
                bundle.name,
                self.vendor()
            ),
        })
    }

    /// A command that kills a sandbox with no async runtime in the room.
    ///
    /// `Drop` cannot await. `None` leaves a dropped handle's sandbox running
    /// until its deadline.
    fn reaper(&self, _id: &str) -> Option<(String, Vec<String>)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle;

    struct Bare;

    #[async_trait]
    impl RemoteApi for Bare {
        fn vendor(&self) -> &str {
            "bare"
        }
        async fn available(&self) -> Result<()> {
            Ok(())
        }
        async fn create(&self, _plan: &SandboxPlan) -> Result<Sandbox> {
            Ok(Sandbox::new("one"))
        }
        async fn find(&self, _name: &str) -> Result<Option<Sandbox>> {
            Ok(None)
        }
        async fn kill(&self, _id: &str) -> Result<()> {
            Ok(())
        }
        async fn exec(
            &self,
            _sandbox: &Sandbox,
            _argv: &[String],
            _env: &BTreeMap<String, String>,
        ) -> Result<ExecResult> {
            Ok(ExecResult::default())
        }
        async fn read(&self, _sandbox: &Sandbox, _path: &str) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }
        async fn write(&self, _sandbox: &Sandbox, _path: &str, _bytes: &[u8]) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_a_port_becomes_a_host_of_the_vendors_own() {
        let sandbox = Sandbox::new("i7q3")
            .published_as([6080, 6081], |port, id| format!("{port}-{id}.x.dev"));

        assert_eq!(sandbox.url(6080), Some("https://6080-i7q3.x.dev"));
        assert_eq!(sandbox.url(6081), Some("https://6081-i7q3.x.dev"));
        assert_eq!(sandbox.url(9223), None, "what was not published has no URL");
    }

    #[test]
    fn test_a_token_does_not_survive_a_debug() {
        let printed = format!("{:?}", Sandbox::new("i7q3").with_token("hunter2"));

        assert!(!printed.contains("hunter2"));
        assert!(printed.contains("i7q3"), "the ID is what a log is read for");
    }

    #[tokio::test]
    async fn test_a_bundled_image_is_refused_with_the_way_across() {
        let config = Config {
            image: bundle::DESKTOP.tag(),
            ..Config::default()
        };

        let error = Bare
            .ensure_image(&config)
            .await
            .expect_err("not a template");
        assert!(error.to_string().contains("Bundle::materialize"));
    }

    #[tokio::test]
    async fn test_a_vendors_own_image_name_passes() {
        let config = Config {
            image: "snapshot-abc".to_string(),
            ..Config::default()
        };

        assert!(Bare.ensure_image(&config).await.is_ok());
    }
}
