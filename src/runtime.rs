//! The container runtime, reached through its command line.
//!
//! `docker`, `podman` and `nerdctl` take the same arguments, so one
//! implementation reaches all three without a client library or a socket path.
//! Turning a [`Config`] into flags stays a pure function, testable with no
//! daemon.

use crate::ExecResult;
use crate::bundle;
use crate::error::{Error, Result};
use crate::image;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Everything this crate asks a container runtime to do.
#[async_trait]
pub trait ContainerCli: Send + Sync {
    /// Run the runtime with these arguments and wait for it.
    async fn run(&self, args: &[String]) -> Result<ExecResult>;

    /// The program being run, for the message when it is missing.
    fn program(&self) -> &str;
}

/// The `docker` on this host — or `podman`, or `nerdctl`.
#[derive(Debug, Clone)]
pub struct SystemDocker {
    program: String,
}

impl Default for SystemDocker {
    fn default() -> Self {
        Self::new("docker")
    }
}

impl SystemDocker {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

#[async_trait]
impl ContainerCli for SystemDocker {
    async fn run(&self, args: &[String]) -> Result<ExecResult> {
        let output = tokio::process::Command::new(&self.program)
            .args(args)
            // Killed with the future that owns it. A timeout drops that
            // future, and the process would otherwise carry on with nobody
            // reading it.
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => Error::Unavailable {
                    runtime: self.program.clone(),
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

    fn program(&self) -> &str {
        &self.program
    }
}

/// How a box is started.
///
/// Held apart from the box itself so the flags are a pure function of it, and
/// so a caller can print what would be run before anything runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub image: String,
    pub width: u32,
    pub height: u32,
    /// Off means `--network none`: a desktop with no way out.
    pub network: bool,
    /// The ports inside the box to map onto free host ports.
    ///
    /// Numbers rather than a flag, because which ports exist is the image's
    /// answer and this crate's images do not all use the same ones.
    pub publish: Vec<u16>,
    pub env: BTreeMap<String, String>,
    pub memory: Option<String>,
    pub cpus: Option<String>,
    /// Chromium's shared memory. The image already passes
    /// `--disable-dev-shm-usage`, so this is only for a caller who would
    /// rather give it the memory than the workaround.
    pub shm_size: Option<String>,
    pub labels: BTreeMap<String, String>,
    /// Packages to install into the image, which make it a different image.
    pub extras: bundle::Extras,
    /// The bytes to build this image from, where this crate carries them.
    ///
    /// `None` is an image to fetch. Named rather than guessed from the tag, so
    /// a caller's own `computer-desktop:mine` is not handed ours.
    pub bundle: Option<bundle::Bundle>,
    /// A caller-owned Docker build context.
    ///
    /// Its content-derived tag is built when absent. This and [`Config::bundle`]
    /// are mutually exclusive.
    pub image_dir: Option<PathBuf>,
    /// What to run to bring the box up, where the place has no entrypoint.
    ///
    /// Empty where the image starts itself, which is every container.
    pub boot: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            image: bundle::DESKTOP.tag(),
            // A bare configuration has no profile to ask. A builder replaces
            // both from the one it was given.
            width: image::WIDTH,
            height: image::HEIGHT,
            network: true,
            // Filled from the profile at launch: a bare configuration cannot
            // know which ports an image it has not been told about serves.
            publish: Vec::new(),
            env: BTreeMap::new(),
            memory: None,
            cpus: None,
            shm_size: None,
            labels: BTreeMap::new(),
            extras: bundle::Extras::none(),
            bundle: Some(bundle::DESKTOP),
            image_dir: None,
            boot: Vec::new(),
        }
    }
}

fn arg(value: impl Into<String>) -> String {
    value.into()
}

/// Turn a configuration into `docker run` arguments.
///
/// A pure function on purpose: it decides what the box is allowed to do, and
/// it is the one part that can be checked without starting anything.
pub fn run_args(name: &str, config: &Config) -> Vec<String> {
    let mut args = vec![
        arg("run"),
        arg("--detach"),
        arg("--name"),
        arg(name),
        // A screen that dies takes the container with it, so a box that looks
        // healthy is one with a screen in it.
        arg("--init"),
    ];

    for (key, value) in &config.env {
        args.push(arg("--env"));
        args.push(format!("{key}={value}"));
    }

    if !config.network {
        args.push(arg("--network"));
        args.push(arg("none"));
    }

    for port in &config.publish {
        // A free host port rather than the same number: two boxes on one
        // machine both want 6080. Loopback only, because the screen has no
        // password on it.
        args.push(arg("--publish"));
        args.push(format!("127.0.0.1::{port}"));
    }

    if let Some(memory) = &config.memory {
        args.push(arg("--memory"));
        args.push(memory.clone());
    }
    if let Some(cpus) = &config.cpus {
        args.push(arg("--cpus"));
        args.push(cpus.clone());
    }
    if let Some(shm) = &config.shm_size {
        args.push(arg("--shm-size"));
        args.push(shm.clone());
    }

    args.push(arg("--label"));
    args.push(arg("computer-rs=1"));
    for (key, value) in &config.labels {
        args.push(arg("--label"));
        args.push(format!("{key}={value}"));
    }

    // No command. The image's own entrypoint is the supervisor that brings up
    // the X server, the window manager and the browser; replacing it with a
    // sleep gives a container that answers exec and has no screen in it.
    args.push(config.image.clone());
    args
}

