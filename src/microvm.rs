//! Boxes on a hypervisor.
//!
//! A microVM boots a kernel of its own, which is a stronger boundary than a
//! namespace and a slower start. [`MicroVm`] implements [`Machine`], so the
//! driver, the screens, the takeover gate and the descriptor above it are the
//! same code a container uses.
//!
//! [`MicroVmApi`] is the seam: create, running, remove, exec, read, write, and
//! whether an image is held. Who implements it lives in
//! [`crate::sandboxes`]: a vendor has its own command, its own idea of an
//! image and its own answers, and none of that belongs in the abstraction.
//!
//! Three things belong to this module. Free host ports are found before the
//! machine is created, because a hypervisor forwards the pairs it is given.
//! The screen is brought up with `computer-desktop --once`, because a machine
//! lives until it is stopped. And the image has to be handed over:
//! [`import_image`] loads one built by a container runtime, an OCI reference
//! is pulled by the hypervisor itself, and [`export_rootfs`] flattens an image
//! for a hypervisor that keeps no store.

use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::machine::{Machine, PortMap};
use crate::runtime::{Config, ContainerCli};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// What to create, decided before anything exists.
///
/// A plain value, so the mapping is testable with no hypervisor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub name: String,
    /// An OCI reference the runtime can pull, or a root filesystem directory.
    pub image: String,
    pub cpus: Option<u8>,
    /// Mebibytes, which is the unit hypervisor builders take.
    pub memory_mib: Option<u64>,
    pub network: bool,
    pub env: BTreeMap<String, String>,
    /// Host port to guest port. Chosen here: a hypervisor forwards the pairs
    /// it is given and has no "pick me a free one".
    pub ports: Vec<(u16, u16)>,
    /// Take over a machine of this name left by a run that did not clean up,
    /// rather than refusing and stranding it.
    pub replace: bool,
}

/// The one seam between this crate and a hypervisor.
#[async_trait]
pub trait MicroVmApi: Send + Sync {
    /// Whether the runtime is installed and answering.
    async fn available(&self) -> Result<()>;

    /// Whether the hypervisor already holds this image.
    ///
    /// Defaults to yes: most references are ones the runtime fetches itself, and
    /// a wrong no would refuse a machine that would have started.
    async fn has_image(&self, _image: &str) -> Result<bool> {
        Ok(true)
    }

    async fn create(&self, plan: &Plan) -> Result<()>;
    async fn running(&self, name: &str) -> Result<bool>;
    async fn remove(&self, name: &str) -> Result<()>;

    async fn exec(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult>;

    async fn read(&self, name: &str, path: &str) -> Result<Vec<u8>>;
    async fn write(&self, name: &str, path: &str, bytes: &[u8]) -> Result<()>;

    /// Move a whole file in without carrying it through this process.
    ///
    /// The default reads it into memory first. A runtime that copies disk to
    /// disk should override this.
    async fn copy_in(&self, name: &str, from: &Path, to: &str) -> Result<()> {
        let bytes = tokio::fs::read(from)
            .await
            .map_err(|error| Error::denied(format!("{}: {error}", from.display())))?;
        self.write(name, to, &bytes).await
    }

    /// Move a whole file out the same way.
    async fn copy_out(&self, name: &str, from: &str, to: &Path) -> Result<()> {
        let bytes = self.read(name, from).await?;
        tokio::fs::write(to, bytes)
            .await
            .map_err(|error| Error::denied(format!("{}: {error}", to.display())))
    }

    /// What the machine has said. Empty where the runtime keeps no log.
    async fn logs(&self, _name: &str) -> Result<String> {
        Ok(String::new())
    }
}

/// A free port on this host, or none.
///
/// Bound and released, so there is a gap before the hypervisor takes it. A
/// collision shows up as a machine that will not start.
pub fn free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
    listener.local_addr().ok().map(|address| address.port())
}

/// Everything the box needs published, as host-to-guest pairs.
pub fn port_pairs(guests: &[u16], pick: impl Fn() -> Option<u16>) -> Vec<(u16, u16)> {
    guests
        .iter()
        .filter_map(|guest| pick().map(|host| (host, *guest)))
        .collect()
}

/// What a desktop needs when the caller names no ceiling.
///
/// microsandbox gives 512 MiB by default, in which chromium dies.
pub const DESKTOP_MEMORY_MIB: u64 = 2048;

/// How long to wait for a guest to find its way out before starting a browser.
pub const NETWORK_WAIT: Duration = Duration::from_secs(15);

