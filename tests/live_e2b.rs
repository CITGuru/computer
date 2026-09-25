//! The same box, in an E2B sandbox. Ignored by default.
//!
//! ```text
//! export E2B_API_KEY=...
//! cargo test --features e2b --test live_e2b -- --ignored --nocapture
//! ```
//!
//! Without `COMPUTER_E2B_TEMPLATE`, the template is built from the bundled desktop image.

#![cfg(feature = "e2b")]

use computer::sandboxes::e2b::{self, E2bApi, cloud::Cloud};
use computer::{
    Auth, BrowserEndpoint, Button, Computer, Delta, Devtools, Point, ScreenId, Selection,
    X11Profile,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEMPLATE_ENV: &str = "COMPUTER_E2B_TEMPLATE";

#[tokio::test]
#[ignore = "needs an E2B account and a built template"]
async fn a_real_sandbox_runs_the_same_desktop() {
    let template = std::env::var(TEMPLATE_ENV).ok();

    let Ok(cloud) = Cloud::from_env() else {
        eprintln!("E2B_API_KEY is not set; nothing to test against");
        return;
    };

    if let Err(error) = cloud.available().await {
        eprintln!("e2b is not reachable: {error}");
        return;
    }

    let (machine, profile) = e2b::pair(Arc::new(cloud), Arc::new(X11Profile));

    let mut builder = Computer::builder()
        .auth(Auth::Token)
        .machine(Arc::new(machine.public_viewer(true)))
        .profile(profile);
    if let Some(template) = &template {
        builder = builder.image(template);
    }
    let computer = builder.launch().await.expect("a sandbox");

    println!("  {} on {}", computer.name(), computer.provider());

    let gate = gates(&computer).await;
    let outcome = exercise(&computer).await;
    let removed = computer.shutdown().await;

    assert!(
        gate.is_ok() && outcome.is_ok(),
        "the gate: {gate:?}\nevery step: {outcome:?}\na sandbox left running would keep \
         whatever the gate failed to guard, so it was removed first"
    );
    removed.expect("it goes away");
}

async fn upgrade(url: &str, headers: &[(String, String)]) -> String {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let Ok(mut wire) = computer::cdp::dial(url).await else {
        return "no connection".to_string();
    };

    let mut request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Protocol: binary\r\n"
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");

    if wire.write_all(request.as_bytes()).await.is_err() || wire.flush().await.is_err() {
        return "no request".to_string();
    }
    let mut answer = vec![0u8; 256];
    let read = tokio::time::timeout(Duration::from_secs(20), wire.read(&mut answer))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(0);
    String::from_utf8_lossy(&answer[..read])
        .split_whitespace()
        .nth(1)
        .unwrap_or("no answer")
        .to_string()
}

async fn gates(computer: &Computer) -> Result<(), String> {
    if computer.viewer_url().is_some() {
        return Err("a browser cannot send the traffic token, so no link should be given".into());
    }

    let screen = computer.primary();
    let headers = screen.socket_headers();
    let socket = screen.viewer_socket().ok_or("no viewer socket")?;

    let opened = upgrade(&socket, &headers).await;
    println!("  the viewer with the traffic token: {opened}");
    if opened != "101" {
        return Err(format!("the viewer refused the right ticket: {opened}"));
    }

    let bare = upgrade(&socket, &[]).await;
    println!("  the viewer without the traffic token: {bare}");
    if bare != "403" {
        return Err(format!(
            "the sandbox let a request in without its token: {bare}"
        ));
    }

    let (base, _) = socket
        .split_once("?token=")
        .ok_or("no ticket in the socket")?;
    let wrong = upgrade(&format!("{base}?token=not-the-ticket"), &headers).await;
    println!("  a wrong ticket with the traffic token: {wrong}");
    if wrong == "101" {
        return Err("a wrong ticket opened the viewer".to_string());
    }

    let endpoint = computer.devtools().ok_or("no DevTools endpoint")?;
    let no_secret = Devtools::from_endpoint(&BrowserEndpoint {
        headers: endpoint
            .headers
            .iter()
            .filter(|(name, _)| name != "x-computer-devtools")
            .cloned()
            .collect(),
        ..endpoint
    })
    .map_err(|error| error.to_string())?;
    match no_secret.version().await {
        Ok(_) => Err("the DevTools bridge answered without its secret".to_string()),
        Err(error) => {
            println!("  DevTools without the bridge secret: {error}");
            Ok(())
        }
    }
}

async fn browser(computer: &Computer) -> computer::Result<()> {
    let browser = computer.browser().expect("a DevTools endpoint");

    let started = std::time::Instant::now();
    let mut page = browser
        .open_page("https://example.com", Duration::from_secs(30))
        .await?;
    assert_eq!(page.title().await?, "Example Domain");
    let snapshot = page.snapshot(None, Some(20)).await?;
    assert!(snapshot.total > 0, "example.com has a link to list");
    println!(
        "  the browser drove itself over wss in {:?}",
        started.elapsed()
    );

    page.bring_to_front().await?;
    assert!(page.visible().await?);

    computer.open_url("https://example.net").await?;
    tokio::time::sleep(Duration::from_secs(4)).await;

    let showing = browser.visible_page().await?;
    let mut showing = showing.expect("something is on the screen");
    assert!(
        showing.url().await?.contains("example.net"),
        "open_url joined the browser DevTools reaches, not a second one"
    );
    assert!(!page.visible().await?);
    println!("  open_url and DevTools see the same browser");

    let id = page.target().id.clone();
    browser.close(&id).await?;
    Ok(())
}

async fn exercise(computer: &Computer) -> computer::Result<()> {
    assert_eq!(computer.provider(), "e2b");
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

    let bytes = b"a file that crossed the internet\n";
    computer.write_file("/tmp/in.txt", bytes).await?;
    let read = computer.exec(["cat", "/tmp/in.txt"]).await?;
    assert_eq!(read.stdout, bytes, "what went in is what came out");
    assert_eq!(computer.read_file("/tmp/in.txt").await?, bytes);

    // Container runtimes create missing parents, so envd has to as well.
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

    browser(computer).await?;

    let second = computer.screen(ScreenId(1)).await?;
    assert_eq!(second.display(), ":2");
    second.move_to(Point::new(20, 30)).await?;
    assert_eq!(second.cursor().await?, Point::new(20, 30));
    assert_eq!(computer.primary().cursor().await?, Point::new(640, 400));
    computer.close_screen(ScreenId(1)).await?;
    println!("  screen 1 is its own display");

    let audit = computer::audit::audit_strictly(computer, Duration::from_secs(60)).await?;
    println!("  audit: {audit}");
    assert!(
        audit.met.contains(&"browser"),
        "DevTools reaches the box, so the audit checks it"
    );

    Ok(())
}
