use super::{MIB_PER_CPU, MOST_PORTS, MOST_TAG_KEY, MOST_TAG_VALUE, MOST_TAGS};
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::sandboxes::remote::{NAME_KEY, Sandbox, SandboxPlan};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

const LIVE: [&str; 3] = ["pending", "running", "snapshotting"];

pub fn new_sandbox(project: &str, plan: &SandboxPlan) -> Result<Value> {
    let mut body = json!({
        "projectId": project,
        "name": plan.name,
        "image": plan.image,
        "ports": fitted(&plan.publish)?,
        "timeout": millis(plan.ttl),
        "persistent": false,
        "env": plan.env,
        "tags": tags(&plan.metadata),
    });

    if !plan.network {
        body["networkPolicy"] = json!({ "mode": "deny-all" });
    }

    if let Some(vcpus) = vcpus(plan.cpus.as_deref(), plan.memory.as_deref())? {
        body["resources"] = json!({ "vcpus": vcpus });
    }

    Ok(body)
}

pub fn vcpus(cpus: Option<&str>, memory: Option<&str>) -> Result<Option<u64>> {
    let cpus = cpus
        .map(|cpus| {
            cpus.trim()
                .parse::<f64>()
                .ok()
                .filter(|cpus| *cpus > 0.0)
                .map(|cpus| cpus.ceil() as u64)
                .ok_or_else(|| Error::denied(format!("{cpus} is not a number of CPUs")))
        })
        .transpose()?;

    let for_memory = memory
        .map(|memory| {
            crate::microvm::mebibytes(memory)
                .map(|mib| mib.div_ceil(MIB_PER_CPU))
                .ok_or_else(|| Error::denied(format!("{memory} is not an amount of memory")))
        })
        .transpose()?;

    Ok(cpus.max(for_memory).map(|vcpus| match vcpus {
        0 | 1 => 1,
        odd if odd % 2 == 1 => odd + 1,
        even => even,
    }))
}

fn fitted(publish: &[u16]) -> Result<Vec<u16>> {
    if publish.len() <= MOST_PORTS {
        return Ok(publish.to_vec());
    }

    let run = publish
        .windows(2)
        .take_while(|pair| pair[1] == pair[0] + 1)
        .count()
        + 1;
    let (screens, rest) = publish.split_at(run);

    let room = MOST_PORTS.saturating_sub(rest.len()) / 2 * 2;
    if room == 0 {
        return Err(Error::Unsupported {
            gaps: vec!["more than 14 published ports on vercel"],
        });
    }

    let kept: Vec<u16> = screens[..room].iter().chain(rest).copied().collect();
    tracing::warn!(
        dropped = ?&screens[room..],
        "vercel publishes at most {MOST_PORTS} ports, so the screens past these have no viewer"
    );
    Ok(kept)
}

pub fn sandbox_from(answer: &Value) -> Result<(Sandbox, String)> {
    let name = answer
        .pointer("/sandbox/name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::transport("a sandbox with no name", false))?;

    let session = answer
        .pointer("/session/id")
        .or_else(|| answer.pointer("/sandbox/currentSessionId"))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::transport(format!("{name} came back with no session"), false))?;

    let mut sandbox = Sandbox::new(name);
    for route in answer
        .get("routes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let port = route.get("port").and_then(Value::as_u64);
        let url = route.get("url").and_then(Value::as_str);

        if let (Some(port), Some(url)) = (port.and_then(|port| u16::try_from(port).ok()), url) {
            sandbox.endpoints.insert(port, url.to_string());
        }
    }

    Ok((sandbox, session.to_string()))
}

pub fn live(answer: &Value) -> bool {
    answer
        .pointer("/sandbox/status")
        .and_then(Value::as_str)
        .is_some_and(|status| LIVE.contains(&status))
}

const ENCODED: &str =
    r#"out=$(mktemp) || exit 125; "$@" >"$out"; code=$?; base64 "$out"; rm -f "$out"; exit $code"#;

pub fn command(argv: &[String], env: &BTreeMap<String, String>) -> Result<Value> {
    if argv.is_empty() {
        return Err(Error::denied("an empty command has nothing to run"));
    }

    let args: Vec<&str> = ["-c", ENCODED, "computer"]
        .into_iter()
        .chain(argv.iter().map(String::as_str))
        .collect();

    Ok(json!({
        "command": "sh",
        "args": args,
        "env": env,
        "wait": true,
        "logs": true,
    }))
}

pub fn parse_command(body: &[u8]) -> Result<ExecResult> {
    let mut result = parse_stream(body)?;
    result.stdout = crate::cdp::base64_decode(&String::from_utf8_lossy(&result.stdout))
        .ok_or_else(|| Error::transport("stdout that is not the base64 it was sent as", false))?;
    Ok(result)
}

