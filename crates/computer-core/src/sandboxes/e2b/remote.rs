use super::api::{self, E2bApi, Sandbox, SandboxPlan};
use super::template;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::sandboxes::remote::{self, RemoteApi};
use async_trait::async_trait;
use computer_types::{Capabilities, Environment, Resources, Start};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct E2bVendor {
    api: Arc<dyn E2bApi>,
    /// [`remote::Sandbox`] carries one token; E2B needs two and a domain.
    known: Mutex<BTreeMap<String, Sandbox>>,
}

pub const TEMPLATE_CPUS: u32 = 2;

pub const TEMPLATE_MEMORY_MIB: u64 = 2048;

pub const BUILD_WAIT: Duration = Duration::from_secs(20 * 60);

impl E2bVendor {
    async fn wait_for(&self, built: &api::Built, name: &str) -> Result<()> {
        let deadline = std::time::Instant::now() + BUILD_WAIT;

        loop {
            let answer = self.api.build_status(built).await?;
            let status = super::wire::status_of(&answer);

            match status.as_str() {
                "ready" | "uploaded" => {
                    tracing::info!(template = %name, "the template is built");
                    return Ok(());
                }
                "error" => {
                    return Err(Error::Failed {
                        code: 1,
                        stderr: format!("{name}: {}", super::wire::why_of(&answer)),
                    });
                }
                _ => {}
            }

            if std::time::Instant::now() >= deadline {
                return Err(Error::Timeout {
                    after: BUILD_WAIT,
                    detail: format!("{name} was still {status}"),
                });
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

impl E2bVendor {
    pub fn new(api: Arc<dyn E2bApi>) -> Self {
        Self {
            api,
            known: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn api(&self) -> &Arc<dyn E2bApi> {
        &self.api
    }

    fn remember(&self, sandbox: &Sandbox) {
        if let Ok(mut known) = self.known.lock() {
            known.insert(sandbox.id.clone(), sandbox.clone());
        }
    }

    fn described(&self, sandbox: &remote::Sandbox) -> Result<Sandbox> {
        self.known
            .lock()
            .ok()
            .and_then(|known| known.get(&sandbox.id).cloned())
            .ok_or_else(|| Error::Gone(sandbox.id.clone()))
    }

    fn published(sandbox: &Sandbox, ports: &[u16]) -> remote::Sandbox {
        remote::Sandbox {
            id: sandbox.id.clone(),
            endpoints: ports
                .iter()
                .map(|port| (*port, sandbox.url(*port)))
                .collect(),
            token: sandbox.envd_token.clone(),
        }
    }
}

#[async_trait]
impl RemoteApi for E2bVendor {
    fn vendor(&self) -> &str {
        "e2b"
    }

    fn environment(&self) -> Environment {
        Environment::MicroVm(serde_json::json!({ "hypervisor": "firecracker" }))
    }

    fn can(&self) -> Capabilities {
        Capabilities {
            start: Start::Snapshot,
            pause: true,
            fork: true,
            volumes: true,
            resources: Resources::AtImage,
            ..Capabilities::default()
        }
    }

    async fn available(&self) -> Result<()> {
        self.api.available().await
    }

    async fn create(&self, plan: &remote::SandboxPlan) -> Result<remote::Sandbox> {
        let sandbox = self
            .api
            .create(&SandboxPlan {
                name: plan.name.clone(),
                template: plan.image.clone(),
                env: plan.env.clone(),
                metadata: plan.metadata.clone(),
                network: plan.network,
                ttl: plan.ttl,
            })
            .await?;

        if sandbox.traffic_token.is_none() {
            tracing::warn!(
                sandbox = %sandbox.id,
                "e2b returned no traffic token; every published port on this \
                 sandbox is reachable by anyone with its URL, and the screen \
                 has no password"
            );
        }

        self.remember(&sandbox);
        Ok(Self::published(&sandbox, &plan.publish))
    }

    /// No endpoints: which ports the box serves depends on an image nothing
    /// here was told.
    async fn find(&self, name: &str) -> Result<Option<remote::Sandbox>> {
        let Some(sandbox) = self.api.find(name).await? else {
            return Ok(None);
        };

        self.remember(&sandbox);
        Ok(Some(Self::published(&sandbox, &[])))
    }

    async fn kill(&self, id: &str) -> Result<()> {
        self.api.kill(id).await?;

        if let Ok(mut known) = self.known.lock() {
            known.remove(id);
        }
        Ok(())
    }

    async fn keep_alive(&self, id: &str, ttl: Duration) -> Result<()> {
        self.api.keep_alive(id, ttl).await
    }

    async fn logs(&self, id: &str) -> Result<String> {
        self.api.logs(id).await
    }

    async fn carrying(&self, key: &str) -> Result<Vec<(String, String)>> {
        self.api.carrying(key).await
    }

    fn sweepable(&self) -> bool {
        true
    }

    async fn exec(
        &self,
        sandbox: &remote::Sandbox,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        self.api.exec(&self.described(sandbox)?, argv, env).await
    }

    async fn read(&self, sandbox: &remote::Sandbox, path: &str) -> Result<Vec<u8>> {
        self.api.read(&self.described(sandbox)?, path).await
    }

    async fn write(&self, sandbox: &remote::Sandbox, path: &str, bytes: &[u8]) -> Result<()> {
        self.api.write(&self.described(sandbox)?, path, bytes).await
    }

    async fn ensure_image(&self, config: &Config) -> Result<Option<String>> {
        let Some(bundle) = config
            .bundle
            .as_ref()
            .filter(|held| held.owns(&config.image))
        else {
            return Ok(None);
        };

        let name = template::named(&config.image);
        if let Some(held) = self.api.find_template(&name).await? {
            return Ok(Some(held));
        }

        let Some(plan) = template::plan(bundle, &config.extras) else {
            return Err(Error::Unavailable {
                runtime: "e2b".to_string(),
                detail: format!("{} carries no Dockerfile to build from", bundle.name),
            });
        };

        let cpus = config
            .cpus
            .as_deref()
            .and_then(|cpus| cpus.parse().ok())
            .unwrap_or(TEMPLATE_CPUS);
        let memory = config
            .memory
            .as_deref()
            .and_then(crate::microvm::mebibytes)
            .unwrap_or(TEMPLATE_MEMORY_MIB);

        tracing::info!(template = %name, steps = plan.steps.len(), "building a template");
        let built = self.api.create_template(&name, cpus, memory as u32).await?;

        for carried in &plan.carries {
            self.api.carry_files(&built.template, carried).await?;
        }

        self.api.start_build(&built, &plan).await?;
        self.wait_for(&built, &name).await?;

        Ok(Some(built.template))
    }

    fn reaper(&self, id: &str) -> Option<(String, Vec<String>)> {
        self.api.reaper(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle;
    use crate::testing::ScriptedE2b;

    fn vendor(api: Arc<ScriptedE2b>) -> E2bVendor {
        E2bVendor::new(api)
    }

    fn plan() -> remote::SandboxPlan {
        remote::SandboxPlan {
            name: "box".to_string(),
            image: "tmpl-abc".to_string(),
            publish: vec![6080, 6081],
            ..remote::SandboxPlan::default()
        }
    }

    #[tokio::test]
    async fn test_a_bundled_image_is_built_into_a_template() {
        let api = Arc::new(ScriptedE2b::new());

        let started_from = vendor(Arc::clone(&api))
            .ensure_image(&Config {
                image: bundle::DESKTOP.tag(),
                ..Config::default()
            })
            .await
            .expect("a container image becomes a template here");

        assert!(
            started_from.is_some_and(|template| template.starts_with("tmpl-")),
            "the box starts from the template, not from the container image"
        );
        assert_eq!(api.built().len(), 1);
    }

    #[tokio::test]
    async fn test_a_template_somebody_else_built_is_left_alone() {
        let api = Arc::new(ScriptedE2b::new());

        let started_from = vendor(Arc::clone(&api))
            .ensure_image(&Config {
                image: "tmpl-abc".to_string(),
                ..Config::default()
            })
            .await
            .expect("a template of somebody else's is used as it is");

        assert_eq!(
            started_from, None,
            "nothing here changes what to start from"
        );
        assert!(api.built().is_empty(), "and nothing is built");
    }

    #[tokio::test]
    async fn test_a_port_becomes_a_subdomain_of_the_sandbox() {
        let sandbox = vendor(Arc::new(ScriptedE2b::new()))
            .create(&plan())
            .await
            .expect("a sandbox");

        assert_eq!(sandbox.url(6080), Some("https://6080-sbx-0.e2b.app"));
        assert_eq!(sandbox.url(6081), Some("https://6081-sbx-0.e2b.app"));
        assert_eq!(
            sandbox.url(9223),
            None,
            "a port the box does not serve has no host of its own"
        );
    }

    #[tokio::test]
    async fn test_the_template_is_what_the_image_becomes() {
        let api = Arc::new(ScriptedE2b::new());
        vendor(Arc::clone(&api))
            .create(&plan())
            .await
            .expect("made");

        let asked = api.plans().pop().expect("one plan");
        assert_eq!(asked.template, "tmpl-abc");
        assert_eq!(asked.name, "box");
    }

    #[tokio::test]
    async fn test_a_sandbox_nothing_here_described_cannot_be_driven() {
        let error = vendor(Arc::new(ScriptedE2b::new()))
            .exec(&remote::Sandbox::new("sbx-9"), &[], &BTreeMap::new())
            .await
            .expect_err("no tokens for it");

        assert!(
            matches!(error, Error::Gone(_)),
            "a call with no credential would be refused for a reason nobody \
             could act on"
        );
    }

    #[tokio::test]
    async fn test_a_box_from_another_process_is_driveable_and_unwatched() {
        let api = Arc::new(ScriptedE2b::new().holding("left-over", "sbx-9"));
        let vendor = vendor(api);

        let found = vendor
            .find("left-over")
            .await
            .expect("a listing")
            .expect("it is there");

        assert!(
            found.endpoints.is_empty(),
            "which ports the box serves is the image's answer, and nothing \
             here was told which image"
        );
        assert!(
            vendor.exec(&found, &[], &BTreeMap::new()).await.is_ok(),
            "it still drives: connect answered with both tokens"
        );
    }
}
