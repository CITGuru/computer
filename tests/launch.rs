//! Opening a box, and what happens when it will not open.
//!
//! Against a scripted runtime, so the order of operations is pinned without a
//! daemon: a box that is checked after it is created, or created after a
//! failed image build, is a box whose failure arrives in the wrong shape.

use computer::testing::{ScriptedCli, ScriptedDriver, ScriptedProfile};
use computer::{Bind, Computer, DisplayServer, Error, ExecResult, HolderId, ScreenId, bundle};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

fn ok() -> ExecResult {
    ExecResult::default()
}

fn saying(stdout: &str) -> ExecResult {
    ExecResult {
        stdout: stdout.as_bytes().to_vec(),
        ..ExecResult::default()
    }
}

fn failing(stderr: &str) -> ExecResult {
    ExecResult {
        code: 1,
        stderr: stderr.as_bytes().to_vec(),
        ..ExecResult::default()
    }
}

/// version, image inspect, contract label, run, port — the whole opening
/// sequence.
///
/// The label comes back empty, which is an image that declares nothing: the
/// pairing check has nothing to refuse on and the launch carries on.
fn a_working_runtime(ports: &str) -> Arc<ScriptedCli> {
    Arc::new(
        ScriptedCli::new()
            .replying(ok())
            .replying(ok())
            .replying(ok())
            .replying(ok())
            .replying(saying(ports)),
    )
}

#[tokio::test]
async fn a_runtime_that_is_not_answering_is_reported_before_anything_is_created() {
    let cli = Arc::new(ScriptedCli::new().replying(failing("Cannot connect to the daemon")));

    let error = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .launch()
        .await
        .expect_err("the daemon is down");

    assert!(matches!(error, Error::Unavailable { .. }));
    assert_eq!(
        cli.count(),
        1,
        "asking first is what keeps 'the daemon is down' from arriving as \
         'the container would not start'"
    );
}

#[tokio::test]
async fn the_image_is_made_sure_of_before_a_container_is_created() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let calls = cli.calls();
    assert_eq!(calls[0][0], "version");
    assert_eq!(calls[1][..2], ["image".to_string(), "inspect".to_string()]);
    // And again for the contract the image declares, which is asked before
    // anything is created rather than after it fails to answer.
    assert_eq!(calls[2][..2], ["image".to_string(), "inspect".to_string()]);
    assert_eq!(calls[3][0], "run");
    assert!(calls[3].contains(&computer.name().to_string()));
}

#[tokio::test]
async fn the_ports_the_runtime_mapped_are_the_ones_the_caller_is_handed() {
    let cli = a_working_runtime("6080/tcp -> 127.0.0.1:32768\n9223/tcp -> 0.0.0.0:32769\n");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert_eq!(
        computer.viewer_url().as_deref(),
        Some("http://127.0.0.1:32768/vnc.html?autoconnect=1&resize=scale")
    );
    assert_eq!(
        computer.devtools().map(|endpoint| endpoint.http_url),
        Some("http://127.0.0.1:32769".to_string()),
        "the port inside the box is of no use to a client out here"
    );
}

#[tokio::test]
async fn devtools_is_read_from_the_bridge_and_not_from_chromiums_own_port() {
    // 9222 is mapped here and 9223 is not. Chromium holds 9222 on loopback
    // inside the box, so a URL built from that mapping answers nothing.
    let cli = a_working_runtime("9222/tcp -> 127.0.0.1:32770\n");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert!(computer.devtools().is_none());
}

#[tokio::test]
async fn a_box_with_nothing_published_offers_no_url_rather_than_a_dead_one() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .publish_ports(false)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert!(computer.viewer_url().is_none());
    assert!(
        computer.devtools().is_none(),
        "a URL that refuses the connection is worse than no URL"
    );
}

#[tokio::test]
async fn the_size_the_box_was_started_at_is_the_size_it_reports() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .size(1920, 1080)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let display = computer.support().display.expect("a screen");
    assert_eq!((display.width, display.height), (1920, 1080));
}