pub fn parse_stream(body: &[u8]) -> Result<ExecResult> {
    let mut result = ExecResult::default();
    let mut code = None;

    for line in body.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        let message: Value = serde_json::from_slice(line)
            .map_err(|error| Error::transport(format!("a command line: {error}"), false))?;

        if let Some(exit) = message.pointer("/command/exitCode") {
            code = exit.as_i64().or(code);
            continue;
        }

        let data = message.get("data");
        match message.get("stream").and_then(Value::as_str) {
            Some("stdout") => result
                .stdout
                .extend(data.and_then(Value::as_str).unwrap_or_default().bytes()),
            Some("stderr") => result
                .stderr
                .extend(data.and_then(Value::as_str).unwrap_or_default().bytes()),
            Some("error") => {
                let said = |key: &str| {
                    data.and_then(|data| data.get(key))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                return Err(Error::transport(
                    format!("{}: {}", said("code"), said("message")),
                    false,
                ));
            }
            _ => {}
        }
    }

    let code = code.ok_or_else(|| Error::transport("the command never finished", true))?;
    result.code = i32::try_from(code).unwrap_or(i32::MAX);
    Ok(result)
}

pub fn tags(metadata: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let fits = |(key, value): &(&String, &String)| {
        key.len() <= MOST_TAG_KEY && value.len() <= MOST_TAG_VALUE
    };

    metadata
        .get_key_value(NAME_KEY)
        .into_iter()
        .chain(metadata.iter().filter(|(key, _)| *key != NAME_KEY))
        .filter(fits)
        .take(MOST_TAGS)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub fn untagged(metadata: &BTreeMap<String, String>) -> bool {
    tags(metadata).len() < metadata.len()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub session: Option<String>,
    pub tags: BTreeMap<String, String>,
}

pub fn listed(listing: &Value) -> Vec<Listed> {
    listing
        .get("sandboxes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|sandbox| {
            sandbox
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| LIVE.contains(&status))
        })
        .filter_map(|sandbox| {
            Some(Listed {
                name: sandbox.get("name")?.as_str()?.to_string(),
                session: sandbox
                    .get("currentSessionId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tags: serde_json::from_value(sandbox.get("tags")?.clone()).unwrap_or_default(),
            })
        })
        .collect()
}

pub fn labels(file: &[u8]) -> BTreeMap<String, String> {
    serde_json::from_slice(file).unwrap_or_default()
}

