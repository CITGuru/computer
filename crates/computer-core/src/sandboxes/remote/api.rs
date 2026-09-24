use crate::config::Config;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use async_trait::async_trait;
use computer_types::{Capabilities, Environment};
use std::collections::BTreeMap;
use std::time::Duration;

pub const NAME_KEY: &str = "computer.name";

/// The vendors' own defaults are too short for an image pull.
pub const DEFAULT_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Sandbox {
    pub id: String,

    /// A map, not a pattern: Modal hands back tunnel URLs no pattern produces.
    pub endpoints: BTreeMap<u16, String>,

    pub token: Option<String>,
}

/// Redacted: this ends up in `tracing` fields.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPlan {
    pub name: String,

    pub image: String,

    pub publish: Vec<u16>,

    pub env: BTreeMap<String, String>,
    pub metadata: BTreeMap<String, String>,

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

#[async_trait]
pub trait RemoteApi: Send + Sync {
    fn vendor(&self) -> &str;

    async fn available(&self) -> Result<()>;

    async fn create(&self, plan: &SandboxPlan) -> Result<Sandbox>;

    /// Must return the token; a listing alone often carries no credentials.
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

    async fn keep_alive(&self, _id: &str, _ttl: Duration) -> Result<()> {
        Ok(())
    }

    async fn pause(&self, _id: &str) -> Result<()> {
        Err(Error::Unsupported {
            gaps: vec!["pausing a box"],
        })
    }

    async fn resume(&self, _id: &str, _ttl: Duration) -> Result<()> {
        Err(Error::Unsupported {
            gaps: vec!["pausing a box"],
        })
    }

    async fn paused(&self, _id: &str) -> Result<bool> {
        Ok(false)
    }

    async fn logs(&self, _id: &str) -> Result<String> {
        Ok(String::new())
    }

    async fn carrying(&self, _key: &str) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    /// A sweep over a vendor that lists nothing reports every box as gone.
    fn sweepable(&self) -> bool {
        false
    }

    fn environment(&self) -> Environment {
        Environment::default()
    }

    fn can(&self) -> Capabilities {
        Capabilities::default()
    }

    /// A vendor that pulls an OCI reference overrides this with `Ok(())`.
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

    /// `Drop` cannot await. `None` leaves a dropped sandbox running until its
    /// deadline.
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
