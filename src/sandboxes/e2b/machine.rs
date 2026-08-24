//! A box in an E2B sandbox.
//!
//! [`Machine`] is the only thing that knows where a box is, so the driver, the
//! screens, the takeover gate and the descriptor above this are the same code
//! a container runs.
//!
//! Three things are this module's own. A sandbox is found by metadata rather
//! than by name, because E2B assigns the ID. The screen is brought up with
//! `computer-desktop --once`, because a sandbox lives until its deadline. And
//! the deadline is pushed out lazily from [`Machine::exec`], because every
//! call reaches the box through there and refreshing on each one would put a
//! round trip in front of every click.

use super::api::{DEFAULT_TTL, E2bApi, NAME_KEY, Sandbox, SandboxPlan};
use super::profile::Reachable;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::machine::{Machine, PortMap};
use crate::runtime::Config;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// What a box created by this process is, and when its deadline was last set.
#[derive(Debug, Clone)]
struct Held {
    sandbox: Sandbox,
    env: BTreeMap<String, String>,
    published: Vec<u16>,
    ttl: Duration,
    refreshed_at: SystemTime,
}

pub struct E2bMachine {
    api: Arc<dyn E2bApi>,
    /// Shared with the profile that formats the viewer URL.
    reachable: Arc<Reachable>,
    held: Mutex<BTreeMap<String, Held>>,
    ttl: Duration,
    public_viewer: bool,
}

impl E2bMachine {
    /// Pair this with the [`E2bProfile`](super::profile::E2bProfile) holding
    /// the same [`Reachable`]; [`super::pair`] builds both.
    pub fn new(api: Arc<dyn E2bApi>, reachable: Arc<Reachable>) -> Self {
        Self {
            api,
            reachable,
            held: Mutex::new(BTreeMap::new()),
            ttl: DEFAULT_TTL,
            public_viewer: false,
        }
    }

