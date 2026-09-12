//! Find things on a page and act on them by name.
//!
//! ```bash
//! cargo run --example elements -- <box>
//! ```

use computer::{Button, Computer, Reading};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::args().nth(1).ok_or("usage: elements <box>")?;
    let computer = Computer::attach(&name).await?;
    let browser = computer.browser().ok_or("no DevTools port")?;

    let mut page = browser
        .open_page("https://httpbin.org/forms/post", Duration::from_secs(30))
        .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    println!("=== find ===");
    for element in page.find("input", Some(6), None, None).await? {
        println!(
            "  {:<9} {:<10} {:<14} {}x{}  enabled={}",
            element.tag,
            element.kind.unwrap_or_default(),
            match element.at {
                Some(at) => format!("at {},{}", at.x, at.y),
                None => "out of view".to_string(),
            },
            element.width,
            element.height,
            element.enabled
        );
    }

    println!("\n=== fill, by the words next to it ===");
    page.fill("custname", "Toby").await?;
    let filled = page.find("custname", Some(1), None, None).await?;
    println!(
        "  custname now {:?}",
        filled.first().and_then(|e| e.value.clone())
    );

    println!("\n=== a dropdown, listed then chosen ===");
    let mut menu = browser
        .open_page("file:///tmp/form.html", Duration::from_secs(30))
        .await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let options = menu.options("select").await?;
    println!("  options: {options:?}");
    if let Some(pick) = options.get(1) {
        menu.choose("select", pick).await?;
        let now = menu.find("select", Some(1), None, None).await?;
        println!(
            "  value now {:?}",
            now.first().and_then(|e| e.value.clone())
        );
        println!("  chose {pick:?}");
    }

    println!("\n=== a file input, which no click can fill ===");
    menu.upload("doc", &["/tmp/form.html".to_string()]).await?;
    let got = menu.find("doc", Some(1), None, None).await?;
    println!("  doc now {:?}", got.first().and_then(|e| e.value.clone()));

    println!("\n=== click by name ===");
    let clicked = page.click_on("Submit order", Button::Left).await?;
    println!("  clicked {:?} at {:?}", clicked.text, clicked.at);
    tokio::time::sleep(Duration::from_secs(3)).await;

    let read = page.read(Reading::Text, Some(300), None).await?;
    println!(
        "\n=== where it landed ===\n  {}\n  {}",
        read.url,
        read.text.trim()
    );

    page.close().await.ok();

    Ok(())
}
