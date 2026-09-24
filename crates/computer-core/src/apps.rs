use crate::error::{Error, Result};
use computer_types::{App, Source, Spec, WindowMatch};
use std::collections::BTreeMap;

/// Measured: GIMP settles ~300ms after it maps, VS Code ~3s.
pub const SETTLE_MS: u64 = 600;

pub const READY_MS: u64 = 30_000;

pub fn builtin() -> BTreeMap<String, App> {
    let mut apps = BTreeMap::new();

    apps.insert(
        "xterm".to_string(),
        App {
            packages: vec!["xterm".to_string()],
            command: vec!["xterm".to_string()],
            window: Some(WindowMatch::Class("xterm".to_string())),
            ..App::default()
        },
    );

    apps.insert(
        "gimp".to_string(),
        App {
            packages: vec!["gimp".to_string()],
            command: vec!["gimp".to_string()],
            window: Some(WindowMatch::Class("gimp".to_string())),
            ..App::default()
        },
    );

    apps.insert(
        "files".to_string(),
        App {
            packages: vec!["thunar".to_string()],
            command: vec!["thunar".to_string()],
            window: Some(WindowMatch::Class("Thunar".to_string())),
            ..App::default()
        },
    );

    apps.insert(
        "text-editor".to_string(),
        App {
            packages: vec!["mousepad".to_string()],
            command: vec!["mousepad".to_string()],
            window: Some(WindowMatch::Class("Mousepad".to_string())),
            ..App::default()
        },
    );

    apps.insert(
        "vscode".to_string(),
        App {
            packages: vec!["code".to_string()],
            source: Some(Source {
                key_url: "https://packages.microsoft.com/keys/microsoft.asc".to_string(),
                list: "https://packages.microsoft.com/repos/code stable main".to_string(),
            }),
            // As root, without both flags it prints a refusal and exits.
            command: vec![
                "code".to_string(),
                "--no-sandbox".to_string(),
                "--user-data-dir=/var/lib/computer/vscode".to_string(),
            ],
            window: Some(WindowMatch::Class("code".to_string())),
            // Maps immediately and paints seconds later.
            settle_ms: Some(1500),
        },
    );

    apps
}

pub fn resolve(spec: &Spec, name: &str) -> Result<App> {
    if let Some(app) = spec.apps.get(name) {
        return match app == &App::default() {
            true => builtin().get(name).cloned().ok_or_else(|| unknown(name)),
            false => Ok(app.clone()),
        };
    }

    builtin().get(name).cloned().ok_or_else(|| unknown(name))
}

pub fn resolve_all(spec: &Spec) -> Result<BTreeMap<String, App>> {
    let mut resolved = BTreeMap::new();

    for name in spec.apps.keys() {
        let app = resolve(spec, name)?;

        if app.source.is_some() && !spec.policy.custom_sources && !is_builtin_source(name, &app) {
            return Err(Error::invalid(format!(
                "app {name} names its own apt source, which the image build \
                 would fetch and trust: set policy.custom_sources to allow it"
            )));
        }

        resolved.insert(name.clone(), app);
    }

    Ok(resolved)
}

fn is_builtin_source(name: &str, app: &App) -> bool {
    builtin()
        .get(name)
        .is_some_and(|known| known.source == app.source)
}

fn unknown(name: &str) -> Error {
    let known: Vec<String> = builtin().keys().cloned().collect();

    Error::invalid(format!(
        "no app named {name}: this catalog holds {}",
        known.join(", ")
    ))
}

pub async fn install(
    computer: &crate::Computer,
    names: &[String],
    spec: &Spec,
    within: std::time::Duration,
) -> Result<Vec<String>> {
    if names.is_empty() {
        return Err(Error::invalid("name an app to install, such as gimp"));
    }

    let mut wanted = BTreeMap::new();
    for name in names {
        wanted.insert(name.clone(), resolve(spec, name)?);
    }

    let result = computer
        .exec_within(["sh", "-c", &script(&wanted)], within)
        .await?;

    if !result.ok() {
        return Err(Error::Failed {
            code: result.code,
            stderr: format!(
                "installing {}: {}",
                names.join(", "),
                result
                    .stderr_utf8()
                    .trim()
                    .lines()
                    .last()
                    .unwrap_or_default()
            ),
        });
    }

    Ok(wanted.into_keys().collect())
}

