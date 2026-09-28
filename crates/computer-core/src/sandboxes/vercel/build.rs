use crate::bundle::{Bundle, Extras};
use crate::error::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub const REGISTRY: &str = "vcr.vercel.com";

pub const BUILDER_CPUS: u32 = 4;

pub const BUILDER_LIFETIME: Duration = Duration::from_secs(30 * 60);

pub const BUILD_WAIT: Duration = Duration::from_secs(20 * 60);

pub const CONTEXT_DIR: &str = "/tmp/computer-build";

pub const AUTH_ENV: &str = "COMPUTER_REGISTRY_AUTH";

pub const IMAGE_ENV: &str = "COMPUTER_IMAGE";

pub const SCRIPT: &str = r#"set -eu
export DEBIAN_FRONTEND=noninteractive
if ! command -v docker >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y -qq docker.io docker-buildx >/dev/null
fi
if ! docker info >/dev/null 2>&1; then
  (dockerd >/tmp/dockerd.log 2>&1 &)
  for _ in $(seq 1 60); do docker info >/dev/null 2>&1 && break; sleep 1; done
fi
docker info >/dev/null 2>&1 || { tail -20 /tmp/dockerd.log >&2; exit 1; }
export DOCKER_CONFIG=/tmp/computer-docker
mkdir -p "$DOCKER_CONFIG"
printf '{"auths":{"vcr.vercel.com":{"auth":"%s"}}}' "$COMPUTER_REGISTRY_AUTH" >"$DOCKER_CONFIG/config.json"
docker buildx build --platform linux/amd64 --progress plain \
  --output "type=image,name=$COMPUTER_IMAGE,push=true,oci-mediatypes=true,compression=zstd,compression-level=3,force-compression=true" \
  /tmp/computer-build
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mode: u32,
}

impl File {
    pub fn new(path: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            path: path.into(),
            bytes,
            mode: 0o644,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub repository: String,
    pub tag: String,
    pub files: Vec<File>,
}

pub fn tag(bundle: &Bundle, extras: &Extras) -> String {
    format!("{}-x86_64", bundle.fingerprint(extras))
}

pub fn bundled(bundle: &Bundle, extras: &Extras) -> Context {
    Context {
        repository: bundle.name.to_string(),
        tag: tag(bundle, extras),
        files: context(bundle),
    }
}

pub fn directory(root: &Path, image: &str) -> Result<Context> {
    let (repository, rest) = image
        .split_once(':')
        .ok_or_else(|| Error::denied(format!("{image} has no tag")))?;
    let hash = rest.rsplit_once('-').map_or(rest, |(hash, _)| hash);

    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(Context {
        repository: repository.to_string(),
        tag: format!("{hash}-x86_64"),
        files,
    })
}

fn walk(root: &Path, directory: &Path, files: &mut Vec<File>) -> Result<()> {
    let failed = |path: &Path, error: std::io::Error| {
        Error::denied(format!("image directory {}: {error}", path.display()))
    };

    for entry in std::fs::read_dir(directory).map_err(|error| failed(directory, error))? {
        let path = entry.map_err(|error| failed(directory, error))?.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| failed(&path, error))?;

        if metadata.is_dir() {
            walk(root, &path, files)?;
            continue;
        }

        if !metadata.is_file() {
            return Err(Error::Unsupported {
                gaps: vec!["a symlink in an image directory built on vercel"],
            });
        }

        let relative = path.strip_prefix(root).unwrap_or(&path);
        #[cfg(unix)]
        let mode = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o777;
        #[cfg(not(unix))]
        let mode = 0o644;

        files.push(File {
            path: format!("{CONTEXT_DIR}/{}", relative.display()),
            bytes: std::fs::read(&path).map_err(|error| failed(&path, error))?,
            mode,
        });
    }

    Ok(())
}

pub fn reference(slug: &str, project: &str, repository: &str, tag: &str) -> String {
    format!("{REGISTRY}/{slug}/{project}/{repository}:{tag}")
}

pub fn manifest_url(slug: &str, project: &str, repository: &str, tag: &str) -> String {
    format!("https://{REGISTRY}/v2/{slug}/{project}/{repository}/manifests/{tag}")
}

fn context(bundle: &Bundle) -> Vec<File> {
    bundle
        .files
        .iter()
        .map(|(name, body)| File::new(format!("{CONTEXT_DIR}/{name}"), body.as_bytes().to_vec()))
        .collect()
}

pub fn builder(project: &str, name: &str, image: &str) -> Value {
    json!({
        "projectId": project,
        "name": name,
        "timeout": BUILDER_LIFETIME.as_millis() as u64,
        "persistent": false,
        "resources": { "vcpus": BUILDER_CPUS },
        "tags": { "computer.build": image },
    })
}

pub fn command(auth: &str, image: &str) -> Value {
    let env = BTreeMap::from([
        (AUTH_ENV.to_string(), auth.to_string()),
        (IMAGE_ENV.to_string(), image.to_string()),
    ]);

    json!({
        "command": "bash",
        "args": ["-c", SCRIPT],
        "env": env,
        "sudo": true,
        "wait": true,
        "logs": true,
    })
}