#[tokio::test]
async fn a_container_that_will_not_start_is_reported_and_not_half_opened() {
    let cli = Arc::new(
        ScriptedCli::new()
            .replying(ok())
            .replying(ok())
            .replying(ok())
            .replying(failing("port is already allocated")),
    );

    let error = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .launch()
        .await
        .expect_err("it did not start");

    assert!(matches!(error, Error::Unavailable { .. }));
    assert!(error.to_string().contains("already allocated"));
}

#[tokio::test]
async fn attaching_to_a_box_that_is_not_running_says_it_is_gone() {
    let cli = Arc::new(ScriptedCli::new().replying(saying("false\n")));

    let error = Computer::attach_with(cli as Arc<dyn computer::ContainerCli>, "computer-7")
        .await
        .expect_err("it is not running");

    assert!(matches!(error, Error::Gone(name) if name == "computer-7"));
}

#[tokio::test]
async fn an_attached_box_reports_the_geometry_it_was_started_with() {
    let cli = Arc::new(
        ScriptedCli::new()
            .replying(saying("true\n"))
            .replying(saying(
                "PATH=/usr/bin\nCOMPUTER_SCREEN_WIDTH=1600\nCOMPUTER_SCREEN_HEIGHT=900\n",
            ))
            .replying(saying("")),
    );

    let computer = Computer::attach_with(cli as Arc<dyn computer::ContainerCli>, "computer-7")
        .await
        .expect("it is running");

    let display = computer.support().display.expect("a screen");
    assert_eq!(
        (display.width, display.height),
        (1600, 900),
        "reading it back is the difference between a descriptor and a wish"
    );
}

#[tokio::test]
async fn an_attached_box_is_not_removed_when_the_handle_is_dropped() {
    let cli = Arc::new(
        ScriptedCli::new()
            .replying(saying("true\n"))
            .replying(saying(""))
            .replying(saying("")),
    );

    {
        let _computer = Computer::attach_with(
            Arc::clone(&cli) as Arc<dyn computer::ContainerCli>,
            "somebody-elses-box",
        )
        .await
        .expect("it is running");
    }

    assert!(
        !cli.calls().iter().any(|argv| argv[0] == "rm"),
        "this process did not create it, so it does not get to take it away"
    );
}

#[tokio::test]
async fn an_empty_command_is_refused_rather_than_run() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let empty: Vec<String> = Vec::new();
    assert!(matches!(
        computer.exec(empty).await,
        Err(Error::Denied { .. })
    ));
}

#[tokio::test]
async fn shutting_down_removes_the_container_and_its_volumes() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .launch()
        .await
        .expect("a box");
    let name = computer.name().to_string();

    computer.shutdown().await.expect("it goes away");

    let last = cli.last().expect("a command");
    assert_eq!(last, vec!["rm", "--force", "--volumes", &name]);
}

#[tokio::test]
async fn a_box_given_a_life_records_it_on_itself_and_not_only_in_here() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .expires_after(Duration::from_secs(3600))
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert!(computer.expires_at().is_some());
    assert!(!computer.expired());

    let run = cli
        .calls()
        .into_iter()
        .find(|argv| argv[0] == "run")
        .expect("the run command");
    assert!(
        run.iter()
            .any(|arg| arg.starts_with("computer.expires-at=")),
        "a box that outlives this process must still be findable by a sweeper"
    );
}

#[tokio::test]
async fn a_box_with_no_life_given_to_it_never_expires() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert_eq!(computer.expires_at(), None);
    assert!(!computer.expired());
}

#[tokio::test]
async fn a_leased_screen_is_given_back_when_the_handle_is_dropped() {
    let cli = a_working_runtime("");
    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let holder = HolderId::new("run-1");
    {
        let leased = computer.claim(&holder, 1).await.expect("a screen");
        assert_eq!(leased.id(), ScreenId(0));
        assert_eq!(computer.leases().in_use(std::time::SystemTime::now()), 1);
    }

    assert_eq!(
        computer.leases().in_use(std::time::SystemTime::now()),
        0,
        "a lease nobody gave back blocks the screen until it runs out"
    );
}

