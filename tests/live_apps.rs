//! Launching real apps in a real box. Ignored by default.

use computer::{Arrange, Computer, Launch, Point, WaylandProfile};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "needs a container runtime and installs GIMP and VS Code"]
async fn a_launch_waits_for_the_app_to_have_drawn() {
    let spec: computer_types::Spec = serde_json::from_str(
        r#"{"apps":{"gimp":{},"vscode":{}},"desktop":{"width":1280,"height":800}}"#,
    )
    .expect("a spec");

    let computer = computer::Builder::from_spec(&spec)
        .expect("a builder")
        .launch()
        .await
        .expect("a box with both apps");

    let outcome = exercise(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn exercise(computer: &Computer) -> computer::Result<()> {
    let screen = computer.primary();

    for (name, class, command) in [
        ("gimp", "gimp", vec!["gimp".to_string()]),
        (
            "vscode",
            "code",
            vec![
                "code".to_string(),
                "--no-sandbox".to_string(),
                "--user-data-dir=/var/lib/computer/vscode".to_string(),
            ],
        ),
    ] {
        let at = Instant::now();
        let window = screen
            .launch(&Launch {
                command,
                class: class.to_string(),
                settle: Duration::from_millis(600),
                within: Duration::from_secs(60),
            })
            .await?;

        println!(
            "  {name}: window {} [{}] after {}ms",
            window.id,
            window.title,
            at.elapsed().as_millis()
        );

        assert!(
            !window.title.contains("Startup"),
            "{name} answered with its splash screen: {}",
            window.title
        );
        assert!(!window.title.is_empty(), "{name} has no title");
    }

    let windows = screen.windows().await?;
    println!("  {} windows on screen", windows.len());
    assert!(windows.len() >= 2, "both apps are on screen: {windows:?}");

    Ok(())
}

/// A Wayland-native program: this image runs sway with `xwayland disable`, so
/// an X11 app has no display here at all.
#[tokio::test]
#[ignore = "needs a container runtime and installs a terminal"]
async fn a_wayland_launch_waits_for_the_app_too() {
    let computer = Computer::builder()
        .profile(Arc::new(WaylandProfile))
        .packages(["foot"])
        .launch()
        .await
        .expect("a wayland box with a terminal");

    let outcome = wayland_exercise(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn wayland_exercise(computer: &Computer) -> computer::Result<()> {
    let screen = computer.primary();
    let at = Instant::now();

    let window = screen
        .launch(&Launch {
            command: vec!["foot".to_string()],
            class: "foot".to_string(),
            settle: Duration::from_millis(600),
            within: Duration::from_secs(60),
        })
        .await?;

    println!(
        "  foot on wayland: window {} after {}ms",
        window.id,
        at.elapsed().as_millis()
    );

    let windows = screen.windows().await?;
    println!("  {} windows on screen", windows.len());
    assert!(
        windows.iter().any(|w| w.id == window.id),
        "the launched window is one the compositor lists: {windows:?}"
    );

    screen.focus(&window.id).await?;

    let active = screen.active_window().await?;
    println!("  active: {active:?}");
    assert_eq!(
        active.map(|one| one.id),
        Some(window.id.clone()),
        "the window that was focused is the one holding the keyboard"
    );

    let sized = screen
        .arrange(
            &window.id,
            Arrange::Size {
                width: 600,
                height: 400,
            },
        )
        .await?;
    println!("  sized: {}x{}", sized.width, sized.height);
    assert_eq!((sized.width, sized.height), (600, 400));

    let moved = screen
        .arrange(&window.id, Arrange::At(Point::new(80, 60)))
        .await?;
    println!("  moved: {},{}", moved.at.x, moved.at.y);
    assert_eq!(moved.at, Point::new(80, 60));

    let big = screen.arrange(&window.id, Arrange::Maximise).await?;
    println!("  maximised: {}x{}", big.width, big.height);
    assert!(big.width > sized.width, "full screen is wider than 600");

    screen.arrange(&window.id, Arrange::Minimise).await?;
    let back = screen.arrange(&window.id, Arrange::Restore).await?;
    println!("  back: {}x{}", back.width, back.height);
    assert_eq!(back.id, window.id);

    let again = screen
        .wait_for_window("foot", Duration::from_secs(10))
        .await?;
    assert_eq!(again.class, "foot");

    screen.close_window(&window.id).await?;
    Ok(())
}

/// An X11 program on the Wayland desktop. Without the feature the image has
/// no X server, and GIMP fails to open a display rather than failing to draw.
#[tokio::test]
#[ignore = "needs a container runtime and builds a wayland image with Xwayland"]
async fn an_x11_app_runs_on_wayland_when_the_feature_is_asked_for() {
    let spec: computer_types::Spec = serde_json::from_str(
        r#"{"desktop":{"server":"wayland","features":["x11_apps"]},"apps":{"gimp":{}}}"#,
    )
    .expect("a spec");

    let computer = computer::Builder::from_spec(&spec)
        .expect("a builder")
        .launch()
        .await
        .expect("a wayland box carrying Xwayland");

    let outcome = x11_on_wayland(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn x11_on_wayland(computer: &Computer) -> computer::Result<()> {
    let at = Instant::now();
    let window = computer
        .primary()
        .launch(&Launch {
            command: vec!["gimp".to_string()],
            class: "gimp".to_string(),
            settle: Duration::from_millis(600),
            within: Duration::from_secs(90),
        })
        .await?;

    println!(
        "  gimp under xwayland: window {} after {}ms",
        window.id,
        at.elapsed().as_millis()
    );

    let windows = computer.primary().windows().await?;
    assert!(
        windows.iter().any(|one| one.id == window.id),
        "the compositor lists an X11 client like any other: {windows:?}"
    );
    Ok(())
}

/// Every name this crate ships, started for real.
///
/// Only running them says whether an entry holds: `xterm` sets no
/// `_NET_WM_WINDOW_TYPE` and was invisible to a launch for a while.
#[tokio::test]
#[ignore = "needs a container runtime and installs the whole catalog"]
async fn every_app_in_the_catalog_starts_and_draws() {
    let named: Vec<String> = computer::apps::builtin().into_keys().collect();
    let apps = named
        .iter()
        .map(|name| format!(r#""{name}":{{}}"#))
        .collect::<Vec<_>>()
        .join(",");

    let spec: computer_types::Spec =
        serde_json::from_str(&format!(r#"{{"apps":{{{apps}}}}}"#)).expect("a spec");

    let computer = computer::Builder::from_spec(&spec)
        .expect("a builder")
        .launch()
        .await
        .expect("a box carrying every app");

    let outcome = whole_catalog(&computer, &named).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every app");
}

async fn whole_catalog(computer: &Computer, named: &[String]) -> computer::Result<()> {
    let screen = computer.primary();

    for name in named {
        let app = computer::apps::resolve(&computer_types::Spec::default(), name)?;
        let Some(computer_types::WindowMatch::Class(class)) = app.window else {
            panic!("{name} names no window class");
        };

        let at = Instant::now();
        let window = screen
            .launch(&Launch {
                command: app.command.clone(),
                class,
                settle: Duration::from_millis(app.settle_ms.unwrap_or(600)),
                within: Duration::from_secs(60),
            })
            .await?;

        println!(
            "  {name:12} {:>6}ms  [{}]",
            at.elapsed().as_millis(),
            window.title
        );

        assert!(
            !window.title.is_empty(),
            "{name} answered with a window that has no title, which is what a \
             splash screen looks like"
        );
    }

    Ok(())
}
