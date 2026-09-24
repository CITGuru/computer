use crate::bundle::{Bundle, Extras};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const START_COMMAND: &str = "/usr/local/bin/computer-desktop";

pub const READY_COMMAND: &str = "true";

const GUEST: u32 = 1000;

const BUILDER: &str = "root";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    #[serde(rename = "type")]
    pub kind: String,
    pub args: Vec<String>,
    #[serde(rename = "filesHash", skip_serializing_if = "Option::is_none")]
    pub files_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    pub hash: String,
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub from_image: String,
    pub steps: Vec<Step>,
    pub carries: Vec<Carried>,
}

pub fn plan(bundle: &Bundle, extras: &Extras) -> Option<Plan> {
    let dockerfile = bundle
        .files
        .iter()
        .find(|(name, _)| *name == "Dockerfile")?
        .1;
    let held = |name: &str| {
        bundle
            .files
            .iter()
            .find(|(held, _)| *held == name)
            .map(|(_, body)| (*body).to_string())
    };

    let mut from_image = String::new();
    let mut steps = Vec::new();
    let mut carries = Vec::new();
    let mut home = "/root".to_string();

    for line in joined(dockerfile) {
        let (verb, rest) = match line.split_once(char::is_whitespace) {
            Some((verb, rest)) => (verb.to_ascii_uppercase(), rest.trim().to_string()),
            None => continue,
        };

        match verb.as_str() {
            "FROM" => from_image = rest,
            "ARG" => continue,
            "LABEL" | "CMD" | "USER" | "ENTRYPOINT" => continue,
            "WORKDIR" => {
                home = rest.clone();
                steps.push(Step {
                    kind: "WORKDIR".to_string(),
                    args: vec![rest],
                    files_hash: None,
                });
            }
            "ENV" => {
                let Some((key, value)) = rest.split_once('=') else {
                    continue;
                };
                if key == "HOME" {
                    home = value.to_string();
                }
                steps.push(Step {
                    kind: "ENV".to_string(),
                    args: vec![key.to_string(), value.to_string()],
                    files_hash: None,
                });
            }
            "COPY" => {
                let mut parts = rest.split_whitespace();
                let (Some(source), Some(target)) = (parts.next(), parts.next()) else {
                    continue;
                };
                let Some(body) = held(source) else {
                    continue;
                };

                let hash = hashed(source, target, &body);
                steps.push(Step {
                    kind: "COPY".to_string(),
                    args: vec![
                        source.to_string(),
                        target.to_string(),
                        BUILDER.to_string(),
                        String::new(),
                    ],
                    files_hash: Some(hash.clone()),
                });
                carries.push(Carried {
                    hash,
                    name: source.to_string(),
                    body,
                });
            }
            "RUN" => {
                let Some(command) = carrying(&rest, extras) else {
                    continue;
                };
                steps.push(Step {
                    kind: "RUN".to_string(),
                    args: vec![command, BUILDER.to_string()],
                    files_hash: None,
                });
            }
            _ => continue,
        }
    }

    steps.push(Step {
        kind: "RUN".to_string(),
        args: vec![format!(
            "mkdir -p {home} && chown -R {GUEST}:{GUEST} {home}"
        )],
        files_hash: None,
    });

    match from_image.is_empty() {
        true => None,
        false => Some(Plan {
            from_image,
            steps,
            carries,
        }),
    }
}

fn joined(dockerfile: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut held = String::new();

    for line in dockerfile.lines() {
        let trimmed = line.trim();

        if held.is_empty() && (trimmed.is_empty() || trimmed.starts_with('#')) {
            continue;
        }

        let carries_on = trimmed.ends_with('\\');
        let body = trimmed.trim_end_matches('\\').trim_end();

        if !held.is_empty() && body.starts_with('#') {
            continue;
        }

        held.push_str(body);
        held.push(' ');

        if !carries_on {
            lines.push(held.trim().to_string());
            held.clear();
        }
    }

    if !held.trim().is_empty() {
        lines.push(held.trim().to_string());
    }

    lines
}

fn carrying(command: &str, extras: &Extras) -> Option<String> {
    let mut written = command.to_string();

    for (name, value) in [
        ("EXTRA_PACKAGES", extras.build_arg()),
        ("EXTRA_SOURCES", extras.sources_arg()),
        ("EXTRA_APPS", extras.apps_arg()),
    ] {
        if !written.contains(name) {
            continue;
        }
        if value.trim().is_empty() {
            return None;
        }

        written = written
            .replace(&format!("\"${name}\""), &quoted(&value))
            .replace(&format!("${name}"), &quoted(&value));
    }

    Some(written)
}

fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn hashed(source: &str, target: &str, body: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(format!("COPY {source} {target}").as_bytes());
    digest.update(body.as_bytes());

    format!("{:x}", digest.finalize())
}

