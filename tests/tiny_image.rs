//! The small build context, checked against the one it was cut from.
//!
//! `images/tiny/` implements the X11 contract that `tests/image.rs` already
//! proves, so this file does not restate it. It asks the two questions that
//! cutting an image raises: is this still the same desktop, and does what was
//! taken out reach anything that runs.
//!
//! Read as text. No Docker, no build, no daemon.

use computer::bundle::{
    BROWSER_DESKTOP, BROWSER_SH, DOCKERFILE, EMBED_HTML, FLUXBOX_APPS, FLUXBOX_INIT, FLUXBOX_MENU,
    FLUXBOX_STYLE, INPUT_GUARD, LAUNCH_SH, SCREEN_SH, START_SH, TERMINAL_DESKTOP, TINT2RC,
    WALLPAPER_SH,
};
use computer::image::{
    BROWSER_COMMAND, DESKTOP_COMMAND, DEVTOOLS_BRIDGE_PORT, HEIGHT, HEIGHT_ENV, SCREEN_COMMAND,
    WIDTH, WIDTH_ENV,
};
use computer::{Profile, X11Profile};
use std::path::PathBuf;

fn directory() -> PathBuf {
    PathBuf::from(computer::bundle::IMAGES).join("tiny")
}

fn read(name: &str) -> String {
    let path = directory().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// A script with its prose taken out.
///
/// Comments are where two copies of one script are allowed to differ: they are
/// reflowed, rewritten and argued with, and none of it changes what runs. The
/// shebang stays, because a script that lost one is a script the image cannot
/// execute.
fn instructions(script: &str) -> Vec<&str> {
    script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with('#') || line.starts_with("#!"))
        .collect()
}

/// The packages the last stage installs, in the order it lists them.
///
/// The earlier stages are thrown away, so what they install says nothing about
/// what the box carries.
fn installed(dockerfile: &str) -> Vec<&str> {
    let last = dockerfile
        .rsplit("\nFROM ")
        .next()
        .expect("a Dockerfile has a stage");

    last.split("install -y --no-install-recommends \\\n")
        .nth(1)
        .expect("the image installs its packages in one list")
        .lines()
        .map(str::trim)
        .take_while(|line| line.ends_with('\\') && !line.starts_with("&&"))
        .map(|line| line.trim_end_matches('\\').trim())
        .collect()
}

/// The paths dpkg is told to drop on unpack.
fn excluded(dockerfile: &str) -> Vec<&str> {
    quoted(dockerfile, "path-exclude ")
}

/// The paths kept back out of a wider exclusion.
fn included(dockerfile: &str) -> Vec<&str> {
    quoted(dockerfile, "path-include ")
}

fn quoted<'a>(dockerfile: &'a str, keyword: &str) -> Vec<&'a str> {
    dockerfile
        .lines()
        .filter_map(|line| line.split('\'').nth(1))
        .filter_map(|entry| entry.strip_prefix(keyword))
        .collect()
}

fn build_args(dockerfile: &str) -> Vec<&str> {
    dockerfile
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ARG "))
        .filter_map(|arg| arg.split('=').next())
        .collect()
}

#[test]
fn the_scripts_are_the_ones_the_contract_is_proven_against() {
    for (name, carried) in [
        ("screen.sh", SCREEN_SH),
        ("start.sh", START_SH),
        ("browser.sh", BROWSER_SH),
        ("input-guard.sh", INPUT_GUARD),
        ("wallpaper.sh", WALLPAPER_SH),
        ("launch.sh", LAUNCH_SH),
        ("fluxbox.init", FLUXBOX_INIT),
    ] {
        assert_eq!(
            instructions(&read(name)),
            instructions(carried),
            "images/tiny/{name} no longer runs what images/desktop/{name} \
             runs, so the contract proven against one says nothing about the \
             other. Comments may differ; instructions may not."
        );
    }
}

/// The desk is dressed from these files, and a caller works out where to click
/// from a picture of it. A menu, a dock or a viewer page that drifted here
/// would move a window somewhere the other image does not have one.
#[test]
fn the_desk_is_dressed_the_same_way() {
    for (name, carried) in [
        ("fluxbox.menu", FLUXBOX_MENU),
        ("fluxbox.apps", FLUXBOX_APPS),
        ("fluxbox.style", FLUXBOX_STYLE),
        ("tint2rc", TINT2RC),
        ("embed.html", EMBED_HTML),
        ("terminal.desktop", TERMINAL_DESKTOP),
        ("browser.desktop", BROWSER_DESKTOP),
    ] {
        assert_eq!(read(name), carried, "images/tiny/{name} has drifted");
    }
}

#[test]
fn the_commands_the_code_names_are_the_ones_this_image_installs() {
    let dockerfile = read("Dockerfile");

    for command in [DESKTOP_COMMAND, SCREEN_COMMAND, BROWSER_COMMAND] {
        assert!(
            dockerfile.contains(&format!("/usr/local/bin/{command}")),
            "{command} is run by the code and installed here under another name"
        );
    }
    assert!(
        dockerfile.contains("input-guard.sh /usr/local/bin/xdotool"),
        "the guard has to shadow the real binary to be on the path, or every \
         caller that reaches past the API drives through a takeover"
    );
}

