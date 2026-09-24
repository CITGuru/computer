use crate::error::Result;
use crate::exec::ExecResult;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::time::Duration;

pub const DEFAULT_DOMAIN: &str = "e2b.app";

pub fn api_url(domain: &str) -> String {
    format!("https://api.{domain}")
}

pub const ENVD_PORT: u16 = 49983;

pub use crate::sandboxes::remote::NAME_KEY;

/// E2B's builder appends `USER user` to every Dockerfile, so the display and
/// the takeover token belong to that account.
pub const DEFAULT_USER: &str = "user";

/// E2B's own default of 15 seconds is shorter than one image pull.
pub use crate::sandboxes::remote::DEFAULT_TTL;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub template: String,
    pub build: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sandbox {
    pub id: String,
    pub domain: String,
    pub envd_token: Option<String>,
    /// Can be `None` even for a secure sandbox, and then every published port
    /// is open to whoever has the URL.
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

    /// A subdomain label, not a translation, so the port map is an identity.
    pub fn host(&self, port: u16) -> String {
        format!("{port}-{}.{}", self.id, self.domain())
    }

    pub fn url(&self, port: u16) -> String {
        format!("https://{}", self.host(port))
    }

    pub fn envd_url(&self) -> String {
        self.url(ENVD_PORT)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPlan {
    pub name: String,
    pub template: String,
    pub env: BTreeMap<String, String>,
    pub metadata: BTreeMap<String, String>,
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

#[async_trait]
pub trait E2bApi: Send + Sync {
    async fn available(&self) -> Result<()>;

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox>;

    /// Must return tokens; a listing alone carries no credentials.
    async fn find(&self, name: &str) -> Result<Option<Sandbox>>;

    async fn kill(&self, id: &str) -> Result<()>;

    async fn keep_alive(&self, id: &str, ttl: Duration) -> Result<()>;

    async fn pause(&self, _id: &str) -> Result<()> {
        Err(crate::Error::Unsupported {
            gaps: vec!["pausing a box"],
        })
    }

    async fn resume(&self, _id: &str, _ttl: Duration) -> Result<()> {
        Err(crate::Error::Unsupported {
            gaps: vec!["pausing a box"],
        })
    }

    async fn paused(&self, _id: &str) -> Result<bool> {
        Ok(false)
    }

    async fn find_template(&self, _name: &str) -> Result<Option<String>> {
        Ok(None)
    }

    async fn create_template(&self, _name: &str, _cpus: u32, _memory_mb: u32) -> Result<Built> {
        Err(crate::Error::Unsupported {
            gaps: vec!["building a template"],
        })
    }

    async fn carry_files(
        &self,
        _template: &str,
        _carried: &super::template::Carried,
    ) -> Result<()> {
        Err(crate::Error::Unsupported {
            gaps: vec!["building a template"],
        })
    }

    async fn start_build(&self, _built: &Built, _plan: &super::template::Plan) -> Result<()> {
        Err(crate::Error::Unsupported {
            gaps: vec!["building a template"],
        })
    }

    async fn build_status(&self, _built: &Built) -> Result<serde_json::Value> {
        Err(crate::Error::Unsupported {
            gaps: vec!["building a template"],
        })
    }

    async fn logs(&self, id: &str) -> Result<String>;

    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>>;

    async fn exec(
        &self,
        sandbox: &Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult>;

    async fn read(&self, sandbox: &Sandbox, path: &str) -> Result<Vec<u8>>;
    async fn write(&self, sandbox: &Sandbox, path: &str, bytes: &[u8]) -> Result<()>;

    /// `Drop` cannot await. `None` leaves a dropped sandbox running until its
    /// deadline.
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
