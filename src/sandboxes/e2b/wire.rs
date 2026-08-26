//! What goes on the wire, as pure functions.
//!
//! Bodies in, bytes out, and no client anywhere near it. The parts worth
//! testing — which JSON a plan turns into, and what a stream of Connect
//! envelopes means — are checkable with no account and no network, which is
//! the same trade [`crate::sandboxes::microsandbox::msb`] makes by shelling
//! out.

use super::api::{DEFAULT_USER, NAME_KEY, Sandbox, SandboxPlan};
use crate::error::{Error, Result};
use crate::exec::ExecResult;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The body of `POST /sandboxes`.
///
/// `secure` is not optional. Every other [`Machine`](crate::Machine) publishes
/// on loopback, where the bind is the whole authentication story; a sandbox is
/// on the public internet, and the screen still has no password on it.
pub fn new_sandbox(plan: &SandboxPlan) -> Value {
    let mut metadata: BTreeMap<&str, &str> = plan
        .metadata
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    metadata.insert(NAME_KEY, plan.name.as_str());

    json!({
        "templateID": plan.template,
        "timeout": plan.ttl.as_secs(),
        "secure": true,
        "allow_internet_access": plan.network,
        "metadata": metadata,
        "envVars": plan.env,
    })
}

pub fn sandbox_from(body: &Value) -> Result<Sandbox> {
    let id = body
        .get("sandboxID")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::transport("a sandbox with no sandboxID", false))?;

    Ok(Sandbox {
        id: id.to_string(),
        domain: body
            .get("domain")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        envd_token: body
            .get("envdAccessToken")
            .and_then(Value::as_str)
            .map(str::to_string),
        traffic_token: body
            .get("trafficAccessToken")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// The IDs in a listing whose metadata carries `key`, with its value.
///
/// Filtered here rather than trusted from the query. The control plane takes
/// the filter as one encoded string, and a filter it does not understand
/// answers with everything — which would read as a match.
pub fn carrying(listing: &Value, key: &str) -> Vec<(String, String)> {
    listing
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            let id = entry.get("sandboxID").and_then(Value::as_str)?;
            let value = entry.get("metadata")?.get(key).and_then(Value::as_str)?;
            Some((id.to_string(), value.to_string()))
        })
        .collect()
}

/// The `metadata` filter for a listing, as one encoded parameter.
pub fn metadata_query(key: &str, value: &str) -> String {
    format!("{}%3D{}", escape(key), escape(value))
}

/// The body of `POST /process.Process/Start`.
///
/// `argv` runs directly rather than through a shell. Everything this crate
/// sends is already a built argument list — a screen command, or `xdotool`
/// with coordinates — and putting a shell in front of it would make quoting
/// this side's problem for no gain.
pub fn start_request(argv: &[String], env: &BTreeMap<String, String>) -> Result<Value> {
    let (cmd, args) = argv
        .split_first()
        .ok_or_else(|| Error::denied("an empty command has nothing to run"))?;

    Ok(json!({
        "process": {
            "cmd": cmd,
            "args": args,
            "envs": env,
        },
    }))
}

/// Basic auth naming the user envd should run as, which is how envd takes it.
pub fn user_header(user: &str) -> String {
    format!("Basic {}", base64_encode(format!("{user}:").as_bytes()))
}

/// The same, for the user this crate always runs as.
pub fn default_user_header() -> String {
    user_header(DEFAULT_USER)
}

/// A payload wrapped as one Connect envelope.
///
/// **The request is framed too.** `Process.Start` is a streaming RPC, and a
/// streaming Connect body is enveloped in both directions. Sent as bare JSON
/// the server reads the first five bytes of `{"process"…` as a header and
/// refuses with *promised 577794671 bytes in enveloped message*.
pub fn enveloped(payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(payload.len() + 5);
    framed.push(0);
    framed.extend((payload.len() as u32).to_be_bytes());
    framed.extend(payload);
    framed
}

/// One envelope off a Connect stream: its flags, its payload, and what is left.
type Envelope<'a> = (u8, &'a [u8], &'a [u8]);

fn envelope(bytes: &[u8]) -> Result<Option<Envelope<'_>>> {
    if bytes.is_empty() {
        return Ok(None);
    }

    let header = bytes
        .first_chunk::<5>()
        .ok_or_else(|| Error::transport("a Connect envelope cut short of its header", false))?;

    let flags = header[0];
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;

    let rest = bytes.get(5..).unwrap_or_default();
    let payload = rest
        .get(..length)
        .ok_or_else(|| Error::transport("a Connect envelope shorter than its length", false))?;

    Ok(Some((
        flags,
        payload,
        rest.get(length..).unwrap_or_default(),
    )))
}

