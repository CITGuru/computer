use crate::ScreenId;
use crate::bundle;
use crate::config::Config;
use crate::engine::{Engine, parse_ports, run_args};
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::profile::{PROFILE_LABEL, Profile};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Container port to host port.
pub type PortMap = BTreeMap<u16, u16>;

#[async_trait]
pub trait Machine: Send + Sync {
    fn provider(&self) -> &str;

    async fn preflight(&self) -> Result<()>;

    async fn ensure_image(&self, config: &Config) -> Result<()>;

    async fn image_contract(&self, _image: &str) -> Option<String> {
        None
    }

    async fn start(&self, name: &str, config: &Config) -> Result<PortMap>;

    async fn running(&self, name: &str) -> Result<bool>;

    async fn ports(&self, name: &str) -> PortMap;

    async fn env(&self, name: &str) -> BTreeMap<String, String>;

    async fn exec(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult>;

    async fn read_file(&self, name: &str, path: &Path) -> Result<Vec<u8>>;
    async fn write_file(&self, name: &str, path: &Path, bytes: &[u8]) -> Result<()>;

    /// Reads into memory; a runtime that can move bytes directly should override this.
    async fn upload(&self, name: &str, from: &Path, to: &Path) -> Result<()> {
        let bytes = tokio::fs::read(from)
            .await
            .map_err(|error| Error::denied(format!("{}: {error}", from.display())))?;
        self.write_file(name, to, &bytes).await
    }

    async fn download(&self, name: &str, from: &Path, to: &Path) -> Result<()> {
        let bytes = self.read_file(name, from).await?;
        tokio::fs::write(to, bytes)
            .await
            .map_err(|error| Error::denied(format!("{}: {error}", to.display())))
    }

    async fn logs(&self, name: &str) -> Result<String>;

    /// Not recoverable, unlike [`Machine::halt`].
    async fn remove(&self, name: &str) -> Result<()>;

    /// Keeps the filesystem but not memory.
    async fn halt(&self, name: &str) -> Result<()> {
        let _ = name;
        Err(Error::Unsupported {
            gaps: vec!["stopping a box without removing it"],
        })
    }

    /// A container runtime picks new host ports on every start, so the old map is stale.
    async fn wake(&self, name: &str) -> Result<PortMap> {
        let _ = name;
        Err(Error::Unsupported {
            gaps: vec!["stopping a box without removing it"],
        })
    }

    /// Keeps memory and published ports, unlike [`Machine::halt`].
    async fn pause(&self, name: &str) -> Result<()> {
        let _ = name;
        Err(Error::Unsupported {
            gaps: vec!["pausing a box"],
        })
    }

    async fn resume(&self, name: &str) -> Result<()> {
        let _ = name;
        Err(Error::Unsupported {
            gaps: vec!["pausing a box"],
        })
    }

    async fn paused(&self, name: &str) -> Result<bool> {
        let _ = name;
        Ok(false)
    }

    async fn labelled(&self, _label: &str) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    fn image_used(&self, _config: &Config) -> Option<String> {
        None
    }

    async fn forget_image(&self, _reference: &str) -> Result<()> {
        Err(Error::Unsupported {
            gaps: vec!["removing an image"],
        })
    }

    fn reach(&self, config: &Config) -> crate::Reach {
        config.bind.reach()
    }

    fn exposes_every_port(&self) -> bool {
        false
    }

    fn sweepable(&self) -> bool {
        false
    }

    /// `Drop` cannot await. `None` means a dropped handle leaks the box.
    fn reaper(&self, name: &str) -> Option<(String, Vec<String>)>;
}

#[async_trait]
pub trait ScreenHost: Send + Sync {
    async fn run(&self, argv: &[String], screen: ScreenId) -> Result<ExecResult>;
}

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct MachineHost {
    machine: Arc<dyn Machine>,
    profile: Arc<dyn Profile>,
    name: String,
    timeout: Duration,
    /// Nanoseconds since the epoch.
    active_at: Arc<std::sync::atomic::AtomicU64>,
    advertised: (crate::Scheme, String),
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

    pub fn gated_by(mut self, auth: crate::Auth, credentials: Option<crate::Credentials>) -> Self {
        self.gate = (auth, credentials);
        self
    }

    pub fn gate(&self) -> (crate::Auth, Option<&crate::Credentials>) {
        (self.gate.0, self.gate.1.as_ref())
    }

    pub fn view_ticket(&self) -> Option<&crate::Secret> {
        self.ticket(|pair| &pair.view)
    }

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

    pub fn advertised_at(mut self, scheme: crate::Scheme, host: impl Into<String>) -> Self {
        self.advertised = (scheme, host.into());
        self
    }

    pub fn address(&self, port: u16) -> crate::Address {
        crate::Address {
            scheme: self.advertised.0,
            host: self.advertised.1.clone(),
            port,
        }
    }

    pub fn active_at(&self) -> Arc<std::sync::atomic::AtomicU64> {
        Arc::clone(&self.active_at)
    }

    pub fn idle_for(&self) -> Duration {
        let last = self.active_at.load(std::sync::atomic::Ordering::Relaxed);
        Duration::from_nanos(now_nanos().saturating_sub(last))
    }

    pub fn touch(&self) {
        self.active_at
            .store(now_nanos(), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// A timeout is reported as `timed_out`, since a program can exit 124 itself.
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

async fn discard(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::debug!(file = %path.display(), %error, "a scratch file stayed behind");
    }
}

fn if_stopped(name: &str, stderr: &str) -> Option<Error> {
    stderr
        .contains("is not running")
        .then(|| Error::denied(format!("box {name} is stopped. Start it first.")))
}

/// The runtime's words for a path the box does not have, in ours. It names a
/// container id the caller never saw.
fn if_absent(stderr: &str) -> Option<Error> {
    let path = stderr.split_once("Could not find the file ")?.1;
    let path = path.split_once(" in container")?.0;

    Some(Error::invalid(format!("the box has no file at {path}")))
}

pub const SETTLE_PROFILE: &str = r#"grep -q ' /home/computer/.browser-profiles ' /proc/mounts || exit 0
export LC_ALL=C
devtools() {
  exec 3<>/dev/tcp/127.0.0.1/9222 || return 1
  printf 'GET %s HTTP/1.1\r\nHost: 127.0.0.1:9222\r\n\r\n' "$1" >&3
  local line length=0 body=""
  while IFS= read -r -t 3 line <&3; do
    line=${line%$'\r'}
    [ -z "$line" ] && break
    case "$line" in [Cc]ontent-[Ll]ength:*) length=${line#*:} ;; esac
  done
  [ "${length// /}" -gt 0 ] 2>/dev/null && IFS= read -r -N "${length// /}" -t 3 body <&3
  exec 3<&-
  printf '%s' "$body"
}
for id in $(devtools /json/list 2>/dev/null | grep -o '"id": *"[^"]*"' | cut -d '"' -f 4); do
  devtools "/json/close/$id" >/dev/null 2>&1
done
for i in $(seq 50); do
  pgrep -x chromium >/dev/null || exit 0
  sleep 0.1
done
exit 1"#;

fn arg(value: impl Into<String>) -> String {
    value.into()
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0)
}

pub struct EngineMachine {
    cli: Arc<dyn Engine>,
}

impl Default for EngineMachine {
    fn default() -> Self {
        Self::new(Arc::new(crate::engine::SystemEngine::default()))
    }
}

impl EngineMachine {
    pub fn new(cli: Arc<dyn Engine>) -> Self {
        Self { cli }
    }

    pub fn cli(&self) -> &Arc<dyn Engine> {
        &self.cli
    }

    /// `cp` refuses a target whose parent is missing.
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

    async fn settle_profile(&self, name: &str) {
        let _ = self
            .cli
            .run(&[
                arg("exec"),
                arg(name),
                arg("bash"),
                arg("-c"),
                arg(SETTLE_PROFILE),
            ])
            .await;
    }

    async fn freeze(&self, verb: &str, name: &str) -> Result<()> {
        let result = self.cli.run(&[arg(verb), arg(name)]).await?;

        if result.code != 0 {
            let said = result.stderr_utf8();
            return Err(
                if_stopped(name, &said).unwrap_or_else(|| Error::denied(said.trim().to_string()))
            );
        }
        Ok(())
    }

    /// Not base64 over exec: its flags differ between coreutils and BusyBox,
    /// and an argument list has a ceiling a screenshot exceeds.
    async fn copy(&self, from: &str, to: &str) -> Result<()> {
        let result = self.cli.run(&[arg("cp"), arg(from), arg(to)]).await?;

        if result.code != 0 {
            let said = result.stderr_utf8();
            return Err(if_absent(&said).unwrap_or_else(|| Error::denied(said.trim().to_string())));
        }
        Ok(())
    }

    /// Unique per call, or concurrent reads delete each other's bytes; under a
    /// `0700` directory, so the path cannot be a planted symlink.
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
impl Machine for EngineMachine {
    fn provider(&self) -> &str {
        self.cli.program()
    }

    async fn preflight(&self) -> Result<()> {
        let alive = self.cli.run(&[arg("version")]).await?;
        if alive.code != 0 {
            return Err(Error::Unavailable {
                provider: self.provider().to_string(),
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

        // A missing label prints an empty line, or "<no value>".
        let declared = said.stdout_utf8().trim().to_string();
        (said.code == 0 && !declared.is_empty() && declared != "<no value>").then_some(declared)
    }

    async fn start(&self, name: &str, config: &Config) -> Result<PortMap> {
        if let Some(volume) = &config.profiles {
            let holders = self
                .cli
                .run(&[
                    arg("ps"),
                    arg("--all"),
                    arg("--filter"),
                    format!("volume={volume}"),
                    arg("--format"),
                    arg("{{.Names}}"),
                ])
                .await?;
            let holders = holders.stdout_utf8();
            let holders: Vec<&str> = holders.split_whitespace().collect();
            if !holders.is_empty() {
                return Err(Error::denied(format!(
                    "the profile {volume} is held by {}: two browsers on one profile corrupt \
                     it, so remove that box first",
                    holders.join(", ")
                )));
            }
        }

        let started = self.cli.run(&run_args(name, config)).await?;
        if started.code != 0 {
            return Err(Error::Unavailable {
                provider: self.provider().to_string(),
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

    async fn pause(&self, name: &str) -> Result<()> {
        self.freeze("pause", name).await
    }

    async fn halt(&self, name: &str) -> Result<()> {
        self.settle_profile(name).await;
        self.freeze("stop", name).await
    }

    async fn wake(&self, name: &str) -> Result<PortMap> {
        self.freeze("start", name).await?;

        // The previous host ports may belong to someone else now.
        Ok(self.ports(name).await)
    }

    async fn resume(&self, name: &str) -> Result<()> {
        self.freeze("unpause", name).await
    }

    async fn paused(&self, name: &str) -> Result<bool> {
        let state = self
            .cli
            .run(&[
                arg("inspect"),
                arg("--format"),
                arg("{{.State.Paused}}"),
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

        let result = self.cli.run(&args).await?;
        if result.code != 0
            && let Some(stopped) = if_stopped(name, &result.stderr_utf8())
        {
            return Err(stopped);
        }

        Ok(result)
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

    async fn forget_image(&self, reference: &str) -> Result<()> {
        let result = self
            .cli
            .run(&[arg("image"), arg("rm"), arg("--force"), arg(reference)])
            .await?;

        if result.code != 0 {
            return Err(Error::transport(
                result.stderr_utf8().trim().to_string(),
                false,
            ));
        }
        Ok(())
    }

    async fn remove(&self, name: &str) -> Result<()> {
        self.settle_profile(name).await;
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

    #[test]
    fn test_the_script_that_closes_a_profiles_browser_is_shell_bash_reads() {
        let checked = std::process::Command::new("bash")
            .args(["-n", "-c", SETTLE_PROFILE])
            .output()
            .expect("bash is on every host this builds on");
        assert!(
            checked.status.success(),
            "{}",
            String::from_utf8_lossy(&checked.stderr)
        );
        assert!(
            SETTLE_PROFILE
                .starts_with("grep -q ' /home/computer/.browser-profiles ' /proc/mounts || exit 0"),
            "a box with no profile has nothing to lose, and is left alone"
        );
    }
    use crate::testing::ScriptedEngine;

    fn docker(cli: Arc<ScriptedEngine>) -> EngineMachine {
        EngineMachine::new(cli as Arc<dyn Engine>)
    }

    #[tokio::test]
    async fn test_a_command_carries_the_environment_it_was_given() {
        let cli = Arc::new(ScriptedEngine::new());
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
        let cli = Arc::new(ScriptedEngine::new());
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

    #[tokio::test]
    async fn test_two_reads_of_one_box_do_not_stage_through_one_path() {
        let cli = Arc::new(ScriptedEngine::new());
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

    #[tokio::test]
    async fn test_upload_makes_the_directory_write_file_would_have_made() {
        let cli = Arc::new(ScriptedEngine::new());
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
        let machine = docker(Arc::new(ScriptedEngine::new()));
        let (program, args) = machine.reaper("box").expect("a reaper");

        assert_eq!(program, "docker");
        assert!(args.contains(&"--force".to_string()));
        assert!(args.contains(&"box".to_string()));
    }
}
