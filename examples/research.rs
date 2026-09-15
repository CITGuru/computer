//! cargo run --example research -- <box> "a subject"

use computer::{Computer, Reading, SearchProvider};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let name = args.next().ok_or("usage: research <box> <subject>")?;
    let subject = args.next().ok_or("usage: research <box> <subject>")?;

    let computer = Computer::attach(&name).await?;
    let browser = computer
        .browser()
        .ok_or("this box publishes no DevTools port")?;

    let mut page = browser
        .search(
            &format!("\"{subject}\""),
            SearchProvider::default(),
            Duration::from_secs(30),
        )
        .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let links = page
        .evaluate(
            r#"Array.from(document.querySelectorAll('.result__a'))
                 .map(a => a.href)
                 .filter(h => h && !h.includes('duckduckgo.com/y.js'))
                 .slice(0, 10)
                 .join('\n')"#,
        )
        .await?;

    let links: Vec<String> = links
        .as_str()
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();

    println!("{} results to open\n", links.len());
    page.close().await.ok();

    for link in links {
        println!("=== {link}");

        let opened = browser.open_page(&link, Duration::from_secs(40)).await;
        let Ok(mut page) = opened else {
            println!("  did not load\n");
            continue;
        };
        // Redirect URLs: `open_page` waited for the redirector, not the page it lands on.
        page.wait_for("body", false, Duration::from_secs(10))
            .await
            .ok();
        tokio::time::sleep(Duration::from_millis(600)).await;

        let read = match page.read(Reading::Markdown, Some(1400), None).await {
            Ok(read) => read,
            Err(error) => {
                println!("  unreadable: {error}\n");
                continue;
            }
        };

        println!("  title: {}", read.title);
        match read.text.trim().is_empty() {
            true => println!("  body:  (nothing readable)\n"),
            false => println!("  body:  {}\n", read.text.trim()),
        }

        page.close().await.ok();
    }

    Ok(())
}
