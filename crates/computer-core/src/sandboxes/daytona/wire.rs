use super::dockerfile::quote;
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use crate::sandboxes::remote::SandboxPlan;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

pub const DEFAULT_CPUS: u64 = 2;

pub const DEFAULT_MEMORY_GIB: u64 = 4;

pub const DEFAULT_DISK_GIB: u64 = 10;

pub const LIVE: [&str; 6] = [
    "creating",
    "pending_build",
    "building_snapshot",
    "starting",
    "started",
    "restoring",
];

pub const FAILED: [&str; 3] = ["error", "build_failed", "destroyed"];

const STDERR_MARK: &str = "@@computer-stderr@@";

const ENCODED: &str = r#"out=$(mktemp) && err=$(mktemp) || exit 125; "$@" >"$out" 2>"$err"; code=$?; base64 "$out"; echo @@computer-stderr@@; base64 "$err"; rm -f "$out" "$err"; exit $code"#;

pub enum Source<'a> {
    Dockerfile(&'a str),
    Snapshot(&'a str),
}

pub fn new_sandbox(plan: &SandboxPlan, source: Source<'_>) -> Result<Value> {
    let mut body = json!({
        "name": plan.name,
        "env": plan.env,
        "labels": plan.metadata,
        "public": plan.public,
        "networkBlockAll": !plan.network,
        "cpu": cpus(plan.cpus.as_deref())?,
        "memory": memory_gib(plan.memory.as_deref())?,
        "disk": DEFAULT_DISK_GIB,
        "autoStopInterval": minutes(plan.ttl),
        "autoDeleteInterval": 0,
    });

    match source {
        Source::Dockerfile(content) => {
            body["buildInfo"] = json!({ "dockerfileContent": content });
        }
        Source::Snapshot(name) => body["snapshot"] = json!(name),
    }

    Ok(body)
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

fn memory_gib(memory: Option<&str>) -> Result<u64> {
    let Some(memory) = memory else {
        return Ok(DEFAULT_MEMORY_GIB);
    };

    crate::microvm::mebibytes(memory)
        .map(|mib| mib.div_ceil(1024).max(1))
        .ok_or_else(|| Error::denied(format!("{memory} is not an amount of memory")))
}

pub fn minutes(duration: Duration) -> u64 {
    duration.as_secs().div_ceil(60).max(1)
}

pub fn parse(body: &[u8]) -> Result<Value> {
    let text = String::from_utf8_lossy(body);

    serde_json::from_str(&text)
        .or_else(|_| serde_json::from_str(&escape_controls(&text)))
        .map_err(|error| Error::transport(format!("an answer that is not JSON: {error}"), false))
}

fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;

    for character in text.chars() {
        match character {
            '"' if !escaped => in_string = !in_string,
            '\n' if in_string => {
                out.push_str("\\n");
                continue;
            }
            '\r' if in_string => {
                out.push_str("\\r");
                continue;
            }
            '\t' if in_string => {
                out.push_str("\\t");
                continue;
            }
            _ => {}
        }
        escaped = character == '\\' && !escaped;
        out.push(character);
    }

    out
}

pub fn id_of(sandbox: &Value) -> Result<String> {
    sandbox
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::transport("a sandbox with no id", false))
}

pub fn state_of(sandbox: &Value) -> String {
    sandbox
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn why_of(sandbox: &Value) -> String {
    sandbox
        .get("errorReason")
        .and_then(Value::as_str)
        .unwrap_or("no reason given")
        .to_string()
}

pub fn toolbox_of(sandbox: &Value) -> Option<String> {
    let base = sandbox
        .get("toolboxProxyUrl")?
        .as_str()?
        .trim_end_matches('/');
    let id = sandbox.get("id")?.as_str()?;
    Some(format!("{base}/{id}"))
}

pub fn command(
    argv: &[String],
    env: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<Value> {
    if argv.is_empty() {
        return Err(Error::denied("an empty command has nothing to run"));
    }

    let words: Vec<String> = ["sh", "-c", ENCODED, "computer"]
        .into_iter()
        .map(quote)
        .chain(argv.iter().map(|arg| quote(arg)))
        .collect();

    Ok(json!({
        "command": words.join(" "),
        "envs": env,
        "timeout": timeout.as_secs(),
    }))
}

pub fn parse_command(answer: &Value) -> Result<ExecResult> {
    let text = answer
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let code = answer
        .get("exitCode")
        .or_else(|| answer.get("code"))
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::transport("a command with no exit code", false))?;

    let Some((stdout, stderr)) = text.split_once(STDERR_MARK) else {
        return Err(Error::Failed {
            code: i32::try_from(code).unwrap_or(i32::MAX),
            stderr: text.trim().to_string(),
        });
    };

    let decode = |part: &str| {
        crate::cdp::base64_decode(part.trim())
            .ok_or_else(|| Error::transport("output that is not the base64 it was sent as", false))
    };

    Ok(ExecResult {
        code: i32::try_from(code).unwrap_or(i32::MAX),
        stdout: decode(stdout)?,
        stderr: decode(stderr)?,
        timed_out: false,
    })
}

pub fn carrying(listing: &Value, key: &str) -> Vec<(String, String)> {
    listing
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|sandbox| LIVE.contains(&state_of(sandbox).as_str()))
        .filter_map(|sandbox| {
            let id = sandbox.get("id")?.as_str()?;
            let value = sandbox.get("labels")?.get(key)?.as_str()?;
            Some((id.to_string(), value.to_string()))
        })
        .collect()
}

