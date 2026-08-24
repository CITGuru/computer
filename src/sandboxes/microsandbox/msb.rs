//! microsandbox through the command it installs.
//!
//! Arguments in, text out. That keeps the part worth testing — which flags a
//! plan turns into, and what the answers mean — a pure function, checkable
//! with no hypervisor on the machine.

use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::microvm::{ImageLoader, MicroVm, MicroVmApi, Plan};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Where microsandbox puts its command when nothing added it to `PATH`.
pub const DEFAULT_HOME: &str = ".microsandbox/bin/msb";

pub struct Msb {
    program: String,
}

impl Default for Msb {
    fn default() -> Self {
        Self::found()
    }
}

impl Msb {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// `msb` on the path, or where the installer puts it.
    ///
    /// microsandbox installs into a directory it does not add to `PATH`.
    pub fn found() -> Self {
        let installed = std::env::var("HOME")
            .map(|home| std::path::PathBuf::from(home).join(DEFAULT_HOME))
            .ok()
            .filter(|path| path.exists());

        match installed {
            Some(path) => Self::new(path.display().to_string()),
            None => Self::new("msb"),
        }
    }

    pub fn program(&self) -> &str {
        &self.program
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
                    runtime: "microsandbox".to_string(),
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
}

/// The arguments that create one machine, as a pure function.
pub fn create_args(plan: &Plan) -> Vec<String> {
    let mut args = vec!["create".to_string(), "-n".to_string(), plan.name.clone()];

    if let Some(cpus) = plan.cpus {
        args.push("-c".to_string());
        args.push(cpus.to_string());
    }
    if let Some(mib) = plan.memory_mib {
        // Mebibytes, which is what the flag takes.
        args.push("-m".to_string());
        args.push(mib.to_string());
    }
    if !plan.network {
        // The interface goes entirely, which is stronger than a
        // container's empty network namespace.
        args.push("--no-net".to_string());
    }

    for (key, value) in &plan.env {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }
    for (host, guest) in &plan.ports {
        // Loopback, like everywhere else in this crate: the screen has no
        // password on it.
        args.push("-p".to_string());
        args.push(format!("127.0.0.1:{host}:{guest}"));
    }

    args.push("--label".to_string());
    args.push("computer-rs=1".to_string());

    args.push(plan.image.clone());
    args
}

/// Whether `msb ls` says this machine is running.
pub fn parse_running(output: &str, name: &str) -> bool {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?, fields.nth(1)?))
        })
        .any(|(listed, status)| listed == name && status == "running")
}

/// Whether `msb image ls` holds this reference.
pub fn parse_has_image(output: &str, image: &str) -> bool {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .any(|reference| reference == image)
}

#[async_trait]
impl MicroVmApi for Msb {
    async fn available(&self) -> Result<()> {
        let version = self.run(&Self::argv(&["--version"])).await?;
        if version.code != 0 {
            return Err(Error::Unavailable {
                runtime: "microsandbox".to_string(),
                detail: version.stderr_utf8().trim().to_string(),
            });
        }
        Ok(())
    }

    async fn has_image(&self, image: &str) -> Result<bool> {
        let listed = self.run(&Self::argv(&["image", "ls"])).await?;
        Ok(parse_has_image(&listed.stdout_utf8(), image))
    }

    async fn create(&self, plan: &Plan) -> Result<()> {
        if plan.replace {
            // A name left by a run that did not clean up is this same
            // machine; refusing would strand it.
            let _ = self.run(&Self::argv(&["rm", "-f", "-q", &plan.name])).await;
        }

        let created = self.run(&create_args(plan)).await?;
        if created.code != 0 {
            return Err(Error::Unavailable {
                runtime: "microsandbox".to_string(),
                detail: created.stderr_utf8().trim().to_string(),
            });
        }
        Ok(())
    }

    async fn running(&self, name: &str) -> Result<bool> {
        let listed = self.run(&Self::argv(&["ls"])).await?;
        Ok(parse_running(&listed.stdout_utf8(), name))
    }

    async fn remove(&self, name: &str) -> Result<()> {
        let removed = self.run(&Self::argv(&["rm", "-f", "-q", name])).await?;
        if removed.code != 0 {
            return Err(Error::transport(
                removed.stderr_utf8().trim().to_string(),
                true,
            ));
        }
        Ok(())
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

        let mut args = vec!["exec".to_string()];
        for (key, value) in env {
            args.push("-e".to_string());
            args.push(format!("{key}={value}"));
        }
        args.push(name.to_string());
        args.push("--".to_string());
        args.extend(argv.iter().cloned());

        self.run(&args).await
    }

