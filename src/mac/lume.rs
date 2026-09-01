//! lume, as a backend.

use super::{Backend, Plan, Viewer, ViewerMode, shell_quote};
use std::collections::BTreeMap;

/// The user a lume guest is prepared with, and the one auto-login logs in.
pub const GUEST_USER: &str = "lume";

/// The `lume` command.
#[derive(Debug, Clone, Copy, Default)]
pub struct Lume;

fn arg(value: impl Into<String>) -> String {
    value.into()
}

impl Lume {
    /// Put a command in the logged-in GUI session.
    fn in_the_gui_session(command: &str) -> String {
        format!(
            "sudo launchctl asuser \"$(id -u {user})\" sudo -u {user} sh -c {}",
            shell_quote(command),
            user = GUEST_USER,
        )
    }
}

impl Backend for Lume {
    fn program(&self) -> &str {
        "lume"
    }

    /// No identity flags: lume's `clone` takes only names and storage.
    fn clone_args(&self, plan: &Plan) -> Vec<String> {
        vec![arg("clone"), plan.base.clone(), plan.name.clone()]
    }

    /// No unit on the display size: lume takes `WIDTHxHEIGHT`.
    fn shape_args(&self, plan: &Plan) -> Vec<String> {
        let mut args = vec![arg("set"), plan.name.clone()];

        if let Some((width, height)) = plan.display {
            args.push(arg("--display"));
            args.push(format!("{width}x{height}"));
        }
        if let Some(cpus) = plan.cpus {
            args.push(arg("--cpu"));
            args.push(cpus.to_string());
        }
        if let Some(memory) = plan.memory_mib {
            // Bare numbers are read as gigabytes, so the unit is written.
            args.push(arg("--memory"));
            args.push(format!("{memory}MB"));
        }
        args
    }

    /// `--detach` rather than a process this crate has to hold open.
    fn run_args(&self, plan: &Plan) -> Vec<String> {
        let mut args = vec![
            arg("run"),
            plan.name.clone(),
            arg("--detach"),
            arg("--no-display"),
        ];

        // There is no way to switch the listener off: lume 0.5.3 has
        // `--vnc-port` and `--vnc-password` but no `--vnc disabled`, and its
        // help says the VNC server remains available in every display mode. So
        // `ViewerMode::None` means this crate offers no viewer, not that the
        // guest has none — the same shape of caveat as an unfiltered port.
        if plan.viewer != ViewerMode::None {
            if let Some(port) = plan.viewer_port {
                args.push(arg("--vnc-port"));
                args.push(port.to_string());
            }
            if let Some(secret) = &plan.viewer_secret {
                args.push(arg("--vnc-password"));
                args.push(secret.expose().to_string());
            }
        }

        // Clipboard sharing is off unless asked for, which is the isolation a
        // box promises. Tart needed `--no-clipboard` to get here.
        args
    }

    fn exec_args(
        &self,
        name: &str,
        argv: &[String],
        env: &BTreeMap<String, String>,
    ) -> Vec<String> {
        let quoted: Vec<String> = argv.iter().map(|part| shell_quote(part)).collect();
        let mut command = String::new();

        if !env.is_empty() {
            let assignments: Vec<String> = env
                .iter()
                .map(|(key, value)| format!("{key}={}", shell_quote(value)))
                .collect();
            command.push_str(&format!("export {}; ", assignments.join(" ")));
        }
        command.push_str(&format!("exec {}", quoted.join(" ")));

        vec![
            arg("ssh"),
            arg(name),
            arg("--"),
            Self::in_the_gui_session(&command),
        ]
    }

    /// A file goes in on standard input, as it does everywhere else.
    fn write_args(&self, name: &str, path: &str) -> Vec<String> {
        let quoted = shell_quote(path);
        vec![
            arg("ssh"),
            arg(name),
            arg("--"),
            format!("mkdir -p \"$(dirname {quoted})\" && cat > {quoted}"),
        ]
    }

    /// There is no `ip` command; the address is a field of the VM's details.
    fn address_args(&self, name: &str) -> Vec<String> {
        vec![arg("get"), arg(name), arg("--format"), arg("json")]
    }

