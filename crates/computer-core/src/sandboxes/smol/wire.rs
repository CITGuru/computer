use super::{MOST_PORTS, REGISTRY};
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::sandboxes::remote::SandboxPlan;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const DEFAULT_CPUS: u64 = 2;

pub const DEFAULT_MEMORY_MB: u64 = 4096;

pub const PACK_REPOSITORY: &str = "computer-pack";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Image,
    Smolmachine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub kind: Kind,
    pub reference: String,
    pub env: BTreeMap<String, String>,
    pub workdir: Option<String>,
}

impl Source {
    pub fn image(reference: impl Into<String>) -> Self {
        Self {
            kind: Kind::Image,
            reference: reference.into(),
            env: BTreeMap::new(),
            workdir: None,
        }
    }
}

pub fn new_machine(plan: &SandboxPlan, source: &Source, ports: &[u16]) -> Result<Value> {
    let mut env = source.env.clone();
    env.extend(plan.env.clone());

    let mut body = json!({
        "name": plan.name,
        "source": {
            "type": match source.kind {
                Kind::Image => "image",
                Kind::Smolmachine => "smolmachine",
            },
            "reference": source.reference,
        },
        "resources": {
            "cpus": cpus(plan.cpus.as_deref())?,
            "memoryMb": memory_mb(plan.memory.as_deref())?,
        },
        "network": { "mode": if plan.network { "open" } else { "blocked" } },
        "env": env,
        "ports": ports
            .iter()
            .map(|port| json!({ "port": port }))
            .collect::<Vec<_>>(),
        "autoStopSeconds": plan.ttl.as_secs().max(60),
        "ephemeral": true,
    });

    if let Some(workdir) = &source.workdir {
        body["workdir"] = json!(workdir);
    }

    Ok(body)
}

pub fn published(plan: &SandboxPlan) -> Result<Vec<u16>> {
    crate::sandboxes::remote::fit_ports("smol", &plan.publish, MOST_PORTS)
}

pub fn ports_of(machine: &Value) -> Vec<u16> {
    machine
        .get("ports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|port| port.get("port")?.as_u64())
        .filter_map(|port| u16::try_from(port).ok())
        .collect()
}

fn cpus(cpus: Option<&str>) -> Result<u64> {
    let Some(cpus) = cpus else {
        return Ok(DEFAULT_CPUS);
    };

    cpus.trim()
        .parse::<f64>()
        .ok()
        .filter(|cpus| *cpus > 0.0)
        .map(|cpus| cpus.ceil() as u64)
        .ok_or_else(|| Error::denied(format!("{cpus} is not a number of CPUs")))
}

fn memory_mb(memory: Option<&str>) -> Result<u64> {
    let Some(memory) = memory else {
        return Ok(DEFAULT_MEMORY_MB);
    };

    crate::microvm::mebibytes(memory)
        .ok_or_else(|| Error::denied(format!("{memory} is not an amount of memory")))
}

pub fn id_of(machine: &Value) -> Result<String> {
    machine
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::transport("a machine with no id", false))
}

pub fn live(machine: &Value) -> bool {
    machine.get("state").and_then(Value::as_str) == Some("started")
}

pub fn named<'a>(listing: &'a Value, name: &str) -> Option<&'a Value> {
    listing
        .as_array()?
        .iter()
        .find(|machine| machine.get("name").and_then(Value::as_str) == Some(name) && live(machine))
}

pub fn started(listing: &Value) -> Vec<String> {
    listing
        .as_array()
        .into_iter()
        .flatten()
        .filter(|machine| live(machine))
        .filter_map(|machine| id_of(machine).ok())
        .collect()
}

pub fn command(
    argv: &[String],
    env: &BTreeMap<String, String>,
    stdin: Option<&str>,
    timeout_secs: u64,
) -> Result<Value> {
    if argv.is_empty() {
        return Err(Error::denied("an empty command has nothing to run"));
    }

    let mut body = json!({
        "command": argv,
        "env": env,
        "timeoutSeconds": timeout_secs,
    });
    if let Some(stdin) = stdin {
        body["stdin"] = json!(stdin);
    }
    Ok(body)
}