#[tokio::test]
async fn two_holders_cannot_both_have_one_screen() {
    let cli = a_working_runtime("");
    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let first = computer
        .claim(&HolderId::new("a"), 1)
        .await
        .expect("a screen");
    let second = computer
        .claim(&HolderId::new("b"), 1)
        .await
        .expect("another");

    assert_ne!(
        first.id(),
        second.id(),
        "handing one screen to two callers is the whole thing leases prevent"
    );
}

#[tokio::test]
async fn a_stale_holder_cannot_take_a_screen_back_from_its_replacement() {
    let cli = a_working_runtime("");
    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let slow = computer
        .claim(&HolderId::new("a"), 1)
        .await
        .expect("a screen");
    let newer = computer
        .take(ScreenId(0), &HolderId::new("b"), 2)
        .await
        .expect("a newer holder wins");

    drop(slow);
    assert_eq!(
        computer
            .leases()
            .holder_of(ScreenId(0), std::time::SystemTime::now()),
        Some(HolderId::new("b")),
        "the slow holder's release must not tear down its replacement"
    );
    drop(newer);
}

#[tokio::test]
async fn asking_for_extra_packages_asks_for_a_different_image() {
    let plain = Computer::builder().preview().expect("the built-in image");
    let wide = Computer::builder()
        .wide_fonts()
        .preview()
        .expect("the built-in image, with fonts");

    let plain_image = plain.last().expect("an image");
    let wide_image = wide.last().expect("an image");

    assert_eq!(plain_image, &bundle::DESKTOP.tag());
    assert_ne!(
        plain_image, wide_image,
        "one tag for two different images hands a box fonts it was not built with"
    );
}

// A paused clock: the reaper waits ten seconds between asks, and a test that
// really waited would be the slowest thing in the suite by an order of
// magnitude.
#[tokio::test(start_paused = true)]
async fn a_box_whose_life_ran_out_is_asked_for_again_when_the_first_ask_fails() {
    // `rm` refused once — a runtime mid-restart — then accepted. A reaper that
    // asked once would have left the box holding its memory for ever.
    let cli = Arc::new(
        ScriptedCli::new()
            .replying(ok())
            .replying(ok())
            .replying(ok())
            .replying(ok())
            .replying(saying(""))
            .replying(failing("Cannot connect to the Docker daemon"))
            .replying(ok()),
    );

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .expires_after(Duration::from_millis(50))
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let name = computer.name().to_string();
    drop(computer);

    // Past the deadline and one pause: the second ask is what proves it did
    // not give up on the first refusal.
    tokio::time::sleep(Duration::from_secs(11)).await;
    tokio::task::yield_now().await;

    let removals = cli
        .calls()
        .into_iter()
        .filter(|call| call[0] == "rm" && call.contains(&name))
        .count();
    assert_eq!(
        removals, 2,
        "a reaper that asks once leaves a box running whenever the runtime \
         happened to be busy, and nothing anywhere records it"
    );
}

#[tokio::test]
async fn an_image_built_for_another_contract_is_refused_before_it_starts() {
    // version, image inspect, build/pull, then the label inspect this refuses on.
    let cli = Arc::new(
        ScriptedCli::new()
            .replying(ok())
            .replying(ok())
            .saying("computer-wayland\n"),
    );

    let error = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .launch()
        .await
        .expect_err("the image says it is not what this profile drives");

    let message = error.to_string();
    assert!(
        message.contains("computer-wayland") && message.contains("computer-desktop"),
        "the refusal has to name both halves, or it sends the caller to look \
         at the display server: {message}"
    );
    assert!(
        !cli.calls().iter().any(|call| call[0] == "run"),
        "the box must be refused before it is created, not after ninety \
         seconds of waiting for a screen that was never going to answer"
    );
}

