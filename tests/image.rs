//! The image and the claim about it, checked against each other.
//!
//! "One claim, tested once" is the reason for owning a desktop image rather
//! than renting one, and it holds only while [`X11Profile`] and
//! `images/desktop/` agree. Nothing else notices when they stop: the code
//! keeps reporting 1280x800 and eight screens, the image quietly builds
//! something else, and a caller chooses on a claim that was true last month.
//!
//! A drift check belongs to a profile rather than to the trait. Every other
//! profile is another image and answers for itself; this pair is the one this
//! crate ships and is therefore the one it has to prove.
//!
//! So these read the image as text. No Docker, no build, no daemon — the drift
//! is visible in the files themselves.

use computer::bundle::{
    BROWSER_DESKTOP, BROWSER_SH, DOCKERFILE, FLUXBOX_INIT, FLUXBOX_MENU, FLUXBOX_STYLE,
    INPUT_GUARD, LAUNCH_SH, SCREEN_SH, START_SH, TERMINAL_DESKTOP, TINT2RC, WALLPAPER_SH,
};
use computer::image::{
    BROWSER_COMMAND, DESKTOP_COMMAND, DEVTOOLS_BRIDGE_PORT, DEVTOOLS_PORT, HEIGHT, HEIGHT_ENV,
    MAX_SCREENS, SCREEN_COMMAND, ScreenAction, WIDTH, WIDTH_ENV, support,
};
use computer::{Profile, ScreenId, X11Profile};

/// Every viewer port the built-in profile serves.
fn viewer_ports() -> Vec<u16> {
    X11Profile.ports().viewer_ports()
}

/// The `EXPOSE` lines, flattened into port numbers.
fn exposed() -> Vec<u16> {
    let mut ports: Vec<u16> = DOCKERFILE
        .lines()
        .filter_map(|line| line.trim().strip_prefix("EXPOSE "))
        .flat_map(str::split_whitespace)
        .filter_map(|port| port.parse().ok())
        .collect();
    ports.sort_unstable();
    ports
}

#[test]
fn every_screen_port_the_code_computes_is_published_by_the_image() {
    let exposed = exposed();

    for port in viewer_ports() {
        assert!(
            exposed.contains(&port),
            "port {port} is computed by the profile and never EXPOSEd — the \
             viewer would be unreachable and nothing would say why"
        );
    }
}

#[test]
fn the_image_publishes_no_port_the_code_does_not_know_about() {
    let mut known = viewer_ports();
    known.push(DEVTOOLS_PORT);
    known.push(DEVTOOLS_BRIDGE_PORT);

    for port in exposed() {
        assert!(
            known.contains(&port),
            "port {port} is published by the image and unknown to the code"
        );
    }
}

#[test]
fn the_resolution_in_the_image_is_the_one_the_code_claims() {
    assert!(
        DOCKERFILE.contains(&format!("{WIDTH_ENV}={WIDTH}")),
        "the Dockerfile's default width must match what the code reports"
    );
    assert!(DOCKERFILE.contains(&format!("{HEIGHT_ENV}={HEIGHT}")));

    let display = support().display.expect("the image has a screen");
    assert_eq!((display.width, display.height), (WIDTH, HEIGHT));
}

#[test]
fn the_screen_script_builds_the_geometry_from_those_variables() {
    assert!(
        SCREEN_SH.contains("${width}x${height}x24"),
        "a hardcoded geometry would ignore the variables the Dockerfile sets"
    );
    assert!(SCREEN_SH.contains(WIDTH_ENV));
    assert!(SCREEN_SH.contains(HEIGHT_ENV));
}

#[test]
fn the_screen_script_uses_the_same_port_arithmetic_as_the_code() {
    let first = X11Profile.ports().screen(ScreenId(0)).expect("screen 0");

    assert!(
        SCREEN_SH.contains(&format!("view_port=$(({} + screen * 2))", first.view)),
        "the script and the profile must agree on the base and the stride"
    );
    assert!(SCREEN_SH.contains(&format!("control_port=$(({} + screen * 2))", first.control)));
    assert!(SCREEN_SH.contains(&format!("view_vnc=$(({} + screen * 2))", first.view_vnc)));
    assert!(SCREEN_SH.contains(&format!(
        "control_vnc=$(({} + screen * 2))",
        first.control_vnc
    )));
}

