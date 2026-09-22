use computer_api::CreateBox;
use computer_types::{DisplayServer, Feature};

const FEATURES: [(&str, Feature); 6] = [
    ("--wide-fonts", Feature::WideFonts),
    ("--audio", Feature::Audio),
    ("--video", Feature::Video),
    ("--dock", Feature::Dock),
    ("--x11-apps", Feature::X11Apps),
    ("--accessibility", Feature::Accessibility),
];

pub fn asked(args: &[String]) -> Result<CreateBox, String> {
    let mut body = match crate::flag(args, "--spec") {
        Some(path) => read(path)?,
        None => CreateBox::default(),
    };

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = |what: &str| {
            rest.next()
                .map(String::as_str)
                .ok_or_else(|| format!("{arg} takes {what}"))
        };

        if let Some((_, feature)) = FEATURES.iter().find(|(name, _)| name == arg) {
            if !body.spec.desktop.features.contains(feature) {
                body.spec.desktop.features.push(*feature);
            }
            continue;
        }

        match arg.as_str() {
            "--size" => {
                let given = value("WIDTHxHEIGHT, such as 1920x1080")?;
                let (width, height) = given
                    .split_once(['x', 'X'])
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .ok_or_else(|| {
                        format!("--size takes WIDTHxHEIGHT, such as 1920x1080: {given}")
                    })?;
                body.spec.desktop.width = Some(width);
                body.spec.desktop.height = Some(height);
            }
            "--screens" => {
                body.spec.desktop.screens = Some(number(arg, value("a number of screens")?)?);
            }
            "--wayland" => body.spec.desktop.server = DisplayServer::Wayland,
            "--app" => {
                for name in listed(value("an app, such as gimp")?) {
                    body.spec.apps.entry(name).or_default();
                }
            }
            "--package" => {
                for package in listed(value("an apt package")?) {
                    if !body.spec.desktop.packages.contains(&package) {
                        body.spec.desktop.packages.push(package);
                    }
                }
            }
            "--no-network" => body.spec.policy.network = false,
            "--memory" => body.placement.memory = Some(value("a size, such as 4g")?.to_string()),
            "--cpus" => body.placement.cpus = Some(value("a number, such as 2")?.to_string()),
            "--runtime" => {
                body.placement.runtime = Some(value("a runtime, such as podman")?.to_string());
            }
            "--ttl" => {
                let minutes: u64 = number(arg, value("a number of minutes")?)?;
                body.placement.expires_after_secs = Some(minutes * 60);
            }
            "--idle" => {
                let minutes: u64 = number(arg, value("a number of minutes")?)?;
                body.placement.idle_timeout_secs = Some(minutes * 60);
            }
            "--profile" => {
                body.placement.profile = Some(value("a profile name, such as work")?.to_string());
            }
            "--spec" | "--url" | "--name" => {
                value("a value")?;
            }
            other => return Err(format!("unknown option for new: {other}")),
        }
    }

    Ok(body)
}

fn read(path: &str) -> Result<CreateBox, String> {
    let raw = match path {
        "-" => {
            let mut read = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut read)
                .map_err(|error| format!("could not read the spec: {error}"))?;
            read
        }
        path => std::fs::read_to_string(path)
            .map_err(|error| format!("could not read {path}: {error}"))?,
    };

    serde_json::from_str(raw.trim()).map_err(|error| format!("the spec would not parse: {error}"))
}

fn listed(given: &str) -> impl Iterator<Item = String> + '_ {
    given
        .split(',')
        .map(str::trim)
        .filter(|one| !one.is_empty())
        .map(str::to_string)
}

