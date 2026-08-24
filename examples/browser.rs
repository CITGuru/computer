//! Driving the browser through DevTools instead of pointing at it.
//!
//! ```text
//! cargo run --example browser                # a new box
//! cargo run --example browser -- <box-name>  # one that is already up
//! ```
//!
//! Nothing here touches the screen. `Page.navigate` goes to a URL whether or
//! not the address bar is where the last screenshot showed it, and
//! `Runtime.evaluate` answers questions about the page that no screenshot can
//! answer. Both work on a box with no display at all.

use computer::Computer;
use std::time::Duration;

#[tokio::main]
async fn main() -> computer::Result<()> {
    let computer = match std::env::args().nth(1) {
        Some(name) => Computer::attach(name).await?,
        None => Computer::builder().keep_on_drop(true).launch().await?,
    };
    println!("{} is up", computer.name());

    let browser = computer
        .browser()
        .expect("the box published its DevTools port");
    let version = browser.version().await?;
    println!(
        "  {}",
        version
            .get("Browser")
            .and_then(|browser| browser.as_str())
            .unwrap_or("unknown")
    );

    let mut page = browser
        .open_page("https://example.com", Duration::from_secs(20))
        .await?;
    println!("  opened {}", page.target().id);

    println!("  title: {}", page.title().await?);
    println!("  url:   {}", page.url().await?);

    // A question no screenshot can answer.
    let links = page
        .evaluate("Array.from(document.links).map(a => a.href)")
        .await?;
    println!("  links: {links}");

    // The page as the browser renders it: no window frame, no address bar.
    let shot = page.screenshot().await?;
    std::fs::write("page.png", &shot).ok();
    println!("  captured {} bytes → page.png", shot.len());

    page.navigate("https://example.org").await?;
    page.wait_for_load(Duration::from_secs(20)).await?;
    println!("  navigated to {}", page.url().await?);

    let id = page.target().id.clone();
    browser.close(&id).await?;
    println!("  closed the tab");

    Ok(())
}
