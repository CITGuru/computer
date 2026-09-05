//! Launching real apps in a real box. Ignored by default.

use computer::{Computer, Launch};
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

        // The failure this whole design exists to prevent: a splash screen or
        // an empty frame answering as if it were the app.
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
