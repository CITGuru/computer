use computer::machine::Machine;
use computer::sandboxes::e2b::{self, E2bApi, Sandbox, SandboxPlan};
use computer::testing::ScriptedE2b;
use computer::{Auth, Button, Computer, Config, Delta, Point, ScreenId, X11Profile};
use std::sync::Arc;
use std::time::Duration;

fn bundled(profile: Arc<dyn computer::Profile>) -> Config {
    Computer::builder()
        .profile(profile)
        .config()
        .expect("a resolved configuration")
}

async fn launched(api: Arc<ScriptedE2b>, public_viewer: bool) -> Computer {
    let (machine, profile) = e2b::pair(Arc::clone(&api) as Arc<dyn E2bApi>, Arc::new(X11Profile));

    let auth = match public_viewer {
        true => Auth::Token,
        false => Auth::Open,
    };

    Computer::builder()
        .machine(Arc::new(machine.public_viewer(public_viewer)))
        .profile(profile)
        .image("tmpl-abc")
        .name("box")
        .auth(auth)
        .launch()
        .await
        .expect("a sandbox")
}

#[tokio::test]
async fn a_sandbox_runs_the_same_desktop() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), true).await;

    assert_eq!(computer.provider(), "e2b");

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

    let socket = computer.primary().viewer_socket().expect("a viewer socket");
    assert!(
        socket.starts_with("wss://6080-sbx-0.e2b.app/websockify?token="),
        "got {socket}, which is not the sandbox's own host"
    );
    assert!(
        computer.primary().socket_headers().contains(&(
            "e2b-traffic-access-token".to_string(),
            "traffic".to_string()
        )),
        "the proxy refuses the socket without the traffic token"
    );
    assert!(
        computer.viewer_url().is_none(),
        "a browser cannot send the token, so a direct link would refuse"
    );
}

#[tokio::test]
async fn a_public_sandbox_hands_out_the_links_a_browser_can_open() {
    let api = Arc::new(ScriptedE2b::new());
    let (machine, profile) = e2b::pair(Arc::clone(&api) as Arc<dyn E2bApi>, Arc::new(X11Profile));
    let computer = Computer::builder()
        .machine(Arc::new(machine.public_viewer(true).public_traffic(true)))
        .profile(profile)
        .image("tmpl-abc")
        .name("box")
        .auth(Auth::Token)
        .launch()
        .await
        .expect("a sandbox");

    let viewer = computer.viewer_url().expect("a viewer link");
    assert!(
        viewer.starts_with("https://6080-sbx-0.e2b.app/vnc.html"),
        "got {viewer}"
    );

    let takeover = computer.hand_over().await.expect("the screen goes over");
    let control = takeover.url().expect("a takeover link");
    assert!(control.starts_with("https://6081-sbx-0."), "got {control}");
    takeover.end().await.expect("it comes back");

    let devtools = computer.devtools().expect("a DevTools endpoint");
    assert!(
        devtools
            .headers
            .iter()
            .any(|(name, _)| name == "x-computer-devtools"),
        "open ports or not, the bridge still wants its secret"
    );
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
async fn devtools_is_the_sandboxs_own_host_behind_the_traffic_token() {
    for public_viewer in [true, false] {
        let api = Arc::new(ScriptedE2b::new());
        let computer = launched(Arc::clone(&api), public_viewer).await;

        let endpoint = computer.devtools().expect("a DevTools endpoint");
        assert_eq!(endpoint.http_url, "https://9223-sbx-0.e2b.app");
        assert_eq!(endpoint.ws_url, "wss://9223-sbx-0.e2b.app/devtools/browser");
        assert!(
            endpoint.headers.contains(&(
                "e2b-traffic-access-token".to_string(),
                "traffic".to_string()
            )),
            "a secure sandbox refuses the upgrade without it"
        );
        assert!(
            endpoint
                .headers
                .iter()
                .any(|(name, _)| name == "x-computer-devtools"),
            "and the bridge in the box refuses it without this"
        );
        assert!(computer.browser().is_some());
        assert_eq!(
            computer.support().browser.as_ref().map(|b| b.cdp),
            Some(true)
        );
    }
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
    let socket = second.viewer_socket().expect("a viewer socket");

    assert!(
        socket.starts_with("wss://6082-sbx-0."),
        "got {socket}; screen 1 views on 6082"
    );
}

#[tokio::test]
async fn a_takeover_is_a_second_host_and_not_a_mode_on_the_first() {
    let api = Arc::new(ScriptedE2b::new());
    let computer = launched(Arc::clone(&api), true).await;

    let takeover = computer.hand_over().await.expect("the screen goes over");
    let control = computer
        .primary()
        .control_socket()
        .expect("a control socket");

    assert!(control.starts_with("wss://6081-sbx-0."), "got {control}");
    assert!(
        takeover.url().is_none(),
        "the person takes over through the server, which can send the token"
    );
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
async fn a_container_image_becomes_a_template_nobody_had_to_build() {
    let api = Arc::new(ScriptedE2b::new());
    let (machine, profile) = e2b::pair(Arc::clone(&api) as Arc<dyn E2bApi>, Arc::new(X11Profile));
    let config = bundled(profile);

    machine
        .ensure_image(&config)
        .await
        .expect("E2B runs templates, so one is built from the image this crate holds");

    assert_eq!(api.built().len(), 1, "one build, for one image");
    assert!(
        api.carried().contains(&"start.sh".to_string()),
        "the files the image copies went over first: {:?}",
        api.carried()
    );

    machine.start("desk", &config).await.expect("a sandbox");
    let plan = api.plans().pop().expect("a plan");
    assert!(
        plan.template.starts_with("tmpl-"),
        "the box starts from the template that was built, not from a container image: {}",
        plan.template
    );
}

#[tokio::test]
async fn a_template_that_is_already_there_is_not_built_again() {
    let api = Arc::new(ScriptedE2b::new().holding_template("computer-desktop-abc", "tmpl-held"));
    let (machine, profile) = e2b::pair(Arc::clone(&api) as Arc<dyn E2bApi>, Arc::new(X11Profile));

    let mut config = bundled(profile);
    config.image = "computer-desktop:abc".to_string();

    machine.ensure_image(&config).await.expect("it is there");

    assert!(
        api.built().is_empty(),
        "a template this vendor already holds is used as it is"
    );
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
