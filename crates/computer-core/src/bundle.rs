use crate::error::{Error, Result};
use crate::runtime::ContainerCli;
use std::fs;
use std::path::{Path, PathBuf};

pub const IMAGES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/images");

pub const DOCKERFILE: &str = include_str!("../images/desktop/Dockerfile");
pub const START_SH: &str = include_str!("../images/desktop/start.sh");
pub const SCREEN_SH: &str = include_str!("../images/desktop/screen.sh");
pub const BROWSER_SH: &str = include_str!("../images/desktop/browser.sh");
pub const FLUXBOX_INIT: &str = include_str!("../images/desktop/fluxbox.init");
pub const FLUXBOX_MENU: &str = include_str!("../images/desktop/fluxbox.menu");
pub const FLUXBOX_APPS: &str = include_str!("../images/desktop/fluxbox.apps");
pub const EMBED_HTML: &str = include_str!("../images/desktop/embed.html");
pub const FLUXBOX_STYLE: &str = include_str!("../images/desktop/fluxbox.style");
pub const WALLPAPER_SH: &str = include_str!("../images/desktop/wallpaper.sh");
pub const LAUNCH_SH: &str = include_str!("../images/desktop/launch.sh");
pub const TINT2RC: &str = include_str!("../images/desktop/tint2rc");
pub const TERMINAL_DESKTOP: &str = include_str!("../images/desktop/terminal.desktop");
pub const BROWSER_DESKTOP: &str = include_str!("../images/desktop/browser.desktop");
pub const INPUT_GUARD: &str = include_str!("../images/desktop/input-guard.sh");

pub const A11Y_PY: &str = include_str!("../images/desktop/a11y.py");

pub const WAYLAND_DOCKERFILE: &str = include_str!("../images/wayland/Dockerfile");
pub const WAYLAND_START_SH: &str = include_str!("../images/wayland/start.sh");
pub const WAYLAND_SCREEN_SH: &str = include_str!("../images/wayland/screen.sh");
pub const WAYLAND_BROWSER_SH: &str = include_str!("../images/wayland/browser.sh");
pub const WAYLAND_INPUT_SH: &str = include_str!("../images/wayland/input.sh");
pub const SWAY_CONFIG: &str = include_str!("../images/wayland/sway.config");
pub const POINTER_C: &str = include_str!("../images/wayland/pointer.c");
pub const VIRTUAL_POINTER_XML: &str =
    include_str!("../images/wayland/wlr-virtual-pointer-unstable-v1.xml");
pub const VIRTUAL_KEYBOARD_XML: &str =
    include_str!("../images/wayland/virtual-keyboard-unstable-v1.xml");

pub const IMAGE_NAME: &str = "computer-desktop";

pub const WAYLAND_IMAGE_NAME: &str = "computer-wayland";

