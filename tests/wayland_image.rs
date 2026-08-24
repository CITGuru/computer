//! The Wayland image and the claim about it, checked against each other.
//!
//! The same job `tests/image.rs` does for the X11 pair, for the second one.
//! A drift check belongs to a profile: `src/wayland.rs` names commands, ports
//! and verbs, and `images/wayland/` is what answers them. Nothing else notices
//! when they stop agreeing — the code keeps sending `computer-input click`,
//! the image quietly renames it, and the failure arrives as a screen that does
//! not move.
//!
//! Read as text. No Docker, no build, no daemon.

use computer::bundle::{
    POINTER_C, SWAY_CONFIG, VIRTUAL_POINTER_XML, WAYLAND_BROWSER_SH, WAYLAND_DOCKERFILE,
    WAYLAND_INPUT_SH, WAYLAND_SCREEN_SH, WAYLAND_START_SH,
};
use computer::image::{DEVTOOLS_BRIDGE_PORT, DEVTOOLS_PORT, HEIGHT_ENV, WIDTH_ENV};
use computer::servers::wayland::{DISPLAY_NAME, INPUT_COMMAND};
use computer::{Profile, ScreenAction, ScreenId, WaylandProfile};

/// The `EXPOSE` lines, flattened into port numbers.
fn exposed() -> Vec<u16> {
    let mut ports: Vec<u16> = WAYLAND_DOCKERFILE
        .lines()
        .filter_map(|line| line.trim().strip_prefix("EXPOSE "))
        .flat_map(str::split_whitespace)
        .filter_map(|port| port.parse().ok())
        .collect();
    ports.sort_unstable();
    ports
}

#[test]
fn every_screen_port_the_profile_computes_is_published_by_the_image() {
    let exposed = exposed();

    for port in WaylandProfile.ports().viewer_ports() {
        assert!(
            exposed.contains(&port),
            "port {port} is computed by the profile and never EXPOSEd — the \
             viewer would be unreachable and nothing would say why"
        );
    }
}

#[test]
fn the_image_publishes_no_port_the_profile_does_not_know_about() {
    let mut known = WaylandProfile.ports().viewer_ports();
    known.push(DEVTOOLS_PORT);
    known.push(DEVTOOLS_BRIDGE_PORT);

    for port in exposed() {
        assert!(
            known.contains(&port),
            "port {port} is published by the image and unknown to the profile"
        );
    }
}

#[test]
fn the_script_uses_the_same_port_arithmetic_as_the_profile() {
    let first = WaylandProfile
        .ports()
        .screen(ScreenId(0))
        .expect("screen 0");

    assert!(
        WAYLAND_SCREEN_SH.contains(&format!("view_port=$(({} + screen * 2))", first.view)),
        "the script and the profile must agree on the base and the stride"
    );
    assert!(
        WAYLAND_SCREEN_SH.contains(&format!("control_port=$(({} + screen * 2))", first.control))
    );
    assert!(WAYLAND_SCREEN_SH.contains(&format!("view_vnc=$(({} + screen * 2))", first.view_vnc)));
    assert!(WAYLAND_SCREEN_SH.contains(&format!(
        "control_vnc=$(({} + screen * 2))",
        first.control_vnc
    )));
}