    /// How long a sandbox lives with nothing asked of it.
    ///
    /// Pushed out from [`Machine::exec`] while work is arriving, so this is
    /// how long a box survives silence rather than how long it survives.
    pub fn expiring_after(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Hand out the sandbox's own URL as the viewer.
    ///
    /// **Off is not privacy, and saying so matters.** Every other [`Machine`]
    /// publishes on loopback, where the bind is the whole authentication
    /// story; `x11vnc` still runs with `-nopw` here. A sandbox's ports are
    /// published at `6080-<id>.<domain>` by E2B, not by this crate, so that
    /// host answers whether or not anything here prints it.
    ///
    /// What the setting decides is only whether the URL is handed to a caller.
    /// Off, the viewer ports are left out of the port map and
    /// [`crate::Computer::viewer_url`] answers `None`; the desktop is still
    /// driveable from here, and the URL still exists.
    ///
    /// Whether that URL is *gated* is E2B's decision, not this one. A sandbox
    /// created secure carries a `trafficAccessToken` and its proxy refuses
    /// anything without an `e2b-traffic-access-token` header — which this
    /// crate sends and a browser cannot. Where the API returns no such token,
    /// nothing is refused and the URL is the only secret there is; `start`
    /// warns when that happens.
    pub fn public_viewer(mut self, public: bool) -> Self {
        self.public_viewer = public;
        self
    }

    pub fn api(&self) -> &Arc<dyn E2bApi> {
        &self.api
    }

    fn plan_for(&self, name: &str, config: &Config) -> SandboxPlan {
        SandboxPlan {
            name: name.to_string(),
            template: config.image.clone(),
            env: config.env.clone(),
            // A caller's labels travel as metadata, which is what makes an
            // expiry written by `expires_after` findable by a sweeper.
            metadata: config.labels.clone(),
            network: config.network,
            ttl: self.ttl,
        }
    }

    fn remember(&self, name: &str, sandbox: &Sandbox, config: &Config) {
        if let Ok(mut held) = self.held.lock() {
            held.insert(
                name.to_string(),
                Held {
                    sandbox: sandbox.clone(),
                    env: config.env.clone(),
                    published: config.publish.clone(),
                    ttl: self.ttl,
                    refreshed_at: SystemTime::now(),
                },
            );
        }
    }

    fn recall(&self, name: &str) -> Option<Held> {
        self.held.lock().ok()?.get(name).cloned()
    }

    /// The sandbox behind a name, from this process or from the control plane.
    ///
    /// A box this process did not start has to be asked about, and asking
    /// costs a round trip — so what was started here is answered from memory.
    async fn sandbox(&self, name: &str) -> Result<Sandbox> {
        if let Some(held) = self.recall(name) {
            return Ok(held.sandbox);
        }

        let found = self
            .api
            .find(name)
            .await?
            .ok_or_else(|| Error::Gone(name.to_string()))?;

        self.reachable.set(found.clone());
        if let Ok(mut held) = self.held.lock() {
            held.insert(
                name.to_string(),
                Held {
                    sandbox: found.clone(),
                    env: BTreeMap::new(),
                    published: Vec::new(),
                    ttl: self.ttl,
                    refreshed_at: SystemTime::now(),
                },
            );
        }
        Ok(found)
    }

    /// Push the deadline out, but only once it is worth a round trip.
    ///
    /// Half the deadline, so a box in use never comes within half a lifetime
    /// of expiring and a busy screen does not spend a call per click saying so.
    async fn keep_alive(&self, name: &str) {
        let Some(held) = self.recall(name) else {
            return;
        };

        let due = held
            .refreshed_at
            .elapsed()
            .map(|since| since * 2 >= held.ttl)
            .unwrap_or(true);

        if !due {
            return;
        }

        // Best effort. A refresh that fails is reported by the next real call
        // failing, and turning it into an error here would fail a command that
        // would otherwise have worked.
        if self
            .api
            .keep_alive(&held.sandbox.id, held.ttl)
            .await
            .is_ok()
            && let Ok(mut all) = self.held.lock()
            && let Some(entry) = all.get_mut(name)
        {
            entry.refreshed_at = SystemTime::now();
        }
    }

    /// What is reachable from out here, as the port map everything above
    /// builds URLs from.
    ///
    /// Identity, and not a fudge: E2B publishes a port as a subdomain label
    /// rather than translating it, so the number out here is the number
    /// inside. Empty on a secure sandbox, because a URL that refuses the
    /// connection is worse than no URL.
    fn published(&self, ports: &[u16]) -> PortMap {
        match self.public_viewer {
            true => ports.iter().map(|port| (*port, *port)).collect(),
            false => PortMap::new(),
        }
    }
}

#[async_trait]
impl Machine for E2bMachine {
    fn runtime(&self) -> &str {
        "e2b"
    }