#[tokio::test]
async fn an_image_that_declares_nothing_is_not_refused() {
    // The last reply is the empty label: a caller's own image owes this crate
    // no declaration, and absence is not a mismatch.
    let cli = Arc::new(
        ScriptedCli::new()
            .replying(ok())
            .replying(ok())
            .saying("")
            .replying(ok())
            .replying(saying("")),
    );

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await;

    assert!(computer.is_ok(), "{:?}", computer.err());
}

#[tokio::test]
async fn a_box_is_driven_through_x11_unless_it_is_told_otherwise() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert_eq!(
        computer.support().display.map(|display| display.server),
        Some(DisplayServer::X11),
        "X11 is what the built-in image runs, so it is what a caller who \
         chose nothing gets"
    );
}

#[tokio::test]
async fn a_swapped_driver_opens_every_screen_and_is_reported_as_the_one_in_use() {
    // version, image inspect, run, port, then `computer-screen start 1`.
    let cli = Arc::new(
        ScriptedCli::new()
            .replying(ok())
            .replying(ok())
            .replying(ok())
            .replying(saying(""))
            .replying(ok()),
    );
    let driver = Arc::new(ScriptedDriver::new());

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .driver(Arc::clone(&driver) as Arc<dyn computer::DesktopFactory>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert_eq!(
        computer.support().display.map(|display| display.server),
        Some(DisplayServer::Wayland),
        "a descriptor still naming the image's server is a claim the caller \
         cannot act on"
    );

    let second = computer.screen(ScreenId(1)).await.expect("a second screen");
    assert_eq!(second.id(), ScreenId(1));

    assert_eq!(
        driver.opened(),
        vec![ScreenId(0), ScreenId(1)],
        "a screen started on demand has to get the driver the box opened \
         with, not the default"
    );
}

#[tokio::test]
async fn a_swapped_driver_takes_the_input_and_the_takeover_gate_with_it() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .driver(Arc::new(ScriptedDriver::new()) as Arc<dyn computer::DesktopFactory>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    computer
        .double_click((5, 6), computer::Button::Left)
        .await
        .expect("a double click");

    computer
        .primary()
        .control()
        .hand_over("a person", SystemTime::now());

    assert!(
        computer
            .click((1, 2), computer::Button::Left)
            .await
            .is_err(),
        "the gate is on the trait, so it holds for a driver this crate never \
         wrote"
    );
    assert!(
        computer.screenshot().await.is_ok(),
        "reads stay open while a person drives"
    );
}

#[tokio::test]
async fn sweeping_removes_the_boxes_whose_deadline_has_passed() {
    // Two boxes labelled with deadlines: one last year, one next year.
    let listed = "old-box\t1000000000\nyoung-box\t4000000000\n";
    let cli = Arc::new(ScriptedCli::new().replying(saying(listed)));
    let machine = computer::DockerMachine::new(cli.clone() as Arc<dyn computer::ContainerCli>);

    let swept = computer::sweep_expired(
        &machine,
        std::time::UNIX_EPOCH + Duration::from_secs(2_000_000_000),
    )
    .await
    .expect("a sweep");

    assert_eq!(swept, vec!["old-box".to_string()]);

    let removed = cli.calls().into_iter().find(|argv| argv[0] == "rm");
    assert_eq!(
        removed.map(|argv| argv.last().cloned().unwrap_or_default()),
        Some("old-box".to_string()),
        "a box still inside its deadline must not be swept"
    );
}

#[tokio::test]
async fn a_label_nobody_here_wrote_is_left_alone() {
    let cli = Arc::new(ScriptedCli::new().replying(saying("odd-box\tsoon\n")));
    let machine = computer::DockerMachine::new(cli.clone() as Arc<dyn computer::ContainerCli>);

    let swept = computer::sweep_expired(&machine, std::time::SystemTime::now())
        .await
        .expect("a sweep");

    assert!(swept.is_empty());
    assert!(
        !cli.calls().iter().any(|argv| argv[0] == "rm"),
        "a deadline this crate cannot read is not a deadline it may act on"
    );
}

#[tokio::test]
async fn a_runtime_that_cannot_be_asked_what_it_holds_says_so() {
    // A microVM machine cannot list by label, and an empty list would read
    // as "nothing to sweep" rather than "cannot look".
    let api = Arc::new(computer::testing::ScriptedMicroVm::new());
    let machine = computer::MicroVm::new(api as Arc<dyn computer::microvm::MicroVmApi>);

    let error = computer::sweep_expired(&machine, std::time::SystemTime::now())
        .await
        .expect_err("it cannot look");

    assert!(matches!(error, Error::Unsupported { .. }));
}

#[tokio::test]
async fn a_swapped_profile_supplies_the_image_the_ports_and_the_driver() {
    // version, image inspect, run, port, then `scripted-screen start 1`.
    let cli = Arc::new(
        ScriptedCli::new()
            .replying(ok())
            .replying(ok())
            .replying(ok())
            .replying(saying(""))
            .replying(ok()),
    );

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .profile(Arc::new(ScriptedProfile))
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let run = cli.calls().into_iter().nth(2).expect("the run call");
    assert!(
        run.contains(&ScriptedProfile::IMAGE.to_string()),
        "the profile names its own image, and nothing builds ours under it"
    );

    assert_eq!(
        computer.primary().ports().view,
        ScriptedProfile::VIEW_PORT_BASE,
        "6080 is the built-in image's number, not every image's"
    );

    assert_eq!(
        computer.support().display.map(|display| display.server),
        Some(DisplayServer::Wayland),
        "the profile named a driver and the driver names the server; a \
         profile that got this wrong is reported as the driver in use"
    );

    computer
        .screen(ScreenId(1))
        .await
        .expect("a second screen")
        .open_url("https://example.com")
        .await
        .expect("a page");

    let sent = cli.last().expect("the open call");
    assert!(
        sent.contains(&ScriptedProfile::SCREEN_COMMAND.to_string())
            && sent.contains(&"open".to_string()),
        "a screen started long after launch has to speak the same image's \
         contract, not the default's"
    );
}

#[tokio::test]
async fn a_profiles_viewer_url_is_the_one_a_person_is_handed() {
    let cli = a_working_runtime("7100/tcp -> 127.0.0.1:32768\n");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .profile(Arc::new(ScriptedProfile))
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert_eq!(
        computer.viewer_url().as_deref(),
        Some("http://127.0.0.1:32768/scripted"),
        "the noVNC query string belongs to the image that serves noVNC"
    );
}

#[tokio::test]
async fn a_screens_environment_is_the_profiles_and_not_the_runtimes() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .profile(Arc::new(ScriptedProfile))
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    computer
        .exec_on(ScreenId(0), ["xdotool", "key", "a"])
        .await
        .expect("a command against screen 0");

    let sent = cli.last().expect("the exec call");
    assert!(
        sent.contains(&"SCRIPTED_SCREEN=0".to_string()),
        "the runtime moves the environment the profile built; it does not \
         decide that a screen means DISPLAY"
    );
    assert!(
        !sent.iter().any(|part| part.starts_with("DISPLAY=")),
        "an X11 variable against an image that is not X11 is a command that \
         goes in and moves nothing"
    );
}

