//! What the machine needs from E2B, and nothing more.
//!
//! The seam sits here rather than on [`crate::microvm::MicroVmApi`] because a
//! hypervisor forwards host-to-guest port pairs and E2B forwards none: it
//! publishes a hostname per port, and every port field on a
//! [`Plan`](crate::microvm::Plan) would be dead.

use crate::error::Result;
use crate::exec::ExecResult;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::time::Duration;

/// Where E2B publishes sandboxes when nothing names another domain.
pub const DEFAULT_DOMAIN: &str = "e2b.app";

/// The control plane, given the domain its sandboxes live on.
pub fn api_url(domain: &str) -> String {
    format!("https://api.{domain}")
}

/// Where the sandbox agent listens. Every data-plane call goes through it.
pub const ENVD_PORT: u16 = 49983;

/// The metadata key carrying the name this crate gave a box.
///
/// E2B assigns the sandbox ID, so a caller's name has to live somewhere the
/// control plane can be filtered by, or [`Machine::running`](crate::Machine)
/// has nothing to ask about.
pub const NAME_KEY: &str = "computer.name";

/// The user envd runs commands as.
///
/// E2B's builder appends `USER user` to whatever Dockerfile it is given, so
/// the template's own start command runs as that account and `/tmp/computer`
/// and the X socket belong to it. Exec as anybody else and the takeover token
/// and the display have two different owners. A template that arranges
/// otherwise wants [`super::cloud::Cloud::as_user`].
pub const DEFAULT_USER: &str = "user";

/// What a sandbox gets when the caller names no deadline.
///
/// E2B's own default is 15 seconds, which is shorter than one image pull and
/// far shorter than a desktop session.
pub const DEFAULT_TTL: Duration = Duration::from_secs(5 * 60);

/// One sandbox, as the control plane described it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sandbox {
    pub id: String,
    /// Empty means [`DEFAULT_DOMAIN`]. The API names one per sandbox because
    /// not every deployment is `e2b.app`.
    pub domain: String,
    /// Proves a data-plane call to envd.
    pub envd_token: Option<String>,
    /// Proves a request to the proxy in front of every published port.
    ///
    /// Travels as an `e2b-traffic-access-token` header, which this crate sets
    /// on its own calls and a browser cannot set at all.
    ///
    /// `None` even after asking for a secure sandbox, which happens: the API
    /// decides. Then the proxy gates nothing and every published port answers
    /// to whoever has the URL. See
    /// [`E2bMachine::public_viewer`](super::machine::E2bMachine::public_viewer).
    pub traffic_token: Option<String>,
}

impl Sandbox {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }

    pub fn domain(&self) -> &str {
        match self.domain.is_empty() {
            true => DEFAULT_DOMAIN,
            false => &self.domain,
        }
    }

    /// The host one of the box's ports is published on.
    ///
    /// A subdomain label rather than a translation, which is why the machine
    /// reports an identity port map: the number out here really is the number
    /// inside.
    pub fn host(&self, port: u16) -> String {
        format!("{port}-{}.{}", self.id, self.domain())
    }

    pub fn url(&self, port: u16) -> String {
        format!("https://{}", self.host(port))
    }

    /// Where envd answers, which is every data-plane call.
    pub fn envd_url(&self) -> String {
        self.url(ENVD_PORT)
    }
}

/// What to create, decided before anything exists.
///
/// A plain value, so the request body is a pure function of it and testable
/// with no account anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPlan {
    /// The name this crate knows the box by, stored as metadata.
    pub name: String,
    /// E2B runs templates, not images. This is a template ID or alias.
    pub template: String,
    pub env: BTreeMap<String, String>,
    pub metadata: BTreeMap<String, String>,
    /// Off closes egress, which E2B applies as a deny of everything.
    pub network: bool,
    pub ttl: Duration,
}

impl Default for SandboxPlan {
    fn default() -> Self {
        Self {
            name: String::new(),
            template: String::new(),
            env: BTreeMap::new(),
            metadata: BTreeMap::new(),
            network: true,
            ttl: DEFAULT_TTL,
        }
    }
}

/// The one seam between this crate and E2B.
///
/// A caller with their own HTTP client implements this and gets the machine,
/// the profile and everything above them; [`super::cloud`] is the
/// implementation that ships, and the only part behind a feature.
#[async_trait]
pub trait E2bApi: Send + Sync {
    /// Whether the control plane answers, and the key is accepted.
    async fn available(&self) -> Result<()>;

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox>;

    /// The sandbox carrying this name, with its tokens.
    ///
    /// A listing alone is not enough: it reports IDs and metadata and no
    /// credentials, so nothing found this way could be driven.
    async fn find(&self, name: &str) -> Result<Option<Sandbox>>;

    async fn kill(&self, id: &str) -> Result<()>;

    /// Push the deadline out to `ttl` from now.
    async fn keep_alive(&self, id: &str, ttl: Duration) -> Result<()>;

    async fn logs(&self, id: &str) -> Result<String>;

    /// Every sandbox carrying this metadata key, and its value.
    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>>;

    async fn exec(
        &self,
        sandbox: &Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult>;

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>>;
    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()>;

    /// A command that kills a sandbox with no async runtime in the room.
    ///
    /// `Drop` cannot await. `None` means a dropped handle leaves the sandbox
    /// running until its deadline, which is a fact about the implementation
    /// rather than something to hide.
    fn reaper(&self, _id: &str) -> Option<(String, Vec<String>)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_port_becomes_a_subdomain_label() {
        let sandbox = Sandbox::new("i7q3");
        assert_eq!(sandbox.host(6080), "6080-i7q3.e2b.app");
        assert_eq!(sandbox.url(6081), "https://6081-i7q3.e2b.app");
        assert_eq!(sandbox.envd_url(), "https://49983-i7q3.e2b.app");
    }

    #[test]
    fn test_a_named_domain_wins_over_the_default() {
        let sandbox = Sandbox {
            domain: "e2b-foxtrot.dev".to_string(),
            ..Sandbox::new("i7q3")
        };
        assert_eq!(sandbox.host(6080), "6080-i7q3.e2b-foxtrot.dev");
        assert_eq!(api_url(sandbox.domain()), "https://api.e2b-foxtrot.dev");
    }
}