pub fn next_page(listing: &Value) -> Option<String> {
    listing
        .pointer("/pagination/next")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn shortfall(session: &Value, ttl: Duration, now_ms: u64) -> Option<u64> {
    let session = session.get("session").unwrap_or(session);
    let started = session
        .get("startedAt")
        .or_else(|| session.get("requestedAt"))?
        .as_u64()?;
    let timeout = session.get("timeout")?.as_u64()?;

    let wanted = now_ms.checked_add(millis(ttl))?;
    wanted
        .checked_sub(started.saturating_add(timeout))
        .filter(|short| *short > 0)
}

pub fn image_missing(body: &str) -> bool {
    body.contains("Image not found")
}

pub fn image_not_ready(body: &str) -> bool {
    body.contains("image_not_ready")
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandboxes::remote::NAME_KEY;

    fn plan() -> SandboxPlan {
        SandboxPlan {
            name: "desk-1".to_string(),
            image: "computer-desktop:latest".to_string(),
            publish: vec![6080, 9223],
            metadata: BTreeMap::from([(NAME_KEY.to_string(), "desk-1".to_string())]),
            ..SandboxPlan::default()
        }
    }

    #[test]
    fn test_a_sandbox_is_asked_for_by_name_and_is_not_kept() {
        let body = new_sandbox("prj_1", &plan()).expect("a body");

        assert_eq!(body["name"], "desk-1");
        assert_eq!(body["projectId"], "prj_1");
        assert_eq!(body["ports"], json!([6080, 9223]));
        assert_eq!(body["timeout"], 300_000, "milliseconds, not seconds");
        assert_eq!(
            body["persistent"], false,
            "a persistent sandbox snapshots itself on stop, and that is paid for"
        );
        assert_eq!(body["tags"][NAME_KEY], "desk-1");
        assert!(body.get("networkPolicy").is_none());
    }

    #[test]
    fn test_memory_asks_for_the_vcpus_that_carry_it() {
        assert_eq!(
            vcpus(None, None).expect("none"),
            None,
            "vercel's own default"
        );
        assert_eq!(vcpus(Some("4"), None).expect("cpus"), Some(4));
        assert_eq!(vcpus(Some("1.5"), None).expect("cpus"), Some(2));
        assert_eq!(
            vcpus(None, Some("6g")).expect("memory"),
            Some(4),
            "3 vCPUs carry 6 GiB, and vercel takes 1 or an even count"
        );
        assert_eq!(vcpus(Some("1"), None).expect("one"), Some(1));
        assert_eq!(vcpus(Some("3"), None).expect("odd"), Some(4));
        assert_eq!(
            vcpus(Some("1"), Some("4g")).expect("both"),
            Some(2),
            "the larger of the two, so neither is short"
        );
        assert!(vcpus(Some("lots"), None).is_err());
    }

    #[test]
    fn test_resources_go_in_only_when_asked_for() {
        assert!(
            new_sandbox("prj_1", &plan())
                .expect("a body")
                .get("resources")
                .is_none()
        );

        let body = new_sandbox(
            "prj_1",
            &SandboxPlan {
                cpus: Some("4".to_string()),
                ..plan()
            },
        )
        .expect("a body");
        assert_eq!(body["resources"]["vcpus"], 4);
    }

    #[test]
    fn test_a_box_without_network_is_denied_all_of_it() {
        let body = new_sandbox(
            "prj_1",
            &SandboxPlan {
                network: false,
                ..plan()
            },
        )
        .expect("a body");

        assert_eq!(body["networkPolicy"]["mode"], "deny-all");
    }

    #[test]
    fn test_the_top_screens_give_way_and_the_bridge_stays() {
        let mut publish: Vec<u16> = (6080..6096).collect();
        publish.push(9223);

        let kept = fitted(&publish).expect("fitted");

        assert_eq!(kept.len(), 13, "vercel answers 500 to a 15th port");
        assert_eq!(
            kept.last(),
            Some(&9223),
            "DevTools is worth more than screens 6 and 7"
        );
        assert_eq!(
            kept[..12],
            (6080..6092).collect::<Vec<_>>()[..],
            "a screen keeps both of its ports or neither"
        );
    }

    #[test]
    fn test_ports_that_already_fit_are_left_alone() {
        assert_eq!(
            fitted(&[6080, 6081, 9223]).expect("fitted"),
            [6080, 6081, 9223]
        );
    }

    #[test]
    fn test_a_box_with_nothing_but_extra_ports_is_refused() {
        let publish: Vec<u16> = (1..=16).map(|port| port * 100).collect();

        assert!(matches!(
            fitted(&publish).expect_err("no screen fits"),
            Error::Unsupported { .. }
        ));
    }

    #[test]
    fn test_each_route_is_a_host_of_its_own() {
        let answer = json!({
            "sandbox": { "name": "desk-1", "status": "running", "currentSessionId": "sbx_old" },
            "session": { "id": "sbx_new" },
            "routes": [
                { "port": 6080, "url": "https://sb-aaa.vercel.run", "subdomain": "sb-aaa" },
                { "port": 9223, "url": "https://sb-bbb.vercel.run", "subdomain": "sb-bbb" },
            ],
        });

        let (sandbox, session) = sandbox_from(&answer).expect("a sandbox");

        assert_eq!(sandbox.id, "desk-1");
        assert_eq!(
            session, "sbx_new",
            "the session in the answer is the current one"
        );
        assert_eq!(sandbox.url(6080), Some("https://sb-aaa.vercel.run"));
        assert_eq!(
            sandbox.url(9223),
            Some("https://sb-bbb.vercel.run"),
            "no pattern gives a port's host: each is a random subdomain"
        );
    }

    #[test]
    fn test_a_command_carries_its_output_and_exit_code() {
        let body = concat!(
            r#"{"command":{"id":"cmd_1","exitCode":null}}"#,
            "\n",
            r#"{"data":"err\n","stream":"stderr"}"#,
            "\n",
            r#"{"data":"b25lCnR3bwo=\n","stream":"stdout"}"#,
            "\n",
            r#"{"command":{"id":"cmd_1","exitCode":3}}"#,
            "\n",
        );

        let result = parse_command(body.as_bytes()).expect("a result");

        assert_eq!(result.code, 3);
        assert_eq!(result.stdout, b"one\ntwo\n");
        assert_eq!(result.stderr, b"err\n");
    }

    #[test]
    fn test_a_command_runs_through_a_shell_that_encodes_its_output() {
        let argv = ["printf".to_string(), "%s".to_string(), "a b".to_string()];
        let body = command(&argv, &BTreeMap::new()).expect("a body");

        assert_eq!(body["command"], "sh");
        assert_eq!(
            body["args"],
            json!(["-c", ENCODED, "computer", "printf", "%s", "a b"]),
            "each argument stays one argument"
        );
    }

    #[test]
    fn test_binary_output_survives_the_trip() {
        let png = [0x89, b'P', b'N', b'G', 0x00, 0xff];
        let body = format!(
            "{}\n{}\n",
            json!({ "stream": "stdout", "data": crate::cdp::base64_encode(&png) }),
            json!({ "command": { "exitCode": 0 } }),
        );

        assert_eq!(
            parse_command(body.as_bytes()).expect("a result").stdout,
            png
        );
    }

    #[test]
    fn test_a_stream_that_ends_early_is_not_a_success() {
        let body = r#"{"command":{"id":"cmd_1","exitCode":null}}"#;

        let error = parse_command(body.as_bytes()).expect_err("no exit code");
        assert!(error.retryable());
    }

    #[test]
    fn test_an_error_in_the_stream_is_an_error() {
        let body = r#"{"stream":"error","data":{"code":"sandbox_stopped","message":"gone"}}"#;

        let error = parse_command(body.as_bytes()).expect_err("an error line");
        assert!(error.to_string().contains("sandbox_stopped"));
    }

    #[test]
    fn test_a_sweep_lists_live_sandboxes_only() {
        let listing = json!({
            "sandboxes": [
                { "name": "a", "status": "running", "currentSessionId": "sbx_a", "tags": { "computer.expires": "1" } },
                { "name": "b", "status": "stopped", "currentSessionId": "sbx_b", "tags": { "computer.expires": "2" } },
                { "name": "c", "status": "running", "tags": {} },
            ],
            "pagination": { "count": 3, "next": null },
        });

        let live = listed(&listing);

        assert_eq!(
            live.iter().map(|one| one.name.as_str()).collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!(live[0].session.as_deref(), Some("sbx_a"));
        assert_eq!(
            live[0].tags.get("computer.expires").map(String::as_str),
            Some("1")
        );
        assert_eq!(next_page(&listing), None);
    }

    #[test]
    fn test_the_name_is_always_a_tag_and_the_rest_fit_vercels_limits() {
        let mut metadata: BTreeMap<String, String> = (0..8)
            .map(|at| (format!("a.{at}"), "short".to_string()))
            .collect();
        metadata.insert(NAME_KEY.to_string(), "desk-1".to_string());
        metadata.insert("a.long".to_string(), "x".repeat(MOST_TAG_VALUE + 1));

        let tags = tags(&metadata);

        assert_eq!(tags.len(), MOST_TAGS, "vercel refuses a sixth tag");
        assert_eq!(
            tags.get(NAME_KEY).map(String::as_str),
            Some("desk-1"),
            "the name is how a box is found again, so it is never the one left out"
        );
        assert!(
            !tags.contains_key("a.long"),
            "vercel refuses a value over 256"
        );
        assert!(
            untagged(&metadata),
            "what was left out goes in the labels file"
        );
    }

    #[test]
    fn test_labels_that_all_fit_need_no_file() {
        let metadata = BTreeMap::from([(NAME_KEY.to_string(), "desk-1".to_string())]);

        assert!(!untagged(&metadata));
    }

    #[test]
    fn test_a_labels_file_that_is_not_json_carries_nothing() {
        assert_eq!(
            labels(br#"{"computer.server.box":"{}"}"#)
                .get("computer.server.box")
                .map(String::as_str),
            Some("{}")
        );
        assert!(labels(b"not json").is_empty());
    }

    #[test]
    fn test_the_deadline_is_pushed_only_as_far_as_the_ttl() {
        let session = json!({ "session": { "startedAt": 1_000, "timeout": 600_000 } });

        assert_eq!(
            shortfall(&session, Duration::from_secs(600), 301_000),
            Some(300_000),
            "half the TTL gone, so half of it is added back"
        );
        assert_eq!(
            shortfall(&session, Duration::from_secs(600), 1_000),
            None,
            "a deadline already far enough out is left alone"
        );
    }

    #[test]
    fn test_a_missing_image_is_told_apart_from_a_missing_sandbox() {
        assert!(image_missing(
            r#"{"error":{"code":"not_found","message":"Image not found."}}"#
        ));
        assert!(!image_missing(
            r#"{"error":{"code":"not_found","message":"Named sandbox 'x' not found for this project."}}"#
        ));
    }

    #[test]
    fn test_a_stopped_sandbox_is_not_live() {
        assert!(live(&json!({ "sandbox": { "status": "running" } })));
        assert!(!live(&json!({ "sandbox": { "status": "stopped" } })));
    }
}
