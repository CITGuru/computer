//! A box in somebody else's cloud, whichever cloud that is.
//!
//! [`Machine`] is the only thing that knows where a box is, so the driver, the
//! screens, the takeover gate and the descriptor above this are the same code
//! a container runs.
//!
//! Four things are this module's own rather than any vendor's. A sandbox is
//! found by metadata, because the vendor assigns the ID. The screen is brought
//! up with an explicit boot command, because a sandbox has no entrypoint. The
//! deadline is pushed out lazily from [`Machine::exec`], because every call
//! reaches the box through there and refreshing on each one would put a round
//! trip in front of every click. And the viewer URL is withheld by default,
//! for the reason [`RemoteMachine::public_viewer`] gives.

use super::api::{DEFAULT_TTL, NAME_KEY, RemoteApi, Sandbox, SandboxPlan};
use super::profile::Remote;
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
    ttl: Duration,
    refreshed_at: SystemTime,
}

/// The generic half of every cloud sandbox.
///
/// Built with [`super::pair`], which ties it to the profile that formats the
/// viewer URL.
pub struct RemoteMachine {
    api: Arc<dyn RemoteApi>,
    /// Shared with that profile: a sandbox's host contains an ID assigned
    /// after the profile was built.
    remote: Arc<Remote>,
    held: Mutex<BTreeMap<String, Held>>,
    ttl: Duration,
    public_viewer: bool,
}

