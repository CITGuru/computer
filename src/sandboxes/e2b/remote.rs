//! E2B as [`crate::sandboxes::remote`] sees it: the part that is E2B's and
//! nobody else's, which is a port that becomes a subdomain, a second token the
//! seam has nowhere to put, and an image that is a template.

use super::api::{E2bApi, Sandbox, SandboxPlan};
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::runtime::Config;
use crate::sandboxes::remote::{self, RemoteApi};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct E2bVendor {
    api: Arc<dyn E2bApi>,
    /// ID to the sandbox as the control plane described it.
    ///
    /// [`remote::Sandbox`] carries one token; E2B needs two and a domain that
    /// is not always `e2b.app`, so what does not fit lives here.
    known: Mutex<BTreeMap<String, Sandbox>>,
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

    /// The whole description behind an ID.
    ///
    /// `create` and `find` record what they answered, so an ID this has never
    /// seen is one no credential here can drive — [`Error::Gone`], rather than
    /// a call refused for a reason nobody could act on.
    fn described(&self, sandbox: &remote::Sandbox) -> Result<Sandbox> {
        self.known
            .lock()
            .ok()
            .and_then(|known| known.get(&sandbox.id).cloned())
            .ok_or_else(|| Error::Gone(sandbox.id.clone()))
    }

    /// A sandbox as the seam carries it.
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

        // Asked for secure and not given a token: the proxy is gating nothing,
        // so every published port answers to whoever has the URL. Said out
        // loud because the alternative is a caller believing otherwise.
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

    /// A sandbox somebody else started, with the credentials that drive it.
    ///
    /// No endpoints: which ports the box serves is the image's answer, and
    /// nothing here was told which image it was. It drives; it hands out no
    /// viewer URL.
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

    /// E2B runs templates, and this crate builds container images.
    ///
    /// Refused with the way across rather than left to fail later as an
    /// unknown template, which sends the caller looking for a typo.
    async fn ensure_image(&self, config: &Config) -> Result<()> {
        let Some(bundle) = config.bundle.as_ref().filter(|b| b.owns(&config.image)) else {
            return Ok(());
        };

        Err(Error::Unavailable {
            runtime: "e2b".to_string(),
            detail: format!(
                "{} is a container image; E2B runs templates. Write the build \
                 context out with Bundle::materialize and build it there:\n  \
                 e2b template build -n {} -c \"/usr/local/bin/computer-desktop\"\n\
                 then pass the template ID to Builder::image",
                config.image, bundle.name
            ),
        })
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
    async fn test_a_bundled_image_is_refused_with_the_way_across() {
        let error = vendor(Arc::new(ScriptedE2b::new()))
            .ensure_image(&Config {
                image: bundle::DESKTOP.tag(),
                ..Config::default()
            })
            .await
            .expect_err("a container image is not a template");

        assert!(error.to_string().contains("e2b template build"));
        assert!(error.needs_another_place());
    }

    #[tokio::test]
    async fn test_a_template_somebody_else_built_is_left_alone() {
        assert!(
            vendor(Arc::new(ScriptedE2b::new()))
                .ensure_image(&Config {
                    image: "tmpl-abc".to_string(),
                    ..Config::default()
                })
                .await
                .is_ok()
        );
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
