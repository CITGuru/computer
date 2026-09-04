//! Where a box comes from, and how it is reached.
//!
//! [`Machine`] starts a box, publishes its ports, runs commands in it, moves
//! files in and out, and takes it away. [`ScreenHost`] is the other half of the
//! coupling: one command against one display.
//!
//! [`DockerMachine`] runs containers through `docker`, `podman` or `nerdctl`.
//! [`MicroVm`](crate::microvm::MicroVm) runs microVMs. Everything above them is
//! written once.

use crate::ScreenId;
use crate::bundle;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::profile::{PROFILE_LABEL, Profile};
use crate::runtime::{Config, ContainerCli, parse_ports, run_args};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Container port to host port, for everything the box published.
pub type PortMap = BTreeMap<u16, u16>;

/// A place that can hold a desktop.
#[async_trait]
pub trait Machine: Send + Sync {
    /// What to call this in an error message.
    fn runtime(&self) -> &str;

    /// Whether the runtime answers at all.
    ///
    /// Asked before anything is created, so a runtime that is down reports
    /// itself rather than a box that would not start.
    async fn preflight(&self) -> Result<()>;

    /// Make sure the image exists, building or fetching it if it does not.
    ///
    /// Takes the whole configuration: what is installed into an image is part
    /// of which image it is.
    async fn ensure_image(&self, config: &Config) -> Result<()>;

    /// Which contract this image says it implements, if it says.
    ///
    /// Read from the image's labels before anything starts, so an image and the
    /// profile driving it can be checked against each other. `None` where the
    /// runtime cannot be asked.
    async fn image_contract(&self, _image: &str) -> Option<String> {
        None
    }

    /// Start a box, and report what its ports were mapped to.
    async fn start(&self, name: &str, config: &Config) -> Result<PortMap>;

    async fn running(&self, name: &str) -> Result<bool>;

    /// The mapping again, for a box this process did not start.
    ///
    /// Empty where nothing was published, which makes every URL `None`.
    async fn ports(&self, name: &str) -> PortMap;

    /// The environment the box was started with, which is where the screen
    /// size is recorded.
    async fn env(&self, name: &str) -> BTreeMap<String, String>;

    /// Run a command with this environment set.
    ///
    /// An environment rather than a screen: which variables a screen needs is
    /// the image's answer, not this trait's.
    async fn exec(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult>;

    async fn read_file(&self, name: &str, path: &Path) -> Result<Vec<u8>>;
    async fn write_file(&self, name: &str, path: &Path, bytes: &[u8]) -> Result<()>;

    /// A whole file in, without holding it in memory.
    ///
    /// The default reads it into this process. A runtime that can move bytes
    /// directly should override this.
    async fn upload(&self, name: &str, from: &Path, to: &Path) -> Result<()> {
        let bytes = tokio::fs::read(from)
            .await
            .map_err(|error| Error::denied(format!("{}: {error}", from.display())))?;
        self.write_file(name, to, &bytes).await
    }

    /// A whole file out, without holding it in memory.
    async fn download(&self, name: &str, from: &Path, to: &Path) -> Result<()> {
        let bytes = self.read_file(name, from).await?;
        tokio::fs::write(to, bytes)
            .await
            .map_err(|error| Error::denied(format!("{}: {error}", to.display())))
    }

    /// What the box itself has said, which is where a screen that never came
    /// up explains itself.
    async fn logs(&self, name: &str) -> Result<String>;

    async fn stop(&self, name: &str) -> Result<()>;

    /// Every box this runtime holds that carries the label, and its value.
    ///
    /// The default is empty, and [`Machine::sweepable`] says which that means.
    async fn labelled(&self, _label: &str) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    /// Whether what this publishes can be reached beyond this host.
    ///
    /// Asked of the `Machine` because it is the only thing that knows: a bind
    /// address is a container idea, and a sandbox that publishes a hostname per
    /// port has no host side at all. The default is the safe answer, so an
    /// implementation that has not thought about it is not treated as though it
    /// had.
    fn reach(&self, config: &Config) -> crate::Reach {
        config.bind.reach()
    }

    /// Whether this runtime can be asked what it holds.
    fn sweepable(&self) -> bool {
        false
    }

    /// A command that takes the box away with no async runtime in the room.
    ///
    /// `Drop` cannot await. `None` means a dropped handle leaks the box.
    fn reaper(&self, name: &str) -> Option<(String, Vec<String>)>;
}

/// Something that can run a command against one screen.
///
/// The whole coupling between a driver and whatever holds the desktop: a
/// container answers it with `docker exec`, a test from a script.
#[async_trait]
pub trait ScreenHost: Send + Sync {
    async fn run(&self, argv: &[String], screen: ScreenId) -> Result<ExecResult>;
}

/// How long any one command may take before it is given up on.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// One box, as something a driver can drive.
///
/// A command, which display it goes to, and how long it may take.
pub struct MachineHost {
    machine: Arc<dyn Machine>,
    /// The only place a screen becomes an environment. Below here nothing
    /// knows that screens exist.
    profile: Arc<dyn Profile>,
    name: String,
    timeout: Duration,
    /// When something last ran in this box, as nanoseconds since the epoch.
    ///
    /// Everything reaches the box through here: a driver call, an exec, a copy.
    active_at: Arc<std::sync::atomic::AtomicU64>,
    /// The host a person is told to use, and the scheme to reach it with.
    ///
    /// Kept beside the box rather than worked out at each call: a URL built
    /// from the bind is right only for loopback, and by the time a `Screen`
    /// wants one the configuration that knew better is gone.
    advertised: (crate::Scheme, String),
    /// The gate in front of this box's viewers, and what opens it.
    ///
    /// Held here because a `Screen` builds URLs and a screen created long
    /// after launch has to build the same ones as the first.
    gate: (crate::Auth, Option<crate::Credentials>),
}

impl MachineHost {
    pub fn new(
        machine: Arc<dyn Machine>,
        profile: Arc<dyn Profile>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            machine,
            profile,
            name: name.into(),
            timeout: DEFAULT_TIMEOUT,
            active_at: Arc::new(std::sync::atomic::AtomicU64::new(now_nanos())),
            advertised: (crate::Scheme::Http, "127.0.0.1".to_string()),
            gate: (crate::Auth::Open, None),
        }
    }

