//! The Ubuntu build context, checked against the contract it implements.
//!
//! `images/ubuntu/` is not carried in the binary — it is a directory a caller
//! points [`computer::Builder::image_dir`] at, and this repository ships one so
//! there is a worked example of a second base. That makes it the one image with
//! nothing watching it: `tests/image.rs` and `tests/wayland_image.rs` prove the
//! two bundled ones against their code, and a verb renamed here would be caught
//! only by a live test that needs Docker.
//!
//! **It is checked by comparison rather than by restatement.** Its scripts are
//! the X11 image's, and that image's contract is already proven test by test —
//! so the useful question is not "does this answer `start`" but "is this still
//! the same script". Restating the contract here would be a second copy of it,
//! free to drift from the first.
//!
//! Read as text. No Docker, no build, no daemon.

use computer::bundle::{BROWSER_SH, FLUXBOX_INIT, INPUT_GUARD, SCREEN_SH, START_SH};
use computer::image::{
    BROWSER_COMMAND, DESKTOP_COMMAND, DEVTOOLS_BRIDGE_PORT, DEVTOOLS_PORT, HEIGHT, HEIGHT_ENV,
    SCREEN_COMMAND, WIDTH, WIDTH_ENV,
};
use computer::{AUTH_ENV, CONTROL_SECRET_ENV, VIEW_SECRET_ENV, VIEWER_USER};
use computer::{Profile, X11Profile};
use std::path::PathBuf;

fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("images/ubuntu")
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

#[test]
fn the_scripts_are_the_ones_the_contract_is_proven_against() {
    for (name, carried) in [
        ("screen.sh", SCREEN_SH),
        ("start.sh", START_SH),
        ("browser.sh", BROWSER_SH),
        ("input-guard.sh", INPUT_GUARD),
        ("fluxbox.init", FLUXBOX_INIT),
    ] {
        let theirs = read(name);
        assert_eq!(
            instructions(&theirs),
            instructions(carried),
            "images/ubuntu/{name} no longer runs what images/desktop/{name} \
             runs, so the contract proven against one says nothing about the \
             other. Comments may differ; instructions may not."
        );
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
    assert_eq!(
        X11Profile.default_size(),
        (WIDTH, HEIGHT),
        "a box that comes up at another size is one every coordinate is \
         worked out against wrongly"
    );
}

#[test]
fn this_image_carries_every_binary_the_driver_calls() {
    let dockerfile = read("Dockerfile");

    // The X11 driver shells these by name. A missing one is a refusal the
    // caller reads as a broken tool rather than as a missing package.
    for binary in [
        "xvfb",
        "x11vnc",
        "fluxbox",
        "xdotool",
        "x11-utils",
        "imagemagick",
        "xclip",
        "socat",
        "bash",
    ] {
        assert!(
            dockerfile.contains(binary),
            "{binary} is called by the driver and not installed by this image"
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

#[test]
fn chromium_is_a_package_this_container_can_actually_run() {
    let dockerfile = read("Dockerfile");

    // Ubuntu's `chromium-browser` is a Snap launcher: it installs, it is on
    // the path, and it exits without a browser because snapd is not running in
    // a container. The failure looks like a screen that never gets a window.
    assert!(
        dockerfile.contains("xtradeb"),
        "Ubuntu ships Chromium as a Snap launcher that cannot run here, so a \
         native package has to come from somewhere else"
    );
    assert!(
        BROWSER_SH.contains(&format!("--remote-debugging-port={DEVTOOLS_PORT}")),
        "devtools() hands out this port; the flag must open it"
    );
}

/// The crate writes the gate into the box as environment and the script reads
/// it back. Neither half means anything without the other, and a rename on one
/// side leaves a viewer that refuses everybody or, worse, one that does not.
#[test]
fn the_script_reads_the_gate_the_crate_writes() {
    for name in [AUTH_ENV, VIEW_SECRET_ENV, CONTROL_SECRET_ENV] {
        assert!(SCREEN_SH.contains(name), "{name} is not read by the script");
    }

    assert!(
        SCREEN_SH.contains("--token-plugin TokenFile"),
        "the token gate is what puts a credential in a link"
    );
    assert!(
        SCREEN_SH.contains("--auth-plugin BasicHTTPAuth"),
        "the password gate is what keeps one out of a link"
    );
    assert!(
        SCREEN_SH.contains("--web-auth"),
        "without it the noVNC page is served to anyone and only the socket is gated"
    );
    assert!(
        SCREEN_SH.contains(&format!("--auth-source \"{VIEWER_USER}:")),
        "the user half of the prompt is what a caller tells a person to type"
    );
}

/// The two doors must not read one variable. They differ by a port number in a
/// URL, so one credential across both makes every watch link a control link,
/// and `input-guard.sh` does not close that — it shadows `xdotool`, and a
/// person on the control port drives over VNC without going near it.
#[test]
fn each_door_carries_its_own_credential() {
    let gate = SCREEN_SH
        .split("build_gate() {")
        .nth(1)
        .expect("the script builds the gate in one place");
    let gate = gate.split("\n}").next().unwrap_or(gate);

    assert!(gate.contains(&format!("view) secret=\"${{{VIEW_SECRET_ENV}")));
    assert!(gate.contains(&format!("control) secret=\"${{{CONTROL_SECRET_ENV}")));
}

/// A gate that cannot find its secret must refuse. Starting the viewer anyway
/// would serve an open desktop while the crate reported it locked, which is the
/// one failure this whole arrangement exists to prevent.
#[test]
fn a_gate_with_no_secret_refuses_rather_than_opening() {
    let gate = SCREEN_SH
        .split("build_gate() {")
        .nth(1)
        .expect("the script builds the gate in one place");

    assert!(gate.contains("if [ -z \"$secret\" ]; then"));
    assert!(
        gate.contains("return 1"),
        "the refusal has to stop the viewer, not warn beside it"
    );
}

/// Both viewers go through the gate. One that took its target directly would
/// serve an ungated desktop on a port the crate believes is locked.
#[test]
fn neither_viewer_reaches_websockify_around_the_gate() {
    for door in ["view", "control"] {
        assert!(
            SCREEN_SH.contains(&format!("build_gate {door} ")),
            "the {door} viewer does not build a gate"
        );
    }
    assert!(
        !SCREEN_SH.contains("\"0.0.0.0:${view_port}\" \"127.0.0.1:${view_vnc}\""),
        "the read-only viewer still passes its target around the gate"
    );
    assert!(
        !SCREEN_SH.contains("\"0.0.0.0:${control_port}\" \"127.0.0.1:${control_vnc}\""),
        "the control viewer still passes its target around the gate"
    );
}