const LOCAL_IMAGE_NAME: &str = "computer-local";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bundle {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

pub static DESKTOP: Bundle = Bundle {
    name: IMAGE_NAME,
    files: &[
        ("Dockerfile", DOCKERFILE),
        ("start.sh", START_SH),
        ("screen.sh", SCREEN_SH),
        ("browser.sh", BROWSER_SH),
        ("fluxbox.init", FLUXBOX_INIT),
        ("fluxbox.menu", FLUXBOX_MENU),
        ("fluxbox.apps", FLUXBOX_APPS),
        ("embed.html", EMBED_HTML),
        ("fluxbox.style", FLUXBOX_STYLE),
        ("wallpaper.sh", WALLPAPER_SH),
        ("launch.sh", LAUNCH_SH),
        ("tint2rc", TINT2RC),
        ("terminal.desktop", TERMINAL_DESKTOP),
        ("browser.desktop", BROWSER_DESKTOP),
        ("input-guard.sh", INPUT_GUARD),
        ("a11y.py", A11Y_PY),
    ],
};

pub static WAYLAND: Bundle = Bundle {
    name: WAYLAND_IMAGE_NAME,
    files: &[
        ("Dockerfile", WAYLAND_DOCKERFILE),
        ("start.sh", WAYLAND_START_SH),
        ("screen.sh", WAYLAND_SCREEN_SH),
        ("browser.sh", WAYLAND_BROWSER_SH),
        ("input.sh", WAYLAND_INPUT_SH),
        ("sway.config", SWAY_CONFIG),
        ("pointer.c", POINTER_C),
        ("wlr-virtual-pointer-unstable-v1.xml", VIRTUAL_POINTER_XML),
        ("virtual-keyboard-unstable-v1.xml", VIRTUAL_KEYBOARD_XML),
        ("a11y.py", A11Y_PY),
    ],
};

impl Bundle {
    pub fn fingerprint(&self, extras: &Extras) -> String {
        // FNV-1a, not a security hash: different files need different tags.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };

        // The name too: two images with the same bytes still need two tags.
        eat(self.name.as_bytes());
        for (path, body) in self.files {
            eat(path.as_bytes());
            eat(body.as_bytes());
        }
        eat(extras.build_arg().as_bytes());
        eat(extras.sources_arg().as_bytes());
        eat(extras.apps_arg().as_bytes());

        format!("{hash:016x}")
    }

    pub fn tag(&self) -> String {
        self.tag_with(&Extras::none())
    }

    /// Carries the architecture: a wrong-arch image fails every command with exec format error.
    pub fn tag_with(&self, extras: &Extras) -> String {
        format!(
            "{}:{}-{}",
            self.name,
            self.fingerprint(extras),
            std::env::consts::ARCH
        )
    }

    pub fn owns(&self, tag: &str) -> bool {
        tag.starts_with(&format!("{}:", self.name))
    }

    /// Per version and per image: files and constants only agree within one release.
    pub fn scratch_dir(&self) -> PathBuf {
        std::env::temp_dir().join(format!(
            "computer-rs-image-{}-{}",
            env!("CARGO_PKG_VERSION"),
            self.name
        ))
    }

    pub async fn materialize(&self) -> Result<PathBuf> {
        let dir = self.scratch_dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| Error::transport(format!("{}: {error}", dir.display()), false))?;

        for (name, body) in self.files {
            write(&dir.join(name), body).await?;
        }
        Ok(dir)
    }

    pub async fn build(&self, cli: &dyn ContainerCli, tag: &str, extras: &Extras) -> Result<()> {
        let dir = self.materialize().await?;
        tracing::info!(image = %tag, from = %dir.display(), "building the image");
        let mut args = vec!["build".to_string(), "--tag".to_string(), tag.to_string()];

        if !extras.is_empty() {
            args.push("--build-arg".to_string());
            args.push(format!("EXTRA_PACKAGES={}", extras.build_arg()));

            if !extras.sources.is_empty() {
                args.push("--build-arg".to_string());
                args.push(format!("EXTRA_SOURCES={}", extras.sources_arg()));
            }

            if !extras.launchers.is_empty() {
                args.push("--build-arg".to_string());
                args.push(format!("EXTRA_APPS={}", extras.apps_arg()));
            }
        }
        args.push(dir.display().to_string());

        let result = cli.run(&args).await?;
        if result.code != 0 {
            return Err(Error::Unavailable {
                runtime: cli.program().to_string(),
                detail: format!("could not build {tag}: {}", result.stderr_utf8().trim()),
            });
        }
        Ok(())
    }
}

fn directory_error(path: &Path, detail: impl std::fmt::Display) -> Error {
    Error::denied(format!("image directory {}: {detail}", path.display()))
}

fn directory_entries(directory: &Path, entries: &mut Vec<PathBuf>) -> Result<()> {
    let children = fs::read_dir(directory).map_err(|error| directory_error(directory, error))?;

    for child in children {
        let child = child.map_err(|error| directory_error(directory, error))?;
        let path = child.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| directory_error(&path, error))?;
        entries.push(path.clone());
        if metadata.file_type().is_dir() {
            directory_entries(&path, entries)?;
        }
    }

    Ok(())
}

