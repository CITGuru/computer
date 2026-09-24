use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::microvm::{ImageLoader, MicroVmApi, Plan};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const RUNTIME: &str = "smolvm";

pub const DEFAULT_HOME: &str = ".local/bin/smolvm";

pub const IMAGES: &str = ".smolvm/computer-images";

const HELD_OPEN: &str = "sleep infinity";

pub struct SmolVm {
    program: String,
    images: PathBuf,
}

impl Default for SmolVm {
    fn default() -> Self {
        Self::found()
    }
}

impl SmolVm {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            images: under(IMAGES),
        }
    }

    pub fn found() -> Self {
        let installed = home().map(|home| home.join(DEFAULT_HOME));

        match installed.filter(|path| path.exists()) {
            Some(path) => Self::new(path.display().to_string()),
            None => Self::new(RUNTIME),
        }
    }

    pub fn keeping_images(mut self, images: impl Into<PathBuf>) -> Self {
        self.images = images.into();
        self
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn archive_for(&self, image: &str) -> PathBuf {
        self.images
            .join(format!("{}.tar", image.replace([':', '/'], "-")))
    }

    async fn run(&self, args: &[String]) -> Result<ExecResult> {
        let output = tokio::process::Command::new(&self.program)
            .args(args)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => Error::Unavailable {
                    provider: RUNTIME.to_string(),
                    detail: format!("{} is not on PATH", self.program),
                },
                _ => Error::transport(error.to_string(), false),
            })?;

        Ok(ExecResult {
            code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            timed_out: false,
        })
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    async fn verbose(&self) -> Result<String> {
        let listed = self
            .run(&Self::argv(&["machine", "ls", "--verbose"]))
            .await?;

        match listed.code {
            0 => Ok(listed.stdout_utf8()),
            _ => Err(Error::Unavailable {
                provider: RUNTIME.to_string(),
                detail: listed.stderr_utf8().trim().to_string(),
            }),
        }
    }

    async fn lifecycle(&self, what: &str, name: &str) -> Result<()> {
        let done = self
            .run(&Self::argv(&["machine", what, "--name", name]))
            .await?;

        match done.code {
            0 => Ok(()),
            _ => Err(Error::Failed {
                code: done.code,
                stderr: done.stderr_utf8().trim().to_string(),
            }),
        }
    }

    async fn listed(&self) -> Result<String> {
        let listed = self.run(&Self::argv(&["machine", "ls", "--json"])).await?;

        match listed.code {
            0 => Ok(listed.stdout_utf8()),
            _ => Err(Error::Unavailable {
                provider: RUNTIME.to_string(),
                detail: listed.stderr_utf8().trim().to_string(),
            }),
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn under(rest: &str) -> PathBuf {
    home().unwrap_or_else(|| PathBuf::from(".")).join(rest)
}

pub fn create_args(plan: &Plan, image: &str) -> Vec<String> {
    let mut args = vec![
        "machine".to_string(),
        "create".to_string(),
        "--name".to_string(),
        plan.name.clone(),
        "--image".to_string(),
        image.to_string(),
    ];

    if let Some(cpus) = plan.cpus {
        args.push("--cpus".to_string());
        args.push(cpus.to_string());
    }
    if let Some(mib) = plan.memory_mib {
        args.push("--mem".to_string());
        args.push(mib.to_string());
    }
    if plan.network {
        args.push("--net".to_string());
    }

    for (key, value) in &plan.env {
        args.push("--env".to_string());
        args.push(format!("{key}={value}"));
    }
    for (host, guest) in &plan.ports {
        args.push("--port".to_string());
        args.push(format!("{host}:{guest}"));
    }
    for (key, value) in &plan.labels {
        args.push("--label".to_string());
        args.push(format!("{key}={value}"));
    }

    args.push("--label".to_string());
    args.push("computer-rs=1".to_string());

    args.push("--".to_string());
    args.extend(HELD_OPEN.split(' ').map(str::to_string));
    args
}

pub fn parse_running(listing: &str, name: &str) -> bool {
    machines(listing)
        .iter()
        .any(|machine| named(machine) == Some(name) && state(machine) == Some("running"))
}

pub fn parse_state(listing: &str, name: &str) -> Option<String> {
    machines(listing)
        .iter()
        .find(|machine| named(machine) == Some(name))
        .and_then(|machine| state(machine).map(str::to_string))
}

pub fn parse_ports(listing: &str, name: &str) -> Vec<(u16, u16)> {
    let mut ports = Vec::new();
    let mut theirs = false;

    for line in listing.lines() {
        let indented = line.starts_with(' ');

        if !indented {
            theirs = line.split_whitespace().next() == Some(name);
            continue;
        }

        if !theirs {
            continue;
        }

        let Some(pair) = line.trim().strip_prefix("Port:") else {
            continue;
        };
        let Some((host, guest)) = pair.split_once("->") else {
            continue;
        };

        if let (Ok(host), Ok(guest)) = (host.trim().parse(), guest.trim().parse()) {
            ports.push((host, guest));
        }
    }

    ports
}

pub fn parse_labelled(listing: &str, key: &str) -> Vec<(String, String)> {
    machines(listing)
        .iter()
        .filter_map(|machine| {
            let name = named(machine)?.to_string();
            let value = machine.get("labels")?.get(key)?.as_str()?.to_string();

            Some((name, value))
        })
        .collect()
}

fn machines(listing: &str) -> Vec<serde_json::Value> {
    serde_json::from_str::<Vec<serde_json::Value>>(listing).unwrap_or_default()
}

fn named(machine: &serde_json::Value) -> Option<&str> {
    machine.get("name")?.as_str()
}

fn state(machine: &serde_json::Value) -> Option<&str> {
    machine.get("state")?.as_str()
}

#[async_trait]
impl MicroVmApi for SmolVm {
    async fn available(&self) -> Result<()> {
        let version = self.run(&Self::argv(&["--version"])).await?;

        match version.code {
            0 => Ok(()),
            _ => Err(Error::Unavailable {
                provider: RUNTIME.to_string(),
                detail: version.stderr_utf8().trim().to_string(),
            }),
        }
    }

    async fn has_image(&self, image: &str) -> Result<bool> {
        Ok(self.archive_for(image).exists() || Path::new(image).exists())
    }

    async fn create(&self, plan: &Plan) -> Result<()> {
        if plan.replace {
            let _ = self.remove(&plan.name).await;
        }

        let archive = self.archive_for(&plan.image);
        let image = match archive.exists() {
            true => archive.display().to_string(),
            false => plan.image.clone(),
        };

        let created = self.run(&create_args(plan, &image)).await?;
        if created.code != 0 {
            return Err(Error::Unavailable {
                provider: RUNTIME.to_string(),
                detail: created.stderr_utf8().trim().to_string(),
            });
        }

        let started = self
            .run(&Self::argv(&["machine", "start", "--name", &plan.name]))
            .await?;
        if started.code != 0 {
            let _ = self.remove(&plan.name).await;
            return Err(Error::Unavailable {
                provider: RUNTIME.to_string(),
                detail: started.stderr_utf8().trim().to_string(),
            });
        }

        Ok(())
    }

    async fn running(&self, name: &str) -> Result<bool> {
        Ok(parse_running(&self.listed().await?, name))
    }

    async fn remove(&self, name: &str) -> Result<()> {
        let removed = self
            .run(&Self::argv(&[
                "machine",
                "delete",
                "--name",
                name,
                "--force",
                "--cascade",
            ]))
            .await?;

        match removed.code {
            0 => Ok(()),
            _ => Err(Error::transport(
                removed.stderr_utf8().trim().to_string(),
                true,
            )),
        }
    }

    async fn exec(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<ExecResult> {
        if argv.is_empty() {
            return Err(Error::denied("an empty command has nothing to run"));
        }

        let mut args = Self::argv(&["machine", "exec", "--name", name]);
        for (key, value) in env {
            args.push("--env".to_string());
            args.push(format!("{key}={value}"));
        }
        args.push("--".to_string());
        args.extend(argv.iter().cloned());

        self.run(&args).await
    }

    async fn read(&self, name: &str, path: &str) -> Result<Vec<u8>> {
        let local = std::env::temp_dir().join(format!("computer-smolvm-{name}-read"));
        let _ = tokio::fs::remove_file(&local).await;

        self.copy_out(name, path, &local).await?;

        let bytes = tokio::fs::read(&local)
            .await
            .map_err(|error| Error::transport(error.to_string(), false))?;
        let _ = tokio::fs::remove_file(&local).await;

        Ok(bytes)
    }

    async fn write(&self, name: &str, path: &str, bytes: &[u8]) -> Result<()> {
        let local = std::env::temp_dir().join(format!("computer-smolvm-{name}-write"));
        tokio::fs::write(&local, bytes)
            .await
            .map_err(|error| Error::transport(error.to_string(), false))?;

        let copied = self.copy_in(name, &local, path).await;
        let _ = tokio::fs::remove_file(&local).await;

        copied
    }

    async fn copy_in(&self, name: &str, from: &Path, to: &str) -> Result<()> {
        let copied = self
            .run(&Self::argv(&[
                "machine",
                "cp",
                &from.display().to_string(),
                &format!("{name}:{to}"),
            ]))
            .await?;

        match copied.code {
            0 => Ok(()),
            _ => Err(Error::denied(copied.stderr_utf8().trim().to_string())),
        }
    }

    async fn copy_out(&self, name: &str, from: &str, to: &Path) -> Result<()> {
        let copied = self
            .run(&Self::argv(&[
                "machine",
                "cp",
                &format!("{name}:{from}"),
                &to.display().to_string(),
            ]))
            .await?;

        match copied.code {
            0 => Ok(()),
            _ => Err(Error::denied(copied.stderr_utf8().trim().to_string())),
        }
    }

    async fn labelled(&self, key: &str) -> Result<Vec<(String, String)>> {
        Ok(parse_labelled(&self.listed().await?, key))
    }

    async fn ports(&self, name: &str) -> Result<Vec<(u16, u16)>> {
        Ok(parse_ports(&self.verbose().await?, name))
    }

    async fn pause(&self, name: &str) -> Result<()> {
        self.lifecycle("pause", name).await
    }

    async fn resume(&self, name: &str) -> Result<()> {
        self.lifecycle("resume", name).await
    }

    async fn paused(&self, name: &str) -> Result<bool> {
        Ok(parse_state(&self.listed().await?, name).as_deref() == Some("paused"))
    }

    async fn halt(&self, name: &str) -> Result<()> {
        self.lifecycle("stop", name).await
    }

    async fn wake(&self, name: &str) -> Result<()> {
        self.lifecycle("start", name).await
    }

    fn can(&self) -> computer_types::Capabilities {
        computer_types::Capabilities {
            start: computer_types::Start::Entrypoint,
            reach: computer_types::PortReach::HostPort,
            stop: true,
            ..computer_types::Capabilities::default()
        }
    }
}

#[async_trait]
impl ImageLoader for SmolVm {
    async fn load(&self, archive: &Path, tag: &str) -> Result<()> {
        let kept = self.archive_for(tag);

        if let Some(parent) = kept.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                Error::transport(format!("{}: {error}", parent.display()), false)
            })?;
        }

        tokio::fs::copy(archive, &kept)
            .await
            .map_err(|error| Error::transport(format!("{}: {error}", kept.display()), false))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan {
            name: "desk".to_string(),
            image: "computer-desktop:abc".to_string(),
            cpus: Some(2),
            memory_mib: Some(2048),
            network: true,
            env: BTreeMap::from([("SCREEN_WIDTH".to_string(), "1280".to_string())]),
            ports: vec![(51234, 6080)],
            labels: BTreeMap::from([("computer.server.box".to_string(), "{}".to_string())]),
            replace: true,
        }
    }

    fn pairs(args: &[String], flag: &str) -> Vec<String> {
        args.windows(2)
            .filter(|pair| pair[0] == flag)
            .map(|pair| pair[1].clone())
            .collect()
    }

    #[test]
    fn test_a_machine_is_created_with_what_the_box_needs() {
        let args = create_args(&plan(), "computer-desktop:abc");

        assert_eq!(pairs(&args, "--name"), vec!["desk".to_string()]);
        assert_eq!(
            pairs(&args, "--image"),
            vec!["computer-desktop:abc".to_string()]
        );
        assert_eq!(pairs(&args, "--cpus"), vec!["2".to_string()]);
        assert_eq!(pairs(&args, "--mem"), vec!["2048".to_string()]);
        assert_eq!(pairs(&args, "--env"), vec!["SCREEN_WIDTH=1280".to_string()]);
        assert!(args.contains(&"--net".to_string()));
    }

    #[test]
    fn test_a_port_is_forwarded_to_the_host_port_that_was_picked() {
        let args = create_args(&plan(), "image");

        assert_eq!(
            pairs(&args, "--port"),
            vec!["51234:6080".to_string()],
            "the host side comes first, and it is the one this crate chose"
        );
    }

    #[test]
    fn test_a_box_carries_its_label_so_a_restart_finds_it() {
        let args = create_args(&plan(), "image");

        assert!(pairs(&args, "--label").contains(&"computer.server.box={}".to_string()));
        assert!(pairs(&args, "--label").contains(&"computer-rs=1".to_string()));
    }

    #[test]
    fn test_the_image_start_is_held_off_so_the_desktop_is_started_once() {
        let args = create_args(&plan(), "image");
        let at = args
            .iter()
            .position(|arg| arg == "--")
            .expect("a separator");

        assert_eq!(
            args[at + 1..],
            ["sleep".to_string(), "infinity".to_string()],
            "the image's own entrypoint would race the boot command this crate runs"
        );
    }

    #[test]
    fn test_a_machine_with_no_network_is_not_given_one() {
        let quiet = Plan {
            network: false,
            ..plan()
        };

        assert!(!create_args(&quiet, "image").contains(&"--net".to_string()));
    }

    #[test]
    fn test_what_is_running_is_read_from_the_listing() {
        let listing = r#"[
            {"name": "desk", "state": "running", "labels": {"computer.server.box": "{}"}},
            {"name": "other", "state": "stopped", "labels": {}}
        ]"#;

        assert!(parse_running(listing, "desk"));
        assert!(!parse_running(listing, "other"));
        assert!(!parse_running(listing, "never-made"));
        assert!(!parse_running("not json", "desk"));
    }

    #[test]
    fn test_a_label_is_read_back_from_the_listing() {
        let listing = r#"[
            {"name": "desk", "state": "running", "labels": {"computer.server.box": "{\"width\":1280}"}},
            {"name": "plain", "state": "running", "labels": {"computer-rs": "1"}}
        ]"#;

        assert_eq!(
            parse_labelled(listing, "computer.server.box"),
            vec![("desk".to_string(), "{\"width\":1280}".to_string())],
            "a machine somebody else made is not ours to take"
        );
    }

    #[test]
    fn test_the_ports_a_machine_holds_are_read_back() {
        let listing = "\
NAME  STATE         CPUS     MEMORY  MOUNTS   PORTS
--------------------------------------------------
desk running          2   2048 MiB       0       2
  PID: 42079
  Port: 51234 -> 6080
  Port: 51235 -> 9222
  Created: 2026-09-24T14:41:22Z
other running         2   1024 MiB       0       1
  PID: 42080
  Port: 40000 -> 80
";

        assert_eq!(
            parse_ports(listing, "desk"),
            vec![(51234, 6080), (51235, 9222)]
        );
        assert_eq!(
            parse_ports(listing, "other"),
            vec![(40000, 80)],
            "a machine's detail belongs to the machine it sits under"
        );
        assert!(parse_ports(listing, "never-made").is_empty());
    }

    #[test]
    fn test_a_paused_machine_says_so() {
        let listing = r#"[
            {"name": "desk", "state": "paused", "labels": {}},
            {"name": "other", "state": "running", "labels": {}}
        ]"#;

        assert_eq!(parse_state(listing, "desk").as_deref(), Some("paused"));
        assert_eq!(parse_state(listing, "other").as_deref(), Some("running"));
        assert_eq!(parse_state(listing, "never-made"), None);
        assert!(
            !parse_running(listing, "desk"),
            "a paused box is not running, or the reaper would think it went away"
        );
    }

    #[test]
    fn test_an_image_handed_over_is_kept_under_a_name_no_path_can_escape() {
        let smolvm = SmolVm::new("smolvm").keeping_images("/tmp/images");

        assert_eq!(
            smolvm.archive_for("computer-desktop:abc"),
            PathBuf::from("/tmp/images/computer-desktop-abc.tar")
        );
        assert_eq!(
            smolvm.archive_for("ghcr.io/owner/image:1"),
            PathBuf::from("/tmp/images/ghcr.io-owner-image-1.tar")
        );
    }
}