#[test]
fn screen_n_is_display_n_plus_one_in_the_script_too() {
    assert!(
        SCREEN_SH.contains("display=\":$((screen + 1))\""),
        ":0 is a real console on a real host, and the offset must live in one place"
    );
}

#[test]
fn every_verb_the_code_sends_is_one_the_script_answers() {
    let verbs: Vec<&str> = [
        ScreenAction::Start,
        ScreenAction::Stop,
        ScreenAction::Control,
        ScreenAction::Release,
        ScreenAction::Viewers,
        ScreenAction::Open,
    ]
    .iter()
    .map(|action| action.verb())
    .collect();

    for verb in verbs {
        assert!(
            SCREEN_SH.contains(&format!("\n  {verb})")),
            "{verb} is sent by the code and has no case in the script, which \
             answers with a usage message the caller reads as a broken screen"
        );
    }
}

#[test]
fn the_image_carries_every_binary_the_driver_calls() {
    // The X11 driver shells these by name, and a missing one reads as a
    // broken tool rather than a missing package.
    for binary in [
        "xdotool",
        "imagemagick",
        "xvfb",
        "x11vnc",
        "chromium",
        "fluxbox",
        "x11-utils",
        "bash",
    ] {
        assert!(
            DOCKERFILE.contains(binary),
            "{binary} is called by the driver and not installed by the image"
        );
    }
}

#[test]
fn the_commands_the_code_names_are_the_ones_the_image_installs() {
    for command in [DESKTOP_COMMAND, SCREEN_COMMAND, BROWSER_COMMAND] {
        assert!(
            DOCKERFILE.contains(&format!("/usr/local/bin/{command}")),
            "{command} is run by the code and installed under another name"
        );
    }
}

#[test]
fn the_browser_speaks_devtools_on_the_port_the_code_reports() {
    assert!(
        BROWSER_SH.contains(&format!("--remote-debugging-port={DEVTOOLS_PORT}")),
        "devtools() hands out this port; the flag must open it"
    );
    assert!(
        support()
            .browser
            .map(|browser| browser.cdp)
            .unwrap_or(false),
        "claiming cdp while the flag is absent would be a dead endpoint"
    );
}

#[test]
fn devtools_is_published_through_a_bridge_and_not_straight_out() {
    // Chromium binds the debugging port to loopback whatever
    // --remote-debugging-address says, so a host port forwarded onto 9222
    // reaches nothing and answers with an empty reply.
    assert!(
        START_SH.contains(&format!("TCP-LISTEN:{DEVTOOLS_BRIDGE_PORT}")),
        "nothing listens where the runtime can reach it"
    );
    assert!(START_SH.contains(&format!("TCP:127.0.0.1:{DEVTOOLS_PORT}")));
    assert!(
        DOCKERFILE.contains("socat"),
        "the bridge is run by the image and not installed by it"
    );
    assert!(DOCKERFILE.contains(&format!("EXPOSE {DEVTOOLS_PORT} {DEVTOOLS_BRIDGE_PORT}")));
}

#[test]
fn the_browser_gets_a_profile_per_screen() {
    assert!(
        SCREEN_SH.contains("--user-data-dir=\"$profile\""),
        "a shared profile makes one screen's login every screen's, and the \
         singleton lock stops the second launch outright"
    );
    assert!(SCREEN_SH.contains("screen-${number}"));
}

#[test]
fn the_browser_starts_without_a_first_run_interstitial() {
    // A first-run dialogue is a modal that has to be dismissed before
    // anything else works.
    for flag in [
        "--no-first-run",
        "--no-default-browser-check",
        "--no-sandbox",
    ] {
        assert!(BROWSER_SH.contains(flag), "{flag} is missing");
    }
}