impl RemoteMachine {
    pub fn new(api: Arc<dyn RemoteApi>, remote: Arc<Remote>) -> Self {
        Self {
            api,
            remote,
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
    /// **Off is not privacy, and saying so matters.** Every [`Machine`] on
    /// this host publishes on loopback, where the bind is the whole
    /// authentication story. A sandbox's ports are published by the vendor, at
    /// an address the vendor chose, so that address answers whether or not
    /// anything here prints it.
    ///
    /// What this setting decides is only whether the URL is handed to a
    /// caller. Off, the viewer ports are left out of the port map and
    /// [`crate::Computer::viewer_url`] answers `None`; the desktop is still
    /// driveable from here, and the URL still exists.
    ///
    /// On, [`Machine::reach`] says [`Routable`](crate::Reach::Routable), so a
    /// launch with an open viewer is refused the same way `Bind::Any` is.
    /// Whether the vendor's own proxy gates that URL is the vendor's decision
    /// and not this one — check it rather than assume it.
    pub fn public_viewer(mut self, public: bool) -> Self {
        self.public_viewer = public;
        self
    }

    pub fn api(&self) -> &Arc<dyn RemoteApi> {
        &self.api
    }

    fn plan_for(&self, name: &str, config: &Config) -> SandboxPlan {
        let mut metadata = config.labels.clone();
        // The vendor assigns the ID, so the caller's name has to be somewhere
        // a listing can be filtered by or `running` and a sweep have nothing
        // to match on.
        metadata.insert(NAME_KEY.to_string(), name.to_string());

        SandboxPlan {
            name: name.to_string(),
            image: config.image.clone(),
            publish: config.publish.clone(),
            env: config.env.clone(),
            metadata,
            network: config.network,
            ttl: self.ttl,
        }
    }

    fn remember(&self, name: &str, sandbox: &Sandbox, env: BTreeMap<String, String>) {
        if let Ok(mut held) = self.held.lock() {
            held.insert(
                name.to_string(),
                Held {
                    sandbox: sandbox.clone(),
                    env,
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

        self.remote.set(found.clone());
        self.remember(name, &found, BTreeMap::new());
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
    /// Identity, and not a fudge: these vendors publish a port at an address
    /// of their own rather than translating it, so the number out here is the
    /// number inside. Empty where the viewer is withheld, because a URL the
    /// caller is not meant to have is worse than no URL.
    fn published(&self, sandbox: &Sandbox) -> PortMap {
        match self.public_viewer {
            true => sandbox
                .endpoints
                .keys()
                .map(|port| (*port, *port))
                .collect(),
            false => PortMap::new(),
        }
    }
}

#[async_trait]
impl Machine for RemoteMachine {
    fn runtime(&self) -> &str {
        self.api.vendor()
    }

    async fn preflight(&self) -> Result<()> {
        self.api.available().await
    }

    /// A bind address means nothing here: the vendor publishes a port at an
    /// address of its own, so [`Config::bind`] is never consulted and a rule
    /// phrased as "loopback or not" would miss this machine entirely.
    ///
    /// `public_viewer` is the whole of the question, and it lines up with what
    /// [`crate::Reach`] claims — not that nobody else can connect, but that
    /// this crate did not hand out the address.
    fn reach(&self, _config: &Config) -> crate::Reach {
        match self.public_viewer {
            true => crate::Reach::Routable,
            false => crate::Reach::Loopback,
        }
    }

    async fn ensure_image(&self, config: &Config) -> Result<()> {
        self.api.ensure_image(config).await
    }

    async fn start(&self, name: &str, config: &Config) -> Result<PortMap> {
        // A sandbox's image was built by somebody else, so there is no build
        // to fold packages into and no way to say so afterwards.
        if !config.extras.is_empty() {
            return Err(Error::Unsupported {
                gaps: vec!["packages in an image this crate does not build"],
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

        self.remember(name, &sandbox, config.env.clone());
        self.remote.set(sandbox.clone());

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

        Ok(self.published(&sandbox))
    }

    async fn running(&self, name: &str) -> Result<bool> {
        Ok(self.api.find(name).await?.is_some())
    }

    async fn ports(&self, name: &str) -> PortMap {
        self.recall(name)
            .map(|held| self.published(&held.sandbox))
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
        // The runtimes on this host make it, so a caller who writes into a
        // fresh directory has to get the same answer here.
        if let Some(parent) = path.parent() {
            let _ = self
                .exec(
                    name,
                    &[
                        "mkdir".to_string(),
                        "-p".to_string(),
                        parent.display().to_string(),
                    ],
                    &BTreeMap::new(),
                )
                .await;
        }
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

        self.remote.clear();
        if let Ok(mut held) = self.held.lock() {
            held.remove(name);
        }
        Ok(())
    }

    /// Sandboxes carrying this metadata key, keyed by the name this crate gave
    /// them rather than by the ID the vendor did.
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
        self.api.sweepable()
    }

    fn reaper(&self, name: &str) -> Option<(String, Vec<String>)> {
        self.api.reaper(&self.recall(name)?.sandbox.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle;
    use crate::testing::ScriptedRemote;

    fn machine(api: Arc<ScriptedRemote>) -> RemoteMachine {
        RemoteMachine::new(api, Arc::new(Remote::new()))
    }

    fn config() -> Config {
        Config {
            image: "snapshot-abc".to_string(),
            publish: vec![6080, 6081],
            boot: vec!["computer-desktop".to_string(), "--once".to_string()],
            bundle: None,
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn test_a_bundled_image_is_refused_with_the_way_across() {
        let machine = machine(Arc::new(ScriptedRemote::new()));

        let error = machine
            .ensure_image(&Config {
                image: bundle::DESKTOP.tag(),
                ..Config::default()
            })
            .await
            .expect_err("a container image is not what a vendor runs");

        assert!(error.to_string().contains("Bundle::materialize"));
    }

    #[tokio::test]
    async fn test_the_name_travels_as_metadata() {
        let api = Arc::new(ScriptedRemote::new());
        machine(Arc::clone(&api))
            .start("desk-1", &config())
            .await
            .expect("started");

        let plan = api.plans().pop().expect("one plan");
        assert_eq!(plan.metadata.get(NAME_KEY), Some(&"desk-1".to_string()));
        assert_eq!(plan.publish, vec![6080, 6081]);
    }

    #[tokio::test]
    async fn test_the_box_is_booted_because_a_sandbox_has_no_entrypoint() {
        let api = Arc::new(ScriptedRemote::new());
        machine(Arc::clone(&api))
            .start("desk-1", &config())
            .await
            .expect("started");

        assert_eq!(
            api.commands(),
            vec![vec!["computer-desktop".to_string(), "--once".to_string()]]
        );
    }

    #[tokio::test]
    async fn test_a_boot_that_fails_takes_the_sandbox_with_it() {
        let api = Arc::new(ScriptedRemote::new().failing(1, "no display"));

        let error = machine(Arc::clone(&api))
            .start("desk-1", &config())
            .await
            .expect_err("the screen never came up");

        assert!(matches!(error, Error::Failed { code: 1, .. }));
        assert_eq!(
            api.killed().len(),
            1,
            "a sandbox nothing can drive is one that has to be paid for"
        );
    }

    #[tokio::test]
    async fn test_a_box_with_no_boot_command_is_refused() {
        let machine = machine(Arc::new(ScriptedRemote::new()));

        let error = machine
            .start(
                "desk-1",
                &Config {
                    boot: Vec::new(),
                    ..config()
                },
            )
            .await
            .expect_err("nothing would put a screen in it");

        assert!(matches!(error, Error::Unsupported { .. }));
    }

    #[tokio::test]
    async fn test_the_viewer_is_withheld_until_it_is_asked_for() {
        let private = machine(Arc::new(ScriptedRemote::new()));
        assert!(
            private.start("desk-1", &config()).await.unwrap().is_empty(),
            "the URL exists; this crate does not hand it over"
        );

        let public = machine(Arc::new(ScriptedRemote::new())).public_viewer(true);
        let mapped = public.start("desk-1", &config()).await.unwrap();

        assert_eq!(mapped.get(&6080), Some(&6080), "published, not translated");
        assert_eq!(mapped.get(&6081), Some(&6081));
    }

    #[tokio::test]
    async fn test_a_public_viewer_is_routable_and_a_private_one_is_not() {
        let config = config();

        assert_eq!(
            machine(Arc::new(ScriptedRemote::new())).reach(&config),
            crate::Reach::Loopback
        );
        assert_eq!(
            machine(Arc::new(ScriptedRemote::new()))
                .public_viewer(true)
                .reach(&config),
            crate::Reach::Routable
        );
    }

    #[tokio::test]
    async fn test_a_box_this_process_started_is_not_looked_up_again() {
        let api = Arc::new(ScriptedRemote::new());
        let machine = machine(Arc::clone(&api));
        machine.start("desk-1", &config()).await.expect("started");

        machine
            .exec("desk-1", &["true".to_string()], &BTreeMap::new())
            .await
            .expect("ran");

        assert!(
            api.found().is_empty(),
            "a round trip per command is what holding it avoids"
        );
    }

    #[tokio::test]
    async fn test_a_box_from_another_process_is_found_by_name() {
        let api = Arc::new(ScriptedRemote::new().holding("desk-1", "sbx-9"));
        let machine = machine(Arc::clone(&api));

        machine
            .exec("desk-1", &["true".to_string()], &BTreeMap::new())
            .await
            .expect("ran");

        assert_eq!(api.found(), vec!["desk-1".to_string()]);
    }

    #[tokio::test]
    async fn test_a_box_that_is_gone_says_so() {
        let machine = machine(Arc::new(ScriptedRemote::new()));

        let error = machine
            .exec("desk-1", &["true".to_string()], &BTreeMap::new())
            .await
            .expect_err("nothing by that name");

        assert!(matches!(error, Error::Gone(_)));
    }

    #[tokio::test]
    async fn test_the_deadline_is_not_pushed_out_on_every_command() {
        let api = Arc::new(ScriptedRemote::new());
        let machine = machine(Arc::clone(&api)).expiring_after(Duration::from_secs(600));
        machine.start("desk-1", &config()).await.expect("started");

        for _ in 0..5 {
            machine
                .exec("desk-1", &["true".to_string()], &BTreeMap::new())
                .await
                .expect("ran");
        }

        assert!(
            api.refreshes().is_empty(),
            "a click should not cost a control-plane call"
        );
    }

    #[tokio::test]
    async fn test_the_deadline_is_pushed_out_once_it_is_half_gone() {
        let api = Arc::new(ScriptedRemote::new());
        let machine = machine(Arc::clone(&api)).expiring_after(Duration::from_millis(20));
        machine.start("desk-1", &config()).await.expect("started");

        tokio::time::sleep(Duration::from_millis(30)).await;
        machine
            .exec("desk-1", &["true".to_string()], &BTreeMap::new())
            .await
            .expect("ran");

        assert_eq!(api.refreshes(), vec!["sbx-0".to_string()]);
    }

    #[tokio::test]
    async fn test_a_sweep_reads_names_and_not_ids() {
        let api = Arc::new(ScriptedRemote::new().holding("desk-1", "sbx-9"));
        api.metadata("sbx-9", "computer.expires", "1700000000");

        let found = machine(api).labelled("computer.expires").await.unwrap();

        assert_eq!(
            found,
            vec![("desk-1".to_string(), "1700000000".to_string())],
            "a sweeper removes a box by the name it was given"
        );
    }

    #[tokio::test]
    async fn test_a_vendor_that_lists_nothing_is_not_swept() {
        assert!(
            !machine(Arc::new(ScriptedRemote::new().unlistable())).sweepable(),
            "sweeping a vendor that answers nothing would report every box as gone"
        );
    }

    #[tokio::test]
    async fn test_stopping_takes_the_box_and_the_address_with_it() {
        let api = Arc::new(ScriptedRemote::new());
        let remote = Arc::new(Remote::new());
        let machine =
            RemoteMachine::new(Arc::clone(&api) as Arc<dyn RemoteApi>, Arc::clone(&remote));

        machine.start("desk-1", &config()).await.expect("started");
        machine.stop("desk-1").await.expect("stopped");

        assert_eq!(api.killed(), vec!["sbx-0".to_string()]);
        assert!(remote.get().is_none(), "the URL outlives nothing");
    }

    #[tokio::test]
    async fn test_a_written_file_gets_the_directory_it_needs() {
        let api = Arc::new(ScriptedRemote::new());
        let machine = machine(Arc::clone(&api));
        machine.start("desk-1", &config()).await.expect("started");

        machine
            .write_file("desk-1", Path::new("/tmp/deep/one.txt"), b"x")
            .await
            .expect("written");

        assert!(
            api.commands()
                .contains(&vec!["mkdir".into(), "-p".into(), "/tmp/deep".into()]),
            "the runtimes on this host make it, so this one has to"
        );
        assert_eq!(api.written("/tmp/deep/one.txt"), Some(b"x".to_vec()));
    }
}