/// A configuration, as the plan for one machine.
pub fn plan_for(name: &str, config: &Config, ports: Vec<(u16, u16)>) -> Plan {
    Plan {
        name: name.to_string(),
        image: config.image.clone(),
        cpus: config.cpus.as_ref().and_then(|cpus| cpus.parse().ok()),
        memory_mib: config
            .memory
            .as_deref()
            .and_then(mebibytes)
            .or(Some(DESKTOP_MEMORY_MIB)),
        network: config.network,
        env: config.env.clone(),
        ports,
        replace: true,
    }
}

/// `"2g"`, `"512m"`, `"1073741824"` as mebibytes, rounded up.
pub fn mebibytes(limit: &str) -> Option<u64> {
    let limit = limit.trim().to_ascii_lowercase();
    let (digits, scale) = match limit.chars().last()? {
        'g' => (limit.get(..limit.len() - 1)?, 1024),
        'm' => (limit.get(..limit.len() - 1)?, 1),
        'k' => {
            return limit
                .get(..limit.len() - 1)?
                .parse::<u64>()
                .ok()
                .map(|k| k.div_ceil(1024));
        }
        'b' => {
            let bytes: u64 = limit.get(..limit.len() - 1)?.parse().ok()?;
            return Some(bytes.div_ceil(1024 * 1024));
        }
        _ => {
            let bytes: u64 = limit.parse().ok()?;
            return Some(bytes.div_ceil(1024 * 1024));
        }
    };
    digits.parse::<u64>().ok().map(|value| value * scale)
}

/// Boxes on a hypervisor.
pub struct MicroVm {
    api: Arc<dyn MicroVmApi>,
    runtime: String,
    /// What was published, per machine. The hypervisor was told these pairs,
    /// so this side already knows them and does not ask for them back.
    published: Mutex<BTreeMap<String, PortMap>>,
    started_with: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    reaper: Option<(String, Vec<String>)>,
}

impl MicroVm {
    pub fn new(api: Arc<dyn MicroVmApi>) -> Self {
        Self {
            api,
            runtime: "microvm".to_string(),
            published: Mutex::new(BTreeMap::new()),
            started_with: Mutex::new(BTreeMap::new()),
            reaper: None,
        }
    }

    pub fn named(mut self, runtime: impl Into<String>) -> Self {
        self.runtime = runtime.into();
        self
    }

    /// A command that removes a machine with no async runtime in the room.
    ///
    /// `{}` in an argument becomes the machine's name. Without one, a dropped
    /// handle leaves the machine running.
    pub fn reaping_with<I, S>(mut self, program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.reaper = Some((program.into(), args.into_iter().map(Into::into).collect()));
        self
    }

    pub fn api(&self) -> &Arc<dyn MicroVmApi> {
        &self.api
    }

    fn remember(&self, name: &str, plan: &Plan) {
        if let Ok(mut published) = self.published.lock() {
            published.insert(
                name.to_string(),
                plan.ports
                    .iter()
                    .map(|(host, guest)| (*guest, *host))
                    .collect(),
            );
        }
        if let Ok(mut started) = self.started_with.lock() {
            started.insert(name.to_string(), plan.env.clone());
        }
    }

    async fn run(&self, name: &str, argv: &[&str]) -> Result<ExecResult> {
        let argv: Vec<String> = argv.iter().map(|part| (*part).to_string()).collect();
        self.run_argv(name, &argv).await
    }

    async fn run_argv(&self, name: &str, argv: &[String]) -> Result<ExecResult> {
        self.api.exec(name, argv, &BTreeMap::new()).await
    }

