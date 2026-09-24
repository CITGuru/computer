use crate::config::Config;
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::machine::{Machine, PortMap};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub name: String,
    pub image: String,
    pub cpus: Option<u8>,
    pub memory_mib: Option<u64>,
    pub network: bool,
    pub env: BTreeMap<String, String>,
    /// Chosen here: a hypervisor forwards the pairs it is given and picks none.
    pub ports: Vec<(u16, u16)>,
    pub labels: BTreeMap<String, String>,
    pub replace: bool,
}

#[async_trait]
pub trait MicroVmApi: Send + Sync {
    async fn available(&self) -> Result<()>;

    /// Defaults to yes: a wrong no would refuse a machine that would have started.
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

    /// The default reads the file into memory; override where the runtime copies disk to disk.
    async fn copy_in(&self, name: &str, from: &Path, to: &str) -> Result<()> {
        let bytes = tokio::fs::read(from)
            .await
            .map_err(|error| Error::denied(format!("{}: {error}", from.display())))?;
        self.write(name, to, &bytes).await
    }

    async fn copy_out(&self, name: &str, from: &str, to: &Path) -> Result<()> {
        let bytes = self.read(name, from).await?;
        tokio::fs::write(to, bytes)
            .await
            .map_err(|error| Error::denied(format!("{}: {error}", to.display())))
    }

    async fn labelled(&self, _key: &str) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    async fn logs(&self, _name: &str) -> Result<String> {
        Ok(String::new())
    }
}

/// Bound and released, so another process can take it before the hypervisor does.
pub fn free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
    listener.local_addr().ok().map(|address| address.port())
}

pub fn port_pairs(guests: &[u16], pick: impl Fn() -> Option<u16>) -> Vec<(u16, u16)> {
    guests
        .iter()
        .filter_map(|guest| pick().map(|host| (host, *guest)))
        .collect()
}

/// microsandbox gives 512 MiB by default, in which chromium dies.
pub const DESKTOP_MEMORY_MIB: u64 = 2048;

pub const NETWORK_WAIT: Duration = Duration::from_secs(15);

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
        labels: config.labels.clone(),
        replace: true,
    }
}

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

pub struct MicroVm {
    api: Arc<dyn MicroVmApi>,
    runtime: String,
    published: Mutex<BTreeMap<String, PortMap>>,
    started_with: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    reaper: Option<(String, Vec<String>)>,
}

impl MicroVm {
    async fn ensure_parent(&self, name: &str, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = self
                .run(name, &["mkdir", "-p", &parent.display().to_string()])
                .await;
        }
    }

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

    /// `{}` in an argument becomes the machine's name.
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

        // A hypervisor cannot read a container runtime's images; refuse before booting.
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

        Ok(())
    }

    async fn start(&self, name: &str, config: &Config) -> Result<PortMap> {
        if config.profiles.is_some() {
            return Err(Error::Unsupported {
                gaps: vec!["a named browser profile, which needs a Docker volume"],
            });
        }
        let plan = plan_for(name, config, port_pairs(&config.publish, free_port));
        self.api.create(&plan).await?;
        self.remember(name, &plan);

        // chromium started before the interface is up fails with ERR_NETWORK_CHANGED.
        if config.network {
            self.wait_for_network(name).await;
        }

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

    async fn labelled(&self, label: &str) -> Result<Vec<(String, String)>> {
        self.api.labelled(label).await
    }

    async fn ports(&self, name: &str) -> PortMap {
        self.published
            .lock()
            .ok()
            .and_then(|published| published.get(name).cloned())
            .unwrap_or_default()
    }

    async fn env(&self, name: &str) -> BTreeMap<String, String> {
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
        self.ensure_parent(name, path).await;
        self.api
            .write(name, &path.display().to_string(), bytes)
            .await
    }

    async fn upload(&self, name: &str, from: &Path, to: &Path) -> Result<()> {
        self.ensure_parent(name, to).await;
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

    async fn remove(&self, name: &str) -> Result<()> {
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

pub async fn export_rootfs(
    cli: &dyn Engine,
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

pub async fn import_image(cli: &dyn Engine, loader: &dyn ImageLoader, image: &str) -> Result<()> {
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