#[tokio::test]
async fn a_box_publishes_the_ports_its_own_profile_serves() {
    let run = Computer::builder()
        .profile(Arc::new(ScriptedProfile))
        .name("preview-box")
        .preview()
        .expect("a scripted image");

    let published: Vec<&String> = run
        .windows(2)
        .filter(|pair| pair[0] == "--publish")
        .map(|pair| &pair[1])
        .collect();

    assert_eq!(
        published,
        vec![
            "127.0.0.1::7100",
            "127.0.0.1::7101",
            "127.0.0.1::7102",
            "127.0.0.1::7103"
        ],
        "two screens, two ports each, and no DevTools bridge — this image has \
         no browser to debug"
    );
    assert!(
        run.contains(&format!("{}=640", ScriptedProfile::WIDTH_ENV)),
        "the variables the image reads are the ones its profile names, at the \
         size that profile comes up at — 1280x800 is the built-in image's"
    );
}

#[tokio::test]
async fn a_wayland_box_is_driven_through_the_compositor_and_says_so() {
    // version, image inspect, run, port, then the screenshot's exec.
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .profile(Arc::new(computer::WaylandProfile))
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert_eq!(
        computer.support().display.map(|display| display.server),
        Some(DisplayServer::Wayland),
        "the profile named its driver, and the driver names the server"
    );

    let run = cli.calls().into_iter().nth(2).expect("the run call");
    assert!(
        run.iter().any(|part| part.starts_with("computer-wayland:")),
        "a Wayland box must not be started from the X11 image's tag"
    );

    computer
        .exec_on(ScreenId(0), ["grim", "-"])
        .await
        .expect("a command against screen 0");

    let sent = cli.last().expect("the exec call");
    assert!(
        sent.contains(&"WAYLAND_DISPLAY=wayland-1".to_string())
            && sent.contains(&"XDG_RUNTIME_DIR=/tmp/computer/run-1".to_string()),
        "a compositor is reached through its socket, not through a display \
         number"
    );
    assert!(!sent.iter().any(|part| part.starts_with("DISPLAY=")));
}