pub fn named(image: &str) -> String {
    let plain: String = image
        .chars()
        .map(|one| match one.is_ascii_alphanumeric() || one == '-' {
            true => one.to_ascii_lowercase(),
            false => '-',
        })
        .collect();

    plain.trim_matches('-').chars().take(128).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{AptSource, DESKTOP, Launcher};

    fn plain() -> Plan {
        plan(&DESKTOP, &Extras::none()).expect("the desktop image is a template")
    }

    fn steps(plan: &Plan, kind: &str) -> Vec<Vec<String>> {
        plan.steps
            .iter()
            .filter(|step| step.kind == kind)
            .map(|step| step.args.clone())
            .collect()
    }

    #[test]
    fn test_the_template_starts_from_the_image_the_dockerfile_names() {
        assert_eq!(plain().from_image, "debian:bookworm-slim");
    }

    #[test]
    fn test_what_the_builder_refuses_is_left_out() {
        let plan = plain();

        for gone in ["LABEL", "CMD", "USER", "ARG", "FROM"] {
            assert!(
                !plan.steps.iter().any(|step| step.kind == gone),
                "{gone} is not a step E2B takes"
            );
        }
    }

    #[test]
    fn test_every_file_the_image_copies_travels_with_it() {
        let plan = plain();
        let copied = steps(&plan, "COPY");

        assert!(
            copied
                .iter()
                .any(|args| args[0] == "start.sh" && args[1] == "/usr/local/bin/computer-desktop"),
            "the desktop's own start script is copied in: {copied:?}"
        );
        assert_eq!(
            copied.len(),
            plan.carries.len(),
            "a copy the build cannot find its files for would fail four minutes in"
        );

        for step in plan.steps.iter().filter(|step| step.kind == "COPY") {
            let hash = step.files_hash.as_deref().expect("a copy names its files");
            assert!(
                plan.carries.iter().any(|carried| carried.hash == hash),
                "{hash} is referenced and never uploaded"
            );
        }
    }

    #[test]
    fn test_the_same_file_hashes_the_same_and_two_files_do_not() {
        let one = hashed("start.sh", "/usr/local/bin/computer-desktop", "#!/bin/sh\n");
        let same = hashed("start.sh", "/usr/local/bin/computer-desktop", "#!/bin/sh\n");
        let other = hashed(
            "start.sh",
            "/usr/local/bin/computer-desktop",
            "#!/bin/sh\nexit\n",
        );
        let elsewhere = hashed("start.sh", "/elsewhere", "#!/bin/sh\n");

        assert_eq!(one, same, "a build that changed nothing uploads nothing");
        assert_ne!(one, other);
        assert_ne!(
            one, elsewhere,
            "the same bytes at another path are another layer"
        );
    }

    #[test]
    fn test_a_run_that_needs_a_value_no_argument_can_carry_is_written_in() {
        let extras = Extras {
            packages: vec!["gimp".to_string()],
            sources: vec![AptSource {
                name: "vscode".to_string(),
                key_url: "https://packages.microsoft.com/keys/microsoft.asc".to_string(),
                list: "https://packages.microsoft.com/repos/code stable main".to_string(),
            }],
            launchers: vec![Launcher {
                name: "gimp".to_string(),
                class: "Gimp".to_string(),
                command: vec!["gimp".to_string()],
            }],
        };

        let plan = plan(&DESKTOP, &extras).expect("a template");
        let ran = steps(&plan, "RUN")
            .into_iter()
            .map(|args| args[0].clone())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            ran.contains("'gimp'"),
            "the packages are in the command: {ran}"
        );
        assert!(
            ran.contains("packages.microsoft.com"),
            "and so is the archive"
        );
        assert!(
            !ran.contains("$EXTRA_PACKAGES"),
            "nothing is left for a build argument E2B does not take: {ran}"
        );
    }

    #[test]
    fn test_a_run_that_only_reads_an_empty_value_is_dropped() {
        let ran = steps(&plain(), "RUN")
            .into_iter()
            .map(|args| args[0].clone())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !ran.contains("EXTRA_"),
            "a box with no extra packages asks apt for nothing: {ran}"
        );
    }

    #[test]
    fn test_the_home_is_given_to_the_user_the_vendor_runs_as() {
        let plan = plain();
        let last = plan.steps.last().expect("a step");

        assert_eq!(last.kind, "RUN");
        assert!(
            last.args[0].contains("chown -R 1000:1000 /home/computer"),
            "E2B runs as uid 1000, which cannot write a HOME made for root: {:?}",
            last.args
        );
    }

    #[test]
    fn test_a_template_is_named_after_the_image_it_holds() {
        assert_eq!(
            named("computer-desktop:49735930ac64ae2a-aarch64"),
            "computer-desktop-49735930ac64ae2a-aarch64"
        );
    }
}