    /// What this box's viewers ask of whoever connects.
    pub fn gated_by(mut self, auth: crate::Auth, credentials: Option<crate::Credentials>) -> Self {
        self.gate = (auth, credentials);
        self
    }

    /// What this box's viewers ask, and what opens them.
    ///
    /// The credentials are how a caller tells a person the password under
    /// [`crate::Auth::Password`], where by design no URL carries it.
    pub fn gate(&self) -> (crate::Auth, Option<&crate::Credentials>) {
        (self.gate.0, self.gate.1.as_ref())
    }

    /// The credential a read-only URL carries, where the gate puts it there.
    ///
    /// `None` for an open box and for a browser prompt: a password in a URL is
    /// the one shape [`crate::Auth::Password`] exists to avoid.
    pub fn view_ticket(&self) -> Option<&crate::Secret> {
        self.ticket(|pair| &pair.view)
    }

    /// The credential a control URL carries. See [`MachineHost::view_ticket`].
    pub fn control_ticket(&self) -> Option<&crate::Secret> {
        self.ticket(|pair| &pair.control)
    }

    fn ticket(
        &self,
        door: impl Fn(&crate::Credentials) -> &crate::Secret,
    ) -> Option<&crate::Secret> {
        match self.gate.0.is_in_the_url() {
            true => self.gate.1.as_ref().map(door),
            false => None,
        }
    }

    /// Where the ports this box published are reached from.
    pub fn advertised_at(mut self, scheme: crate::Scheme, host: impl Into<String>) -> Self {
        self.advertised = (scheme, host.into());
        self
    }

    /// A published port, as a person is told it.
    pub fn address(&self, port: u16) -> crate::Address {
        crate::Address {
            scheme: self.advertised.0,
            host: self.advertised.1.clone(),
            port,
        }
    }

    /// When something last ran in this box.
    pub fn active_at(&self) -> Arc<std::sync::atomic::AtomicU64> {
        Arc::clone(&self.active_at)
    }

    /// How long the box has had nothing asked of it.
    pub fn idle_for(&self) -> Duration {
        let last = self.active_at.load(std::sync::atomic::Ordering::Relaxed);
        Duration::from_nanos(now_nanos().saturating_sub(last))
    }

