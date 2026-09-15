//! A login carried between boxes: `cargo test --test live_session -- --ignored`.

use computer::{Carry, Computer, Reading, Session};
use std::time::Duration;

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn a_session_outlives_the_box_it_was_made_in() {
    let first = Computer::launch().await.expect("a box");
    let taken = sign_in_and_export(&first).await.expect("a session");

    println!(
        "  exported {} cookie(s), local {}, session {}, {} database(s)",
        taken.cookies.len(),
        taken.storage.len(),
        taken.session_storage.len(),
        taken.databases.values().map(Vec::len).sum::<usize>(),
    );
    for why in &taken.incomplete {
        println!("  incomplete: {why}");
    }

    assert!(!taken.cookies.is_empty(), "the login set no cookie");
    assert!(
        !taken.session_storage.is_empty(),
        "session storage was asked for"
    );
    assert!(!taken.databases.is_empty(), "the database was asked for");

    first.shutdown().await.expect("it goes away");

    let second = Computer::launch().await.expect("another box");
    let outcome = restored(&second, &taken).await;
    second.shutdown().await.expect("it goes away");
    outcome.expect("the session came back");
}

const ORIGIN: &str = "http://127.0.0.1:8000";

async fn sign_in_and_export(computer: &Computer) -> computer::Result<Session> {
    serve(computer).await?;
    let browser = computer.browser().expect("a published DevTools port");

    let mut page = browser.open_page(ORIGIN, Duration::from_secs(30)).await?;

    page.evaluate("document.cookie = 'sid=who-i-am; path=/; max-age=86400'")
        .await?;
    page.evaluate("localStorage.setItem('token', 'also-who-i-am')")
        .await?;
    page.evaluate("sessionStorage.setItem('tab', 'this-tab-only')")
        .await?;

    page.evaluate(
        r#"new Promise((ok, no) => {
             const r = indexedDB.open('auth', 1);
             r.onupgradeneeded = () => r.result.createObjectStore('users', { keyPath: 'id' });
             r.onsuccess = () => {
               const tx = r.result.transaction('users', 'readwrite');
               tx.objectStore('users').put({ id: 'me', refresh: 'a-refresh-token' });
               tx.oncomplete = () => { r.result.close(); ok('ok'); };
             };
             r.onerror = () => no(r.error);
           })"#,
    )
    .await?;

    // Left open: session storage belongs to this tab.
    let taken = browser
        .export_session(&[ORIGIN.to_string()], Carry::all())
        .await;

    page.close().await.ok();
    taken
}

async fn restored(computer: &Computer, session: &Session) -> computer::Result<()> {
    serve(computer).await?;
    let browser = computer.browser().expect("a published DevTools port");

    let mut fresh = browser.open_page(ORIGIN, Duration::from_secs(30)).await?;
    let before = fresh
        .evaluate("document.cookie + '|' + (localStorage.getItem('token') || '')")
        .await?;
    assert_eq!(
        before.as_str().unwrap_or_default(),
        "|",
        "a new box already knew something"
    );
    fresh.close().await.ok();

    let mut held = browser.import_session(session).await?;
    let mut page = match held.pop() {
        Some(page) => page,
        None => browser.open_page(ORIGIN, Duration::from_secs(30)).await?,
    };
    let cookie = page.evaluate("document.cookie").await?;
    let token = page.evaluate("localStorage.getItem('token')").await?;

    println!("  cookie back: {:?}", cookie.as_str().unwrap_or_default());
    println!("  token back:  {:?}", token.as_str().unwrap_or_default());

    assert!(
        cookie.as_str().unwrap_or_default().contains("who-i-am"),
        "the cookie did not come back"
    );
    assert_eq!(
        token.as_str(),
        Some("also-who-i-am"),
        "storage did not come back"
    );

    let tab = page.evaluate("sessionStorage.getItem('tab')").await?;
    println!("  session:     {:?}", tab.as_str().unwrap_or_default());
    assert_eq!(
        tab.as_str(),
        Some("this-tab-only"),
        "session storage did not come back"
    );

    let refresh = page
        .evaluate(
            r#"new Promise(ok => {
                 const r = indexedDB.open('auth');
                 r.onsuccess = () => {
                   const tx = r.result.transaction('users', 'readonly');
                   const g = tx.objectStore('users').get('me');
                   g.onsuccess = () => { r.result.close(); ok(g.result ? g.result.refresh : ''); };
                   g.onerror = () => { r.result.close(); ok(''); };
                 };
                 r.onerror = () => ok('');
               })"#,
        )
        .await?;
    println!("  database:    {:?}", refresh.as_str().unwrap_or_default());
    assert_eq!(
        refresh.as_str(),
        Some("a-refresh-token"),
        "the database did not come back"
    );

    let read = page.read(Reading::Text, Some(80), None).await?;
    println!("  page says:   {:?}", read.text.trim());

    page.close().await.ok();
    Ok(())
}

/// A cookie needs a real origin, and `file://` has none.
async fn serve(computer: &Computer) -> computer::Result<()> {
    computer
        .write_file(
            "/tmp/site/index.html",
            b"<title>Site</title><h1>You are in</h1>",
        )
        .await?;
    computer
        .exec([
            "sh",
            "-c",
            "cd /tmp/site && setsid python3 -m http.server 8000 >/dev/null 2>&1 & sleep 1",
        ])
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn a_volume_keeps_the_browser_between_boxes() {
    let volume = format!("computer-test-{}", std::process::id());

    let first = Computer::builder()
        .profiles(&volume)
        .launch()
        .await
        .expect("a box with a volume");
    let outcome = sign_in(&first).await;
    first.shutdown().await.expect("it goes away");
    outcome.expect("signing in");

    let second = Computer::builder()
        .profiles(&volume)
        .launch()
        .await
        .expect("another box on the same volume");
    let outcome = still_signed_in(&second).await;
    second.shutdown().await.expect("it goes away");

    // A volume outlives the box, so nothing else removes it.
    tokio::process::Command::new("docker")
        .args(["volume", "rm", "--force", &volume])
        .output()
        .await
        .ok();

    outcome.expect("it was still signed in");
}

async fn sign_in(computer: &Computer) -> computer::Result<()> {
    serve(computer).await?;
    let browser = computer.browser().expect("a published DevTools port");

    let mut page = browser.open_page(ORIGIN, Duration::from_secs(30)).await?;
    page.evaluate("document.cookie = 'sid=kept-on-the-host; path=/; max-age=86400'")
        .await?;
    page.evaluate("localStorage.setItem('token', 'also-kept')")
        .await?;

    page.close().await.ok();

    // Chromium flushes cookies on its own schedule; closing it makes it write.
    computer
        .exec_within(
            ["sh", "-c", "pkill -TERM chromium; sleep 20"],
            Duration::from_secs(40),
        )
        .await?;

    Ok(())
}

async fn still_signed_in(computer: &Computer) -> computer::Result<()> {
    serve(computer).await?;
    let browser = computer.browser().expect("a published DevTools port");

    let mut page = browser.open_page(ORIGIN, Duration::from_secs(30)).await?;
    let cookie = page.evaluate("document.cookie").await?;
    let token = page.evaluate("localStorage.getItem('token')").await?;

    println!(
        "  cookie in the new box: {:?}",
        cookie.as_str().unwrap_or_default()
    );
    println!(
        "  token in the new box:  {:?}",
        token.as_str().unwrap_or_default()
    );

    // Local storage, because cookies may not reach the volume before the kill.
    assert_eq!(
        token.as_str(),
        Some("also-kept"),
        "the volume did not keep the profile"
    );

    page.close().await.ok();
    Ok(())
}