/// `docker port` output, as container port to host port.
///
/// A port bound on IPv4 and IPv6 appears twice; the first wins.
pub fn parse_ports(output: &str) -> BTreeMap<u16, u16> {
    let mut found = BTreeMap::new();

    for line in output.lines() {
        let Some((left, right)) = line.split_once("->") else {
            continue;
        };
        let Some(container) = left
            .trim()
            .split('/')
            .next()
            .and_then(|port| port.parse::<u16>().ok())
        else {
            continue;
        };
        let Some(host) = right
            .trim()
            .rsplit(':')
            .next()
            .and_then(|port| port.parse::<u16>().ok())
        else {
            continue;
        };
        found.entry(container).or_insert(host);
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value the given flag is passed, for a box started with `config`.
    fn values(config: &Config, flag: &str) -> Vec<String> {
        run_args("box", config)
            .windows(2)
            .filter(|pair| pair[0] == flag)
            .map(|pair| pair[1].clone())
            .collect()
    }

    #[test]
    fn test_the_image_command_is_never_overridden() {
        let args = run_args("box", &Config::default());
        assert_eq!(
            args.last().cloned(),
            Some(bundle::DESKTOP.tag()),
            "the image is the last argument, so nothing follows it as a command: \
             its entrypoint is what starts the screen"
        );
    }

    #[test]
    fn test_every_variable_the_configuration_carries_reaches_the_box() {
        let config = Config {
            env: BTreeMap::from([("SCREEN_WIDTH".to_string(), "1920".to_string())]),
            ..Config::default()
        };
        assert!(
            values(&config, "--env").contains(&"SCREEN_WIDTH=1920".to_string()),
            "which variables the image reads was resolved from its profile; \
             this only passes them on"
        );
    }

    #[test]
    fn test_a_box_with_no_network_says_so_to_the_runtime() {
        let config = Config {
            network: false,
            ..Config::default()
        };
        assert!(values(&config, "--network").contains(&"none".to_string()));
    }

    #[test]
    fn test_ports_are_published_on_loopback_and_never_on_a_fixed_host_port() {
        let config = Config {
            publish: vec![6080, 9223],
            ..Config::default()
        };
        let published = values(&config, "--publish");

        assert!(published.contains(&"127.0.0.1::6080".to_string()));
        assert!(published.contains(&"127.0.0.1::9223".to_string()));
        for mapping in &published {
            assert!(
                mapping.starts_with("127.0.0.1::"),
                "{mapping} pins a host port, so a second box on this machine \
                 would fail to start on a conflict that reads as a broken image"
            );
        }
    }

    #[test]
    fn test_publishing_nothing_asks_for_no_mappings() {
        assert!(values(&Config::default(), "--publish").is_empty());
    }

    #[test]
    fn test_only_the_ports_the_configuration_names_are_published() {
        let config = Config {
            publish: vec![6080, 9223],
            ..Config::default()
        };
        assert_eq!(
            values(&config, "--publish"),
            vec!["127.0.0.1::6080", "127.0.0.1::9223"],
            "which ports a box serves was decided by its profile, and this \
             only asks the runtime for them"
        );
    }

    #[test]
    fn test_a_port_mapping_is_read_back_from_the_runtime() {
        let mapped = parse_ports("6080/tcp -> 127.0.0.1:32768\n9222/tcp -> 0.0.0.0:32769\n");

        assert_eq!(mapped.get(&6080), Some(&32768));
        assert_eq!(mapped.get(&9222), Some(&32769));
    }

    #[test]
    fn test_a_port_bound_twice_is_reported_once() {
        let mapped = parse_ports("6080/tcp -> 0.0.0.0:32768\n6080/tcp -> [::]:32768\n");

        assert_eq!(mapped.len(), 1);
        assert_eq!(
            mapped.get(&6080),
            Some(&32768),
            "both lines reach the same listener; the first is the answer"
        );
    }

    #[test]
    fn test_junk_in_the_port_output_is_skipped_rather_than_guessed() {
        let mapped = parse_ports("no mappings\n6080/tcp -> 127.0.0.1:32768\n");
        assert_eq!(mapped.len(), 1);
    }
}