    /// Count this moment as activity, for work that did not go through here.
    pub fn touch(&self) {
        self.active_at
            .store(now_nanos(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Give every command through this host a different ceiling.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Run something, and give up on it after `within`.
    ///
    /// A command that ran out of time is reported as one rather than as exit
    /// 124, which a program can also choose to exit with.
    pub async fn run_within(
        &self,
        argv: &[String],
        env: &BTreeMap<String, String>,
        within: Duration,
    ) -> Result<ExecResult> {
        self.touch();

        match tokio::time::timeout(within, self.machine.exec(&self.name, argv, env)).await {
            Ok(result) => result,
            Err(_) => Ok(ExecResult {
                code: 124,
                timed_out: true,
                ..ExecResult::default()
            }),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn machine(&self) -> &Arc<dyn Machine> {
        &self.machine
    }

    pub fn profile(&self) -> &Arc<dyn Profile> {
        &self.profile
    }

    /// Run something with no display attached.
    pub async fn exec(&self, argv: &[String]) -> Result<ExecResult> {
        self.run_within(argv, &BTreeMap::new(), self.timeout).await
    }
}

#[async_trait]
impl ScreenHost for MachineHost {
    async fn run(&self, argv: &[String], screen: ScreenId) -> Result<ExecResult> {
        let env = self.profile.screen_env(screen);
        self.run_within(argv, &env, self.timeout).await
    }
}

/// Take away a scratch file a transfer went through.
///
/// Best effort: the transfer already succeeded or failed on its own terms, and
/// a file left behind changes neither. It is still said out loud, because
/// enough of them left behind is a disk that fills for a reason nothing names.
async fn discard(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::debug!(file = %path.display(), %error, "a scratch file stayed behind");
    }
}

fn arg(value: impl Into<String>) -> String {
    value.into()
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0)
}

/// A container, through `docker`, `podman` or `nerdctl`.
pub struct DockerMachine {
    cli: Arc<dyn ContainerCli>,
}

impl Default for DockerMachine {
    fn default() -> Self {
        Self::new(Arc::new(crate::runtime::SystemDocker::default()))
    }
}

impl DockerMachine {
    pub fn new(cli: Arc<dyn ContainerCli>) -> Self {
        Self { cli }
    }

    pub fn cli(&self) -> &Arc<dyn ContainerCli> {
        &self.cli
    }

    /// Make the directory a write is about to land in.
    ///
    /// `cp` refuses a target whose parent is missing, so without this `upload`
    /// and `write_file` disagree about the same path. The outcome is not
    /// checked: the copy is the real test, and it reports the failure with the
    /// runtime's own words.
    async fn ensure_parent(&self, name: &str, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = self
                .exec(
                    name,
                    &[arg("mkdir"), arg("-p"), arg(parent.display().to_string())],
                    &BTreeMap::new(),
                )
                .await;
        }
    }

    /// `docker cp`, rather than an encoding round trip: base64 flags differ
    /// between coreutils and BusyBox, and an argument list has a size ceiling
    /// that a screenshot walks straight through.
    async fn copy(&self, from: &str, to: &str) -> Result<()> {
        let result = self.cli.run(&[arg("cp"), arg(from), arg(to)]).await?;

        if result.code != 0 {
            return Err(Error::denied(result.stderr_utf8().trim().to_string()));
        }
        Ok(())
    }

    /// A staging path no other call will pick.
    ///
    /// `docker cp` moves bytes host-side through a file this process names, so
    /// the name has to be unique per call: two reads of one box that shared it
    /// deleted each other's bytes between the copy and the read, and every
    /// concurrent pair failed.
    ///
    /// It lives in a directory this process owns at `0700`, because `cp`
    /// writes wherever the path leads — and in a shared `/tmp`, a path chosen
    /// before we look is a path somebody else can make a symlink.
    fn scratch(&self, name: &str, tag: &str) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let ticket = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let home = std::env::temp_dir().join(format!("computer-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&home);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700));
        }
        home.join(format!("{name}-{tag}-{ticket}"))
    }
}

#[async_trait]
impl Machine for DockerMachine {
    fn runtime(&self) -> &str {
        self.cli.program()
    }

    async fn preflight(&self) -> Result<()> {
        let alive = self.cli.run(&[arg("version")]).await?;
        if alive.code != 0 {
            return Err(Error::Unavailable {
                runtime: self.runtime().to_string(),
                detail: alive.stderr_utf8().trim().to_string(),
            });
        }
        Ok(())
    }

    async fn ensure_image(&self, config: &Config) -> Result<()> {
        bundle::ensure_source(
            self.cli.as_ref(),
            &config.image,
            &config.extras,
            config.bundle.as_ref(),
            config.image_dir.as_deref(),
        )
        .await
    }

    async fn image_contract(&self, image: &str) -> Option<String> {
        let said = self
            .cli
            .run(&[
                arg("image"),
                arg("inspect"),
                arg("--format"),
                arg(format!("{{{{index .Config.Labels \"{PROFILE_LABEL}\"}}}}")),
                arg(image),
            ])
            .await
            .ok()?;

        // `inspect` prints an empty line for a label the image does not carry,
        // and "<no value>" where it carries none at all.
        let declared = said.stdout_utf8().trim().to_string();
        (said.code == 0 && !declared.is_empty() && declared != "<no value>").then_some(declared)
    }

    async fn start(&self, name: &str, config: &Config) -> Result<PortMap> {
        let started = self.cli.run(&run_args(name, config)).await?;
        if started.code != 0 {
            return Err(Error::Unavailable {
                runtime: self.runtime().to_string(),
                detail: started.stderr_utf8().trim().to_string(),
            });
        }
        Ok(self.ports(name).await)
    }

    async fn running(&self, name: &str) -> Result<bool> {
        let state = self
            .cli
            .run(&[
                arg("inspect"),
                arg("--format"),
                arg("{{.State.Running}}"),
                arg(name),
            ])
            .await?;
        Ok(state.stdout_utf8().trim() == "true")
    }

    async fn ports(&self, name: &str) -> PortMap {
        match self.cli.run(&[arg("port"), arg(name)]).await {
            Ok(result) if result.code == 0 => parse_ports(&result.stdout_utf8()),
            _ => PortMap::new(),
        }
    }

    async fn env(&self, name: &str) -> BTreeMap<String, String> {
        let Ok(result) = self
            .cli
            .run(&[
                arg("inspect"),
                arg("--format"),
                arg("{{range .Config.Env}}{{println .}}{{end}}"),
                arg(name),
            ])
            .await
        else {
            return BTreeMap::new();
        };

        result
            .stdout_utf8()
            .lines()
            .filter_map(|line| line.trim().split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    async fn exec(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        let mut args = vec![arg("exec")];
        for (key, value) in env {
            args.push(arg("--env"));
            args.push(format!("{key}={value}"));
        }
        args.push(arg(name));
        args.extend(argv.iter().cloned());
        self.cli.run(&args).await
    }

    async fn read_file(&self, name: &str, path: &Path) -> Result<Vec<u8>> {
        let local = self.scratch(name, "read");
        let _ = tokio::fs::remove_file(&local).await;

        self.copy(
            &format!("{}:{}", name, path.display()),
            &local.display().to_string(),
        )
        .await?;

        let bytes = tokio::fs::read(&local)
            .await
            .map_err(|error| Error::transport(error.to_string(), false))?;
        discard(&local).await;
        Ok(bytes)
    }

    async fn write_file(&self, name: &str, path: &Path, bytes: &[u8]) -> Result<()> {
        let local = self.scratch(name, "write");
        tokio::fs::write(&local, bytes)
            .await
            .map_err(|error| Error::transport(error.to_string(), false))?;

        self.ensure_parent(name, path).await;

        let outcome = self
            .copy(
                &local.display().to_string(),
                &format!("{}:{}", name, path.display()),
            )
            .await;
        discard(&local).await;
        outcome
    }

    async fn upload(&self, name: &str, from: &Path, to: &Path) -> Result<()> {
        // Disk to disk: `docker cp` streams, so nothing is held in memory.
        self.ensure_parent(name, to).await;
        self.copy(
            &from.display().to_string(),
            &format!("{}:{}", name, to.display()),
        )
        .await
    }

    async fn download(&self, name: &str, from: &Path, to: &Path) -> Result<()> {
        self.copy(
            &format!("{}:{}", name, from.display()),
            &to.display().to_string(),
        )
        .await
    }

    async fn logs(&self, name: &str) -> Result<String> {
        let result = self.cli.run(&[arg("logs"), arg(name)]).await?;
        Ok(format!("{}{}", result.stdout_utf8(), result.stderr_utf8()))
    }

    async fn stop(&self, name: &str) -> Result<()> {
        let result = self
            .cli
            .run(&[arg("rm"), arg("--force"), arg("--volumes"), arg(name)])
            .await?;

        if result.code != 0 {
            return Err(Error::transport(
                result.stderr_utf8().trim().to_string(),
                true,
            ));
        }
        Ok(())
    }

    async fn labelled(&self, label: &str) -> Result<Vec<(String, String)>> {
        let listed = self
            .cli
            .run(&[
                arg("ps"),
                arg("--all"),
                arg("--filter"),
                format!("label={label}"),
                arg("--format"),
                format!("{{{{.Names}}}}\t{{{{.Label \"{label}\"}}}}"),
            ])
            .await?;

        Ok(listed
            .stdout_utf8()
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
            .filter(|(name, value)| !name.is_empty() && !value.is_empty())
            .collect())
    }

    fn sweepable(&self) -> bool {
        true
    }

    fn reaper(&self, name: &str) -> Option<(String, Vec<String>)> {
        Some((
            self.cli.program().to_string(),
            vec![arg("rm"), arg("--force"), arg("--volumes"), arg(name)],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ScriptedCli;

    fn docker(cli: Arc<ScriptedCli>) -> DockerMachine {
        DockerMachine::new(cli as Arc<dyn ContainerCli>)
    }

    #[tokio::test]
    async fn test_a_command_carries_the_environment_it_was_given() {
        let cli = Arc::new(ScriptedCli::new());
        let machine = docker(Arc::clone(&cli));
        let env = BTreeMap::from([("DISPLAY".to_string(), ":3".to_string())]);

        machine
            .exec("box", &[arg("xdotool"), arg("key"), arg("a")], &env)
            .await
            .expect("a command");

        let sent = cli.last().expect("a call");
        assert_eq!(
            sent[..4],
            ["exec", "--env", "DISPLAY=:3", "box"],
            "the runtime moves what it was handed; which variable a screen \
             needs was decided above it"
        );
    }

    #[tokio::test]
    async fn test_a_command_with_no_environment_sets_nothing() {
        let cli = Arc::new(ScriptedCli::new());
        let machine = docker(Arc::clone(&cli));

        machine
            .exec("box", &[arg("ls")], &BTreeMap::new())
            .await
            .expect("a command");

        let sent = cli.last().expect("a call");
        assert!(
            !sent.iter().any(|part| part.starts_with("DISPLAY=")),
            "a command that needs no screen must not pick one"
        );
    }

    /// Two reads of one box used to stage through one path, so the first
    /// call's `discard` deleted the second's bytes between the copy and the
    /// read. Every concurrent pair failed.
    #[tokio::test]
    async fn test_two_reads_of_one_box_do_not_stage_through_one_path() {
        let cli = Arc::new(ScriptedCli::new());
        let machine = docker(Arc::clone(&cli));

        let _ = machine.read_file("box", Path::new("/tmp/one")).await;
        let _ = machine.read_file("box", Path::new("/tmp/two")).await;

        let staged: Vec<String> = cli
            .calls()
            .into_iter()
            .filter(|argv| argv.first().map(String::as_str) == Some("cp"))
            .filter_map(|argv| argv.get(2).cloned())
            .collect();

        assert_eq!(staged.len(), 2, "both reads copied");
        assert_ne!(
            staged[0], staged[1],
            "one staging path for two reads is bytes deleted under a caller"
        );
    }

    /// `cp` refuses a target whose parent is missing, so an `upload` that did
    /// not make it disagreed with `write_file` about the same path.
    #[tokio::test]
    async fn test_upload_makes_the_directory_write_file_would_have_made() {
        let cli = Arc::new(ScriptedCli::new());
        let machine = docker(Arc::clone(&cli));

        let _ = machine
            .upload("box", Path::new("/dev/null"), Path::new("/tmp/made/here"))
            .await;

        let made = cli.calls().into_iter().any(|argv| {
            argv.contains(&"mkdir".to_string()) && argv.contains(&"/tmp/made".to_string())
        });
        assert!(made, "upload never made the directory it copies into");
    }

    #[tokio::test]
    async fn test_a_dropped_handle_has_a_command_that_needs_no_runtime() {
        let machine = docker(Arc::new(ScriptedCli::new()));
        let (program, args) = machine.reaper("box").expect("a reaper");

        assert_eq!(program, "docker");
        assert!(args.contains(&"--force".to_string()));
        assert!(args.contains(&"box".to_string()));
    }
}