pub fn slug_of(team: &Value) -> Option<String> {
    team.get("slug")
        .and_then(Value::as_str)
        .filter(|slug| !slug.is_empty())
        .map(str::to_string)
}

pub fn name_of(project: &Value) -> Option<String> {
    project
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

pub fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle;

    #[test]
    fn test_the_tag_is_amd64_whatever_host_builds_it() {
        let tag = tag(&bundle::DESKTOP, &Extras::none());

        assert!(
            tag.ends_with("-x86_64"),
            "vercel runs amd64 only, and an aarch64 host must not name it for itself"
        );
        assert!(tag.starts_with(&bundle::DESKTOP.fingerprint(&Extras::none())));
    }

    #[test]
    fn test_a_changed_image_gets_a_new_tag() {
        assert_ne!(
            tag(&bundle::DESKTOP, &Extras::none()),
            tag(&bundle::WAYLAND, &Extras::none()),
            "an old image in the registry must not stand in for a changed one"
        );
    }

    #[test]
    fn test_the_image_is_named_under_team_and_project() {
        assert_eq!(
            reference("bitpowr", "melt", "computer-desktop", "abc-x86_64"),
            "vcr.vercel.com/bitpowr/melt/computer-desktop:abc-x86_64"
        );
        assert_eq!(
            manifest_url("bitpowr", "melt", "computer-desktop", "abc-x86_64"),
            "https://vcr.vercel.com/v2/bitpowr/melt/computer-desktop/manifests/abc-x86_64"
        );
    }

    #[test]
    fn test_every_file_of_the_bundle_goes_into_the_context() {
        let files = context(&bundle::DESKTOP);

        assert_eq!(files.len(), bundle::DESKTOP.files.len());
        assert!(
            files
                .iter()
                .any(|file| file.path == "/tmp/computer-build/Dockerfile")
        );
        assert!(SCRIPT.trim_end().ends_with(CONTEXT_DIR));
    }

    #[test]
    fn test_the_build_runs_as_root_with_the_login_in_its_environment() {
        let body = command("dGVhbTp0b2tlbg==", "vcr.vercel.com/t/p/r:x");

        assert_eq!(body["sudo"], true, "apt and dockerd need root");
        assert_eq!(body["env"][AUTH_ENV], "dGVhbTp0b2tlbg==");
        assert_eq!(body["env"][IMAGE_ENV], "vcr.vercel.com/t/p/r:x");
        assert!(
            !SCRIPT.contains("dGVhbTp0b2tlbg=="),
            "the login stays out of the command line a listing shows"
        );
    }

    #[test]
    fn test_the_builder_is_short_lived_and_not_kept() {
        let body = builder("prj_1", "computer-build-1", "r:x");

        assert_eq!(body["persistent"], false);
        assert_eq!(body["timeout"], 1_800_000);
        assert!(
            body.get("image").is_none(),
            "vercel's own image, which has apt"
        );
    }

    #[test]
    fn test_slug_and_project_name_are_read_from_their_answers() {
        assert_eq!(
            slug_of(&json!({ "slug": "bitpowr" })),
            Some("bitpowr".to_string())
        );
        assert_eq!(slug_of(&json!({ "error": {} })), None);
        assert_eq!(
            name_of(&json!({ "name": "melt" })),
            Some("melt".to_string())
        );
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "computer-vercel-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("bin")).expect("a scratch directory");
        directory
    }

    #[test]
    fn test_a_directory_image_goes_up_whole_with_its_modes() {
        let root = scratch("whole");
        std::fs::write(root.join("Dockerfile"), "FROM debian\n").expect("written");
        std::fs::write(root.join("bin/start.sh"), "#!/bin/sh\n").expect("written");
        #[cfg(unix)]
        std::fs::set_permissions(
            root.join("bin/start.sh"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("chmod");

        let context =
            directory(&root, "computer-local:0123456789abcdef-aarch64").expect("a context");
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(context.repository, "computer-local");
        assert_eq!(
            context.tag, "0123456789abcdef-x86_64",
            "the hash of the directory, named for the arch vercel runs"
        );
        let paths: Vec<&str> = context
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(
            paths,
            [
                "/tmp/computer-build/Dockerfile",
                "/tmp/computer-build/bin/start.sh"
            ]
        );
        #[cfg(unix)]
        assert_eq!(
            context.files[1].mode, 0o755,
            "a script the Dockerfile runs as it is copied must stay executable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_a_symlink_in_a_directory_image_is_refused() {
        let root = scratch("link");
        std::fs::write(root.join("Dockerfile"), "FROM debian\n").expect("written");
        std::os::unix::fs::symlink("/etc/hostname", root.join("host")).expect("a link");

        let refused = directory(&root, "computer-local:abc-aarch64");
        let _ = std::fs::remove_dir_all(&root);

        assert!(matches!(refused, Err(Error::Unsupported { .. })));
    }

    #[test]
    fn test_only_the_end_of_a_long_log_is_kept() {
        assert_eq!(tail("a\nb\nc\nd", 2), "c\nd");
        assert_eq!(tail("a", 5), "a");
    }
}