    async fn preflight(&self) -> Result<()> {
        self.api.available().await
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

    async fn start(&self, name: &str, config: &Config) -> Result<PortMap> {
        // A sandbox's image is a template somebody else built, so there is no
        // build to fold packages into and no way to say so afterwards.
        if !config.extras.is_empty() {
            return Err(Error::Unsupported {
                gaps: vec!["packages in a template this crate does not build"],
            });
        }

        // A sandbox has no entrypoint of its own, so an empty boot command is
        // a box that starts and never puts a screen in itself.
        if config.boot.is_empty() {
            return Err(Error::Unsupported {
                gaps: vec!["a command to bring the box up"],
            });
        }

        let sandbox = self.api.create(&self.plan_for(name, config)).await?;

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

        self.remember(name, &sandbox, config);
        self.reachable.set(sandbox.clone());

        let booted = match self
            .api
            .exec(&sandbox, &config.boot, &BTreeMap::new())
            .await
        {
            Ok(booted) => booted,
            Err(error) => {
                let _ = self.api.kill(&sandbox.id).await;
                return Err(error);
            }
        };

        if booted.code != 0 {
            let _ = self.api.kill(&sandbox.id).await;
            return Err(Error::Failed {
                code: booted.code,
                stderr: booted.stderr_utf8().trim().to_string(),
            });
        }

        Ok(self.published(&config.publish))
    }

    async fn running(&self, name: &str) -> Result<bool> {
        Ok(self.api.find(name).await?.is_some())
    }

    async fn ports(&self, name: &str) -> PortMap {
        self.recall(name)
            .map(|held| self.published(&held.published))
            .unwrap_or_default()
    }

    async fn env(&self, name: &str) -> BTreeMap<String, String> {
        // What this process asked for, where it was this process that asked. A
        // sandbox somebody else started answers with nothing rather than a
        // guess, and the descriptor falls back to what the profile claims.
        self.recall(name).map(|held| held.env).unwrap_or_default()
    }

    async fn exec(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        let sandbox = self.sandbox(name).await?;
        self.keep_alive(name).await;
        self.api.exec(&sandbox, argv, env).await
    }

    async fn read_file(&self, name: &str, path: &Path) -> Result<Vec<u8>> {
        let sandbox = self.sandbox(name).await?;
        self.api.read(&sandbox, &path.display().to_string()).await
    }

    async fn write_file(&self, name: &str, path: &Path, bytes: &[u8]) -> Result<()> {
        let sandbox = self.sandbox(name).await?;
        self.api
            .write(&sandbox, &path.display().to_string(), bytes)
            .await
    }

    async fn logs(&self, name: &str) -> Result<String> {
        let sandbox = self.sandbox(name).await?;
        self.api.logs(&sandbox.id).await
    }

    async fn stop(&self, name: &str) -> Result<()> {
        let sandbox = self.sandbox(name).await?;
        self.api.kill(&sandbox.id).await?;

        self.reachable.clear();
        if let Ok(mut held) = self.held.lock() {
            held.remove(name);
        }
        Ok(())
    }

    /// Sandboxes carrying this metadata key, keyed by the name this crate gave
    /// them rather than by the ID E2B did.
    ///
    /// A sweeper works from names, and an ID means nothing to it.
    async fn labelled(&self, label: &str) -> Result<Vec<(String, String)>> {
        let named: BTreeMap<String, String> =
            self.api.carrying(NAME_KEY).await?.into_iter().collect();

        Ok(self
            .api
            .carrying(label)
            .await?
            .into_iter()
            .filter_map(|(id, value)| Some((named.get(&id)?.clone(), value)))
            .collect())
    }

    fn sweepable(&self) -> bool {
        true
    }

    fn reaper(&self, name: &str) -> Option<(String, Vec<String>)> {
        self.api.reaper(&self.recall(name)?.sandbox.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle;
    use crate::testing::ScriptedE2b;

    fn machine(api: Arc<ScriptedE2b>) -> E2bMachine {
        E2bMachine::new(api, Arc::new(Reachable::new()))
    }

    fn config() -> Config {
        Config {
            image: "tmpl-abc".to_string(),
            publish: vec![6080, 6081],
            boot: vec!["computer-desktop".to_string(), "--once".to_string()],
            bundle: None,
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn test_a_bundled_image_is_refused_with_the_way_across() {
        let machine = machine(Arc::new(ScriptedE2b::new()));
        let error = machine
            .ensure_image(&Config::default())
            .await
            .expect_err("a container image is not a template");

        let message = error.to_string();
        assert!(message.contains("e2b template build"));
        assert!(message.contains(bundle::DESKTOP.name));
    }

    #[tokio::test]
    async fn test_a_template_somebody_else_built_is_left_alone() {
        let machine = machine(Arc::new(ScriptedE2b::new()));
        assert!(machine.ensure_image(&config()).await.is_ok());
    }

    #[tokio::test]
    async fn test_the_screen_is_started_because_a_sandbox_has_no_entrypoint() {
        let api = Arc::new(ScriptedE2b::new());
        let machine = machine(Arc::clone(&api));

        machine.start("box", &config()).await.expect("a sandbox");

        assert_eq!(
            api.commands(),
            vec![vec!["computer-desktop".to_string(), "--once".to_string()]],
            "--once, because nothing has to hold a sandbox open"
        );
    }

    #[tokio::test]
    async fn test_a_box_whose_screen_never_came_up_is_not_left_running() {
        let api = Arc::new(ScriptedE2b::new().failing(1, "no X server on :1"));
        let machine = machine(Arc::clone(&api));

        machine
            .start("box", &config())
            .await
            .expect_err("the boot failed");

        assert_eq!(api.killed().len(), 1, "a half-started sandbox is billed");
    }

    #[tokio::test]
    async fn test_a_secure_sandbox_publishes_no_viewer_url() {
        let machine = machine(Arc::new(ScriptedE2b::new()));
        let mapped = machine.start("box", &config()).await.expect("a sandbox");

        assert!(
            mapped.is_empty(),
            "a browser cannot send the header the proxy wants"
        );
    }

    #[tokio::test]
    async fn test_a_public_viewer_maps_every_port_to_itself() {
        let machine = machine(Arc::new(ScriptedE2b::new())).public_viewer(true);
        let mapped = machine.start("box", &config()).await.expect("a sandbox");

        assert_eq!(mapped.get(&6080), Some(&6080));
        assert_eq!(mapped.get(&6081), Some(&6081));
    }

    #[tokio::test]
    async fn test_the_name_travels_as_metadata_because_e2b_names_the_sandbox() {
        let api = Arc::new(ScriptedE2b::new());
        let machine = machine(Arc::clone(&api));

        machine.start("my-box", &config()).await.expect("a sandbox");

        let plan = api.plans().pop().expect("one plan");
        assert_eq!(plan.name, "my-box");
        assert!(machine.running("my-box").await.expect("a listing"));
    }

    #[tokio::test]
    async fn test_an_expiry_label_travels_so_a_sweeper_can_find_it() {
        let api = Arc::new(ScriptedE2b::new());
        let machine = machine(Arc::clone(&api));

        let config = Config {
            labels: BTreeMap::from([(crate::EXPIRY_LABEL.to_string(), "1780000000".to_string())]),
            ..config()
        };
        machine.start("box", &config).await.expect("a sandbox");

        assert!(machine.sweepable());
        assert_eq!(
            machine
                .labelled(crate::EXPIRY_LABEL)
                .await
                .expect("a sweep"),
            vec![("box".to_string(), "1780000000".to_string())],
            "a sweeper works from names, and the ID means nothing to it"
        );
    }

    #[tokio::test]
    async fn test_extra_packages_are_refused_rather_than_dropped() {
        let machine = machine(Arc::new(ScriptedE2b::new()));
        let config = Config {
            extras: bundle::Extras::wide_fonts(),
            ..config()
        };

        assert!(
            machine.start("box", &config).await.is_err(),
            "a template this crate did not build got none of them"
        );
    }

    #[tokio::test]
    async fn test_a_box_this_process_never_started_is_found_by_metadata() {
        let api = Arc::new(ScriptedE2b::new().holding("left-over", "sbx-9"));
        let machine = machine(Arc::clone(&api));

        machine
            .exec("left-over", &["true".to_string()], &BTreeMap::new())
            .await
            .expect("it is reachable");

        assert_eq!(api.found(), vec!["left-over".to_string()]);
    }

    #[tokio::test]
    async fn test_a_box_that_is_gone_says_so_rather_than_timing_out() {
        let machine = machine(Arc::new(ScriptedE2b::new()));
        let error = machine
            .exec("absent", &["true".to_string()], &BTreeMap::new())
            .await
            .expect_err("nothing to run it on");

        assert!(error.needs_another_place());
    }

    #[tokio::test]
    async fn test_the_deadline_is_not_pushed_out_on_every_command() {
        let api = Arc::new(ScriptedE2b::new());
        let machine = machine(Arc::clone(&api)).expiring_after(Duration::from_secs(600));
        machine.start("box", &config()).await.expect("a sandbox");

        for _ in 0..5 {
            machine
                .exec("box", &["true".to_string()], &BTreeMap::new())
                .await
                .expect("it runs");
        }

        assert!(
            api.refreshes().is_empty(),
            "a round trip per click buys nothing on a fresh deadline"
        );
    }

    #[tokio::test]
    async fn test_a_deadline_past_its_half_life_is_pushed_out() {
        let api = Arc::new(ScriptedE2b::new());
        let machine = machine(Arc::clone(&api)).expiring_after(Duration::from_millis(20));
        machine.start("box", &config()).await.expect("a sandbox");

        tokio::time::sleep(Duration::from_millis(30)).await;
        machine
            .exec("box", &["true".to_string()], &BTreeMap::new())
            .await
            .expect("it runs");

        assert_eq!(api.refreshes().len(), 1, "a box in use does not expire");
    }
}
