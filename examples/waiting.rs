//! Waiting, history and hover.

use computer::{Button, Computer};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::args().nth(1).ok_or("usage: waiting <box>")?;
    let computer = Computer::attach(&name).await?;
    let browser = computer.browser().ok_or("no DevTools port")?;

    let mut page = browser
        .open_page("file:///tmp/wait.html", Duration::from_secs(30))
        .await?;
    // A tab that is not in front has its timers throttled, which a wait would
    // otherwise be blamed for.
    page.bring_to_front().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    println!("\n=== wait_for: something that arrives 1.5s after a click ===");
    page.click_on("Start", Button::Left).await?;
    let at = Instant::now();
    let late = page
        .wait_for("late", false, Duration::from_secs(10))
        .await?;
    println!(
        "  {:?} after {}ms",
        late.map(|one| one.text).unwrap_or_default(),
        at.elapsed().as_millis()
    );

    println!("\n=== wait_for something that never comes ===");
    let at = Instant::now();
    match page
        .wait_for("nothing-like-this", false, Duration::from_secs(2))
        .await
    {
        Ok(_) => println!("  it found something, which is wrong"),
        Err(error) => println!("  gave up after {}ms: {error}", at.elapsed().as_millis()),
    }

    println!("\n=== hover: a menu that no click can open ===");
    let before = page
        .find("Hidden until hovered", Some(1), None, None)
        .await?;
    println!("  before hovering: {} matches", before.len());
    page.hover("Hover me").await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = page
        .find("Hidden until hovered", Some(1), None, None)
        .await?;
    println!("  after hovering:  {} matches", after.len());

    println!("\n=== history ===");
    page.navigate("file:///tmp/long.html").await?;
    page.wait_for_load(Duration::from_secs(10)).await?;
    println!("  went to      {}", page.url().await?);

    page.back().await?;
    page.wait_for("Hover me", false, Duration::from_secs(10))
        .await?;
    println!("  back to      {}", page.url().await?);

    page.forward().await?;
    page.wait_for("scroll down", false, Duration::from_secs(10))
        .await?;
    println!("  forward to   {}", page.url().await?);

    page.close().await.ok();

    Ok(())
}
