use crate::error::{Error, Result};
use std::time::Duration;

pub const DEFAULT_MILLI_CPU: u32 = 2000;

pub const DEFAULT_MEMORY_MIB: u32 = 4096;

pub fn dockerfile_commands(dockerfile: &str) -> Vec<String> {
    dockerfile
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

pub fn from_registry(tag: &str) -> Vec<String> {
    vec![format!("FROM {tag}")]
}

pub fn is_image_id(image: &str) -> bool {
    image.starts_with("im-") && !image.contains([':', '/'])
}

pub fn milli_cpu(cpus: Option<&str>) -> Result<u32> {
    let Some(cpus) = cpus else {
        return Ok(DEFAULT_MILLI_CPU);
    };

    cpus.trim()
        .parse::<f64>()
        .ok()
        .filter(|cpus| *cpus > 0.0)
        .map(|cpus| (cpus * 1000.0).ceil() as u32)
        .ok_or_else(|| Error::denied(format!("{cpus} is not a number of CPUs")))
}

pub fn memory_mib(memory: Option<&str>) -> Result<u32> {
    let Some(memory) = memory else {
        return Ok(DEFAULT_MEMORY_MIB);
    };

    crate::microvm::mebibytes(memory)
        .and_then(|mib| u32::try_from(mib).ok())
        .ok_or_else(|| Error::denied(format!("{memory} is not an amount of memory")))
}

pub fn seconds(duration: Duration) -> u32 {
    u32::try_from(duration.as_secs().max(1)).unwrap_or(u32::MAX)
}

pub fn tunnel_url(host: &str, port: u32) -> String {
    match port {
        443 | 0 => format!("https://{host}"),
        port => format!("https://{host}:{port}"),
    }
}

pub fn exit_code(code: Option<i32>, signal: Option<i32>) -> Option<i32> {
    code.or(signal.map(|signal| 128 + signal))
}

pub fn jwt_expiry(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let mut standard: String = payload
        .chars()
        .map(|character| match character {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    while standard.len() % 4 != 0 {
        standard.push('=');
    }

    let claims: serde_json::Value =
        serde_json::from_slice(&crate::cdp::base64_decode(&standard)?).ok()?;
    claims.get("exp")?.as_u64()
}

pub fn exec_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| Error::transport(format!("no randomness: {error}"), false))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = |range: std::ops::Range<usize>| -> String {
        bytes[range]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    Ok(format!(
        "{}-{}-{}-{}-{}",
        hex(0..4),
        hex(4..6),
        hex(6..8),
        hex(8..10),
        hex(10..16)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_dockerfile_goes_as_its_lines() {
        assert_eq!(
            dockerfile_commands("FROM debian\n\nRUN a \\\n  && b\n"),
            ["FROM debian", "RUN a \\", "  && b"],
            "modal joins the lines again, so a continued line stays continued"
        );
    }

    #[test]
    fn test_a_modal_image_id_is_told_apart_from_a_registry_tag() {
        assert!(is_image_id("im-abc123"));
        assert!(!is_image_id("debian:bookworm"));
        assert!(!is_image_id("ghcr.io/im-x/y"));
    }

    #[test]
    fn test_resources_default_to_room_for_chromium() {
        assert_eq!(milli_cpu(None).expect("default"), 2000);
        assert_eq!(milli_cpu(Some("1.5")).expect("cpus"), 1500);
        assert_eq!(memory_mib(None).expect("default"), 4096);
        assert_eq!(memory_mib(Some("6g")).expect("memory"), 6144);
        assert!(milli_cpu(Some("lots")).is_err());
    }

    #[test]
    fn test_a_tunnel_on_443_needs_no_port() {
        assert_eq!(tunnel_url("abc.modal.host", 443), "https://abc.modal.host");
        assert_eq!(
            tunnel_url("abc.modal.host", 8443),
            "https://abc.modal.host:8443"
        );
    }

    #[test]
    fn test_a_signal_is_reported_as_a_shell_would() {
        assert_eq!(exit_code(Some(3), None), Some(3));
        assert_eq!(exit_code(None, Some(9)), Some(137));
        assert_eq!(exit_code(None, None), None);
    }

    #[test]
    fn test_a_token_expiry_is_read_from_its_claims() {
        let claims = crate::cdp::base64_encode(br#"{"exp":1790000000}"#)
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_");

        assert_eq!(jwt_expiry(&format!("h.{claims}.s")), Some(1_790_000_000));
        assert_eq!(jwt_expiry("not-a-jwt"), None);
    }

    #[test]
    fn test_an_exec_id_is_a_version_4_uuid() {
        let id = exec_id().expect("an id");

        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert_ne!(id, exec_id().expect("another"));
    }
}