fn number<T: std::str::FromStr>(name: &str, given: &str) -> Result<T, String> {
    given
        .parse()
        .map_err(|_| format!("{name} takes a whole number: {given}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use computer_types::App;

    fn args(listed: &[&str]) -> Vec<String> {
        listed.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn test_no_flags_ask_for_the_default_box() {
        let body = asked(&[]).expect("a default box");

        assert_eq!(body.spec, computer_types::Spec::default());
        assert_eq!(body.placement, computer_types::Placement::default());
    }

    #[test]
    fn test_apps_are_named_one_at_a_time_or_several() {
        let body = asked(&args(&["--app", "gimp", "--app", "vscode, files"])).expect("apps");

        assert_eq!(
            body.spec.apps.keys().collect::<Vec<_>>(),
            ["files", "gimp", "vscode"]
        );
        assert!(
            body.spec.apps.values().all(|app| app == &App::default()),
            "a bare name is left empty, which is what resolves it from the catalog"
        );
    }

    #[test]
    fn test_every_part_of_a_box_has_a_flag() {
        let body = asked(&args(&[
            "--size",
            "1920x1080",
            "--screens",
            "2",
            "--wayland",
            "--package",
            "jq,ripgrep",
            "--audio",
            "--dock",
            "--no-network",
            "--memory",
            "4g",
            "--cpus",
            "2",
            "--runtime",
            "podman",
            "--ttl",
            "60",
            "--idle",
            "10",
            "--profile",
            "work",
        ]))
        .expect("all of it");

        assert_eq!(body.spec.desktop.width, Some(1920));
        assert_eq!(body.spec.desktop.height, Some(1080));
        assert_eq!(body.spec.desktop.screens, Some(2));
        assert_eq!(body.spec.desktop.server, DisplayServer::Wayland);
        assert_eq!(body.spec.desktop.packages, ["jq", "ripgrep"]);
        assert_eq!(body.spec.desktop.features, [Feature::Audio, Feature::Dock]);
        assert!(!body.spec.policy.network);
        assert_eq!(body.placement.memory.as_deref(), Some("4g"));
        assert_eq!(body.placement.cpus.as_deref(), Some("2"));
        assert_eq!(body.placement.runtime.as_deref(), Some("podman"));
        assert_eq!(body.placement.expires_after_secs, Some(3600));
        assert_eq!(body.placement.idle_timeout_secs, Some(600));
        assert_eq!(body.placement.profile.as_deref(), Some("work"));
    }

    #[test]
    fn test_what_the_flags_ask_for_is_a_box_the_engine_builds() {
        let body = asked(&args(&[
            "--app",
            "gimp",
            "--package",
            "jq",
            "--video",
            "--memory",
            "2g",
            "--ttl",
            "60",
        ]))
        .expect("flags");

        let planned = computer::Builder::from_spec(&body.spec)
            .and_then(|builder| builder.place(&body.placement));
        assert!(planned.is_ok(), "{:?}", planned.err());

        let body = asked(&args(&["--app", "no-such-app"])).expect("flags");
        let error = computer::Builder::from_spec(&body.spec)
            .err()
            .expect("an app the catalog does not have");
        assert!(error.to_string().contains("no-such-app"), "{error}");
    }

    #[test]
    fn test_a_misspelt_option_is_refused_rather_than_dropped() {
        let error = asked(&args(&["--apps", "gimp"])).expect_err("not an option");
        assert!(error.contains("unknown option for new: --apps"), "{error}");

        let error = asked(&args(&["--app"])).expect_err("no value");
        assert!(error.contains("--app takes"), "{error}");

        let error = asked(&args(&["--ttl", "soon"])).expect_err("not a number");
        assert!(error.contains("--ttl takes a whole number"), "{error}");
    }

    #[test]
    fn test_a_flag_goes_over_the_file_and_keeps_what_it_does_not_name() {
        let dir = std::env::temp_dir().join(format!("computer-spec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let file = dir.join("box.json");
        std::fs::write(
            &file,
            r#"{
                "spec": {
                    "desktop": { "width": 1280, "height": 720, "features": ["video"] },
                    "apps": { "gimp": { "packages": ["gimp", "gimp-data"] } }
                },
                "placement": { "memory": "2g" }
            }"#,
        )
        .expect("a file");

        let body = asked(&args(&[
            "--spec",
            file.to_str().expect("a path"),
            "--size",
            "1920x1080",
            "--video",
            "--app",
            "gimp,xterm",
        ]))
        .expect("a file and flags");
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(body.spec.desktop.width, Some(1920));
        assert_eq!(
            body.spec.desktop.features,
            [Feature::Video],
            "asked for twice is asked for once"
        );
        assert_eq!(
            body.spec.apps["gimp"].packages,
            ["gimp", "gimp-data"],
            "a name on the command line does not empty the definition in the file"
        );
        assert!(body.spec.apps.contains_key("xterm"));
        assert_eq!(body.placement.memory.as_deref(), Some("2g"));
    }

    #[test]
    fn test_a_file_with_a_field_nobody_reads_is_refused() {
        let dir = std::env::temp_dir().join(format!("computer-spec-odd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let file = dir.join("box.json");
        std::fs::write(&file, r#"{ "spec": { "desktop": { "widht": 1280 } } }"#).expect("a file");

        let error = asked(&args(&["--spec", file.to_str().expect("a path")]))
            .expect_err("a misspelt field");
        std::fs::remove_dir_all(&dir).ok();

        assert!(error.contains("would not parse"), "{error}");
    }
}
