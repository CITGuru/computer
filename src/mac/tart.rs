//! Tart, as a backend.

use super::{Backend, Plan, Viewer, shell_quote};
use std::collections::BTreeMap;

/// The `tart` command.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tart;

impl Backend for Tart {
    fn program(&self) -> &str {
        "tart"
    }

    fn clone_args(&self, plan: &Plan) -> Vec<String> {
        clone_args(plan)
    }

    fn shape_args(&self, plan: &Plan) -> Vec<String> {
        set_args(plan)
    }

    fn run_args(&self, plan: &Plan) -> Vec<String> {
        run_args(plan)
    }

    fn exec_args(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Vec<String> {
        exec_args(name, argv, env)
    }

    fn write_args(&self, name: &str, path: &str) -> Vec<String> {
        let quoted = shell_quote(path);
        vec![
            "exec".to_string(),
            "-i".to_string(),
            name.to_string(),
            "sh".to_string(),
            "-c".to_string(),
            format!("mkdir -p \"$(dirname {quoted})\" && cat > {quoted}"),
        ]
    }

    fn address_args(&self, name: &str) -> Vec<String> {
        vec!["ip".to_string(), name.to_string()]
    }

    fn list_args(&self) -> Vec<String> {
        vec![
            "list".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ]
    }

    fn stop_args(&self, name: &str) -> Vec<String> {
        vec!["stop".to_string(), name.to_string()]
    }

    fn delete_args(&self, name: &str) -> Vec<String> {
        vec!["delete".to_string(), name.to_string()]
    }

    fn pull_args(&self, image: &str) -> Vec<String> {
        vec!["pull".to_string(), image.to_string()]
    }

    fn version_args(&self) -> Vec<String> {
        vec!["--version".to_string()]
    }

    fn parse_viewer(&self, line: &str) -> Option<Viewer> {
        parse_viewer(line)
    }

    fn parse_address(&self, output: &str) -> Option<String> {
        parse_address(output)
    }

    fn running_guests(&self, listing: &str) -> Option<usize> {
        running_guests(listing)
    }
}

fn arg(value: impl Into<String>) -> String {
    value.into()
}

/// The line `tart run --vnc-experimental` prints, as an endpoint.
pub fn parse_viewer(line: &str) -> Option<Viewer> {
    let url = line.split("vnc://").nth(1)?.trim();
    let (credential, authority) = url.rsplit_once('@')?;
    let (host, port) = authority.rsplit_once(':')?;

    Some(Viewer {
        host: host.to_string(),
        port: port.parse().ok()?,
        // `vnc://:password@` — the user half is empty and the password is
        // everything after the colon. Refused rather than truncated if it is
        // too short to gate anything, which is `Secret`'s rule and not ours.
        password: crate::Secret::new(credential.trim_start_matches(':')).ok()?,
    })
}

/// Clone the prepared image into the box this run will use.
pub fn clone_args(plan: &Plan) -> Vec<String> {
    vec![arg("clone"), plan.base.clone(), plan.name.clone()]
}

/// Shape the clone before it boots.
pub fn set_args(plan: &Plan) -> Vec<String> {
    let mut args = vec![
        arg("set"),
        plan.name.clone(),
        arg("--random-serial"),
        arg("--random-mac"),
    ];

    if let Some((width, height)) = plan.display {
        args.push(arg("--display"));
        args.push(format!("{width}x{height}px"));
    }
    if let Some(cpus) = plan.cpus {
        args.push(arg("--cpu"));
        args.push(cpus.to_string());
    }
    if let Some(memory) = plan.memory_mib {
        args.push(arg("--memory"));
        args.push(memory.to_string());
    }
    args
}

/// Bring the guest up.
pub fn run_args(plan: &Plan) -> Vec<String> {
    let mut args = vec![arg("run"), plan.name.clone(), arg("--no-graphics")];

    if let Some(flag) = plan.viewer.flag() {
        args.push(arg(flag));
    }
    args.push(arg("--no-clipboard"));

    // There is no guest with no network at all, so this is the closest thing
    // to `--network none` and must not be described as more than that.
    if !plan.network {
        args.push(arg("--net-host"));
    }
    args
}

/// Run a command inside the guest.
pub fn exec_args(name: &str, argv: &[String], env: &BTreeMap<String, String>) -> Vec<String> {
    let mut args = vec![arg("exec"), arg(name)];

    if env.is_empty() {
        args.extend_from_slice(argv);
        return args;
    }

    // `tart exec` takes no environment of its own, so it is set by the shell
    // that runs the command.
    let assignments: Vec<String> = env
        .iter()
        .map(|(key, value)| format!("{key}={}", shell_quote(value)))
        .collect();
    let command: Vec<String> = argv.iter().map(|part| shell_quote(part)).collect();

    args.push(arg("sh"));
    args.push(arg("-c"));
    args.push(format!(
        "export {}; exec {}",
        assignments.join(" "),
        command.join(" ")
    ));
    args
}

/// How many macOS guests are running, as `tart list --format json` reports it.
pub fn running_guests(listing: &str) -> Option<usize> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(listing).ok()?;

    Some(
        entries
            .iter()
            .filter(|entry| {
                let running = entry
                    .get("State")
                    .and_then(|state| state.as_str())
                    .is_some_and(|state| state.eq_ignore_ascii_case("running"));

                // Linux guests are not capped, so they are not counted.
                let darwin = entry
                    .get("OS")
                    .and_then(|os| os.as_str())
                    .is_none_or(|os| os.eq_ignore_ascii_case("darwin"));

                running && darwin
            })
            .count(),
    )
}

