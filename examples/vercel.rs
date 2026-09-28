use computer::sandboxes::{remote, vercel::cloud::Cloud};
use computer::{Auth, Button, Computer, Point, X11Profile};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> computer::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keep = args.iter().any(|arg| arg == "--keep");

    let Some(image) = args.iter().find(|arg| !arg.starts_with("--")).cloned() else {
        eprintln!("usage: vercel <image> [--keep]");
        std::process::exit(2);
    };

    let (machine, profile) = remote::pair(Arc::new(Cloud::from_env()?), Arc::new(X11Profile));

    println!("starting a sandbox …");
    let computer = Computer::builder()
        .machine(Arc::new(
            machine
                .public_viewer(true)
                .expiring_after(Duration::from_secs(15 * 60)),
        ))
        .profile(profile)
        .image(&image)
        .auth(Auth::Token)
        .keep_on_drop(keep)
        .launch()
        .await?;

    println!("  runtime  {}", computer.provider());
    match computer.viewer_url() {
        Some(url) => println!("  watch it {url}"),
        None => println!("  no viewer"),
    }

    computer.open_url("https://example.com").await?;
    tokio::time::sleep(Duration::from_secs(4)).await;

    let frame = computer.screenshot().await?;
    std::fs::write("vercel.png", &frame).ok();
    println!("  screenshot: {} bytes -> vercel.png", frame.len());

    match computer.browser() {
        Some(devtools) => {
            let started = std::time::Instant::now();
            let pages = devtools.pages().await?;
            let target = pages
                .iter()
                .find(|page| page.url.contains("example.com"))
                .or(pages.first())
                .ok_or_else(|| computer::Error::denied("the browser has no page"))?;
            let mut page = devtools.attach(target).await?;
            let snapshot = page.snapshot(None, Some(20)).await?;
            println!(
                "  devtools: {:?} with {} controls in {:?}",
                snapshot.title,
                snapshot.total,
                started.elapsed()
            );
        }
        None => println!("  devtools: none reaches this box"),
    }

    computer.click(Point::new(640, 400), Button::Left).await?;
    println!("  cursor: {:?}", computer.cursor().await?);

    let geometry = computer.primary().geometry().await?;
    println!("  geometry: {geometry:?}");

    if keep {
        println!("\n  left running as {}", computer.name());
        println!("  it goes away on its own when its deadline runs out");
        return Ok(());
    }

    computer.shutdown().await
}