pub fn next_page(listing: &Value) -> Option<String> {
    listing
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn url_of(answer: &Value) -> Option<String> {
    answer
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandboxes::remote::NAME_KEY;

    fn plan() -> SandboxPlan {
        SandboxPlan {
            name: "desk-1".to_string(),
            image: "computer-desktop:abc".to_string(),
            publish: vec![6080, 9223],
            metadata: BTreeMap::from([(NAME_KEY.to_string(), "desk-1".to_string())]),
            ..SandboxPlan::default()
        }
    }

    #[test]
    fn test_a_sandbox_goes_away_when_it_stops_and_stops_when_idle() {
        let body = new_sandbox(&plan(), Source::Snapshot("snap")).expect("a body");

        assert_eq!(body["name"], "desk-1");
        assert_eq!(body["labels"][NAME_KEY], "desk-1");
        assert_eq!(body["autoStopInterval"], 5, "the TTL, in minutes");
        assert_eq!(
            body["autoDeleteInterval"], 0,
            "a stopped sandbox that is kept is billed for its disk"
        );
        assert_eq!(body["snapshot"], "snap");
        assert!(body.get("buildInfo").is_none());
    }

    #[test]
    fn test_the_desktop_gets_room_by_default() {
        let body = new_sandbox(&plan(), Source::Dockerfile("FROM x")).expect("a body");

        assert_eq!(body["cpu"], 2);
        assert_eq!(
            body["memory"], 4,
            "daytona's own default of 1 GiB is too small for Chromium"
        );
        assert_eq!(body["buildInfo"]["dockerfileContent"], "FROM x");
    }

    #[test]
    fn test_cpus_and_memory_are_whole_units() {
        let body = new_sandbox(
            &SandboxPlan {
                cpus: Some("1.5".to_string()),
                memory: Some("6000m".to_string()),
                ..plan()
            },
            Source::Snapshot("snap"),
        )
        .expect("a body");

        assert_eq!(body["cpu"], 2);
        assert_eq!(body["memory"], 6, "GiB, rounded up");
    }

    #[test]
    fn test_no_network_blocks_it_all() {
        let body = new_sandbox(
            &SandboxPlan {
                network: false,
                ..plan()
            },
            Source::Snapshot("snap"),
        )
        .expect("a body");

        assert_eq!(body["networkBlockAll"], true);
    }

    #[test]
    fn test_each_argument_stays_one_word() {
        let body = command(
            &["echo".to_string(), "it's two words".to_string()],
            &BTreeMap::new(),
            Duration::from_secs(150),
        )
        .expect("a body");

        let line = body["command"].as_str().expect("a string");
        assert!(line.starts_with("'sh' '-c' "));
        assert!(line.ends_with(r"'computer' 'echo' 'it'\''s two words'"));
        assert_eq!(body["timeout"], 150);
    }

    #[test]
    fn test_both_streams_come_back_whole() {
        let png = [0x89, b'P', b'N', b'G', 0x00, 0xff];
        let answer = json!({
            "exitCode": 3,
            "result": format!(
                "{}\n{STDERR_MARK}\n{}\n",
                crate::cdp::base64_encode(&png),
                crate::cdp::base64_encode(b"err\n")
            ),
        });

        let result = parse_command(&answer).expect("a result");

        assert_eq!(result.code, 3);
        assert_eq!(result.stdout, png, "daytona merges the streams into text");
        assert_eq!(result.stderr, b"err\n");
    }

    #[test]
    fn test_a_shell_that_never_ran_says_why() {
        let answer = json!({ "exitCode": 127, "result": "sh: not found" });

        let Err(Error::Failed { code, stderr }) = parse_command(&answer) else {
            panic!("the wrapper never ran");
        };
        assert_eq!(code, 127);
        assert_eq!(stderr, "sh: not found");
    }

    #[test]
    fn test_an_answer_with_raw_newlines_is_read() {
        let answer =
            parse(b"{\"id\":\"a\",\"buildInfo\":{\"dockerfileContent\":\"FROM x\nRUN y\"}}")
                .expect("read");

        assert_eq!(answer["id"], "a");
        assert_eq!(answer["buildInfo"]["dockerfileContent"], "FROM x\nRUN y");
    }

    #[test]
    fn test_a_sweep_reads_labels_of_live_sandboxes() {
        let listing = json!({
            "items": [
                { "id": "a", "state": "started", "labels": { "computer.expires": "1" } },
                { "id": "b", "state": "destroyed", "labels": { "computer.expires": "2" } },
                { "id": "c", "state": "started", "labels": {} },
            ],
            "nextCursor": null,
        });

        assert_eq!(
            carrying(&listing, "computer.expires"),
            vec![("a".to_string(), "1".to_string())]
        );
        assert_eq!(next_page(&listing), None);
    }

    #[test]
    fn test_commands_go_to_the_sandbox_under_its_toolbox() {
        let sandbox = json!({ "id": "abc", "toolboxProxyUrl": "https://proxy.x/toolbox/" });

        assert_eq!(
            toolbox_of(&sandbox).as_deref(),
            Some("https://proxy.x/toolbox/abc")
        );
    }

    #[test]
    fn test_a_ttl_under_a_minute_is_a_minute() {
        assert_eq!(minutes(Duration::from_secs(20)), 1);
        assert_eq!(minutes(Duration::from_secs(61)), 2);
    }
}