#[test]
fn the_control_viewer_is_a_second_server_and_not_a_mode_switch() {
    assert!(
        SCREEN_SH.contains("-viewonly"),
        "the viewer started with the screen must not accept input"
    );

    let control = SCREEN_SH
        .split("control()")
        .nth(1)
        .expect("the script has a control action");
    let control = control.split("release()").next().unwrap_or(control);

    assert!(
        !control.contains("-viewonly"),
        "the control server is the one that accepts input"
    );
    assert!(
        control.contains("$control_vnc") && control.contains("${control_port}"),
        "taking over must open its own ports, so a viewer already connected to \
         the read-only stream is never silently handed the input"
    );
}

#[test]
fn a_takeover_is_fenced_by_a_token_the_box_keeps() {
    let control = SCREEN_SH
        .split("control()")
        .nth(1)
        .and_then(|rest| rest.split("release()").next())
        .expect("the script has a control action");

    assert!(
        control.contains("$control_token"),
        "the token has to be recorded where a caller's memory cannot take it"
    );

    let release = SCREEN_SH
        .split("release()")
        .nth(1)
        .and_then(|rest| rest.split("open_url()").next())
        .expect("the script has a release action");

    assert!(
        release.contains("exit 3"),
        "a release carrying the wrong token must be refused, or a replaced \
         holder takes the keyboard from whoever is driving now"
    );
    assert!(
        release.contains("--force"),
        "a caller that has decided the person is finished needs a way past it"
    );
}

#[test]
fn releasing_control_leaves_the_read_only_viewer_up() {
    let release = SCREEN_SH
        .split("release()")
        .nth(1)
        .and_then(|rest| rest.split("open_url()").next())
        .expect("the script has a release action");

    assert!(release.contains("${control_vnc}"));
    assert!(
        !release.contains("${view_vnc}"),
        "whoever was watching keeps watching"
    );
}

#[test]
fn a_viewer_is_counted_by_connection_and_not_by_whether_a_server_is_up() {
    // The control viewer keeps listening after the last tab closes, so "the
    // server is up" would say somebody is driving long after nobody is.
    assert!(SCREEN_SH.contains("/proc/net/tcp"));
    assert!(
        SCREEN_SH.contains("$4==\"01\""),
        "established connections only"
    );
    assert!(SCREEN_SH.contains("watching=") && SCREEN_SH.contains("driving="));
}

#[test]
fn input_is_refused_by_the_image_and_not_only_by_the_crate() {
    // The gate inside the crate is cooperative. This is on the path, so a
    // caller that reaches past the API meets it too.
    assert!(
        DOCKERFILE.contains("input-guard.sh /usr/local/bin/xdotool"),
        "the guard has to shadow the real binary to be on the path"
    );
    assert!(
        INPUT_GUARD.contains("/usr/bin/xdotool"),
        "the guard must run the real one when it allows the call"
    );
    assert!(
        INPUT_GUARD.contains("COMPUTER_TOKEN"),
        "the holder of a takeover has to be able to drive its own screen"
    );

    for read_only in ["getmouselocation", "getdisplaygeometry"] {
        assert!(
            !INPUT_GUARD.contains(&format!("|{read_only}|")),
            "{read_only} tells a run what a person is doing to the screen, \
             which is the reason the gate withholds input and not observation"
        );
    }
}

#[test]
fn a_sound_card_is_started_only_where_one_is_installed() {
    // The base image has no audio, and asking for a device that is not there
    // fails the whole recording.
    assert!(SCREEN_SH.contains("command -v pulseaudio"));
    assert!(
        SCREEN_SH.contains("module-null-sink"),
        "a sink that goes nowhere is what makes a box recordable"
    );
    assert!(
        SCREEN_SH.contains("sink_name=\"screen${number}\""),
        "the recorder listens to screenN.monitor, so the sink has to be screenN"
    );
    assert!(
        SCREEN_SH.contains("if [ ! -S \"$pulse_socket\" ]; then"),
        "PulseAudio is a singleton per user: a second daemon for a second \
         screen refuses to start, and that screen gets no sound card at all"
    );
    assert!(
        SCREEN_SH.contains("socket=${pulse_socket}"),
        "a socket left where the daemon happens to put it is one a client \
         reports as connection refused beside a daemon that is running"
    );
}