#[tokio::test]
async fn a_wayland_screen_refuses_a_cursor_it_never_measured() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .profile(Arc::new(computer::WaylandProfile))
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert!(
        matches!(computer.cursor().await, Err(Error::Unsupported { .. })),
        "no Wayland protocol reports the global pointer, so a position before \
         the first move is one nobody measured"
    );

    computer.move_to((640, 400)).await.expect("a move");
    assert_eq!(
        computer.cursor().await.ok(),
        Some(computer::Point::new(640, 400))
    );

    computer
        .primary()
        .control()
        .hand_over("a person", SystemTime::now());

    assert!(
        matches!(computer.cursor().await, Err(Error::Unsupported { .. })),
        "a person moved a pointer this driver did not move, and nothing will \
         say where to"
    );
}

/// The takeover token is what the image's input guard refuses on, so anyone who
/// can work one out can drive a screen somebody else has been handed.
///
/// It used to be the process id, the clock and a counter — all three readable
/// or guessable from another process on the same host.
#[tokio::test]
async fn a_takeover_token_cannot_be_worked_out_from_the_clock() {
    /// The token as the box receives it, out of the `control` command's argv.
    async fn mint() -> String {
        let cli = a_working_runtime("");
        let computer = Computer::builder()
            .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
            .driver(Arc::new(ScriptedDriver::new()) as Arc<dyn computer::DesktopFactory>)
            .wait_for_ready(None)
            .keep_on_drop(true)
            .launch()
            .await
            .expect("a box");

        computer.hand_over().await.expect("the screen");

        cli.calls()
            .into_iter()
            .find(|argv| argv.iter().any(|word| word == "control"))
            .and_then(|argv| {
                argv.iter()
                    .find(|word| word.starts_with("takeover-"))
                    .cloned()
            })
            .expect("the control command carries the token")
    }

    let first = mint().await;
    let second = mint().await;

    assert_ne!(first, second, "two takeovers must not share a token");

    let entropy = |token: &str| token.rsplit('-').next().unwrap_or_default().to_string();
    let (one, two) = (entropy(&first), entropy(&second));

    assert_eq!(one.len(), 64, "256 bits, hex encoded: {first}");
    assert!(
        one.chars().all(|c| c.is_ascii_hexdigit()),
        "the random half is hex: {first}"
    );

    // Two mints inside one process, microseconds apart. A token built from the
    // clock and a counter agrees on almost every character here; one drawn from
    // the CSPRNG agrees on about one in sixteen.
    let shared = one.chars().zip(two.chars()).filter(|(a, b)| a == b).count();
    assert!(
        shared < 32,
        "{shared} of 64 characters match between two mints — this is derived \
         from something predictable rather than drawn: {first} / {second}"
    );
}

