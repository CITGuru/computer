//! Packages the base image leaves out: `cargo test --test live_extras -- --ignored`.

use computer::bundle::Extras;
use computer::{Computer, ScreenId};
use std::time::{Duration, SystemTime};

#[tokio::test]
#[ignore = "builds a second image"]
async fn a_box_can_be_given_fonts_a_sound_card_and_a_recorder() {
    let computer = Computer::builder()
        .packages(Extras::everything().packages)
        .keep_on_drop(false)
        .launch()
        .await
        .expect("a box with the extras in it");

    println!("  {} is up", computer.name());
    let outcome = exercise(&computer).await;

    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn exercise(computer: &Computer) -> computer::Result<()> {
    let installed = computer
        .exec(["fc-list", ":lang=ja", "family"])
        .await?
        .stdout_utf8();
    assert!(
        !installed.trim().is_empty(),
        "no font can draw Japanese, so a page in it would be empty boxes"
    );

    let emoji = computer
        .exec(["fc-list", ":family=Noto Color Emoji"])
        .await?;
    assert!(emoji.ok() && !emoji.stdout_utf8().trim().is_empty());
    println!("  fonts: Japanese and emoji can be drawn");

    let socket = format!("PULSE_SERVER=unix:{}", computer.primary().audio_socket());
    let sinks = computer
        .exec(["env", &socket, "pactl", "list", "short", "sinks"])
        .await?;
    assert!(
        sinks.ok() && sinks.stdout_utf8().contains("screen1"),
        "screen 0 has no sink: {}",
        sinks.stderr_utf8()
    );
    println!("  audio: screen 0 has a sink");

    let started = SystemTime::now();
    computer
        .record(Duration::from_secs(3), "/tmp/computer/screen.mp4")
        .await?;

    let elapsed = started.elapsed().unwrap_or_default();
    assert!(
        elapsed >= Duration::from_secs(3),
        "the recording returned before it could have recorded anything"
    );

    let video = computer.read_file("/tmp/computer/screen.mp4").await?;
    assert!(video.len() > 1_000, "the film is {} bytes", video.len());
    println!("  video: {} bytes for {elapsed:?} of screen", video.len());

    let second = computer.screen(ScreenId(1)).await?;
    second
        .record(Duration::from_secs(2), "/tmp/computer/second.mp4")
        .await?;

    let other = computer.read_file("/tmp/computer/second.mp4").await?;
    assert!(other.len() > 1_000);
    computer.close_screen(ScreenId(1)).await?;
    println!("  video: screen 1 records itself");

    Ok(())
}

#[tokio::test]
#[ignore = "builds a second image"]
async fn a_native_window_can_be_driven_by_the_names_of_its_widgets() {
    let mut packages = Extras::accessibility().packages;
    // zenity is the test's own: the base image ships no GTK program but the browser.
    packages.push("zenity".to_string());

    let computer = Computer::builder()
        .packages(packages)
        .keep_on_drop(false)
        .launch()
        .await
        .expect("a box with the accessibility packages in it");

    println!("  {} is up", computer.name());
    let outcome = widgets(&computer).await;

    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

/// Through `exec`, because a launched app's output goes nowhere.
const FORM: &str = "setsid zenity --forms --title=Order --text=Delivery \
                    --add-entry=Street --add-entry=City \
                    >/tmp/zenity.out 2>&1 </dev/null &";

async fn widgets(computer: &Computer) -> computer::Result<()> {
    let screen = computer.primary();
    computer.exec_on(ScreenId(0), ["bash", "-lc", FORM]).await?;
    screen
        .wait_until_still(Duration::from_millis(400), Duration::from_secs(20))
        .await?;

    // The field's own name is empty; "Street" is a separate label beside it.
    let street = computer::NodeQuery {
        query: "Street".to_string(),
        ..computer::NodeQuery::default()
    };
    let found = screen.find_nodes(&street, None).await?;

    let first = found.first().expect("Street matched something");
    println!(
        "  found {} {:?} {:?}",
        first.role, first.name, first.labelled
    );
    assert_eq!(
        first.labelled.as_deref(),
        Some("Street"),
        "the field it labels has to beat the label itself, or a form cannot be \
         filled by the words on it: {found:#?}"
    );
    assert!(
        first.actions.iter().any(|action| action == "activate"),
        "and it has to be the editable one: {first:#?}"
    );
    assert!(
        first.at.is_some(),
        "with a rectangle, so a caller that wants a real click still can"
    );

    screen.set_node(&street, "12 Bishop Street").await?;
    let city = computer::NodeQuery {
        query: "City".to_string(),
        ..computer::NodeQuery::default()
    };
    let filled = screen.set_node(&city, "Lagos").await?;
    assert_eq!(filled.value.as_deref(), Some("Lagos"));

    let ok = computer::NodeQuery {
        query: "OK".to_string(),
        role: Some("push button".to_string()),
        ..computer::NodeQuery::default()
    };
    let pressed = screen.invoke_node(&ok, None).await?;
    assert_eq!(pressed.actions, vec!["click".to_string()]);

    for _ in 0..20 {
        let said = computer
            .read_file("/tmp/zenity.out")
            .await
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
            .unwrap_or_default();

        if !said.is_empty() {
            println!("  zenity returned {said:?}");
            assert!(
                said.contains("12 Bishop Street") && said.contains("Lagos"),
                "the form returned {said:?}, so a value reached the wrong field"
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    panic!("the form never returned: OK was pressed and nothing came of it");
}
