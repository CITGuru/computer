//! A box in a sandbox, checked without one.
//!
//! What is worth testing here is the wiring: that a `Computer` built on
//! [`E2bMachine`] drives the same way a container does, that a port becomes a
//! subdomain rather than a host port, and that the two claims which stop being
//! true off this host are withdrawn rather than left standing.

use computer::machine::Machine;
use computer::sandboxes::e2b::{self, E2bApi, Sandbox, SandboxPlan};
use computer::testing::ScriptedE2b;
use computer::{Button, Computer, Config, Delta, Point, ScreenId, X11Profile};
use std::sync::Arc;
use std::time::Duration;

/// The configuration a box starts with when nobody named an image, which is
/// the one this crate builds for a container runtime.
fn bundled(profile: Arc<dyn computer::Profile>) -> Config {
    Computer::builder()
        .profile(profile)
        .config()
        .expect("a resolved configuration")
}

/// A box on a scripted sandbox, launched the way a caller would.
async fn launched(api: Arc<ScriptedE2b>, public_viewer: bool) -> Computer {
    let (machine, profile) = e2b::pair(Arc::clone(&api) as Arc<dyn E2bApi>, Arc::new(X11Profile));

    Computer::builder()
        .machine(Arc::new(machine.public_viewer(public_viewer)))
        .profile(profile)
        .image("tmpl-abc")
        .name("box")
        .launch()
        .await
        .expect("a sandbox")
}

#[tokio::test]
async fn a_sandbox_runs_the_same_desktop() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), true).await;

    assert_eq!(computer.runtime(), "e2b");

    computer
        .click(Point::new(640, 400), Button::Left)
        .await
        .expect("a click");

    assert_eq!(
        api.commands().pop().expect("a command"),
        vec!["xdotool", "mousemove", "--", "640", "400", "click", "1"],
        "the driver does not know it is talking to a sandbox"
    );
}

#[tokio::test]
async fn the_screen_is_brought_up_once_because_a_sandbox_has_no_entrypoint() {
    let api = Arc::new(ScriptedE2b::new());
    let _computer = launched(Arc::clone(&api), false).await;

    assert_eq!(
        api.commands().first().expect("a first command"),
        &vec!["computer-desktop".to_string(), "--once".to_string()],
    );
}

#[tokio::test]
async fn a_port_becomes_a_subdomain_rather_than_a_host_port() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), true).await;

    let viewer = computer.viewer_url().expect("a public viewer");
    assert!(
        viewer.starts_with("https://6080-sbx-0."),
        "got {viewer}, which is not the sandbox's own host"
    );
    assert!(viewer.ends_with("/vnc.html?autoconnect=1&resize=scale"));
}

#[tokio::test]
async fn a_secure_sandbox_hands_out_no_url_that_would_refuse() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), false).await;

    assert!(
        computer.viewer_url().is_none(),
        "the proxy wants a header a browser cannot send"
    );
    assert!(
        computer.click(Point::new(4, 4), Button::Left).await.is_ok(),
        "not watchable is not the same as not driveable"
    );
}

#[tokio::test]
async fn devtools_is_withdrawn_rather_than_published_to_nowhere() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), true).await;

    assert!(computer.devtools().is_none());
    assert!(computer.browser().is_none());
    assert_eq!(
        computer.support().browser.as_ref().map(|b| b.cdp),
        Some(false),
        "a claim withdrawn, so `audit` skips it rather than failing it"
    );
}

#[tokio::test]
async fn a_browser_that_cannot_be_reached_from_here_still_reports_ready() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), false).await;

    let present = computer.probe().await;
    assert!(
        present.ready(),
        "readiness asks whether chromium is up in the box, not whether \
         this side can reach its debugger"
    );
}

#[tokio::test]
async fn every_screen_gets_its_own_host() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), true).await;

    let second = computer.screen(ScreenId(1)).await.expect("a second screen");
    let viewer = second.viewer_url().expect("a public viewer");

    assert!(
        viewer.starts_with("https://6082-sbx-0."),
        "got {viewer}; screen 1 views on 6082"
    );
}

