//! ```text
//! export E2B_API_KEY=...
//! cargo run --features e2b --example e2b_takeover -- <template-id> [query]
//! ```

use computer::sandboxes::e2b::{self, cloud::Cloud};
use computer::{Button, Computer, Point, X11Profile};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> computer::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(template) = args.first().cloned() else {
        eprintln!("usage: e2b_takeover <template-id> [query]");
        std::process::exit(2);
    };
    let query = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "driving a linux desktop from rust".to_string());

    let (machine, profile) = e2b::pair(Arc::new(Cloud::from_env()?), Arc::new(X11Profile));

    println!("starting a sandbox …");
    let computer = Computer::builder()
        .machine(Arc::new(
            machine
                .public_viewer(true)
                .expiring_after(Duration::from_secs(30 * 60)),
        ))
        .profile(profile)
        .image(&template)
        .keep_on_drop(true)
        .launch()
        .await?;

    println!("  {} on {}", computer.name(), computer.runtime());

    computer.open_url("https://www.google.com").await?;
    tokio::time::sleep(Duration::from_secs(6)).await;
    save(&computer, "google-1-loaded.png").await?;

    // The search field takes focus on load, so no click is needed.
    computer.type_text(&query).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    save(&computer, "google-2-typed.png").await?;

    computer.press("Return").await?;
    tokio::time::sleep(Duration::from_secs(5)).await;
    save(&computer, "google-3-results.png").await?;
    println!("  searched for {query:?}");

    let takeover = computer.hand_over().await?;
    println!(
        "\n  drive it  {}",
        takeover.url().unwrap_or("<no control url>")
    );
    println!("  watch it  {}", computer.viewer_url().unwrap_or_default());

    let refused = computer.click(Point::new(10, 10), Button::Left).await;
    println!("\n  our own input is now refused: {}", refused.is_err());

    println!("  the box stays up until its deadline runs out");
    Ok(())
}

async fn save(computer: &Computer, name: &str) -> computer::Result<()> {
    let frame = computer.screenshot().await?;
    std::fs::write(name, &frame).ok();
    println!("  {name}: {} bytes", frame.len());
    Ok(())
}