#[test]
fn every_screen_port_the_profile_computes_is_published_by_this_image() {
    let dockerfile = read("Dockerfile");
    let mut exposed: Vec<u16> = dockerfile
        .lines()
        .filter_map(|line| line.trim().strip_prefix("EXPOSE "))
        .flat_map(str::split_whitespace)
        .filter_map(|port| port.parse().ok())
        .collect();
    exposed.sort_unstable();

    for port in X11Profile.ports().viewer_ports() {
        assert!(
            exposed.contains(&port),
            "port {port} is computed by the profile and never EXPOSEd — the \
             viewer would be unreachable and nothing would say why"
        );
    }
    assert!(
        exposed.contains(&DEVTOOLS_BRIDGE_PORT),
        "the bridge is the port a client out here connects to"
    );
}

#[test]
fn the_resolution_in_this_image_is_the_one_the_profile_claims() {
    let dockerfile = read("Dockerfile");

    assert!(dockerfile.contains(&format!("{WIDTH_ENV}={WIDTH}")));
    assert!(dockerfile.contains(&format!("{HEIGHT_ENV}={HEIGHT}")));
}

#[test]
fn this_image_carries_every_binary_the_driver_calls() {
    let dockerfile = read("Dockerfile");
    let packages = installed(&dockerfile);

    // The X11 driver and `screen.sh` shell these by name. A missing one is a
    // refusal the caller reads as a broken tool rather than as a cut package.
    for package in [
        "xvfb",
        "x11vnc",
        "fluxbox",
        "xdotool",
        "wmctrl",
        "x11-utils",
        "imagemagick",
        "websockify",
        "xclip",
        "socat",
        "bash",
        "chromium",
    ] {
        assert!(
            packages.contains(&package),
            "{package} is called by the driver and not installed by this image"
        );
    }
}

#[test]
fn this_image_declares_the_contract_it_implements() {
    assert!(
        read("Dockerfile").contains(&format!(
            "LABEL {}=\"{}\"",
            computer::PROFILE_LABEL,
            X11Profile.name()
        )),
        "this is a directory a caller points --image-dir at, so the label is \
         the only thing that stops it being driven by the wrong profile"
    );
}

/// The whole method. `dpkg --purge --force-depends` empties the same files and
/// leaves apt unable to solve, so the `EXTRA_PACKAGES` layer below fails on a
/// dependency of a package nobody asked about. An exclusion drops the bytes and
/// keeps the graph.
#[test]
fn files_are_excluded_rather_than_purged() {
    let dockerfile = read("Dockerfile");
    let runs = instructions(&dockerfile).join("\n");

    assert!(
        !runs.contains("--purge") && !runs.contains("--force-depends"),
        "a purge here breaks every later install in this image and in every \
         image built from it"
    );
    assert!(
        !excluded(&dockerfile).is_empty(),
        "an image called tiny that excludes nothing is the image it was cut from"
    );
    assert!(
        dockerfile.contains("$EXTRA_PACKAGES"),
        "the extras layer is what a purge would break, so it has to be here \
         for the rule above to mean anything"
    );
}

/// What was taken out has to be what nothing opens. A path on `PATH` is the
/// one exclusion that turns into a command not found at run time, long after
/// the build said nothing.
#[test]
fn no_exclusion_reaches_a_directory_commands_are_run_from() {
    let dockerfile = read("Dockerfile");

    for path in excluded(&dockerfile) {
        for directory in [
            "/bin/",
            "/sbin/",
            "/usr/bin/",
            "/usr/sbin/",
            "/usr/local/bin/",
        ] {
            assert!(
                !path.starts_with(directory),
                "{path} is on the path this box runs commands from"
            );
        }
    }
}

/// Chromium reads a locale file at startup and does not start without one.
#[test]
fn cutting_the_locales_keeps_one() {
    let dockerfile = read("Dockerfile");
    let locales = "/usr/lib/chromium/locales";

    if excluded(&dockerfile).iter().any(|p| p.starts_with(locales)) {
        assert!(
            included(&dockerfile)
                .iter()
                .any(|p| p.starts_with(locales) && p.ends_with(".pak")),
            "every locale was cut, so the browser has none to load"
        );
    }
}

/// Debian's `novnc` is static JavaScript with a Node runtime and a Perl
/// interpreter as dependencies. Taking the files instead is the largest single
/// cut in this image, and it only holds while the package stays out of the
/// stage that ships.
#[test]
fn the_viewer_arrives_as_files_rather_than_as_a_dependency_tree() {
    let dockerfile = read("Dockerfile");

    assert!(
        !installed(&dockerfile).contains(&"novnc"),
        "the package is back, and the runtimes it depends on with it"
    );
    assert!(
        dockerfile.contains("COPY --from=viewer /usr/share/novnc /usr/share/novnc"),
        "nothing fills the directory the viewer is served from"
    );
    assert!(
        SCREEN_SH.contains("--web=/usr/share/novnc"),
        "the script serves the viewer from somewhere else, so the copy above \
         lands where nobody looks"
    );
}

/// A caller swaps one directory for the other, so the build arguments the two
/// take have to be the same arguments in the same order.
#[test]
fn this_image_takes_the_extras_the_desktop_image_takes() {
    assert_eq!(build_args(&read("Dockerfile")), build_args(DOCKERFILE));
}