pub(crate) fn directory_image(directory: &Path, extras: &Extras) -> Result<(PathBuf, String)> {
    let root = fs::canonicalize(directory).map_err(|error| directory_error(directory, error))?;
    let metadata = fs::metadata(&root).map_err(|error| directory_error(&root, error))?;
    if !metadata.is_dir() {
        return Err(directory_error(&root, "not a directory"));
    }
    if !root.join("Dockerfile").is_file() {
        return Err(directory_error(&root, "has no Dockerfile"));
    }

    let mut entries = Vec::new();
    directory_entries(&root, &mut entries)?;
    entries.sort_by(|left, right| {
        let left = left.strip_prefix(&root).unwrap_or(left);
        let right = right.strip_prefix(&root).unwrap_or(right);
        left.as_os_str()
            .as_encoded_bytes()
            .cmp(right.as_os_str().as_encoded_bytes())
    });

    // FNV-1a matches bundled images: this detects changes, not adversaries.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };

    for path in entries {
        let relative = path.strip_prefix(&root).unwrap_or(&path);
        eat(relative.as_os_str().as_encoded_bytes());

        let metadata =
            fs::symlink_metadata(&path).map_err(|error| directory_error(&path, error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            eat(&metadata.permissions().mode().to_le_bytes());
        }

        let file_type = metadata.file_type();
        if file_type.is_dir() {
            eat(b"directory");
        } else if file_type.is_file() {
            eat(b"file");
            let body = fs::read(&path).map_err(|error| directory_error(&path, error))?;
            eat(&body);
        } else if file_type.is_symlink() {
            eat(b"symlink");
            let target = fs::read_link(&path).map_err(|error| directory_error(&path, error))?;
            eat(target.as_os_str().as_encoded_bytes());
        } else {
            return Err(directory_error(&path, "unsupported file type"));
        }
    }
    eat(extras.build_arg().as_bytes());
    eat(extras.sources_arg().as_bytes());
    eat(extras.apps_arg().as_bytes());

    let tag = format!("{LOCAL_IMAGE_NAME}:{hash:016x}-{}", std::env::consts::ARCH);
    Ok((root, tag))
}

async fn build_directory(
    cli: &dyn ContainerCli,
    tag: &str,
    extras: &Extras,
    directory: &Path,
) -> Result<()> {
    let mut args = vec!["build".to_string(), "--tag".to_string(), tag.to_string()];
    if !extras.is_empty() {
        args.push("--build-arg".to_string());
        args.push(format!("EXTRA_PACKAGES={}", extras.build_arg()));

        if !extras.sources.is_empty() {
            args.push("--build-arg".to_string());
            args.push(format!("EXTRA_SOURCES={}", extras.sources_arg()));
        }

        if !extras.launchers.is_empty() {
            args.push("--build-arg".to_string());
            args.push(format!("EXTRA_APPS={}", extras.apps_arg()));
        }
    }
    args.push(directory.display().to_string());

    let result = cli.run(&args).await?;
    if result.code != 0 {
        return Err(Error::Unavailable {
            runtime: cli.program().to_string(),
            detail: format!("could not build {tag}: {}", result.stderr_utf8().trim()),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extras {
    pub packages: Vec<String>,
    pub sources: Vec<AptSource>,
    pub launchers: Vec<Launcher>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Launcher {
    pub name: String,
    /// So a second click returns to the open window rather than a second copy.
    pub class: String,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AptSource {
    pub name: String,
    pub key_url: String,
    pub list: String,
}

impl Extras {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn with(packages: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut packages: Vec<String> = packages.into_iter().map(Into::into).collect();
        packages.sort();
        packages.dedup();
        Self {
            packages,
            sources: Vec::new(),
            launchers: Vec::new(),
        }
    }

    pub fn from_sources(
        packages: impl IntoIterator<Item = impl Into<String>>,
        sources: impl IntoIterator<Item = AptSource>,
    ) -> Self {
        let mut sources: Vec<AptSource> = sources.into_iter().collect();
        sources.sort();
        sources.dedup();

        Self {
            sources,
            ..Self::with(packages)
        }
    }

    /// CJK and emoji, which the base image draws as empty boxes.
    pub fn wide_fonts() -> Self {
        Self::with(["fonts-noto-cjk", "fonts-noto-color-emoji"])
    }

    pub fn audio() -> Self {
        Self::with(["pulseaudio", "pulseaudio-utils"])
    }

    pub fn video() -> Self {
        Self::with(["ffmpeg"])
    }

    /// `hsetroot` too: without it tint2 draws its rounded corners as dark squares.
    pub fn dock() -> Self {
        Self::with(["tint2", "hsetroot"])
    }

    pub fn x11_apps() -> Self {
        Self::with(["xwayland"])
    }

    /// `libatk-adaptor` is the bridge: without it a GTK app publishes an empty tree.
    pub fn accessibility() -> Self {
        Self::with(["at-spi2-core", "libatk-adaptor", "python3-pyatspi"])
    }

    pub fn everything() -> Self {
        let mut packages = Self::wide_fonts().packages;
        packages.extend(Self::audio().packages);
        packages.extend(Self::video().packages);
        packages.extend(Self::dock().packages);
        Self::with(packages)
    }

    pub fn with_launchers(mut self, launchers: impl IntoIterator<Item = Launcher>) -> Self {
        let mut launchers: Vec<Launcher> = launchers.into_iter().collect();
        launchers.sort();
        launchers.dedup();

        self.launchers = launchers;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.sources.is_empty() && self.launchers.is_empty()
    }

    pub fn build_arg(&self) -> String {
        self.packages.join(" ")
    }

    /// Tab-separated, one per line; the command is the rest of the line, unquoted.
    pub fn apps_arg(&self) -> String {
        self.launchers
            .iter()
            .map(|app| format!("{}\t{}\t{}", app.name, app.class, app.command.join(" ")))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One record per line, tab-separated: a URL holds neither.
    pub fn sources_arg(&self) -> String {
        self.sources
            .iter()
            .map(|source| format!("{}\t{}\t{}", source.name, source.key_url, source.list))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

async fn write(path: &Path, body: &str) -> Result<()> {
    tokio::fs::write(path, body)
        .await
        .map_err(|error| Error::transport(format!("{}: {error}", path.display()), false))
}

pub async fn present(cli: &dyn ContainerCli, tag: &str) -> Result<bool> {
    let args = vec!["image".to_string(), "inspect".to_string(), tag.to_string()];
    Ok(cli.run(&args).await?.code == 0)
}

pub async fn pull(cli: &dyn ContainerCli, tag: &str) -> Result<()> {
    let args = vec!["pull".to_string(), tag.to_string()];

    tracing::info!(image = %tag, "fetching the image");
    let result = cli.run(&args).await?;
    if result.code != 0 {
        return Err(Error::Unavailable {
            runtime: cli.program().to_string(),
            detail: format!("could not pull {tag}: {}", result.stderr_utf8().trim()),
        });
    }
    Ok(())
}

pub async fn ensure(cli: &dyn ContainerCli, tag: &str) -> Result<()> {
    ensure_with(cli, tag, &Extras::none(), Some(&DESKTOP)).await
}

pub async fn ensure_with(
    cli: &dyn ContainerCli,
    tag: &str,
    extras: &Extras,
    bundle: Option<&Bundle>,
) -> Result<()> {
    ensure_source(cli, tag, extras, bundle, None).await
}

pub(crate) async fn ensure_source(
    cli: &dyn ContainerCli,
    tag: &str,
    extras: &Extras,
    bundle: Option<&Bundle>,
    directory: Option<&Path>,
) -> Result<()> {
    if present(cli, tag).await? {
        return Ok(());
    }

    match (bundle, directory) {
        (Some(bundle), None) => bundle.build(cli, tag, extras).await,
        (None, Some(directory)) => build_directory(cli, tag, extras, directory).await,
        (None, None) => pull(cli, tag).await,
        (Some(_), Some(_)) => Err(Error::denied(
            "an image cannot have both bundled bytes and a local directory",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn local_image() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "computer-local-image-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).expect("a test image directory");
        fs::write(directory.join("Dockerfile"), "FROM scratch\n").expect("a Dockerfile");
        directory
    }

    #[test]
    fn test_every_script_the_image_installs_is_carried_here() {
        assert!(DOCKERFILE.contains("FROM debian"));
        assert!(START_SH.contains(crate::image::SCREEN_COMMAND));
        assert!(SCREEN_SH.contains("Xvfb"));
        assert!(BROWSER_SH.contains("--remote-debugging-port"));
        assert!(FLUXBOX_INIT.contains("session.screen0"));
    }

    #[test]
    fn test_the_tag_is_the_same_for_the_same_bytes() {
        assert_eq!(DESKTOP.tag(), DESKTOP.tag());
        assert!(DESKTOP.tag().starts_with("computer-desktop:"));
    }

    #[test]
    fn test_the_tag_names_the_architecture_it_was_built_for() {
        assert!(
            DESKTOP.tag().ends_with(std::env::consts::ARCH),
            "an image built on another architecture answering to this name is \
             a box whose every command reports exec format error"
        );
    }

    #[test]
    fn test_extra_packages_make_a_different_image() {
        assert_ne!(
            DESKTOP.tag(),
            DESKTOP.tag_with(&Extras::wide_fonts()),
            "one tag for two different images hands a box fonts it was not \
             built with, or withholds fonts it was"
        );
    }

    #[test]
    fn test_the_same_packages_in_two_orders_are_one_image() {
        let one = Extras::with(["b", "a"]);
        let other = Extras::with(["a", "b", "a"]);

        assert_eq!(one, other);
        assert_eq!(DESKTOP.tag_with(&one), DESKTOP.tag_with(&other));
    }

    #[test]
    fn test_nothing_extra_is_the_default_and_costs_nothing() {
        assert!(Extras::none().is_empty());
        assert_eq!(Extras::none().build_arg(), "");
    }

    #[test]
    fn test_the_fingerprint_covers_every_file_in_the_image() {
        let full = DESKTOP.fingerprint(&Extras::none());
        for one in [
            DOCKERFILE,
            START_SH,
            SCREEN_SH,
            BROWSER_SH,
            FLUXBOX_INIT,
            INPUT_GUARD,
        ] {
            assert!(!one.is_empty());
        }
        assert_eq!(full.len(), 16);
    }

    #[test]
    fn test_the_scratch_directory_is_named_for_this_version_and_this_image() {
        let dir = DESKTOP.scratch_dir();
        let name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("");

        assert!(
            name.contains(env!("CARGO_PKG_VERSION")),
            "an image built by an older version must not be reused: the \
             constants and the image only agree within one release"
        );
        assert!(
            name.ends_with(DESKTOP.name),
            "two images sharing a directory each build the other's context"
        );
    }

    #[test]
    fn test_a_local_image_tag_follows_every_file_in_its_directory() {
        let directory = local_image();
        let (_, before) =
            directory_image(&directory, &Extras::none()).expect("the first local image");
        fs::write(directory.join("screen.sh"), "first\n").expect("the first script");
        let (_, with_script) =
            directory_image(&directory, &Extras::none()).expect("the changed local image");
        fs::write(directory.join("screen.sh"), "second\n").expect("the changed script");
        let (_, changed) =
            directory_image(&directory, &Extras::none()).expect("the changed local image");
        fs::remove_dir_all(directory).expect("remove the test image");

        assert_ne!(before, with_script);
        assert_ne!(with_script, changed);
        assert!(changed.starts_with("computer-local:"));
        assert!(changed.ends_with(std::env::consts::ARCH));
    }

    #[test]
    fn test_a_local_image_tag_follows_sources_and_launchers() {
        let directory = local_image();
        let from_one = Extras::from_sources(
            ["code"],
            [AptSource {
                name: "editor".to_string(),
                key_url: "https://one.example/key".to_string(),
                list: "https://one.example stable main".to_string(),
            }],
        );
        let from_another = Extras::from_sources(
            ["code"],
            [AptSource {
                name: "editor".to_string(),
                key_url: "https://two.example/key".to_string(),
                list: "https://two.example stable main".to_string(),
            }],
        );
        let with_launcher = from_one.clone().with_launchers([Launcher {
            name: "editor".to_string(),
            class: "Editor".to_string(),
            command: vec!["editor".to_string()],
        }]);

        let (_, one) = directory_image(&directory, &from_one).expect("the first source");
        let (_, another) = directory_image(&directory, &from_another).expect("the other source");
        let (_, launched) = directory_image(&directory, &with_launcher).expect("the launcher");
        fs::remove_dir_all(directory).expect("remove the test image");

        assert_ne!(one, another);
        assert_ne!(one, launched);
    }

    #[test]
    fn test_a_local_image_needs_a_dockerfile() {
        let directory = local_image();
        fs::remove_file(directory.join("Dockerfile")).expect("remove the Dockerfile");
        let result = directory_image(&directory, &Extras::none());
        fs::remove_dir_all(directory).expect("remove the test image");

        assert!(matches!(result, Err(Error::Denied { .. })));
    }

    #[tokio::test]
    async fn test_a_missing_local_image_is_built_from_its_directory() {
        let directory = local_image();
        let (directory, tag) =
            directory_image(&directory, &Extras::wide_fonts()).expect("a local image");
        let cli = crate::testing::ScriptedCli::new()
            .failing(1, "not present")
            .replying(crate::ExecResult::default());

        ensure_source(&cli, &tag, &Extras::wide_fonts(), None, Some(&directory))
            .await
            .expect("the image builds");
        fs::remove_dir_all(&directory).expect("remove the test image");

        assert_eq!(
            cli.last(),
            Some(vec![
                "build".to_string(),
                "--tag".to_string(),
                tag,
                "--build-arg".to_string(),
                "EXTRA_PACKAGES=fonts-noto-cjk fonts-noto-color-emoji".to_string(),
                directory.display().to_string(),
            ])
        );
    }
}