fn script(wanted: &BTreeMap<String, App>) -> String {
    let mut lines = vec![
        "set -e".to_string(),
        "export DEBIAN_FRONTEND=noninteractive".to_string(),
    ];

    let sourced: Vec<(&String, &Source)> = wanted
        .iter()
        .filter_map(|(name, app)| app.source.as_ref().map(|source| (name, source)))
        .collect();

    if !sourced.is_empty() {
        lines.push("apt-get update".to_string());
        lines.push(
            "apt-get install -y --no-install-recommends ca-certificates curl gnupg".to_string(),
        );

        for (name, source) in sourced {
            lines.push(format!(
                "curl -fsSL {} | gpg --dearmor -o /usr/share/keyrings/{}.gpg",
                quoted(&source.key_url),
                plain(name)
            ));
            lines.push(format!(
                "echo \"deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/{}.gpg] {}\" > /etc/apt/sources.list.d/{}.list",
                plain(name),
                source.list.replace('"', ""),
                plain(name)
            ));
        }
    }

    let packages: Vec<String> = wanted
        .values()
        .flat_map(|app| app.packages.iter().map(|package| quoted(package)))
        .collect();

    lines.push("apt-get update".to_string());
    lines.push(format!(
        "apt-get install -y --no-install-recommends {}",
        packages.join(" ")
    ));
    lines.push("rm -rf /var/lib/apt/lists/*".to_string());

    for (name, app) in wanted {
        let (Some(WindowMatch::Class(class)), false) = (app.window.clone(), app.command.is_empty())
        else {
            continue;
        };

        let command = app.command.join(" ");
        let icon = app
            .command
            .first()
            .and_then(|program| program.rsplit('/').next())
            .unwrap_or_default();

        lines.push("mkdir -p /usr/share/applications".to_string());
        lines.push(format!(
            "printf '%s\\n' '[Desktop Entry]' 'Type=Application' 'Name={}' \
             'Exec=computer-launch {} {}' 'Icon={}' 'Terminal=false' \
             > /usr/share/applications/computer-app-{}.desktop",
            plain(name),
            plain(&class),
            plain(&command),
            plain(icon),
            plain(name)
        ));
    }

    lines.join("\n")
}

fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn plain(value: &str) -> String {
    value
        .chars()
        .filter(|one| one.is_ascii_alphanumeric() || " ._-/:+".contains(*one))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use computer_types::Policy;

    fn spec_with(apps: BTreeMap<String, App>) -> Spec {
        Spec {
            apps,
            ..Spec::default()
        }
    }

    #[test]
    fn test_a_builtin_name_resolves_without_being_repeated() {
        let spec = spec_with(BTreeMap::from([("gimp".to_string(), App::default())]));
        let app = resolve(&spec, "gimp").expect("a known app");

        assert_eq!(app.packages, vec!["gimp".to_string()]);
        assert_eq!(app.command, vec!["gimp".to_string()]);
    }

    #[test]
    fn test_a_callers_own_entry_wins_over_the_builtin() {
        let mine = App {
            packages: vec!["gimp".to_string()],
            command: vec!["gimp".to_string(), "--new-instance".to_string()],
            ..App::default()
        };
        let spec = spec_with(BTreeMap::from([("gimp".to_string(), mine.clone())]));

        assert_eq!(resolve(&spec, "gimp").expect("the caller's"), mine);
    }

    #[test]
    fn test_an_unknown_name_is_refused_and_says_what_is_known() {
        let spec = spec_with(BTreeMap::from([("gimpp".to_string(), App::default())]));
        let error = resolve(&spec, "gimpp").expect_err("no such app");

        let message = error.to_string();
        assert!(message.contains("gimpp"), "{message}");
        assert!(message.contains("gimp"), "the catalog is listed: {message}");
    }

    #[test]
    fn test_the_builtin_vscode_source_needs_no_policy_flag() {
        let spec = spec_with(BTreeMap::from([("vscode".to_string(), App::default())]));

        resolve_all(&spec).expect("this crate's own source is not a caller's");
    }

    #[test]
    fn test_a_callers_own_source_is_refused_by_default() {
        let mine = App {
            packages: vec!["thing".to_string()],
            source: Some(Source {
                key_url: "https://example.invalid/key.asc".to_string(),
                list: "https://example.invalid/repo stable main".to_string(),
            }),
            ..App::default()
        };
        let spec = spec_with(BTreeMap::from([("thing".to_string(), mine)]));

        let error = resolve_all(&spec).expect_err("a build would fetch that key");
        assert!(error.to_string().contains("custom_sources"));
    }

    #[test]
    fn test_a_callers_own_source_is_allowed_once_the_policy_says_so() {
        let mine = App {
            packages: vec!["thing".to_string()],
            source: Some(Source {
                key_url: "https://example.invalid/key.asc".to_string(),
                list: "https://example.invalid/repo stable main".to_string(),
            }),
            ..App::default()
        };
        let spec = Spec {
            apps: BTreeMap::from([("thing".to_string(), mine)]),
            policy: Policy {
                custom_sources: true,
                ..Policy::default()
            },
            ..Spec::default()
        };

        resolve_all(&spec).expect("the deployment opted in");
    }

    #[test]
    fn test_every_builtin_names_a_command_and_a_window() {
        for (name, app) in builtin() {
            assert!(!app.packages.is_empty(), "{name} installs nothing");
            assert!(!app.command.is_empty(), "{name} cannot be started");
            assert!(app.window.is_some(), "{name} cannot be found on screen");
        }
    }
}

#[cfg(test)]
mod installing {
    use super::*;

    fn wanted(names: &[&str]) -> BTreeMap<String, App> {
        names
            .iter()
            .map(|name| {
                (
                    name.to_string(),
                    builtin().get(*name).cloned().expect("a catalog app"),
                )
            })
            .collect()
    }

    #[test]
    fn test_an_app_from_debian_is_one_install() {
        let said = script(&wanted(&["gimp"]));

        assert!(said.contains("apt-get install -y --no-install-recommends 'gimp'"));
        assert!(
            !said.contains("keyrings"),
            "nothing in Debian needs an archive of its own: {said}"
        );
        assert!(
            said.contains("computer-app-gimp.desktop"),
            "and the dock gets a launcher, or open_app has nothing to open: {said}"
        );
    }

    #[test]
    fn test_an_app_outside_debian_brings_its_archive() {
        let said = script(&wanted(&["vscode"]));

        assert!(said.contains("/usr/share/keyrings/vscode.gpg"));
        assert!(said.contains("/etc/apt/sources.list.d/vscode.list"));
        assert!(
            said.find("apt-get update").unwrap_or_default()
                < said
                    .find("apt-get install -y --no-install-recommends 'code'")
                    .unwrap_or_default(),
            "the archive is added before the package is asked for: {said}"
        );
    }

    #[test]
    fn test_two_apps_are_installed_in_one_pass() {
        let said = script(&wanted(&["gimp", "xterm"]));

        assert_eq!(
            said.matches("apt-get install -y --no-install-recommends '")
                .count(),
            1,
            "one apt run, not one for each: {said}"
        );
    }

    #[test]
    fn test_a_name_that_would_leave_the_shell_is_stripped() {
        let mut nasty = BTreeMap::new();
        nasty.insert(
            "evil; rm -rf /".to_string(),
            App {
                packages: vec!["a'; rm -rf /; echo '".to_string()],
                command: vec!["x".to_string()],
                window: Some(WindowMatch::Class("c`whoami`".to_string())),
                ..App::default()
            },
        );

        let said = script(&nasty);

        assert!(
            said.contains("--no-install-recommends 'a'"),
            "a package name stays one quoted word, whatever is in it: {said}"
        );
        assert!(
            !said.contains('`'),
            "and nothing the shell would run reaches the launcher: {said}"
        );
        assert!(!said.contains("evil; rm"), "nor the file name: {said}");
    }
}