    /// `ls`, not `list`.
    fn list_args(&self) -> Vec<String> {
        vec![arg("ls"), arg("--format"), arg("json")]
    }

    fn stop_args(&self, name: &str) -> Vec<String> {
        vec![arg("stop"), arg(name)]
    }

    /// `--force`, or it waits for an answer nothing is there to give.
    fn delete_args(&self, name: &str) -> Vec<String> {
        vec![arg("delete"), arg(name), arg("--force")]
    }

    fn pull_args(&self, image: &str) -> Vec<String> {
        vec![arg("pull"), arg(image)]
    }

    fn version_args(&self) -> Vec<String> {
        vec![arg("--version")]
    }

    /// The viewer is whatever it was told to serve.
    fn announced_viewer(&self, plan: &Plan) -> Option<Viewer> {
        if plan.viewer == ViewerMode::None {
            return None;
        }
        Some(Viewer {
            host: "127.0.0.1".to_string(),
            port: plan.viewer_port?,
            password: plan.viewer_secret.clone()?,
        })
    }

    /// Never reached: [`Backend::announced_viewer`] always answers first.
    fn parse_viewer(&self, _line: &str) -> Option<Viewer> {
        None
    }

    fn parse_address(&self, output: &str) -> Option<String> {
        let details: serde_json::Value = serde_json::from_str(output).ok()?;
        let address = details
            .get("ipAddress")
            .or_else(|| details.get("ip"))?
            .as_str()?
            .trim();

        (!address.is_empty() && address.parse::<std::net::IpAddr>().is_ok())
            .then(|| address.to_string())
    }