#[test]
fn every_wait_in_the_image_is_bounded() {
    assert!(
        SCREEN_SH.contains("SECONDS + 10"),
        "an unbounded wait turns a broken X server into a hung run"
    );
}

#[test]
fn the_container_idles_rather_than_exiting() {
    assert!(
        START_SH.contains("while xdpyinfo"),
        "the container is a place, not a command: work arrives later through exec"
    );
}

#[test]
fn the_image_can_also_bring_a_screen_up_and_return() {
    // A container stops when its command exits, so the supervisor idles to
    // hold it open. A machine lives until it is stopped, and the same loop
    // there would hold an exec open for its whole life.
    assert!(
        START_SH.contains(r#"if [ "${1:-}" = "--once" ]; then"#),
        "computer-desktop --once is what a microVM boots with"
    );
    assert_eq!(
        X11Profile.boot_command(),
        vec![DESKTOP_COMMAND, "--once"],
        "the profile sends this and the script has to answer it"
    );
    assert!(
        START_SH.contains("boot()"),
        "both modes must start the same things, or one of them drifts"
    );
}

#[test]
fn extra_screens_are_not_started_up_front() {
    assert!(
        START_SH.contains(&X11Profile.start_command(ScreenId(0)).join(" ")),
        "screen 0 only — eight X servers nobody asked for is eight X servers of memory"
    );
    assert!(!START_SH.contains("start 1"));
}

#[test]
fn the_window_manager_is_configured_rather_than_left_to_its_defaults() {
    assert!(
        FLUXBOX_INIT.contains("session.screen0.toolbar.visible: false"),
        "a toolbar is screen area that is not the work, in every screenshot"
    );
    assert!(DOCKERFILE.contains("/etc/computer/fluxbox/init"));
    assert!(SCREEN_SH.contains("/etc/computer/fluxbox/init"));
}

/// Every file the Dockerfile copies has to be one the bundle carries.
///
/// `materialize` writes the bundle's list and nothing else, so a `COPY` of a
/// file that is not on it fails the build with "not found" — and the
/// fingerprint would not cover that file either, leaving edits to it answering
/// under a stale tag.
#[test]
fn every_file_the_dockerfile_copies_is_one_the_bundle_carries() {
    let carried: Vec<&str> = computer::bundle::DESKTOP
        .files
        .iter()
        .map(|(name, _)| *name)
        .collect();

    for line in DOCKERFILE.lines() {
        let Some(rest) = line.trim().strip_prefix("COPY ") else {
            continue;
        };
        // `COPY --from=…` takes its source from another stage, not the context.
        if rest.starts_with("--") {
            continue;
        }
        let source = rest.split_whitespace().next().unwrap_or_default();

        assert!(
            carried.contains(&source),
            "the Dockerfile copies {source} and bundle::DESKTOP does not carry \
             it, so the build context will not have it"
        );
    }
}

/// And the window manager has to be given the files, not merely shipped them.
#[test]
fn the_window_manager_is_handed_every_configuration_the_image_installs() {
    for name in ["init", "menu", "apps", "style"] {
        assert!(
            DOCKERFILE.contains(&format!("/etc/computer/fluxbox/{name}")),
            "{name} is not installed by the image"
        );
        assert!(
            SCREEN_SH.contains(&format!("/etc/computer/fluxbox/{name}")),
            "{name} is installed and never put where fluxbox reads it, so it \
             does nothing at all"
        );
    }
}

/// A style fluxbox never loads is default grey with extra steps.
#[test]
fn the_style_is_named_by_the_configuration_that_loads_it() {
    assert!(
        FLUXBOX_INIT.contains("session.styleFile:"),
        "fluxbox reads its style from the file named here; without the line \
         it uses its own default and the theme ships unused"
    );
    assert!(
        FLUXBOX_STYLE.contains("DejaVu"),
        "a style naming a font the image does not install falls back silently"
    );
    assert!(
        DOCKERFILE.contains("fonts-dejavu-core"),
        "the style asks for DejaVu and the image has to carry it"
    );
}

/// The desktop is painted after the window manager, or not at all.
#[test]
fn the_wallpaper_is_set_once_the_window_manager_cannot_overwrite_it() {
    let after_wm = SCREEN_SH
        .split("fluxbox -rc")
        .nth(1)
        .expect("the script starts fluxbox");

    assert!(
        after_wm.contains("computer-wallpaper"),
        "fluxbox paints the root window through fbsetbg when it starts, so a \
         wallpaper set before it is one nobody ever sees"
    );
    assert!(
        WALLPAPER_SH.contains("convert") && WALLPAPER_SH.contains("display -window root"),
        "the painter uses the ImageMagick the image already carries for \
         screenshots, rather than a package installed for one call"
    );
}

/// The dock is opt-in, and the image has to behave whether or not it is there.
#[test]
fn the_dock_is_started_only_where_one_was_installed() {
    assert!(
        SCREEN_SH.contains("command -v tint2"),
        "a box without the extra has no tint2, and a script that starts it \
         anyway logs a failure on every screen that comes up"
    );
    assert!(
        DOCKERFILE.contains("/etc/computer/tint2rc"),
        "the configuration is carried by the image even when the package is \
         not, so installing tint2 is the only thing the extra has to do"
    );
    assert!(
        computer::bundle::Extras::dock()
            .packages
            .contains(&"hsetroot".to_string()),
        "tint2 finds what is behind its rounded corners through _XROOTPMAP_ID, \
         which only hsetroot publishes — without it they render black"
    );
}

/// A launcher pointing at an icon nothing draws is an empty square.
#[test]
fn the_terminal_launcher_has_an_icon_the_image_draws() {
    let icon = TERMINAL_DESKTOP
        .lines()
        .find_map(|line| line.strip_prefix("Icon="))
        .expect("the launcher names an icon");

    assert!(
        DOCKERFILE.contains(icon),
        "{icon} is named by the launcher and never drawn by the image"
    );
    assert!(
        TINT2RC.contains("computer-terminal.desktop"),
        "the dock has to name the launcher the image installs, not xterm's own \
         — whose icon is the X logo and reads as nothing"
    );
}

/// A browser started from the dock has to be the one the crate drives.
#[test]
fn every_launcher_starts_the_browser_this_screen_already_owns() {
    assert!(
        TINT2RC.contains("computer-browser.desktop"),
        "chromium's own launcher runs `/usr/bin/chromium` with no profile, so \
         a browser opened from the dock has different cookies, no DevTools \
         port, and nothing for `computer-screen stop` to match"
    );
    assert!(
        BROWSER_DESKTOP.contains("computer-browser"),
        "the launcher has to go through the wrapper, which is what knows the \
         profile"
    );
    assert!(
        BROWSER_SH.contains("--user-data-dir="),
        "the wrapper has to supply a profile when it was given none"
    );
    assert!(
        BROWSER_SH.contains("screen-${DISPLAY#:}"),
        "and it derives the screen the same way the rest of the image does"
    );
}

/// A command a launcher runs through a shell cannot carry a bare `#`.
///
/// `#rrggbb` is the obvious way to write a colour and the one that breaks: a
/// launcher handing `Exec` to a shell sees the `#` as a comment and truncates
/// there. Nothing appears and nothing is said, so it reads as a slow launch.
#[test]
fn no_launcher_command_carries_a_bare_hash() {
    let exec = TERMINAL_DESKTOP
        .lines()
        .find_map(|line| line.strip_prefix("Exec="))
        .expect("the launcher runs something");

    assert!(
        !exec.contains(" #"),
        "this truncates at the hash and the terminal never opens: {exec}"
    );

    for line in FLUXBOX_MENU.lines() {
        let Some(command) = line.split_once('{').map(|(_, rest)| rest) else {
            continue;
        };
        assert!(
            !command.contains(" #"),
            "the menu runs this through a shell too: {command}"
        );
    }
}

/// A dock icon returns to what is open rather than handing back a second copy.
#[test]
fn a_launcher_focuses_what_is_already_running() {
    for (name, entry) in [("terminal", TERMINAL_DESKTOP), ("browser", BROWSER_DESKTOP)] {
        let exec = entry
            .lines()
            .find_map(|line| line.strip_prefix("Exec="))
            .expect("the launcher runs something");

        assert!(
            exec.starts_with("computer-launch "),
            "the {name} launcher starts another copy every time it is clicked: \
             {exec}"
        );
    }

    assert!(
        LAUNCH_SH.contains("windowactivate"),
        "the wrapper has to raise what it found, not merely find it"
    );
    assert!(
        LAUNCH_SH.contains("--new"),
        "and it needs a way to be told to start another anyway, which is what \
         a new-window action asks for"
    );
    assert!(
        FLUXBOX_MENU.contains("computer-launch --new"),
        "tint2 gives a launcher no context menu, so the desktop menu is where \
         a new window can be asked for"
    );
}

#[test]
fn the_image_declares_the_contract_it_implements() {
    assert!(
        DOCKERFILE.contains(&format!(
            "LABEL {}=\"{}\"",
            computer::PROFILE_LABEL,
            X11Profile.name()
        )),
        "an image that declares nothing is one a box can be driven at with \
         the wrong profile, and the failure arrives as a display that never \
         came up"
    );
}

#[test]
fn the_claim_is_a_constant_and_not_a_survey() {
    let first = support();
    let second = support();
    assert_eq!(first, second);
    assert_eq!(first.max_screens, MAX_SCREENS);
}

/// A hidden dock is only reachable if the strip it leaves behind is somewhere a
/// pointer can get to.
///
/// `panel_margin = 0 12` floated that strip twelve pixels above the bottom of
/// the screen, so the gesture — pointer to the edge — landed under it and
/// nothing came up. tint2 reported no fault: the configuration was fine, and
/// only its idea of where a pointer goes was wrong.
#[test]
fn the_hidden_dock_leaves_its_trigger_on_the_screen_edge() {
    let value = |key: &str| {
        TINT2RC
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(" = "))
            .unwrap_or_else(|| panic!("tint2rc carries no `{key}`"))
    };

    assert_eq!(
        value("autohide"),
        "1",
        "the dock is meant to stay out of frame until it is asked for"
    );

    let bottom = value("panel_margin")
        .split_whitespace()
        .nth(1)
        .expect("panel_margin is `x y`");
    assert_eq!(
        bottom, "0",
        "tint2 hides the panel window, so a bottom margin lifts the trigger \
         strip off the screen edge and leaves dead space under it — the \
         pointer reaches the bottom of the screen and nothing comes up"
    );

    assert_eq!(
        value("panel_background_id"),
        "1",
        "tint2 composites only the panel against the root pixmap, so a slab \
         put on the launcher is drawn over bare window instead — which is \
         black, and the frost is gone"
    );
}

/// A `pkill` pattern built from an environment assignment matches nothing.
///
/// The shell strips `HOME=` before it execs, so it never reaches argv and
/// `pkill -f "HOME=..."` never finds the window manager it names — exiting
/// zero, because matching nothing is not an error. A flag like
/// `--user-data-dir=` carries `=` too and matches perfectly well; a bare
/// NAME=VALUE is the shape that cannot.
#[test]
fn no_teardown_pattern_matches_on_an_environment_assignment() {
    let is_assignment = |pattern: &str| {
        pattern
            .split(|c: char| c.is_whitespace() || c == '"')
            .any(|word| {
                word.split_once('=').is_some_and(|(name, _)| {
                    !name.is_empty()
                        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
            })
    };

    for line in SCREEN_SH.lines().map(str::trim) {
        let Some(pattern) = line.strip_prefix("pkill ") else {
            continue;
        };
        assert!(
            !is_assignment(pattern),
            "`{line}` matches against argv, where a NAME=VALUE assignment never \
             appears — name the command as it is executed instead"
        );
    }
}