    /// Wait until the guest has a route out, or give up quietly.
    ///
    /// A machine with no network is still a usable desktop; the wait is so the
    /// browser does not start into a network about to change under it.
    async fn wait_for_network(&self, name: &str) {
        let deadline = SystemTime::now() + NETWORK_WAIT;

        loop {
            let up = self
                .run(name, &["sh", "-c", "ip route | grep -q default"])
                .await
                .map(|result| result.code == 0)
                .unwrap_or(false);

            if up || SystemTime::now() >= deadline {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[async_trait]
impl Machine for MicroVm {
    fn runtime(&self) -> &str {
        &self.runtime
    }

    async fn preflight(&self) -> Result<()> {
        self.api.available().await
    }

    async fn ensure_image(&self, config: &Config) -> Result<()> {
        let image = config.image.as_str();

        // A directory is a root filesystem, and it has to be there already.
        let path = Path::new(image);
        if image.starts_with('.') || image.starts_with('/') {
            return match tokio::fs::metadata(path).await {
                Ok(_) => Ok(()),
                Err(error) => Err(Error::Unavailable {
                    runtime: self.runtime.clone(),
                    detail: format!("{image}: {error}"),
                }),
            };
        }

        // A container runtime keeps its images where a hypervisor cannot
        // read them, so this refuses before booting rather than failing later
        // as "no such image".
        if (config.bundle.is_some() || config.image_dir.is_some())
            && !self.api.has_image(image).await?
        {
            return Err(Error::Unavailable {
                runtime: self.runtime.clone(),
                detail: format!(
                    "{image} is built in the container runtime's store; \
                     hand it over once with computer::microvm::import_image, or \
                     flatten it with export_rootfs and pass the directory"
                ),
            });
        }

        // Anything else is a reference the hypervisor pulls for itself.
        Ok(())
    }

    async fn start(&self, name: &str, config: &Config) -> Result<PortMap> {
        let plan = plan_for(name, config, port_pairs(&config.publish, free_port));
        self.api.create(&plan).await?;
        self.remember(name, &plan);

        // The network first, then the browser. A machine's interface comes up
        // after the machine does, and chromium started before it answers the
        // first navigation with ERR_NETWORK_CHANGED.
        if config.network {
            self.wait_for_network(name).await;
        }

        // A machine does not run its image's command, so an empty boot command
        // here is a box that starts with no screen in it.
        if config.boot.is_empty() {
            let _ = self.api.remove(name).await;
            return Err(Error::Unsupported {
                gaps: vec!["a command to bring the box up"],
            });
        }

        let booted = self.run_argv(name, &config.boot).await?;
        if booted.code != 0 {
            let _ = self.api.remove(name).await;
            return Err(Error::Failed {
                code: booted.code,
                stderr: booted.stderr_utf8().trim().to_string(),
            });
        }

        Ok(self.ports(name).await)
    }

    async fn running(&self, name: &str) -> Result<bool> {
        self.api.running(name).await
    }

    async fn ports(&self, name: &str) -> PortMap {
        self.published
            .lock()
            .ok()
            .and_then(|published| published.get(name).cloned())
            .unwrap_or_default()
    }

    async fn env(&self, name: &str) -> BTreeMap<String, String> {
        // What this process asked for. A machine somebody else started
        // answers with nothing, and the descriptor falls back to the image.
        self.started_with
            .lock()
            .ok()
            .and_then(|started| started.get(name).cloned())
            .unwrap_or_default()
    }

    async fn exec(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        self.api.exec(name, argv, env).await
    }

    async fn read_file(&self, name: &str, path: &Path) -> Result<Vec<u8>> {
        self.api.read(name, &path.display().to_string()).await
    }

    async fn write_file(&self, name: &str, path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            let _ = self
                .run(name, &["mkdir", "-p", &parent.display().to_string()])
                .await;
        }
        self.api
            .write(name, &path.display().to_string(), bytes)
            .await
    }

    async fn upload(&self, name: &str, from: &Path, to: &Path) -> Result<()> {
        self.api
            .copy_in(name, from, &to.display().to_string())
            .await
    }

    async fn download(&self, name: &str, from: &Path, to: &Path) -> Result<()> {
        self.api
            .copy_out(name, &from.display().to_string(), to)
            .await
    }

    async fn logs(&self, name: &str) -> Result<String> {
        self.api.logs(name).await
    }

    async fn stop(&self, name: &str) -> Result<()> {
        self.api.remove(name).await
    }

    fn reaper(&self, name: &str) -> Option<(String, Vec<String>)> {
        let (program, args) = self.reaper.clone()?;
        Some((
            program,
            args.into_iter()
                .map(|arg| arg.replace("{}", name))
                .collect(),
        ))
    }
}

/// Turn a built container image into a root filesystem a hypervisor can boot.
///
/// For a hypervisor with no image store of its own; where there is one,
/// [`import_image`] hands the image over whole. Needs the container runtime
/// and `tar`, and about a gigabyte of disk.
pub async fn export_rootfs(
    cli: &dyn ContainerCli,
    image: &str,
    into: impl AsRef<Path>,
) -> Result<PathBuf> {
    let into = into.as_ref().to_path_buf();
    tokio::fs::create_dir_all(&into)
        .await
        .map_err(|error| Error::transport(format!("{}: {error}", into.display()), false))?;

    let name = format!("computer-export-{}", std::process::id());
    let tarball = std::env::temp_dir().join(format!("{name}.tar"));

    let created = cli
        .run(&[
            "create".to_string(),
            "--name".to_string(),
            name.clone(),
            image.to_string(),
        ])
        .await?;
    if created.code != 0 {
        return Err(Error::Unavailable {
            runtime: cli.program().to_string(),
            detail: created.stderr_utf8().trim().to_string(),
        });
    }

    // `--output` rather than standard output: the filesystem is hundreds of
    // megabytes.
    let exported = cli
        .run(&[
            "export".to_string(),
            "--output".to_string(),
            tarball.display().to_string(),
            name.clone(),
        ])
        .await;

    let removed = cli
        .run(&["rm".to_string(), "--force".to_string(), name])
        .await;
    let _ = removed;

    let exported = exported?;
    if exported.code != 0 {
        return Err(Error::denied(exported.stderr_utf8().trim().to_string()));
    }

    let unpacked = tokio::process::Command::new("tar")
        .args([
            "-xf",
            &tarball.display().to_string(),
            "-C",
            &into.display().to_string(),
        ])
        .output()
        .await
        .map_err(|error| Error::transport(error.to_string(), false))?;

    let _ = tokio::fs::remove_file(&tarball).await;

    if !unpacked.status.success() {
        return Err(Error::denied(
            String::from_utf8_lossy(&unpacked.stderr).trim().to_string(),
        ));
    }
    Ok(into)
}

/// Hand a locally built container image to a hypervisor that keeps its own
/// image store.
///
/// `docker save` writes a tar archive and the hypervisor's loader reads it.
/// About a gigabyte moves through the disk, so do this once per image.
pub async fn import_image(
    cli: &dyn ContainerCli,
    loader: &dyn ImageLoader,
    image: &str,
) -> Result<()> {
    let archive = std::env::temp_dir().join(format!(
        "computer-import-{}.tar",
        image.replace([':', '/'], "-")
    ));

    let saved = cli
        .run(&[
            "save".to_string(),
            "--output".to_string(),
            archive.display().to_string(),
            image.to_string(),
        ])
        .await?;

    if saved.code != 0 {
        return Err(Error::Unavailable {
            runtime: cli.program().to_string(),
            detail: saved.stderr_utf8().trim().to_string(),
        });
    }

    let outcome = loader.load(&archive, image).await;
    let _ = tokio::fs::remove_file(&archive).await;
    outcome
}

/// A hypervisor that can be handed an image archive.
#[async_trait]
pub trait ImageLoader: Send + Sync {
    async fn load(&self, archive: &Path, tag: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_plan_carries_the_environment_the_configuration_was_given() {
        let config = Config {
            env: BTreeMap::from([("SCREEN_WIDTH".to_string(), "1920".to_string())]),
            ..Config::default()
        };
        let plan = plan_for("box", &config, Vec::new());

        assert_eq!(
            plan.env.get("SCREEN_WIDTH").map(String::as_str),
            Some("1920"),
            "which variables the image reads was resolved at launch; this only \
             carries them"
        );
    }

    #[test]
    fn test_every_published_port_is_a_host_guest_pair() {
        let next = std::sync::atomic::AtomicU16::new(40_000);
        let guests = [6080, 6081, 9223];
        let pairs = port_pairs(&guests, || {
            Some(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
        });

        assert_eq!(pairs.len(), guests.len());
        assert!(
            pairs.iter().all(|(host, guest)| host != guest),
            "a fixed host port collides with the second box on this machine"
        );
    }

    #[test]
    fn test_a_box_that_publishes_nothing_asks_for_no_forwarding() {
        let plan = plan_for("box", &Config::default(), port_pairs(&[], free_port));
        assert!(plan.ports.is_empty());
    }

    #[test]
    fn test_a_desktop_gets_enough_memory_even_when_nobody_asks() {
        let plan = plan_for("box", &Config::default(), Vec::new());
        assert_eq!(
            plan.memory_mib,
            Some(DESKTOP_MEMORY_MIB),
            "a hypervisor's default is sized for a script, and chromium dies in it"
        );

        let asked = Config {
            memory: Some("4g".to_string()),
            ..Config::default()
        };
        assert_eq!(plan_for("box", &asked, Vec::new()).memory_mib, Some(4096));
    }

    #[test]
    fn test_a_memory_ceiling_is_rounded_up_and_not_down() {
        assert_eq!(mebibytes("2g"), Some(2048));
        assert_eq!(mebibytes("512m"), Some(512));
        assert_eq!(mebibytes("1500000000"), Some(1431));
        assert_eq!(
            mebibytes("1048577b"),
            Some(2),
            "rounded down, a ceiling is tighter than the caller asked for"
        );
        assert_eq!(mebibytes("lots"), None);
    }

    #[test]
    fn test_a_name_left_by_a_crashed_run_is_taken_over_rather_than_refused() {
        let plan = plan_for("box", &Config::default(), Vec::new());
        assert!(
            plan.replace,
            "refusing would strand the machine and its memory"
        );
    }

    #[test]
    fn test_a_free_port_is_one_the_system_just_handed_out() {
        let first = free_port().expect("a port");
        assert!(first >= 1024);
    }
}
