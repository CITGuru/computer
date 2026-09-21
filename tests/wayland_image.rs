use computer::bundle::{
    POINTER_C, SWAY_CONFIG, VIRTUAL_KEYBOARD_XML, VIRTUAL_POINTER_XML, WAYLAND_BROWSER_SH,
    WAYLAND_DOCKERFILE, WAYLAND_INPUT_SH, WAYLAND_SCREEN_SH, WAYLAND_START_SH,
};
use computer::image::{DEVTOOLS_BRIDGE_PORT, DEVTOOLS_PORT, HEIGHT_ENV, WIDTH_ENV};
use computer::servers::wayland::{DISPLAY_NAME, INPUT_COMMAND};
use computer::{AUTH_ENV, CONTROL_SECRET_ENV, VIEW_SECRET_ENV, VIEWER_USER};
use computer::{Profile, ScreenAction, ScreenId, WaylandProfile};

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
    let dispatch = WAYLAND_INPUT_SH
        .split("case \"$verb\" in")
        .nth(1)
        .expect("the script dispatches on a verb");

    for verb in [
        "move", "click", "dblclick", "drag", "path", "sweep", "scroll", "down", "up", "type",
        "paced", "press", "with",
    ] {
        // Alone or in an alternation, which is how the pointer verbs share one branch.
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
    for binary in [
        "sway",
        "wayvnc",
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

    // No compositor cursor to overlay, so a capture that shows the pointer draws it with `convert`.
    assert!(
        WAYLAND_DOCKERFILE.contains("imagemagick"),
        "the pointer is drawn with convert, which comes from imagemagick"
    );
}

#[test]
fn the_pointer_arrives_as_a_device_and_not_through_the_compositors_own_seat() {
    // Headless sway has no input devices, so `seat cursor` exits zero and moves nothing.
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
    // Events sent before the compositor has made the device are dropped silently.
    let created = POINTER_C
        .split("create_virtual_pointer(manager, seat)")
        .nth(1)
        .expect("the pointer is created");
    let before_first_event = created
        .split("return 0;")
        .next()
        .expect("the compositor is met before any gesture");

    assert!(
        before_first_event.contains("wl_display_roundtrip"),
        "without a trip in between, the first event of every gesture is lost"
    );
}

#[test]
fn one_pointer_stays_for_the_life_of_the_screen() {
    let start = WAYLAND_SCREEN_SH
        .split("start() {")
        .nth(1)
        .expect("the script starts a screen");
    let compositor_up = start
        .find(r#"test -S "${runtime}/${wayland_display}""#)
        .expect("the compositor's socket is waited for");
    let pointer_up = start
        .rfind("resident_pointer")
        .expect("a screen is given its pointer");

    assert!(
        pointer_up > compositor_up,
        "a pointer asked for before the compositor is up has nothing to connect to"
    );
    assert!(
        WAYLAND_SCREEN_SH.contains(r#"computer-pointer serve "$pointer_door""#),
        "between two clients the seat has no pointer at all, so a button cannot stay down \
         and a menu loses its hover after every command"
    );
    assert!(
        WAYLAND_SCREEN_SH.contains(r#"pointer_door="${runtime}/computer-pointer""#)
            && POINTER_C.contains(r#""%s/computer-pointer", runtime"#),
        "the script and the client have to mean the same socket, and the runtime directory \
         is what tells one screen from another"
    );
}

#[test]
fn a_pointer_that_died_is_brought_back_by_the_next_start() {
    let live = WAYLAND_SCREEN_SH
        .split("if alive; then")
        .nth(1)
        .and_then(|rest| rest.split("exit 0").next())
        .expect("a live screen returns early");

    assert!(
        live.contains("resident_pointer"),
        "start runs before every call, and returning early on a live compositor would \
         leave a screen without its pointer for good"
    );
}

#[test]
fn a_pace_is_a_pause_after_every_key() {
    let typing = POINTER_C
        .split("static const char *type_text(")
        .nth(1)
        .and_then(|rest| rest.split("static const char *one_key(").next())
        .expect("the keyboard types text");

    assert!(
        typing.contains("settle(pause > 0 ? pause : 2)"),
        "the pace is the gap the application sees between two keys, and wtype, which this \
         replaced, was never handed it"
    );
}

#[test]
fn the_script_tells_screens_apart_the_way_the_driver_does() {
    assert!(
        WAYLAND_INPUT_SH.contains(r#"number="${runtime##*/run-}""#)
            && !WAYLAND_INPUT_SH.contains("WAYLAND_DISPLAY#"),
        "every screen's compositor is {DISPLAY_NAME}, so a screen read from that name is \
         always 0: a person on screen 0 stopped input to every screen, and one on \
         screen 1 stopped none"
    );

    let second = computer::servers::wayland::runtime_dir(ScreenId(1));
    assert_eq!(second, "/tmp/computer/run-2");
    assert!(
        WAYLAND_SCREEN_SH.contains(r#"runtime="/tmp/computer/run-${number}""#),
        "the driver, the screen script and the input script have to mean the same directory"
    );
}

#[test]
fn a_gesture_that_was_refused_is_a_gesture_that_failed() {
    assert!(
        WAYLAND_INPUT_SH.contains(r#"computer-pointer "$verb" "$@" || exit $?"#),
        "a press the pointer refused was once reported as a press, because the script \
         ended on a test that was true for every pointer verb"
    );
    assert!(
        !WAYLAND_INPUT_SH.contains("wtype") && !WAYLAND_DOCKERFILE.contains("wtype"),
        "wtype numbers its own key codes and Chrome reads some of them as Backspace or \
         Control, so a text lost its colon and the space before a Korean letter"
    );
}

#[test]
fn ascii_is_typed_on_the_keys_a_us_keyboard_has() {
    assert!(
        POINTER_C.contains(r#"include \"pc+us+inet(evdev)\""#),
        "a page reads the code of a key as well as what it types, and a shortcut is \
         matched on it: H has to arrive as KeyH with Shift, not as the first free code"
    );
    assert!(
        POINTER_C.contains("zwp_virtual_keyboard_v1_modifiers(keys, mods, 0, 0, group)"),
        "a Shift key event alone typed a small a: the compositor takes the modifier state \
         of a virtual keyboard from the keyboard, not from its keys"
    );
}

#[test]
fn every_other_character_is_a_second_group_on_the_same_keys() {
    assert!(
        POINTER_C.contains("symbols[Group%zu]"),
        "Chrome types a character only from a key code it knows, so a letter of another \
         script goes where a keyboard of that script has it: on a printable key, in \
         another group"
    );
    assert!(
        POINTER_C.contains("extra_count = 0;"),
        "three groups of forty-seven keys hold 141 characters, and a text with more of \
         them starts the table again rather than dropping the rest"
    );
}

#[test]
fn text_crosses_to_the_pointer_that_stays_byte_for_byte() {
    assert!(
        POINTER_C.contains(r#"say(door, typed ? "typehex" : "pacedhex")"#)
            && POINTER_C.contains("unhex(words[rest])"),
        "a gesture is one line of words split on spaces, and text has spaces, new lines \
         and a leading dash of its own"
    );
}

#[test]
fn a_modifier_is_held_through_a_gesture_and_let_go_after_it() {
    let with = POINTER_C
        .split(r#"strcmp(verb, "with") == 0"#)
        .nth(1)
        .expect("a gesture can be given modifiers");
    let pressed = with.find("hold(down[at], 1)").expect("they go down");
    let ran = with
        .find("gesture(count - 2, words + 2)")
        .expect("the gesture runs");
    let released = with.find("hold(down[--held], 0)").expect("they come up");

    assert!(
        pressed < ran && ran < released,
        "the release is not skipped when the gesture inside is refused, or shift stays \
         down for everything after it"
    );
}

#[test]
fn both_protocols_are_carried_and_compiled_here() {
    assert!(VIRTUAL_KEYBOARD_XML.contains("zwp_virtual_keyboard_manager_v1"));
    for protocol in [
        "wlr-virtual-pointer-unstable-v1",
        "virtual-keyboard-unstable-v1",
    ] {
        assert!(
            WAYLAND_DOCKERFILE.contains(&format!("{protocol}-protocol.c"))
                && WAYLAND_DOCKERFILE.contains(protocol),
            "{protocol}: Debian packages no client for it"
        );
    }
}

#[test]
fn a_pointer_that_died_is_brought_back_by_whoever_needs_it_next() {
    let forward = POINTER_C
        .split("static int forward(")
        .nth(1)
        .expect("a run hands its gesture to the pointer that stays");
    let revived = forward.find("revive(path)").expect("it starts one");
    let gave_up = forward
        .rfind("return 0;")
        .expect("and falls back only after that");

    assert!(
        revived < gave_up,
        "screen 0 is started once, so nothing else would ever start its pointer again"
    );
    assert!(
        POINTER_C
            .split("static int serve(")
            .nth(1)
            .is_some_and(|serve| serve.find("knock(path)") < serve.find("unlink(path)")),
        "two runs that both found it dead would each start one, and the second would take \
         the socket from under a pointer that may be holding a button"
    );
}

#[test]
fn a_wrong_word_does_not_end_the_pointer_that_stays() {
    let gestures = POINTER_C
        .split("static uint32_t button_code")
        .nth(1)
        .and_then(|rest| rest.split("static int meet_compositor").next())
        .expect("the words are read and acted on between these two");

    assert!(
        !gestures.contains("exit("),
        "the resident reads what any caller sends, and an exit on a bad number would drop \
         every held button and the pointer with it"
    );
}

#[test]
fn a_button_is_held_only_where_something_stays_to_hold_it() {
    assert!(
        POINTER_C.contains("if (!resident)") && POINTER_C.contains("return HOMELESS;"),
        "a press whose device goes away is left down by sway and the application hears \
         nothing from the pointer until the next whole click"
    );
}

#[test]
fn the_keyboard_that_stays_can_let_go_of_every_key_it_put_down() {
    let event = POINTER_C
        .split("static void key_event(")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("one place sends a key");
    assert!(
        event.contains("down_codes[down_count++] = code")
            && event.contains("down_codes[at] = down_codes[--down_count]"),
        "a modifier and a plain key both pass through here, so both are remembered"
    );

    let release = POINTER_C
        .split("static void release_keys(")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("a release of everything");
    assert!(
        release.contains("key_event(down_codes[down_count - 1], 0)")
            && release.contains("mods = 0"),
        "a server that restarted has forgotten which keys it held; the keyboard has not"
    );
    assert!(
        POINTER_C.contains(r#"strcmp(verb, "release") == 0 && rest == 0"#),
        "the driver sends this word when it takes a box back"
    );
}

#[test]
fn a_gesture_is_over_when_its_command_returns() {
    let answer = POINTER_C
        .split("static void answer(int caller)")
        .nth(1)
        .expect("the resident answers a caller");
    let gesture = answer
        .find("gesture(count, words)")
        .expect("it runs the gesture");
    let trip = answer
        .find("wl_display_roundtrip")
        .expect("it waits for the compositor");
    let ok = answer.find(r#"say(caller, "ok\n")"#).expect("it says so");

    assert!(
        gesture < trip && trip < ok,
        "the caller takes a screenshot next, and one taken before the compositor has the \
         click is a picture of the page before it"
    );
}

#[test]
fn the_first_keystroke_is_not_swallowed_by_a_keymap_that_is_not_ready() {
    // The first key races a new keymap, so `KEYBOARD` arrives as `EYBOARD`.
    assert!(
        POINTER_C.contains("#define KEYMAP_SETTLE 120"),
        "an application needs a moment to read a keymap it was just sent"
    );

    for typing in [
        "static const char *type_text(",
        "static const char *press_keys(",
    ] {
        let body = POINTER_C.split(typing).nth(1).expect("the keyboard types");
        let settled = body
            .find("keymap_settled()")
            .expect("it waits for the keymap");
        let first_key = body
            .find("tap(")
            .or_else(|| body.find("one_key(name"))
            .expect("then it presses a key");

        assert!(
            settled < first_key,
            "{typing} presses before the keymap is read"
        );
    }
}

#[test]
fn a_key_nobody_has_is_an_error_and_not_a_silence() {
    assert!(
        POINTER_C.contains(r#""unknown key: %.100s""#)
            && POINTER_C.contains(r#""unknown modifier: %.100s""#),
        "an input command that reports success while the screen stays put is \
         the failure this image is hardest to debug through"
    );
}

#[test]
fn input_is_refused_by_the_image_and_not_only_by_the_crate() {
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
    assert!(WAYLAND_SCREEN_SH.contains("/proc/net/tcp"));
    assert!(
        WAYLAND_SCREEN_SH.contains("$4==\"01\""),
        "established connections only"
    );
    assert!(WAYLAND_SCREEN_SH.contains("watching=") && WAYLAND_SCREEN_SH.contains("driving="));
}

#[test]
fn the_resolution_reaches_the_compositor_through_its_configuration() {
    // sway reads no environment in its config, so geometry arrives by template.
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
    // sway names its IPC socket after its own process.
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
    // Chromium binds DevTools to loopback, so 9222 cannot be forwarded straight on.
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
    // A container needs the idle loop to stay up; on a microVM it would hold an exec open.
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

#[test]
fn the_script_reads_the_gate_the_crate_writes() {
    for name in [AUTH_ENV, VIEW_SECRET_ENV, CONTROL_SECRET_ENV] {
        assert!(
            WAYLAND_SCREEN_SH.contains(name),
            "{name} is not read by the script"
        );
    }

    assert!(
        WAYLAND_SCREEN_SH.contains("--token-plugin TokenFile"),
        "the token gate is what puts a credential in a link"
    );
    assert!(
        WAYLAND_SCREEN_SH.contains("--auth-plugin BasicHTTPAuth"),
        "the password gate is what keeps one out of a link"
    );
    assert!(
        WAYLAND_SCREEN_SH.contains("--web-auth"),
        "without it the noVNC page is served to anyone and only the socket is gated"
    );
    assert!(
        WAYLAND_SCREEN_SH.contains(&format!("--auth-source \"{VIEWER_USER}:")),
        "the user half of the prompt is what a caller tells a person to type"
    );
}

#[test]
fn each_door_carries_its_own_credential() {
    let gate = WAYLAND_SCREEN_SH
        .split("build_gate() {")
        .nth(1)
        .expect("the script builds the gate in one place");
    let gate = gate.split("\n}").next().unwrap_or(gate);

    assert!(gate.contains(&format!("view) secret=\"${{{VIEW_SECRET_ENV}")));
    assert!(gate.contains(&format!("control) secret=\"${{{CONTROL_SECRET_ENV}")));
}

#[test]
fn a_gate_with_no_secret_refuses_rather_than_opening() {
    let gate = WAYLAND_SCREEN_SH
        .split("build_gate() {")
        .nth(1)
        .expect("the script builds the gate in one place");

    assert!(gate.contains("if [ -z \"$secret\" ]; then"));
    assert!(
        gate.contains("return 1"),
        "the refusal has to stop the viewer, not warn beside it"
    );
}

#[test]
fn neither_viewer_reaches_websockify_around_the_gate() {
    for door in ["view", "control"] {
        assert!(
            WAYLAND_SCREEN_SH.contains(&format!("build_gate {door} ")),
            "the {door} viewer does not build a gate"
        );
    }
    assert!(
        !WAYLAND_SCREEN_SH.contains("\"0.0.0.0:${view_port}\" \"127.0.0.1:${view_vnc}\""),
        "the read-only viewer still passes its target around the gate"
    );
    assert!(
        !WAYLAND_SCREEN_SH.contains("\"0.0.0.0:${control_port}\" \"127.0.0.1:${control_vnc}\""),
        "the control viewer still passes its target around the gate"
    );
}

/// Xwayland `enable` without the package stops sway from starting.
#[test]
fn the_second_display_server_is_started_only_where_it_exists() {
    assert!(
        SWAY_CONFIG.contains("xwayland %XWAYLAND%"),
        "the setting is substituted rather than fixed"
    );
    assert!(
        WAYLAND_SCREEN_SH.contains("command -v Xwayland")
            && WAYLAND_SCREEN_SH.contains("s/%XWAYLAND%/"),
        "the script decides it by presence and fills the template in"
    );
    assert!(
        !computer::bundle::Extras::x11_apps().packages.is_empty(),
        "the feature installs something for that check to find"
    );
}
