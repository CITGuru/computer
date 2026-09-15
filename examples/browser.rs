//! ```text
//! cargo run --example browser                # a new box
//! cargo run --example browser -- <box-name>  # one that is already up
//! ```

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

    let links = page
        .evaluate("Array.from(document.links).map(a => a.href)")
        .await?;
    println!("  links: {links}");

    let shot = page.screenshot().await?;
    std::fs::write("page.png", &shot).ok();
    println!("  captured {} bytes → page.png", shot.len());

    page.navigate("https://example.org").await?;
    page.wait_for_load(Duration::from_secs(20)).await?;
    println!("  navigated to {}", page.url().await?);

    page.evaluate("localStorage.setItem('scope', 'default')")
        .await?;
    let group = browser.create_group().await?;
    let mut isolated = group
        .open_page("https://example.org", Duration::from_secs(20))
        .await?;
    println!(
        "  group {} sees default storage: {}",
        group.id(),
        isolated.evaluate("localStorage.getItem('scope')").await?
    );
    drop(isolated);
    group.close().await?;

    let id = page.target().id.clone();
    browser.close(&id).await?;
    println!("  closed the tab");

    Ok(())
}
