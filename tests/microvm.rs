//! A box on a hypervisor, checked without one.
//!
//! A microVM boots a kernel, which takes seconds and a hypervisor to do. What
//! is worth testing is the mapping: which ports were forwarded, what brought
//! the screen up, and what happens to an image a hypervisor cannot read.

use computer::machine::Machine;
use computer::microvm::{MicroVm, MicroVmApi, free_port, plan_for, port_pairs};
use computer::testing::ScriptedMicroVm;
use computer::{Computer, Config, Error, image};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The configuration a box actually starts with: ports, boot command and
/// environment all resolved from its profile.
fn resolved() -> Config {
    Computer::builder().config().expect("the built-in profile")
}

fn micro(api: Arc<ScriptedMicroVm>) -> MicroVm {
    MicroVm::new(api as Arc<dyn MicroVmApi>).named("microsandbox")
}

/// An image the caller named, which is nothing this crate builds.
fn image_named(image: &str) -> Config {
    Config {
        image: image.to_string(),
        bundle: None,
        ..Config::default()
    }
}

/// One of ours, which lives in a container runtime's store.
fn bundled_image(image: &str) -> Config {
    Config {
        image: image.to_string(),
        ..Config::default()
    }
}

#[tokio::test]
async fn the_screen_is_brought_up_once_and_nothing_idles() {
    let api = Arc::new(ScriptedMicroVm::new());
    let machine = micro(Arc::clone(&api));

    machine.start("box", &resolved()).await.expect("a box");

    assert_eq!(
        api.last_line(),
        "computer-desktop --once",
        "a microVM lives until it is stopped, so the supervisor's idle loop \
         would hold an exec open for the life of the machine"
    );
}

#[tokio::test]
async fn the_ports_the_hypervisor_was_given_are_the_ones_the_caller_gets() {
    let api = Arc::new(ScriptedMicroVm::new());
    let machine = micro(Arc::clone(&api));

    let mapped = machine.start("box", &resolved()).await.expect("a box");
    let plan = api.plans().pop().expect("a plan");

    assert_eq!(mapped.len(), plan.ports.len());
    for (host, guest) in &plan.ports {
        assert_eq!(
            mapped.get(guest),
            Some(host),
            "a hypervisor forwards the pairs it was given, so this side \
             already knows them"
        );
    }
    assert!(mapped.contains_key(&image::DEVTOOLS_BRIDGE_PORT));
}

#[tokio::test]
async fn a_command_carries_the_environment_it_was_given() {
    let api = Arc::new(ScriptedMicroVm::new());
    let machine = micro(Arc::clone(&api));
    let env = BTreeMap::from([("DISPLAY".to_string(), ":3".to_string())]);

    machine
        .exec("box", &["xdotool".to_string()], &env)
        .await
        .expect("a command");

    assert_eq!(
        api.last_line(),
        "xdotool DISPLAY=:3",
        "there is no `exec --env` here; the environment goes in the call itself"
    );
}

#[tokio::test]
async fn a_container_image_is_refused_with_the_way_out_named() {
    // A hypervisor that has been handed nothing, which is where one starts.
    let machine = micro(Arc::new(ScriptedMicroVm::new()));

    let error = machine
        .ensure_image(&bundled_image("computer-desktop:a16756c2080fd481"))
        .await
        .expect_err("a hypervisor cannot read a container runtime's store");

    let message = error.to_string();
    assert!(message.contains("import_image"), "{message}");
}

#[tokio::test]
async fn a_root_filesystem_that_is_not_there_is_reported_before_anything_boots() {
    let machine = micro(Arc::new(ScriptedMicroVm::new()));

    assert!(
        machine
            .ensure_image(&image_named("/no/such/rootfs"))
            .await
            .is_err()
    );
    assert!(
        machine
            .ensure_image(&image_named("docker.io/library/debian:bookworm"))
            .await
            .is_ok(),
        "a reference is the hypervisor's to pull"
    );
}