/// An open viewer is a desktop anyone who reaches the port can watch, and on
/// the control port drive. The refusal is what makes the knob safe to have.
#[tokio::test]
async fn an_open_viewer_beyond_loopback_is_refused_before_a_box_exists() {
    for bind in [Bind::Any, Bind::Address("192.168.1.4".parse().unwrap())] {
        let cli = a_working_runtime("");

        let error = Computer::builder()
            .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
            .publish_on(bind.clone())
            .launch()
            .await
            .expect_err("an open viewer on the network");

        assert!(matches!(error, Error::Denied { .. }), "{bind:?}");
        assert_eq!(
            cli.count(),
            1,
            "{bind:?}: the runtime was greeted and then nothing was created — an \
             open box must not exist even for the moment before it is torn down"
        );
    }
}

/// The other half of the rule, and the reason it is not simply "no publishing":
/// either gate satisfies it, and the caller picks which by how they hand the
/// box out.
#[tokio::test]
async fn a_gated_viewer_beyond_loopback_is_allowed() {
    for auth in [computer::Auth::Password, computer::Auth::Token] {
        let cli = a_working_runtime("");

        Computer::builder()
            .cli(cli as Arc<dyn computer::ContainerCli>)
            .publish_on(Bind::Any)
            .auth(auth)
            .wait_for_ready(None)
            .keep_on_drop(true)
            .launch()
            .await
            .unwrap_or_else(|error| panic!("{auth:?} gates the viewer: {error}"));
    }
}

/// The refusal reads the reach and not "this is not the default", or a caller
/// who spelled loopback out would go looking for a secret they do not need.
#[tokio::test]
async fn loopback_spelled_out_opens_like_the_default() {
    let cli = a_working_runtime("");

    Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .publish_on(Bind::Address("127.0.0.1".parse().unwrap()))
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box on loopback");
}

/// The deployment `docs/viewer-auth.md` argues for: the box on loopback behind
/// a proxy, and the URL naming the proxy. That name is not derivable from
/// anything this crate holds, so a URL built from the bind is wrong for every
/// box that is not reached at 127.0.0.1.
#[tokio::test]
async fn the_url_a_person_is_handed_names_the_advertised_host() {
    let cli = a_working_runtime("6080/tcp -> 127.0.0.1:32768\n");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .advertise("boxes.example.com")
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert_eq!(
        computer.viewer_url().as_deref(),
        Some("http://boxes.example.com:32768/vnc.html?autoconnect=1&resize=scale"),
        "the port is still the one the runtime mapped; only the name changes"
    );
}

/// The shape a link has to have to be worth handing out: everything a person
/// needs is in the URL, and the two doors do not share a credential.
#[tokio::test]
async fn a_token_gate_puts_a_different_ticket_on_each_door() {
    let cli = a_working_runtime("6080/tcp -> 127.0.0.1:32768\n6081/tcp -> 127.0.0.1:32769\n");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .auth(computer::Auth::Token)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let credentials = computer.credentials().expect("a gated box holds a pair");
    let viewer = computer.viewer_url().expect("a viewer");
    let takeover = computer.hand_over().await.expect("the screen");
    let control = takeover.url().expect("a control URL");

    assert!(
        viewer.contains(credentials.view.expose()),
        "the watch link carries what opens the watch door"
    );
    assert!(
        control.contains(credentials.control.expose()),
        "the control link carries what opens the control door"
    );
    assert!(
        !viewer.contains(credentials.control.expose()),
        "a watch link that carries the control credential is a control link"
    );
}

/// The reason `Auth::Password` exists: the credential reaches the browser
/// through a prompt, so it lands in no history, no referrer and no proxy log.
#[tokio::test]
async fn a_password_gate_puts_nothing_in_the_url() {
    let cli = a_working_runtime("6080/tcp -> 127.0.0.1:32768\n");

    let computer = Computer::builder()
        .cli(cli as Arc<dyn computer::ContainerCli>)
        .auth(computer::Auth::Password)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let credentials = computer.credentials().expect("a gated box holds a pair");
    let viewer = computer.viewer_url().expect("a viewer");

    assert!(!viewer.contains(credentials.view.expose()), "{viewer}");
    assert!(!viewer.contains(credentials.control.expose()), "{viewer}");
    assert_eq!(
        viewer, "http://127.0.0.1:32768/vnc.html?autoconnect=1&resize=scale",
        "the URL is the ungated one; the prompt is what differs"
    );
}