#[test]
fn screen_n_is_the_socket_the_profile_names() {
    let environment = WaylandProfile.screen_env(ScreenId(0));

    assert_eq!(
        environment.get("WAYLAND_DISPLAY").map(String::as_str),
        Some(DISPLAY_NAME)
    );
    assert!(
        WAYLAND_SCREEN_SH.contains(&format!(r#"wayland_display="{DISPLAY_NAME}""#)),
        "the profile and the script have to name the same socket"
    );
    assert!(
        WAYLAND_SCREEN_SH.contains(r#"test -S "${runtime}/${wayland_display}""#),
        "sway picks the name rather than taking it from the environment, so a          compositor that came up under another one has to be caught at start"
    );
    assert_eq!(
        environment.get("XDG_RUNTIME_DIR").map(String::as_str),
        Some("/tmp/computer/run-1"),
        "two compositors sharing a runtime directory each claim wayland-1"
    );
    assert!(WAYLAND_SCREEN_SH.contains(r#"runtime="/tmp/computer/run-${number}""#));
}

#[test]
fn every_verb_the_profile_sends_is_one_the_script_answers() {
    for action in [
        ScreenAction::Start,
        ScreenAction::Stop,
        ScreenAction::Control,
        ScreenAction::Release,
        ScreenAction::Viewers,
        ScreenAction::Open,
    ] {
        let verb = action.verb();
        assert!(
            WAYLAND_SCREEN_SH.contains(&format!("\n  {verb})")),
            "{verb} is sent by the profile and has no case in the script, \
             which answers with a usage message the caller reads as a broken \
             screen"
        );
    }
}

#[test]
fn every_input_verb_the_driver_sends_is_one_the_script_answers() {
    // The driver builds these by hand, so a rename on either side is a command
    // that goes in and moves nothing.
    let dispatch = WAYLAND_INPUT_SH
        .split("case \"$verb\" in")
        .nth(1)
        .expect("the script dispatches on a verb");

    for verb in ["move", "click", "dblclick", "drag", "scroll", "type", "key"] {
        // Either alone or in an alternation, which is how the pointer verbs
        // share one branch.
        assert!(
            dispatch.contains(&format!("{verb})")) || dispatch.contains(&format!("{verb}|")),
            "{verb} is sent by the driver and has no case in {INPUT_COMMAND}"
        );
    }
}

#[test]
fn the_commands_the_code_names_are_the_ones_the_image_installs() {
    for command in [
        "computer-desktop",
        "computer-screen",
        "computer-browser",
        INPUT_COMMAND,
    ] {
        assert!(
            WAYLAND_DOCKERFILE.contains(&format!("/usr/local/bin/{command}")),
            "{command} is run by the code and installed under another name"
        );
    }
}

#[test]
fn the_image_carries_every_binary_the_driver_calls() {
    // The driver shells these by name. A missing one is a refusal the caller
    // reads as a broken tool rather than as a missing package.
    for binary in [
        "sway",
        "wayvnc",
        "wtype",
        "grim",
        "wl-clipboard",
        "chromium",
        "bash",
        "computer-pointer",
    ] {
        assert!(
            WAYLAND_DOCKERFILE.contains(binary),
            "{binary} is called by the driver and not installed by the image"
        );
    }
}

#[test]
fn the_pointer_arrives_as_a_device_and_not_through_the_compositors_own_seat() {
    // sway's `seat cursor` commands move the seat's own pointer, and a
    // headless backend gives the seat no input devices — so sway accepts every
    // one of them, exits zero, and the screen does not move. The only synthetic
    // pointer Wayland has is a virtual device.
    assert!(
        POINTER_C.contains("zwlr_virtual_pointer_manager_v1_create_virtual_pointer"),
        "the pointer has to be a device the compositor made"
    );
    assert!(
        !WAYLAND_INPUT_SH.contains("cursor set") && !WAYLAND_INPUT_SH.contains("cursor press"),
        "a command that is accepted and moves nothing is worse than one that fails"
    );
    assert!(
        VIRTUAL_POINTER_XML.contains("zwlr_virtual_pointer_v1"),
        "the protocol is carried here, because Debian packages no client for it"
    );
    assert!(
        WAYLAND_DOCKERFILE.contains("wayland-scanner") && WAYLAND_DOCKERFILE.contains("AS pointer"),
        "the client is compiled in a stage the final image does not keep"
    );
}

#[test]
fn the_device_exists_before_any_event_is_sent_through_it() {
    // A virtual device does not exist until the compositor has made it, and
    // events sent into that gap are dropped with nothing to say they were.
    let created = POINTER_C
        .split("create_virtual_pointer(manager, seat)")
        .nth(1)
        .expect("the pointer is created");
    let before_first_event = created
        .split("const char *verb")
        .next()
        .expect("the gesture follows");

    assert!(
        before_first_event.contains("wl_display_roundtrip"),
        "without a trip in between, the first event of every gesture is lost"
    );
}

#[test]
fn the_first_keystroke_is_not_swallowed_by_a_keymap_that_is_not_ready() {
    // wtype makes a virtual keyboard, uploads a keymap and starts typing. The
    // first key goes out before the compositor has applied the keymap, so
    // `KEYBOARD` arrives as `EYBOARD`.
    assert!(
        WAYLAND_INPUT_SH.contains("wtype -s 120"),
        "the new device needs a pause to become real"
    );
}

#[test]
fn a_tool_that_cannot_fail_loudly_is_made_to() {
    // `wtype` exits zero whatever happens — a bad flag, no compositor, a
    // keystroke that never left. What it says is the only signal there is.
    assert!(
        WAYLAND_INPUT_SH.contains("said=$(wtype"),
        "an input command that reports success while the screen stays put is \
         the failure this image is hardest to debug through"
    );
    assert!(WAYLAND_INPUT_SH.contains("exit 1"));
}

#[test]
fn input_is_refused_by_the_image_and_not_only_by_the_crate() {
    // The gate inside the crate is a promise: an owner that reaches past the
    // API is not stopped by an agreement it never made. This is the only path
    // in, so every caller meets it.
    assert!(
        WAYLAND_INPUT_SH.contains("COMPUTER_TOKEN"),
        "the holder of a takeover has to be able to drive its own screen"
    );
    assert!(
        WAYLAND_INPUT_SH.contains("exit 3"),
        "a refusal has to be one the caller can tell from a broken command"
    );
    assert!(
        !WAYLAND_INPUT_SH.contains("ydotool"),
        "input through /dev/uinput would need a device in the box and the \
         privilege to open it, which is the isolation this crate sells"
    );
}

#[test]
fn the_read_only_viewer_cannot_be_talked_out_of_being_read_only() {
    assert!(
        WAYLAND_SCREEN_SH.contains("wayvnc -d 127.0.0.1 \"$view_vnc\""),
        "the viewer started with the screen must refuse input at the server"
    );

    let control = WAYLAND_SCREEN_SH
        .split("control()")
        .nth(1)
        .and_then(|rest| rest.split("record_token()").next())
        .expect("the script has a control action");

    assert!(
        !control.contains("wayvnc -d"),
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
    let control = WAYLAND_SCREEN_SH
        .split("control()")
        .nth(1)
        .and_then(|rest| rest.split("record_token()").next())
        .expect("the script has a control action");

    assert!(
        control.contains("$control_token") || control.contains("record_token"),
        "the token has to be recorded where a caller's memory cannot take it"
    );

    let release = WAYLAND_SCREEN_SH
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
    let release = WAYLAND_SCREEN_SH
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
    // The control viewer keeps listening after the last person closes the tab,
    // so "the server is up" would say somebody is driving long after nobody
    // is — and a run waiting for them to finish would wait for ever.
    assert!(WAYLAND_SCREEN_SH.contains("/proc/net/tcp"));
    assert!(
        WAYLAND_SCREEN_SH.contains("$4==\"01\""),
        "established connections only"
    );
    assert!(WAYLAND_SCREEN_SH.contains("watching=") && WAYLAND_SCREEN_SH.contains("driving="));
}

#[test]
fn the_resolution_reaches_the_compositor_through_its_configuration() {
    // sway reads no environment in its configuration, so a template is the
    // only way the geometry the box was given reaches the output.
    assert!(SWAY_CONFIG.contains("%WIDTH%x%HEIGHT%"));
    assert!(WAYLAND_SCREEN_SH.contains("s/%WIDTH%/${width}/"));
    assert!(WAYLAND_SCREEN_SH.contains("s/%HEIGHT%/${height}/"));

    assert!(WAYLAND_DOCKERFILE.contains(&format!("{WIDTH_ENV}=1280")));
    assert!(WAYLAND_DOCKERFILE.contains(&format!("{HEIGHT_ENV}=800")));
    assert!(WAYLAND_SCREEN_SH.contains(WIDTH_ENV));
    assert!(WAYLAND_SCREEN_SH.contains(HEIGHT_ENV));
}

#[test]
fn the_compositor_socket_is_recorded_rather_than_guessed() {
    // sway names its IPC socket after its own process, so nothing that did not
    // start it can work out the name.
    assert!(SWAY_CONFIG.contains("%SOCKFILE%"));
    assert!(WAYLAND_SCREEN_SH.contains("s|%SOCKFILE%|${sockfile}|"));
    assert!(
        WAYLAND_SCREEN_SH.contains(r#"sockfile="/tmp/computer/screen-${screen}.sway""#),
        "the driver and the script have to look in the same place"
    );
    assert!(WAYLAND_INPUT_SH.contains("/tmp/computer/screen-${screen}.sway"));
}

#[test]
fn the_compositor_is_started_headless_and_told_it_has_no_devices() {
    assert!(
        WAYLAND_SCREEN_SH.contains("WLR_BACKENDS=headless"),
        "there is no display in a box for sway to open"
    );
    assert!(
        WAYLAND_SCREEN_SH.contains("WLR_LIBINPUT_NO_DEVICES=1"),
        "sway refuses to start without a seat, and there is no seat in a box"
    );
}

#[test]
fn the_browser_is_told_which_platform_it_is_on() {
    assert!(
        WAYLAND_BROWSER_SH.contains("--ozone-platform=wayland"),
        "without it chromium looks for a display, finds none, and exits"
    );
    assert!(
        WAYLAND_BROWSER_SH.contains(&format!("--remote-debugging-port={DEVTOOLS_PORT}")),
        "devtools() hands out this port; the flag must open it"
    );
}

#[test]
fn the_browser_gets_a_profile_per_screen() {
    assert!(
        WAYLAND_SCREEN_SH.contains("--user-data-dir=\"$profile\""),
        "a shared profile makes one screen's login every screen's, and the \
         singleton lock stops the second launch outright"
    );
    assert!(WAYLAND_SCREEN_SH.contains("screen-${number}"));
}

#[test]
fn devtools_is_published_through_a_bridge_and_not_straight_out() {
    // Chromium binds the debugging port to loopback whatever
    // --remote-debugging-address says, so a host port forwarded onto 9222
    // reaches nothing and answers with an empty reply.
    assert!(WAYLAND_START_SH.contains(&format!("TCP-LISTEN:{DEVTOOLS_BRIDGE_PORT}")));
    assert!(WAYLAND_START_SH.contains(&format!("TCP:127.0.0.1:{DEVTOOLS_PORT}")));
    assert!(WAYLAND_DOCKERFILE.contains("socat"));
    assert!(WAYLAND_DOCKERFILE.contains(&format!("EXPOSE {DEVTOOLS_PORT} {DEVTOOLS_BRIDGE_PORT}")));
}

#[test]
fn extra_screens_are_not_started_up_front() {
    assert!(
        WAYLAND_START_SH.contains(&WaylandProfile.start_command(ScreenId(0)).join(" ")),
        "screen 0 only — eight compositors nobody asked for is eight \
         compositors' worth of memory"
    );
    assert!(!WAYLAND_START_SH.contains("start 1"));
}

#[test]
fn the_image_can_also_bring_a_screen_up_and_return() {
    // A container stops when its command exits, so the supervisor idles to
    // hold it open. A microVM lives until it is stopped, so the same idle loop
    // there would hold an exec open for the life of the machine.
    assert!(WAYLAND_START_SH.contains(r#"if [ "${1:-}" = "--once" ]; then"#));
    assert_eq!(
        WaylandProfile.boot_command(),
        vec!["computer-desktop", "--once"],
        "the profile sends this and the script has to answer it"
    );
    assert!(WAYLAND_START_SH.contains("boot()"));
}

#[test]
fn every_wait_in_the_image_is_bounded() {
    assert!(
        WAYLAND_SCREEN_SH.contains("SECONDS + 10"),
        "an unbounded wait turns a broken compositor into a hung run"
    );
}

#[test]
fn the_image_declares_the_contract_it_implements() {
    assert!(
        WAYLAND_DOCKERFILE.contains(&format!(
            "LABEL {}=\"{}\"",
            computer::PROFILE_LABEL,
            WaylandProfile.name()
        )),
        "a Wayland image left undeclared can be driven by the X11 profile, \
         and every command goes in and moves nothing"
    );
}

#[test]
fn the_container_idles_rather_than_exiting() {
    assert!(
        WAYLAND_START_SH.contains("while swaymsg"),
        "the container is a place, not a command: work arrives later through \
         exec, and a box that looks healthy is one with a screen in it"
    );
}