#[tokio::test]
async fn a_machine_with_nothing_to_boot_is_refused_rather_than_left_dead() {
    let api = Arc::new(ScriptedMicroVm::new());
    let machine = micro(Arc::clone(&api));

    let error = machine
        .start("box", &Config::default())
        .await
        .expect_err("no boot command");

    assert!(
        matches!(error, Error::Unsupported { .. }),
        "a container's image starts itself; a machine's does not, and one \
         booted with no command is a box that never gets a screen"
    );
    assert_eq!(api.removed(), vec!["box".to_string()]);
}

#[tokio::test]
async fn a_machine_that_will_not_boot_is_removed_rather_than_left_running() {
    let api = Arc::new(ScriptedMicroVm::new().failing(1, "no X server on :1"));
    let machine = micro(Arc::clone(&api));

    // No network, so the boot is the first command run rather than the route
    // check.
    let offline = Config {
        network: false,
        ..resolved()
    };

    let error = machine
        .start("box", &offline)
        .await
        .expect_err("the screen never came up");

    assert!(matches!(error, Error::Failed { .. }));
    assert_eq!(
        api.removed(),
        vec!["box".to_string()],
        "a microVM left behind holds its whole memory ceiling until somebody \
         finds it"
    );
}

#[tokio::test]
async fn a_dropped_handle_reaps_only_when_it_was_told_how() {
    let plain = micro(Arc::new(ScriptedMicroVm::new()));
    assert!(
        plain.reaper("box").is_none(),
        "no command was given, and inventing one would run something unknown"
    );

    let reaping =
        micro(Arc::new(ScriptedMicroVm::new())).reaping_with("msb", ["sandbox", "rm", "{}"]);
    let (program, args) = reaping.reaper("box").expect("a reaper");

    assert_eq!(program, "msb");
    assert_eq!(args, vec!["sandbox", "rm", "box"]);
}

#[tokio::test]
async fn a_whole_box_runs_on_a_hypervisor_the_same_way_it_runs_in_a_container() {
    let api = Arc::new(ScriptedMicroVm::new());
    let machine = Arc::new(micro(Arc::clone(&api)));

    let computer = Computer::builder()
        .machine(machine)
        .image("./rootfs-that-is-not-checked")
        .ensure_image(false)
        .size(1600, 900)
        .wait_for_ready(None)
        .keep_on_drop(true)
        .launch()
        .await
        .expect("a box");

    assert_eq!(computer.runtime(), "microsandbox");
    assert!(computer.viewer_url().is_some(), "the viewer is forwarded");
    assert!(computer.devtools().is_some());

    let display = computer.support().display.expect("a screen");
    assert_eq!((display.width, display.height), (1600, 900));

    // And it drives through the same calls.
    computer.type_text("on a microVM").await.expect("typing");
    assert_eq!(
        api.last_line(),
        "xdotool type --clearmodifiers -- on a microVM DISPLAY=:1"
    );
}

#[tokio::test]
async fn attaching_to_a_machine_somebody_else_started_reports_it_gone_when_it_is() {
    let api = Arc::new(ScriptedMicroVm::new().stopped());
    let machine = Arc::new(micro(api));

    let error = Computer::attach_to(machine, "box-7")
        .await
        .expect_err("it is not running");
    assert!(matches!(error, Error::Gone(name) if name == "box-7"));
}

#[test]
fn a_free_port_is_asked_for_once_per_published_port() {
    let pairs = port_pairs(&[6080, 6081, 6082, 9223], free_port);
    let guests: Vec<u16> = pairs.iter().map(|(_, guest)| *guest).collect();

    let mut sorted = guests.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(guests.len(), sorted.len(), "one host port per guest port");
}

#[test]
fn a_plan_is_a_value_and_not_a_chain_of_builder_calls() {
    let plan = plan_for("box", &Config::default(), vec![(40000, 6080)]);

    assert_eq!(plan.name, "box");
    assert_eq!(plan.ports, vec![(40000, 6080)]);
    assert!(plan.network);
}

#[tokio::test]
async fn an_image_already_handed_over_is_not_refused() {
    let image = "computer-desktop:a16756c2080fd481";
    let machine = micro(Arc::new(ScriptedMicroVm::new().holding(image)));

    machine
        .ensure_image(&image_named(image))
        .await
        .expect("the hypervisor has it, so there is nothing to refuse");
}