/// Connect's compression bit. Nothing here negotiates an encoding, so a set
/// bit is a payload this cannot read rather than one to decompress.
const COMPRESSED: u8 = 0x01;
/// The last envelope, which carries the stream's own outcome.
const END_OF_STREAM: u8 = 0x02;

/// What `timed_out` costs a caller who reads only the code.
///
/// `timeout(1)`'s number, so a deadline from E2B and a deadline from the
/// runner in `MachineHost` report the same thing.
const TIMEOUT_CODE: i32 = 124;

/// A whole `Process.Start` response, as one result.
///
/// Collected rather than streamed: every command here is bounded already, and
/// [`ExecResult`] holds the output whole, so a stream would give nothing back.
pub fn parse_events(body: &[u8]) -> Result<ExecResult> {
    let mut result = ExecResult::default();
    let mut ended = false;
    let mut rest = body;

    while let Some((flags, payload, next)) = envelope(rest)? {
        rest = next;

        if flags & COMPRESSED != 0 {
            return Err(Error::transport(
                "a compressed Connect envelope, which nothing here asked for",
                false,
            ));
        }

        let message: Value = serde_json::from_slice(payload)
            .map_err(|error| Error::transport(format!("a Connect envelope: {error}"), false))?;

        if flags & END_OF_STREAM != 0 {
            return finish(result, ended, &message);
        }

        let Some(event) = message.get("event") else {
            continue;
        };

        if let Some(data) = event.get("data") {
            if let Some(out) = data.get("stdout").and_then(Value::as_str) {
                result.stdout.extend(base64_decode(out)?);
            }
            if let Some(err) = data.get("stderr").and_then(Value::as_str) {
                result.stderr.extend(base64_decode(err)?);
            }
        }

        if let Some(end) = event.get("end") {
            ended = true;
            // Absent means zero: proto3 JSON leaves out a default, so a clean
            // exit carries no exitCode at all.
            result.code = end.get("exitCode").and_then(Value::as_i64).unwrap_or(0) as i32;

            if let Some(detail) = end.get("error").and_then(Value::as_str) {
                result.stderr.extend(detail.as_bytes());
            }
        }
    }

    Err(Error::transport(
        "the process stream ended without an end-of-stream envelope",
        true,
    ))
}

/// The end-of-stream envelope, which reports the stream rather than the
/// process.
///
/// A deadline is the one failure worth keeping as a result: the command really
/// did run and really was cut off, and a caller that only reads the code needs
/// to see that rather than a transport fault.
fn finish(mut result: ExecResult, ended: bool, message: &Value) -> Result<ExecResult> {
    let Some(error) = message.get("error") else {
        return match ended {
            true => Ok(result),
            false => Err(Error::transport(
                "the stream closed before the process did",
                true,
            )),
        };
    };

    let code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let detail = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the process stream failed");

    if code == "deadline_exceeded" {
        result.timed_out = true;
        result.code = TIMEOUT_CODE;
        return Ok(result);
    }

    Err(Error::transport(format!("{code}: {detail}"), false))
}

/// The boundary and body of a one-part upload.
///
/// Hand-built because envd wants exactly one part and a multipart crate would
/// arrive with a MIME database behind it.
pub fn multipart(filename: &str, bytes: &[u8], boundary: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(bytes.len() + 256);

    body.extend(format!("--{boundary}\r\n").as_bytes());
    body.extend(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());

    body
}