/// The address `tart ip` reports, or none while the guest has not been given
/// one.
pub fn parse_address(output: &str) -> Option<String> {
    let address = output.trim();
    (!address.is_empty() && address.parse::<std::net::IpAddr>().is_ok())
        .then(|| address.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac::{DESKTOP_MEMORY_MIB, ViewerMode, plan_for};
    use crate::runtime::Config;

    fn plan() -> Plan {
        plan_for(
            "box",
            &Config {
                image: "ghcr.io/cirruslabs/macos-sequoia-base:latest".to_string(),
                publish: vec![6080],
                ..Config::default()
            },
            vec![(50000, 6080)],
            ViewerMode::default(),
        )
    }

    #[test]
    fn test_a_desktop_gets_enough_memory_even_when_nobody_asks() {
        assert_eq!(plan().memory_mib, Some(DESKTOP_MEMORY_MIB));
    }
    #[test]
    fn test_a_clone_is_given_its_own_identity_after_it_is_cloned() {
        assert_eq!(
            clone_args(&plan()),
            [
                "clone",
                "ghcr.io/cirruslabs/macos-sequoia-base:latest",
                "box"
            ],
            "tart clone takes no shaping flags, and an unknown option there \
             fails the whole start"
        );

        let shaped = set_args(&plan());
        assert!(shaped.contains(&"--random-serial".to_string()));
        assert!(
            shaped.contains(&"--random-mac".to_string()),
            "two clones left with one image's serial and MAC collide: the \
             second refuses to boot or comes up with no network"
        );
    }

    #[test]
    fn test_a_display_size_carries_the_unit_it_is_measured_in() {
        let args = set_args(&plan());
        let size = args
            .iter()
            .position(|part| part == "--display")
            .and_then(|at| args.get(at + 1))
            .expect("a display size");

        assert!(
            size.ends_with("px"),
            "tart reads a bare size as points for a macOS guest, and a \
             screenshot comes back in pixels: a box pinned in points \
             misplaces every click on a 2x display and says nothing"
        );
        assert_eq!(size, "1280x800px");
    }

    #[test]
    fn test_a_box_starts_with_the_viewer_it_can_never_be_given_later() {
        let args = run_args(&plan());

        assert!(
            args.contains(&"--vnc-experimental".to_string()),
            "the host-side server needs nothing in the guest and answers \
             before it has logged in"
        );
        assert!(
            !args.contains(&"--vnc".to_string()),
            "plain --vnc is a different server, inside the guest"
        );
        assert!(
            args.contains(&"--no-clipboard".to_string()),
            "tart shares the host clipboard by default, and a box that reads \
             the host's clipboard is not isolated"
        );
        assert!(!args.contains(&"--net-host".to_string()));
    }

    #[test]
    fn test_a_box_with_no_network_gets_the_closest_thing_there_is() {
        let plan = Plan {
            network: false,
            ..plan()
        };

        assert!(
            run_args(&plan).contains(&"--net-host".to_string()),
            "no guest has no network at all, so this is the nearest thing and \
             must not be described as more"
        );
    }

    #[test]
    fn test_a_guest_with_no_viewer_is_not_asked_for_one() {
        let args = run_args(&Plan {
            viewer: ViewerMode::None,
            ..plan()
        });

        assert!(
            !args.iter().any(|part| part.starts_with("--vnc")),
            "asking a guest with neither server for a viewer starts one that \
             nothing answers on: {args:?}"
        );
    }

    #[test]
    fn test_a_command_with_no_environment_needs_no_shell() {
        let args = exec_args("box", &[arg("screencapture"), arg("-x")], &BTreeMap::new());

        assert_eq!(args, ["exec", "box", "screencapture", "-x"]);
    }

    #[test]
    fn test_an_environment_is_exported_by_the_shell_that_runs_the_command() {
        let env = BTreeMap::from([("COMPUTER_SCREEN".to_string(), "0".to_string())]);
        let args = exec_args("box", &[arg("computer-input"), arg("cursor")], &env);

        assert_eq!(args[2], "sh");
        assert!(args[4].contains("COMPUTER_SCREEN='0'"));
        assert!(
            args[4].contains("exec 'computer-input' 'cursor'"),
            "tart exec takes no environment of its own: {}",
            args[4]
        );
    }

    #[test]
    fn test_a_quote_in_an_argument_does_not_end_the_one_around_it() {
        let env = BTreeMap::from([("TEXT".to_string(), "it's".to_string())]);
        let args = exec_args("box", &[arg("echo")], &env);

        assert!(
            args[4].contains(r"'it'\''s'"),
            "an argument that closes its own quoting is a command the caller \
             never wrote: {}",
            args[4]
        );
    }

    #[test]
    fn test_only_running_macos_guests_count_against_the_limit() {
        let listing = r#"[
            {"Name":"one","State":"running","OS":"darwin"},
            {"Name":"two","State":"stopped","OS":"darwin"},
            {"Name":"three","State":"running","OS":"linux"}
        ]"#;

        assert_eq!(
            running_guests(listing),
            Some(1),
            "a stopped guest holds no framework slot, and Linux guests are \
             not capped at all"
        );
    }

    #[test]
    fn test_a_listing_that_cannot_be_read_is_not_an_empty_host() {
        assert_eq!(
            running_guests("not json"),
            None,
            "zero would let a third box be attempted, and the framework's own \
             refusal reads as a broken box rather than a full host"
        );
    }

    #[test]
    fn test_an_address_is_one_that_parses_rather_than_any_output() {
        assert_eq!(
            parse_address("192.168.64.5\n").as_deref(),
            Some("192.168.64.5")
        );
        assert_eq!(
            parse_address("Error: no IP found\n"),
            None,
            "a message read as an address is a relay pointed at nothing"
        );
        assert_eq!(parse_address(""), None);
    }
}
