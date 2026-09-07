//! A logged-in state carried from one box to another. Ignored by default.
//!
//! ```text
//! cargo test --test live_session -- --ignored --nocapture
//! ```

use computer::{Carry, Computer, Reading, Session};
use std::time::Duration;

/// The whole promise: log in, take the box away, and be logged in again.
///
/// A local page stands in for a real site because the point is the carrying,
/// not the logging in — and a test that depended on somebody's real login
/// would be a test that fails when their session expires.
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

    // The box, and everything in it, is gone.
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

    // What a login leaves behind: a cookie the server set, and a token the
    // page kept for itself.
    page.evaluate("document.cookie = 'sid=who-i-am; path=/; max-age=86400'")
        .await?;
    page.evaluate("localStorage.setItem('token', 'also-who-i-am')")
        .await?;
    page.evaluate("sessionStorage.setItem('tab', 'this-tab-only')")
        .await?;

    // And a database, which is where Firebase would have put it.
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

    // Left open on purpose: session storage belongs to this tab, and an export
    // that opened its own would find none of it.
    let taken = browser
        .export_session(&[ORIGIN.to_string()], Carry::all())
        .await;

    page.close().await.ok();
    taken
}

async fn restored(computer: &Computer, session: &Session) -> computer::Result<()> {
    serve(computer).await?;
    let browser = computer.browser().expect("a published DevTools port");

    // Before: a box that has never seen the site.
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

    // The tabs it left open, because session storage only exists in one.
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

/// Something to be logged in to. A cookie needs a real origin; `file://` has
/// none, and nothing is stored against it.
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