/// The credential has to be in the box before any screen starts, because a
/// screen opened an hour later has to answer to the same one.
#[tokio::test]
async fn the_gate_reaches_the_box_as_environment() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .auth(computer::Auth::Token)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    let credentials = computer.credentials().expect("a pair");
    let run = cli
        .calls()
        .into_iter()
        .find(|argv| argv.iter().any(|word| word == "run"))
        .expect("the box was created");

    let carries = |wanted: &str| run.iter().any(|word| word == wanted);
    assert!(carries("COMPUTER_VIEWER_AUTH=token"));
    assert!(carries(&format!(
        "COMPUTER_VIEW_SECRET={}",
        credentials.view.expose()
    )));
    assert!(carries(&format!(
        "COMPUTER_CONTROL_SECRET={}",
        credentials.control.expose()
    )));
}

/// An open box is what every local box has always been, and it must not start
/// carrying credentials nothing reads.
#[tokio::test]
async fn an_open_box_carries_no_credential_at_all() {
    let cli = a_working_runtime("");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert!(computer.credentials().is_none());
    assert!(
        !cli.calls()
            .concat()
            .iter()
            .any(|word| word.contains("COMPUTER_VIEW_SECRET")
                || word.contains("COMPUTER_VIEWER_AUTH")),
        "an open gate is the absence of one, not a variable saying so"
    );
}

/// `preview` is printed, and a preview that minted a credential would put a
/// desktop in whatever printed it.
#[tokio::test]
async fn a_preview_mints_nothing_it_could_leak() {
    let previewed = Computer::builder()
        .auth(computer::Auth::Token)
        .publish_on(Bind::Any)
        .preview()
        .expect("arguments");

    assert!(
        !previewed
            .iter()
            .any(|word| word.contains("COMPUTER_VIEW_SECRET")
                || word.contains("COMPUTER_CONTROL_SECRET")),
        "{previewed:?}"
    );
}

/// CDP has no authentication and cannot be given one, so it is the one door
/// the gate cannot cover. Publishing it beyond loopback would hand whoever
/// reaches it the whole browser.
#[tokio::test]
async fn devtools_is_withdrawn_rather_than_published_beyond_loopback() {
    let cli = a_working_runtime("6080/tcp -> 127.0.0.1:32768\n");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .publish_on(Bind::Any)
        .auth(computer::Auth::Token)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a gated box");

    let bridge = computer::Profile::ports(&computer::X11Profile)
        .devtools_bridge
        .expect("the built-in image bridges devtools");

    let run = cli
        .calls()
        .into_iter()
        .find(|argv| argv.iter().any(|word| word == "run"))
        .expect("the box was created");

    assert!(
        !run.iter()
            .any(|word| word.ends_with(&format!("::{bridge}"))),
        "the bridge was published: {run:?}"
    );
    assert!(computer.devtools().is_none());
    assert_eq!(
        computer.support().browser.as_ref().map(|b| b.cdp),
        Some(false),
        "a claim withdrawn, so `audit` skips it rather than failing it"
    );
}

/// The other half: on loopback the bridge is exactly as useful as it was, and
/// withdrawing it there would break every local caller for nothing.
#[tokio::test]
async fn devtools_survives_on_loopback() {
    let cli = a_working_runtime("9223/tcp -> 127.0.0.1:32769\n");

    let computer = Computer::builder()
        .cli(Arc::clone(&cli) as Arc<dyn computer::ContainerCli>)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert!(computer.devtools().is_some());
    assert_eq!(
        computer.support().browser.as_ref().map(|b| b.cdp),
        Some(true)
    );
}
