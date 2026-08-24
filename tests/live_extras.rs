//! The parts of a box that are not in the base image. Ignored by default.
//!
//! ```text
//! cargo test --test live_extras -- --ignored --nocapture
//! ```
//!
//! Everything here needs packages the base image deliberately leaves out, so
//! the first run builds a second image and takes as long as installing them.
//! That is the trade: a box that only ever visits English pages carries no
//! font it cannot read, no sound card it cannot hear, and no recorder.

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
    // The fonts. A page the base image cannot draw renders as empty boxes,
    // which a screenshot does not report as a fault.
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

    // The sound card. Nothing plays out of a box; the sink is what makes a
    // recording anything but silent.
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

    // The recorder. Three seconds of the real screen, captured in the box.
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

    // A second screen records its own display, not screen 0's.
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