/// Everything outside the unreserved set, escaped.
///
/// Deliberately strict: a path or a name that needs no escaping is unchanged,
/// and anything else is encoded rather than guessed at.
pub fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                escaped.push(byte as char)
            }
            _ => escaped.push_str(&format!("%{byte:02X}")),
        }
    }
    escaped
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let held = chunk.len();
        let block = chunk
            .iter()
            .chain(std::iter::repeat(&0))
            .take(3)
            .fold(0u32, |block, byte| (block << 8) | u32::from(*byte));

        for slot in 0..4 {
            match slot <= held {
                true => encoded.push(ALPHABET[(block >> (18 - slot * 6)) as usize & 0x3f] as char),
                false => encoded.push('='),
            }
        }
    }
    encoded
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let mut decoded = Vec::with_capacity(text.len() / 4 * 3);
    let mut block = 0u32;
    let mut held = 0u32;

    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' => continue,
            _ => {
                return Err(Error::transport(
                    "output that is not base64, which is not what the protocol sends",
                    false,
                ));
            }
        };

        block = (block << 6) | u32::from(value);
        held += 6;

        if held >= 8 {
            held -= 8;
            decoded.push((block >> held) as u8);
        }
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn framed(flags: u8, payload: &str) -> Vec<u8> {
        let mut frame = vec![flags];
        frame.extend((payload.len() as u32).to_be_bytes());
        frame.extend(payload.as_bytes());
        frame
    }

    fn stream(events: &[&str]) -> Vec<u8> {
        let mut body: Vec<u8> = events.iter().flat_map(|e| framed(0, e)).collect();
        body.extend(framed(END_OF_STREAM, "{}"));
        body
    }

    #[test]
    fn test_a_sandbox_is_always_asked_for_secure() {
        let body = new_sandbox(&SandboxPlan {
            name: "box-7".to_string(),
            template: "tmpl-abc".to_string(),
            network: false,
            ttl: Duration::from_secs(600),
            ..SandboxPlan::default()
        });

        assert_eq!(body["secure"], json!(true));
        assert_eq!(
            body["allow_internet_access"],
            json!(false),
            "network(false) has to reach the sandbox, not just the flag"
        );
        assert_eq!(body["timeout"], json!(600));
        assert_eq!(body["metadata"][NAME_KEY], json!("box-7"));
    }

    #[test]
    fn test_the_name_survives_a_caller_who_set_their_own_metadata() {
        let body = new_sandbox(&SandboxPlan {
            name: "box-7".to_string(),
            metadata: BTreeMap::from([("owner".to_string(), "toby".to_string())]),
            ..SandboxPlan::default()
        });

        assert_eq!(body["metadata"]["owner"], json!("toby"));
        assert_eq!(body["metadata"][NAME_KEY], json!("box-7"));
    }

    #[test]
    fn test_a_created_sandbox_carries_both_tokens() {
        let sandbox = sandbox_from(&json!({
            "sandboxID": "i7q3",
            "domain": "e2b.app",
            "envdAccessToken": "envd-tok",
            "trafficAccessToken": "traffic-tok",
        }))
        .expect("a sandbox");

        assert_eq!(sandbox.id, "i7q3");
        assert_eq!(sandbox.envd_token.as_deref(), Some("envd-tok"));
        assert_eq!(sandbox.traffic_token.as_deref(), Some("traffic-tok"));
    }

    #[test]
    fn test_a_listing_is_filtered_here_rather_than_trusted() {
        let listing = json!([
            {"sandboxID": "a", "metadata": {NAME_KEY: "mine"}},
            {"sandboxID": "b", "metadata": {"other": "x"}},
            {"sandboxID": "c"},
        ]);

        assert_eq!(
            carrying(&listing, NAME_KEY),
            vec![("a".to_string(), "mine".to_string())],
            "a filter the server ignored must not read as a match"
        );
    }

    #[test]
    fn test_a_command_runs_without_a_shell_in_front_of_it() {
        let argv = ["xdotool".to_string(), "mousemove".to_string()];
        let body = start_request(&argv, &BTreeMap::from([("DISPLAY".into(), ":1".into())]))
            .expect("a request");

        assert_eq!(body["process"]["cmd"], json!("xdotool"));
        assert_eq!(body["process"]["args"], json!(["mousemove"]));
        assert_eq!(body["process"]["envs"]["DISPLAY"], json!(":1"));
    }

    #[test]
    fn test_an_empty_command_is_refused_before_it_is_sent() {
        assert!(start_request(&[], &BTreeMap::new()).is_err());
    }

    #[test]
    fn test_output_comes_back_off_the_envelopes_in_order() {
        let body = stream(&[
            r#"{"event":{"start":{"pid":41}}}"#,
            r#"{"event":{"data":{"stdout":"WD00Mgo="}}}"#,
            r#"{"event":{"data":{"stderr":"bm8K"}}}"#,
            r#"{"event":{"data":{"stdout":"WT05OQo="}}}"#,
            r#"{"event":{"end":{"exitCode":3,"exited":true}}}"#,
        ]);

        let result = parse_events(&body).expect("a result");
        assert_eq!(result.stdout_utf8(), "X=42\nY=99\n");
        assert_eq!(result.stderr_utf8(), "no\n");
        assert_eq!(result.code, 3);
        assert!(!result.timed_out);
    }

    #[test]
    fn test_a_clean_exit_carries_no_exit_code_at_all() {
        let body = stream(&[r#"{"event":{"end":{"exited":true}}}"#]);

        assert_eq!(
            parse_events(&body).expect("a result").code,
            0,
            "proto3 JSON leaves out a default, and absent is zero"
        );
    }

    #[test]
    fn test_a_stream_that_stops_before_the_process_is_not_a_success() {
        let body = framed(0, r#"{"event":{"start":{"pid":41}}}"#);
        let error = parse_events(&body).expect_err("no end-of-stream envelope");

        assert!(error.retryable(), "a cut connection is worth trying again");
    }

    #[test]
    fn test_a_closed_stream_with_no_end_event_is_not_exit_zero() {
        let body = stream(&[r#"{"event":{"start":{"pid":41}}}"#]);
        assert!(
            parse_events(&body).is_err(),
            "a process that never reported is not a process that exited cleanly"
        );
    }

    #[test]
    fn test_a_deadline_comes_back_as_a_result_and_not_a_fault() {
        let mut body = framed(0, r#"{"event":{"data":{"stdout":"aGkK"}}}"#);
        body.extend(framed(
            END_OF_STREAM,
            r#"{"error":{"code":"deadline_exceeded","message":"too slow"}}"#,
        ));

        let result = parse_events(&body).expect("a result, not an error");
        assert!(result.timed_out);
        assert_eq!(result.code, TIMEOUT_CODE);
        assert_eq!(
            result.stdout_utf8(),
            "hi\n",
            "what it managed to say is kept"
        );
    }

    #[test]
    fn test_any_other_stream_error_is_a_fault() {
        let body = framed(
            END_OF_STREAM,
            r#"{"error":{"code":"unauthenticated","message":"bad token"}}"#,
        );
        let error = parse_events(&body).expect_err("a refusal");

        assert!(error.to_string().contains("unauthenticated"));
        assert!(!error.retryable(), "a bad token does not improve on retry");
    }

    #[test]
    fn test_a_truncated_envelope_is_reported_rather_than_read_short() {
        let mut body = vec![0u8, 0, 0, 0, 9];
        body.extend(b"{}");
        assert!(parse_events(&body).is_err());
    }

    #[test]
    fn test_a_request_is_framed_the_same_way_an_answer_is() {
        let framed = enveloped(br#"{"process":{}}"#);

        assert_eq!(framed[0], 0, "not compressed, not end of stream");
        assert_eq!(&framed[1..5], &14u32.to_be_bytes());
        assert_eq!(
            envelope(&framed)
                .expect("it parses")
                .expect("one envelope")
                .1,
            br#"{"process":{}}"#,
            "what this sends is what it can read back"
        );
    }

    #[test]
    fn test_the_user_travels_as_basic_auth_with_no_password() {
        assert_eq!(default_user_header(), "Basic dXNlcjo=");
        assert_eq!(user_header("root"), "Basic cm9vdDo=");
    }

    #[test]
    fn test_base64_round_trips_including_the_awkward_lengths() {
        for text in ["", "a", "ab", "abc", "abcd", "X=42\nY=99\n"] {
            let encoded = base64_encode(text.as_bytes());
            let decoded = base64_decode(&encoded).expect("it decodes");
            assert_eq!(String::from_utf8_lossy(&decoded), text);
        }
    }

    #[test]
    fn test_base64_padding_matches_the_reference() {
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }

    #[test]
    fn test_binary_output_survives_the_round_trip() {
        let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe];
        let decoded = base64_decode(&base64_encode(&png)).expect("it decodes");
        assert_eq!(decoded, png, "a screenshot is not text");
    }

    #[test]
    fn test_a_path_is_escaped_but_its_separators_are_not() {
        assert_eq!(escape("/tmp/out.png"), "/tmp/out.png");
        assert_eq!(escape("/tmp/a b&c"), "/tmp/a%20b%26c");
    }

    #[test]
    fn test_the_metadata_filter_escapes_its_own_separator() {
        assert_eq!(
            metadata_query(NAME_KEY, "box-7"),
            "computer.name%3Dbox-7",
            "an unescaped = would end the parameter"
        );
    }

    #[test]
    fn test_an_upload_carries_the_bytes_between_its_boundaries() {
        let body = multipart("out.png", &[0x89, b'P'], "BOUND");
        let text = String::from_utf8_lossy(&body);

        assert!(text.starts_with("--BOUND\r\n"));
        assert!(text.contains("filename=\"out.png\""));
        assert!(text.ends_with("\r\n--BOUND--\r\n"));
        assert!(body.windows(2).any(|pair| pair == [0x89, b'P']));
    }
}