#[tokio::test]
async fn a_takeover_is_a_second_host_and_not_a_mode_on_the_first() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), true).await;

    let takeover = computer.hand_over().await.expect("the screen goes over");
    let control = takeover.url().expect("a control URL");

    assert!(control.starts_with("https://6081-sbx-0."), "got {control}");
    assert!(
        computer
            .click(Point::new(1, 1), Button::Left)
            .await
            .is_err(),
        "the gate closes here whatever runtime the box is on"
    );

    takeover.end().await.expect("it comes back");
    computer
        .scroll(Point::new(4, 4), Delta::down(1))
        .await
        .expect("the gate opens again");
}

#[tokio::test]
async fn the_sandbox_is_killed_when_the_box_is() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), false).await;

    computer.shutdown().await.expect("it goes away");
    assert_eq!(api.killed(), vec!["sbx-0".to_string()]);
}

#[tokio::test]
async fn the_name_travels_as_metadata_because_e2b_names_the_sandbox() {
    let api = Arc::new(ScriptedE2b::new());
    let _computer = launched(Arc::clone(&api), false).await;

    let plan = api.plans().pop().expect("one plan");
    assert_eq!(plan.name, "box");
    assert_eq!(plan.template, "tmpl-abc");
}

#[tokio::test]
async fn a_secure_sandbox_is_asked_for_whatever_the_viewer_setting_is() {
    for public in [true, false] {
        let api = Arc::new(ScriptedE2b::new());
        let _computer = launched(Arc::clone(&api), public).await;

        let plan = api.plans().pop().expect("one plan");
        let body = e2b::wire::new_sandbox(&plan);

        assert_eq!(
            body["secure"],
            serde_json::json!(true),
            "the crate always sends its own tokens; only the browser cannot"
        );
    }
}

#[tokio::test]
async fn a_container_image_is_refused_with_the_way_across() {
    let (machine, profile) = e2b::pair(Arc::new(ScriptedE2b::new()), Arc::new(X11Profile));

    let error = machine
        .ensure_image(&bundled(profile))
        .await
        .expect_err("E2B runs templates, not container images");

    assert!(error.needs_another_place());
    assert!(error.to_string().contains("e2b template build"));
}

#[tokio::test]
async fn a_box_this_process_never_started_is_found_by_metadata() {
    let api = Arc::new(ScriptedE2b::new().holding("left-over", "sbx-9"));
    let (machine, _) = e2b::pair(Arc::clone(&api) as Arc<dyn E2bApi>, Arc::new(X11Profile));

    assert!(machine.running("left-over").await.expect("a listing"));
    assert!(!machine.running("never-was").await.expect("a listing"));
}

#[tokio::test]
async fn an_expiry_is_swept_by_the_name_this_crate_gave_the_box() {
    let api = Arc::new(ScriptedE2b::new());
    let (machine, profile) = e2b::pair(Arc::clone(&api) as Arc<dyn E2bApi>, Arc::new(X11Profile));
    let machine = Arc::new(machine);

    let _computer = Computer::builder()
        .machine(Arc::clone(&machine) as Arc<dyn Machine>)
        .profile(profile)
        .image("tmpl-abc")
        .name("doomed")
        .expires_after(Duration::from_secs(60))
        .launch()
        .await
        .expect("a sandbox");

    assert!(machine.sweepable());
    let found = machine
        .labelled(computer::EXPIRY_LABEL)
        .await
        .expect("a sweep");

    assert_eq!(
        found.first().map(|(name, _)| name.as_str()),
        Some("doomed"),
        "a sweeper works from names, and the sandbox ID means nothing to it"
    );
}

#[test]
fn a_sandbox_url_is_the_port_and_the_id() {
    let sandbox = Sandbox::new("i7q3");

    assert_eq!(sandbox.url(6080), "https://6080-i7q3.e2b.app");
    assert_eq!(
        sandbox.envd_url(),
        "https://49983-i7q3.e2b.app",
        "the data plane is a port like any other"
    );
}

#[test]
fn a_plan_says_what_would_be_created_before_anything_is() {
    let plan = SandboxPlan {
        name: "box".to_string(),
        template: "tmpl-abc".to_string(),
        network: false,
        ..SandboxPlan::default()
    };

    let body = e2b::wire::new_sandbox(&plan);
    assert_eq!(body["templateID"], serde_json::json!("tmpl-abc"));
    assert_eq!(body["allow_internet_access"], serde_json::json!(false));
}