pub fn parse_command(answer: &Value) -> Result<ExecResult> {
    let code = answer
        .get("exitCode")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            Error::transport(
                format!(
                    "a command with no exit code: {}",
                    answer.get("error").unwrap_or(answer)
                ),
                false,
            )
        })?;

    let decode = |key: &str| {
        crate::cdp::base64_decode(answer.get(key).and_then(Value::as_str).unwrap_or_default())
            .ok_or_else(|| Error::transport(format!("{key} that is not base64"), false))
    };

    Ok(ExecResult {
        code: i32::try_from(code).unwrap_or(i32::MAX),
        stdout: decode("stdoutB64")?,
        stderr: decode("stderrB64")?,
        timed_out: false,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub registry: String,
    pub repository: String,
    pub tag: String,
}

pub fn reference(image: &str) -> Reference {
    let (name, tag) = match image.split_once('@') {
        Some((name, digest)) => (name, digest.to_string()),
        None => match image.rsplit_once(':') {
            Some((name, tag)) if !tag.contains('/') => (name, tag.to_string()),
            _ => (image, "latest".to_string()),
        },
    };

    let (registry, path) = match name.split_once('/') {
        Some((first, rest))
            if first.contains('.') || first.contains(':') || first == "localhost" =>
        {
            (first.to_string(), rest.to_string())
        }
        _ => ("registry-1.docker.io".to_string(), name.to_string()),
    };

    let repository = match registry == "registry-1.docker.io" && !path.contains('/') {
        true => format!("library/{path}"),
        false => path,
    };

    Reference {
        registry,
        repository,
        tag,
    }
}

impl Reference {
    pub fn manifest_url(&self) -> String {
        format!(
            "https://{}/v2/{}/manifests/{}",
            self.registry, self.repository, self.tag
        )
    }

    pub fn blob_url(&self, digest: &str) -> String {
        format!(
            "https://{}/v2/{}/blobs/{digest}",
            self.registry, self.repository
        )
    }

    pub fn at(&self, digest: &str) -> Self {
        Self {
            tag: digest.to_string(),
            ..self.clone()
        }
    }

    pub fn is_smol(&self) -> bool {
        self.registry == REGISTRY
    }
}

pub fn in_namespace(namespace: &str, repository: &str, tag: &str) -> String {
    format!("{REGISTRY}/{namespace}/{repository}:{tag}")
}

pub fn pack_tag(digest: &str) -> String {
    let hex = digest.trim_start_matches("sha256:");
    let short: String = hex.chars().take(24).collect();
    format!("pack-{short}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub realm: String,
    pub service: Option<String>,
    pub scope: Option<String>,
}

pub fn challenge(header: &str) -> Option<Challenge> {
    let params = header.strip_prefix("Bearer ")?;
    let mut found = BTreeMap::new();

    let mut rest = params.trim();
    while let Some((key, after)) = rest.split_once("=\"") {
        let (value, tail) = after.split_once('"')?;
        found.insert(
            key.trim().trim_start_matches(',').trim().to_string(),
            value.to_string(),
        );
        rest = tail.trim_start_matches(',').trim();
    }

    Some(Challenge {
        realm: found.remove("realm")?,
        service: found.remove("service"),
        scope: found.remove("scope").filter(|scope| !scope.is_empty()),
    })
}

pub const MANIFEST_TYPES: &str = "application/vnd.oci.image.index.v1+json, \
application/vnd.docker.distribution.manifest.list.v2+json, \
application/vnd.oci.image.manifest.v1+json, \
application/vnd.docker.distribution.manifest.v2+json";

pub fn amd64_of(index: &Value) -> Option<String> {
    index
        .get("manifests")?
        .as_array()?
        .iter()
        .find(|entry| {
            let platform = entry.get("platform");
            platform.and_then(|p| p.get("os")).and_then(Value::as_str) == Some("linux")
                && platform
                    .and_then(|p| p.get("architecture"))
                    .and_then(Value::as_str)
                    == Some("amd64")
        })?
        .get("digest")?
        .as_str()
        .map(str::to_string)
}

pub fn config_digest(manifest: &Value) -> Option<String> {
    manifest
        .pointer("/config/digest")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn image_settings(config: &Value) -> (BTreeMap<String, String>, Option<String>) {
    let inner = config.get("config").unwrap_or(config);

    let env = inner
        .get("Env")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

    let workdir = inner
        .get("WorkingDir")
        .and_then(Value::as_str)
        .filter(|dir| !dir.is_empty())
        .map(str::to_string);

    (env, workdir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandboxes::remote::NAME_KEY;

    fn plan() -> SandboxPlan {
        SandboxPlan {
            name: "desk-1".to_string(),
            image: "img".to_string(),
            publish: (6080..6096).chain([9223]).collect(),
            env: BTreeMap::from([("HOME".to_string(), "/box".to_string())]),
            metadata: BTreeMap::from([(NAME_KEY.to_string(), "desk-1".to_string())]),
            ..SandboxPlan::default()
        }
    }

    #[test]
    fn test_a_machine_publishes_as_many_ports_as_smol_takes() {
        let body = new_machine(
            &plan(),
            &Source::image("r/x:1"),
            &published(&plan()).expect("ports"),
        )
        .expect("a body");

        assert_eq!(
            ports_of(&body),
            vec![6080, 6081, 9223],
            "smol takes 4 ports: one screen's view and control, and DevTools"
        );
        assert_eq!(body["source"]["type"], "image");
        assert_eq!(body["ephemeral"], true, "a stopped machine is not kept");
        assert_eq!(body["autoStopSeconds"], 300);
    }

    #[test]
    fn test_a_pack_carries_the_image_env_that_the_box_can_override() {
        let source = Source {
            kind: Kind::Smolmachine,
            reference: "registry.smolmachines.com/t/p:pack-1".to_string(),
            env: BTreeMap::from([
                ("HOME".to_string(), "/home/computer".to_string()),
                ("DISPLAY".to_string(), ":1".to_string()),
            ]),
            workdir: Some("/home/computer".to_string()),
        };

        let body = new_machine(&plan(), &source, &[]).expect("a body");

        assert_eq!(body["source"]["type"], "smolmachine");
        assert_eq!(body["env"]["DISPLAY"], ":1", "a pack drops the image env");
        assert_eq!(body["env"]["HOME"], "/box", "the box's own env wins");
        assert_eq!(body["workdir"], "/home/computer");
    }

    #[test]
    fn test_no_network_is_blocked() {
        let body = new_machine(
            &SandboxPlan {
                network: false,
                ..plan()
            },
            &Source::image("r/x:1"),
            &[],
        )
        .expect("a body");

        assert_eq!(body["network"], json!({ "mode": "blocked" }));
    }

    #[test]
    fn test_binary_output_comes_back_whole() {
        let png = [0x89, b'P', b'N', b'G', 0x00, 0xff];
        let answer = json!({
            "exitCode": 3,
            "stdoutB64": crate::cdp::base64_encode(&png),
            "stderrB64": crate::cdp::base64_encode(b"err"),
        });

        let result = parse_command(&answer).expect("a result");
        assert_eq!(result.code, 3);
        assert_eq!(result.stdout, png);
        assert_eq!(result.stderr, b"err");
    }

    #[test]
    fn test_references_follow_dockers_rules() {
        assert_eq!(
            reference("debian:bookworm-slim"),
            Reference {
                registry: "registry-1.docker.io".to_string(),
                repository: "library/debian".to_string(),
                tag: "bookworm-slim".to_string(),
            }
        );
        assert_eq!(
            reference("ghcr.io/citguru/computer-desktop"),
            Reference {
                registry: "ghcr.io".to_string(),
                repository: "citguru/computer-desktop".to_string(),
                tag: "latest".to_string(),
            }
        );
        let local = reference("localhost:5000/a/b@sha256:abc");
        assert_eq!(local.registry, "localhost:5000");
        assert_eq!(local.tag, "sha256:abc");
        assert!(reference("registry.smolmachines.com/tenants/t/x:1").is_smol());
    }

    #[test]
    fn test_a_registry_challenge_names_where_to_get_a_token() {
        let found = challenge(
            r#"Bearer realm="https://api.smolmachines.com/v2/auth",service="registry.smolmachines.com",scope="""#,
        )
        .expect("a challenge");

        assert_eq!(found.realm, "https://api.smolmachines.com/v2/auth");
        assert_eq!(found.service.as_deref(), Some("registry.smolmachines.com"));
        assert_eq!(found.scope, None, "an empty scope is no scope");
        assert!(challenge("Basic realm=\"x\"").is_none());
    }

    #[test]
    fn test_the_amd64_manifest_is_picked_from_an_index() {
        let index = json!({ "manifests": [
            { "digest": "sha256:arm", "platform": { "os": "linux", "architecture": "arm64" } },
            { "digest": "sha256:amd", "platform": { "os": "linux", "architecture": "amd64" } },
        ]});

        assert_eq!(amd64_of(&index).as_deref(), Some("sha256:amd"));
    }

    #[test]
    fn test_env_and_workdir_are_read_from_the_image_config() {
        let config = json!({ "config": {
            "Env": ["DISPLAY=:1", "PATH=/usr/bin", "EMPTY="],
            "WorkingDir": "/home/computer",
        }});

        let (env, workdir) = image_settings(&config);

        assert_eq!(env.get("DISPLAY").map(String::as_str), Some(":1"));
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
        assert_eq!(workdir.as_deref(), Some("/home/computer"));
    }

    #[test]
    fn test_a_pack_is_named_by_the_image_it_came_from() {
        assert_eq!(
            pack_tag("sha256:0123456789abcdef0123456789abcdef"),
            "pack-0123456789abcdef01234567"
        );
        assert_eq!(
            in_namespace("tenants/t", PACK_REPOSITORY, "pack-1"),
            "registry.smolmachines.com/tenants/t/computer-pack:pack-1"
        );
    }

    #[test]
    fn test_a_machine_is_found_by_name_only_while_it_runs() {
        let listing = json!([
            { "id": "a", "name": "desk-1", "state": "stopped" },
            { "id": "b", "name": "desk-1", "state": "started" },
            { "id": "c", "name": "desk-2", "state": "started" },
        ]);

        assert_eq!(
            named(&listing, "desk-1").and_then(|m| m.get("id")),
            Some(&json!("b"))
        );
        assert_eq!(started(&listing), ["b", "c"]);
    }
}
