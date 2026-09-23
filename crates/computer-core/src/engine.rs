use crate::ExecResult;
use crate::config::{Config, PROFILES};
use crate::error::{Error, Result};
use async_trait::async_trait;
use std::collections::BTreeMap;

#[async_trait]
pub trait Engine: Send + Sync {
    async fn run(&self, args: &[String]) -> Result<ExecResult>;

    fn program(&self) -> &str;
}

#[derive(Debug, Clone)]
pub struct SystemEngine {
    program: String,
    before: Vec<String>,
}

impl Default for SystemEngine {
    fn default() -> Self {
        Self::new("docker")
    }
}

impl SystemEngine {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            before: Vec::new(),
        }
    }

    pub fn before(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.before = args.into_iter().map(Into::into).collect();
        self
    }
}

#[async_trait]
impl Engine for SystemEngine {
    async fn run(&self, args: &[String]) -> Result<ExecResult> {
        let output = tokio::process::Command::new(&self.program)
            .args(&self.before)
            .args(args)
            // A timeout drops the future; the process would otherwise run on unread.
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

fn arg(value: impl Into<String>) -> String {
    value.into()
}

pub fn run_args(name: &str, config: &Config) -> Vec<String> {
    let mut args = vec![
        arg("run"),
        arg("--detach"),
        arg("--name"),
        arg(name),
        // A screen that dies takes the container with it.
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
        args.push(arg("--publish"));
        args.push(format!("{}::{port}", config.bind.publish_prefix()));
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
    if let Some(isolation) = &config.isolation {
        args.push(arg("--runtime"));
        args.push(isolation.clone());
    }

    if let Some(volume) = &config.profiles {
        args.push(arg("--volume"));
        args.push(format!("{volume}:{PROFILES}"));
    }

    args.push(arg("--label"));
    args.push(arg("computer-rs=1"));
    for (key, value) in &config.labels {
        args.push(arg("--label"));
        args.push(format!("{key}={value}"));
    }

    // No command: the image's entrypoint is the supervisor that brings up the screen.
    args.push(config.image.clone());
    args
}

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
    use crate::bundle;

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
    fn test_a_box_the_engine_runs_elsewhere_names_that_oci_runtime() {
        let config = Config {
            isolation: Some("runsc".to_string()),
            ..Config::default()
        };
        assert!(values(&config, "--runtime").contains(&"runsc".to_string()));
        assert!(
            !run_args("box", &Config::default()).contains(&"--runtime".to_string()),
            "a box that asks for no isolation takes the engine's own default"
        );
    }

    #[tokio::test]
    async fn test_an_engine_flag_goes_before_the_subcommand() {
        let said = SystemEngine::new("echo")
            .before(["--context", "gpu-1"])
            .run(&[arg("version")])
            .await
            .expect("echo runs");

        assert_eq!(
            String::from_utf8_lossy(&said.stdout).trim(),
            "--context gpu-1 version",
            "--context is the engine's flag, not the subcommand's"
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
