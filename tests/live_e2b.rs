//! The same box, in an E2B sandbox. Ignored by default.
//!
//! ```text
//! export E2B_API_KEY=...
//! export COMPUTER_E2B_TEMPLATE=<template id>
//! cargo test --features e2b --test live_e2b -- --ignored --nocapture
//! ```
//!
//! Needs a template, because E2B builds those and this crate builds container
//! images. Its builder is a Docker subset that `images/desktop/Dockerfile`
//! does not clear unchanged, so derive a context first:
//!
//! ```text
//! python3 images/context.py images/desktop /tmp/e2b-ctx --for e2b
//! e2b template create computer-desktop -p /tmp/e2b-ctx -d Dockerfile \
//!     -c "/usr/local/bin/computer-desktop" --ready-cmd "true" \
//!     --cpu-count 2 --memory-mb 2048
//! ```
//!
//! Everything after the launch is the code the container test runs, and that
//! is the point: `Machine` is the only part that knows where the box is.

#![cfg(feature = "e2b")]

use computer::sandboxes::e2b::{self, E2bApi, cloud::Cloud};
use computer::{Auth, Button, Computer, Delta, Point, ScreenId, Selection, X11Profile};
use std::sync::Arc;
use std::time::Duration;

const TEMPLATE_ENV: &str = "COMPUTER_E2B_TEMPLATE";

#[tokio::test]
#[ignore = "needs an E2B account and a built template"]
async fn a_real_sandbox_runs_the_same_desktop() {
    let Ok(template) = std::env::var(TEMPLATE_ENV) else {
        eprintln!("{TEMPLATE_ENV} is not set; nothing to test against");
        return;
    };

    let Ok(cloud) = Cloud::from_env() else {
        eprintln!("E2B_API_KEY is not set; nothing to test against");
        return;
    };

    if let Err(error) = cloud.available().await {
        eprintln!("e2b is not reachable: {error}");
        return;
    }

    let (machine, profile) = e2b::pair(Arc::new(cloud), Arc::new(X11Profile));

    let computer = Computer::builder()
        // A public viewer is reachable from the internet, so it goes through
        // the same gate as any other publish. `Token` because the URL printed
        // below has to carry everything a person needs to open it.
        .auth(Auth::Token)
        .machine(Arc::new(machine.public_viewer(true)))
        .profile(profile)
        .image(&template)
        .launch()
        .await
        .expect("a sandbox");

    println!("  {} on {}", computer.name(), computer.runtime());
    if let Some(url) = computer.viewer_url() {
        println!("  watch it {url}");
    }

    let outcome = exercise(&computer).await;

    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn exercise(computer: &Computer) -> computer::Result<()> {
    assert_eq!(computer.runtime(), "e2b");
    assert!(
        computer.probe().await.ready(),
        "launch waited for the screen and the browser"
    );

    let screen = computer.primary();
    assert_eq!(screen.geometry().await?, (1280, 800));

    let frame = screen.screenshot().await?;
    assert_eq!(
        frame.first_chunk::<4>(),
        Some(&[0x89, b'P', b'N', b'G']),
        "the capture is not a PNG"
    );
    println!("  screenshot: {} bytes", frame.len());

    screen.set_wallpaper(&frame).await?;
    println!("  wallpaper changed from uploaded image bytes");

    // A real pointer, over the internet.
    screen.click(Point::new(640, 400), Button::Left).await?;
    assert_eq!(screen.cursor().await?, Point::new(640, 400));
    screen.scroll(Point::new(640, 400), Delta::down(2)).await?;
    println!("  the pointer moved and read back");

    screen.set_clipboard("driven from rust").await?;
    assert_eq!(screen.clipboard().await?, "driven from rust");
    assert_eq!(
        screen.selection(Selection::Primary).await?,
        "",
        "the two selections are not one selection"
    );
    println!("  the clipboard round-tripped");

    // Files, which go through envd rather than a container runtime.
    let bytes = b"a file that crossed the internet\n";
    computer.write_file("/tmp/in.txt", bytes).await?;
    let read = computer.exec(["cat", "/tmp/in.txt"]).await?;
    assert_eq!(read.stdout, bytes, "what went in is what came out");
    assert_eq!(computer.read_file("/tmp/in.txt").await?, bytes);

    // A directory nothing made yet. The container runtimes create it, so envd
    // has to as well or the same call answers differently per machine.
    computer.write_file("/tmp/made/here/in.txt", bytes).await?;
    assert_eq!(computer.read_file("/tmp/made/here/in.txt").await?, bytes);

    let local = std::env::temp_dir().join("computer-e2b-upload");
    tokio::fs::write(&local, bytes)
        .await
        .expect("a file to send");
    computer.upload(&local, "/tmp/sent/up.txt").await?;
    assert_eq!(computer.read_file("/tmp/sent/up.txt").await?, bytes);

    let back = std::env::temp_dir().join("computer-e2b-download");
    let _ = tokio::fs::remove_file(&back).await;
    computer.download("/tmp/sent/up.txt", &back).await?;
    assert_eq!(
        tokio::fs::read(&back).await.expect("what came back"),
        bytes,
        "upload and download have to agree with write_file and read_file"
    );
    println!("  files went over and came back, directories and all");

    // Withdrawn rather than broken: the browser is up in the box, and nothing
    // out here can reach its debugger.
    assert!(
        computer.devtools().is_none(),
        "an endpoint out here would be wss, and cdp.rs speaks plain TCP"
    );

    let second = computer.screen(ScreenId(1)).await?;
    assert_eq!(second.display(), ":2");
    second.move_to(Point::new(20, 30)).await?;
    assert_eq!(second.cursor().await?, Point::new(20, 30));
    assert_eq!(computer.primary().cursor().await?, Point::new(640, 400));
    computer.close_screen(ScreenId(1)).await?;
    println!("  screen 1 is its own display");

    // The same audit the container test ends with. `browser` is not in it,
    // because the profile stopped claiming it rather than claiming it and
    // failing.
    let audit = computer::audit::audit_strictly(computer, Duration::from_secs(60)).await?;
    println!("  audit: {audit}");
    assert!(
        !audit.met.contains(&"browser"),
        "the CDP claim was withdrawn, so nothing should have checked it"
    );

    Ok(())
}