    fn running_guests(&self, listing: &str) -> Option<usize> {
        let entries: Vec<serde_json::Value> = serde_json::from_str(listing).ok()?;

        Some(
            entries
                .iter()
                .filter(|entry| {
                    let running = entry
                        .get("state")
                        .or_else(|| entry.get("status"))
                        .and_then(|state| state.as_str())
                        .is_some_and(|state| state.eq_ignore_ascii_case("running"));

                    // Linux guests are not capped, so they are not counted.
                    let darwin = entry
                        .get("os")
                        .and_then(|os| os.as_str())
                        .is_none_or(|os| !os.eq_ignore_ascii_case("linux"));

                    running && darwin
                })
                .count(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac::{ViewerMode, plan_for};
    use crate::runtime::Config;

    fn plan() -> Plan {
        let mut plan = plan_for(
            "box",
            &Config {
                image: "ghcr.io/trycua/macos-sequoia-cua:latest".to_string(),
                ..Config::default()
            },
            Vec::new(),
            ViewerMode::default(),
        );
        plan.viewer_port = Some(51000);
        plan.viewer_secret = crate::Secret::new("a-secret-long-enough-to-gate").ok();
        plan
    }

    #[test]
    fn test_the_viewer_is_gated_by_this_crates_secret_and_not_an_invented_one() {
        let viewer = Lume.announced_viewer(&plan()).expect("a viewer");

        assert_eq!(viewer.port, 51000);
        assert_eq!(viewer.password.expose(), "a-secret-long-enough-to-gate");

        let args = Lume.run_args(&plan());
        assert!(args.contains(&"--vnc-port".to_string()));
        assert!(
            args.contains(&"--vnc-password".to_string()),
            "a runtime that takes a secret is one Auth can gate; tart mints \
             its own and this crate can only read it back: {args:?}"
        );
    }

    #[test]
    fn test_a_box_with_no_viewer_is_offered_none_even_though_one_listens() {
        let dark = Plan {
            viewer: ViewerMode::None,
            ..plan()
        };
        let args = Lume.run_args(&dark);

        assert!(!args.contains(&"--vnc-port".to_string()));
        assert!(!args.contains(&"--vnc-password".to_string()));
        assert!(
            !args.iter().any(|part| part == "--vnc"),
            "lume 0.5.3 answers `Unknown option '--vnc'`, so a policy flag \
             here fails the whole start: {args:?}"
        );
        assert!(
            Lume.announced_viewer(&dark).is_none(),
            "the listener cannot be switched off, so this means no viewer is \
             offered rather than none exists"
        );
    }

    #[test]
    fn test_the_box_is_started_detached_rather_than_held_open() {
        let args = Lume.run_args(&plan());

        assert!(
            args.contains(&"--detach".to_string()),
            "holding the process open is what took a guest down when a pipe \
             nobody read filled: {args:?}"
        );
    }

    #[test]
    fn test_every_command_is_put_in_the_logged_in_gui_session() {
        let args = Lume.exec_args(
            "box",
            &["screencapture".to_string(), "-x".to_string()],
            &BTreeMap::new(),
        );
        let command = args.last().expect("a command");

        assert!(
            command.contains("launchctl asuser"),
            "ssh lands outside the GUI session, where screencapture returns \
             nothing and CGEventPost reaches nothing — both reporting \
             success: {command}"
        );
        assert!(command.starts_with("sudo launchctl asuser"));
        assert!(command.contains("screencapture") && command.contains("-x"));
        assert!(
            command.contains(r"'\''"),
            "the inner command is quoted into a shell that is itself an \
             argument, so each of its own quotes has to survive twice: {command}"
        );
    }

    #[test]
    fn test_an_environment_survives_both_layers_of_quoting() {
        let env = BTreeMap::from([("TEXT".to_string(), "it's".to_string())]);
        let args = Lume.exec_args("box", &["echo".to_string()], &env);
        let command = args.last().expect("a command");

        assert!(
            command.contains("TEXT="),
            "the variable has to survive being quoted into a shell that is \
             itself quoted into another: {command}"
        );
        assert!(command.contains("launchctl asuser"));
    }

    #[test]
    fn test_a_file_goes_in_without_crossing_into_the_session() {
        let args = Lume.write_args("box", "/tmp/probe");
        let command = args.last().expect("a command");

        assert!(command.contains("cat >"));
        assert!(
            !command.contains("launchctl"),
            "a file needs no window server, and the extra sudo would land it \
             owned by the wrong user: {command}"
        );
    }

    #[test]
    fn test_a_display_size_carries_no_unit_here() {
        let args = Lume.shape_args(&plan());
        let size = args
            .iter()
            .position(|part| part == "--display")
            .and_then(|at| args.get(at + 1))
            .expect("a size");

        assert_eq!(size, "1280x800", "lume takes WIDTHxHEIGHT, tart wants px");
        assert!(
            args.iter().any(|part| part.ends_with("MB")),
            "a bare number is read as gigabytes: {args:?}"
        );
    }

    #[test]
    fn test_the_listing_verb_is_the_one_lume_has() {
        assert_eq!(Lume.list_args()[0], "ls", "there is no `list`");
        assert!(
            Lume.delete_args("box").contains(&"--force".to_string()),
            "without it, delete waits for an answer nothing is there to give"
        );
    }

    #[test]
    fn test_only_running_macos_guests_count_against_the_limit() {
        let listing = r#"[
            {"name":"one","state":"running","os":"macOS"},
            {"name":"two","state":"stopped","os":"macOS"},
            {"name":"three","state":"running","os":"linux"}
        ]"#;

        assert_eq!(Lume.running_guests(listing), Some(1));
        assert_eq!(
            Lume.running_guests(r#"[{"name":"a","status":"running","os":"darwin"}]"#),
            Some(1),
            "the field is `status` in lume's own VMDetails, and an os value \
             nobody expected has to count rather than be skipped"
        );
        assert_eq!(
            Lume.running_guests("not json"),
            None,
            "a guess too low lets the framework refuse the box instead, which \
             reads as a broken one"
        );
    }

    #[test]
    fn test_an_address_comes_out_of_the_details_rather_than_an_ip_command() {
        let details = r#"{"name":"box","ipAddress":"192.168.64.5","state":"running"}"#;

        assert_eq!(Lume.parse_address(details).as_deref(), Some("192.168.64.5"));
        assert_eq!(
            Lume.parse_address(r#"{"name":"box","state":"stopped"}"#),
            None,
            "a box with no address yet is not one at some default"
        );
        assert_eq!(Lume.parse_address("not json"), None);
    }
}