    async fn read(&self, name: &str, path: &str) -> Result<Vec<u8>> {
        let local = std::env::temp_dir().join(format!("computer-msb-{name}-read"));
        let _ = tokio::fs::remove_file(&local).await;

        let copied = self
            .run(&Self::argv(&[
                "cp",
                "-q",
                &format!("{name}:{path}"),
                &local.display().to_string(),
            ]))
            .await?;
        if copied.code != 0 {
            return Err(Error::denied(copied.stderr_utf8().trim().to_string()));
        }

        let bytes = tokio::fs::read(&local)
            .await
            .map_err(|error| Error::transport(error.to_string(), false))?;
        let _ = tokio::fs::remove_file(&local).await;
        Ok(bytes)
    }

    async fn write(&self, name: &str, path: &str, bytes: &[u8]) -> Result<()> {
        let local = std::env::temp_dir().join(format!("computer-msb-{name}-write"));
        tokio::fs::write(&local, bytes)
            .await
            .map_err(|error| Error::transport(error.to_string(), false))?;

        let copied = self
            .run(&Self::argv(&[
                "cp",
                "-q",
                &local.display().to_string(),
                &format!("{name}:{path}"),
            ]))
            .await;
        let _ = tokio::fs::remove_file(&local).await;

        match copied? {
            result if result.code == 0 => Ok(()),
            result => Err(Error::denied(result.stderr_utf8().trim().to_string())),
        }
    }

    async fn copy_in(&self, name: &str, from: &Path, to: &str) -> Result<()> {
        // Disk to disk: `msb cp` streams, so nothing is held in memory.
        let copied = self
            .run(&Self::argv(&[
                "cp",
                "-q",
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
                "cp",
                "-q",
                &format!("{name}:{from}"),
                &to.display().to_string(),
            ]))
            .await?;

        match copied.code {
            0 => Ok(()),
            _ => Err(Error::denied(copied.stderr_utf8().trim().to_string())),
        }
    }

    async fn logs(&self, name: &str) -> Result<String> {
        let logs = self.run(&Self::argv(&["logs", name])).await?;
        Ok(format!("{}{}", logs.stdout_utf8(), logs.stderr_utf8()))
    }
}

#[async_trait]
impl ImageLoader for Msb {
    async fn load(&self, archive: &Path, tag: &str) -> Result<()> {
        let loaded = self
            .run(&Self::argv(&[
                "load",
                "-q",
                "-i",
                &archive.display().to_string(),
                "-t",
                tag,
            ]))
            .await?;

        if loaded.code != 0 {
            return Err(Error::Unavailable {
                runtime: "microsandbox".to_string(),
                detail: loaded.stderr_utf8().trim().to_string(),
            });
        }
        Ok(())
    }
}

/// A microVM machine backed by the installed `msb`.
pub fn machine() -> MicroVm {
    MicroVm::new(Arc::new(Msb::found()))
        .named("microsandbox")
        .reaping_with(Msb::found().program().to_string(), ["rm", "-f", "-q", "{}"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_msb_arguments_carry_everything_the_plan_decided() {
        let plan = Plan {
            name: "box".to_string(),
            image: "computer-desktop:abc".to_string(),
            cpus: Some(2),
            memory_mib: Some(2048),
            network: false,
            env: BTreeMap::from([("COMPUTER_SCREEN_WIDTH".to_string(), "1280".to_string())]),
            ports: vec![(40000, 6080)],
            replace: true,
        };
        let args = create_args(&plan);

        assert_eq!(args[0], "create");
        assert!(args.contains(&"--no-net".to_string()));
        assert!(args.contains(&"127.0.0.1:40000:6080".to_string()));
        assert!(args.contains(&"COMPUTER_SCREEN_WIDTH=1280".to_string()));
        assert!(args.contains(&"2048".to_string()));
        assert_eq!(
            args.last().map(String::as_str),
            Some("computer-desktop:abc"),
            "the image is last, so nothing after it is read as a command"
        );
    }

    #[test]
    fn test_a_machine_that_is_listed_but_not_running_is_not_running() {
        let listed = "NAME    IMAGE       STATUS     CREATED\n                      box     desktop     stopped    2026-08-23\n                      other   desktop     running    2026-08-23\n";

        assert!(!parse_running(listed, "box"));
        assert!(parse_running(listed, "other"));
        assert!(!parse_running(listed, "absent"));
    }

    #[test]
    fn test_an_image_the_hypervisor_never_received_is_reported_missing() {
        let listed = "REFERENCE                 DIGEST         SIZE\n                      computer-desktop:abc      sha256:1234    396 MiB\n";

        assert!(parse_has_image(listed, "computer-desktop:abc"));
        assert!(
            !parse_has_image(listed, "computer-desktop:def"),
            "a different fingerprint is a different image"
        );
    }
}
