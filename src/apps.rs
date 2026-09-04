//! Named programs a box can install and run.
//!
//! A name is worth more than the package list behind it: it is also what a
//! launch asks for, and it carries the flags and the window match a caller
//! would otherwise have to know. `code` alone does not start — it refuses to
//! run as root without `--no-sandbox` and a user data directory, and
//! everything in a box runs as root.

use crate::error::{Error, Result};
use computer_types::{App, Source, Spec, WindowMatch};
use std::collections::BTreeMap;

/// How long a window has to hold still before it counts as drawn.
///
/// Measured rather than picked: GIMP's window settles about 300ms after it
/// maps, and VS Code's about three seconds after. This is the quiet either has
/// to show, and it is a floor on every launch, which is why a catalog entry
/// can raise or lower it.
pub const SETTLE_MS: u64 = 600;

/// How long a launch waits for a window to appear and settle.
pub const READY_MS: u64 = 30_000;

/// The apps this crate knows by name.
///
/// Small on purpose. A name here is a promise that the packages install, the
/// command starts, and the window match finds the program rather than its
/// splash screen — which is checked by the image tests, not assumed.
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
            // Its splash screen carries this class too, and appears half a
            // second before the program does. Telling them apart is the
            // window type, not the class — see `AppRuntime`.
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
            // Both flags are load-bearing: without them it prints a refusal
            // and exits, because a box runs as root.
            command: vec![
                "code".to_string(),
                "--no-sandbox".to_string(),
                "--user-data-dir=/var/lib/computer/vscode".to_string(),
            ],
            window: Some(WindowMatch::Class("code".to_string())),
            // It maps a window immediately and paints seconds later, so it
            // needs longer quiet than a program that draws when it maps.
            settle_ms: Some(1500),
        },
    );

    apps
}

/// One app, from the caller's own map or from the built-in table.
///
/// The caller's wins, so a spec can correct an entry without waiting for a
/// release. A name in neither is refused rather than launched as itself: a
/// typo that ran `gimpp` would fail later and further from the cause.
pub fn resolve(spec: &Spec, name: &str) -> Result<App> {
    if let Some(app) = spec.apps.get(name) {
        return match app == &App::default() {
            // An empty entry names a built-in without repeating it.
            true => builtin().get(name).cloned().ok_or_else(|| unknown(name)),
            false => Ok(app.clone()),
        };
    }

    builtin().get(name).cloned().ok_or_else(|| unknown(name))
}

/// Every app a spec asks for, resolved.
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

/// Whether this source is the one this crate ships for that name.
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
