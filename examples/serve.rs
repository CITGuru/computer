//! ```text
//! cargo run --example serve
//! cargo run --example serve -- https://news.ycombinator.com
//! cargo run --example serve -- --wayland https://news.ycombinator.com
//! cargo run --example serve -- --dock https://news.ycombinator.com
//! cargo run --example serve -- --image-dir images/ubuntu https://news.ycombinator.com
//! ```

use computer::{Computer, Profile, WaylandProfile, X11Profile};
use std::sync::Arc;

#[tokio::main]
async fn main() -> computer::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let profile: Arc<dyn Profile> = match args.iter().any(|arg| arg == "--wayland") {
        true => Arc::new(WaylandProfile),
        false => Arc::new(X11Profile),
    };
    let image_dir = args
        .windows(2)
        .find(|pair| pair[0] == "--image-dir")
        .map(|pair| pair[1].as_str());
    let url = args.iter().enumerate().find_map(|(index, arg)| {
        let is_image_dir = index > 0 && args[index - 1] == "--image-dir";
        (!arg.starts_with("--") && !is_image_dir).then_some(arg)
    });

    println!("opening a box (the first run builds the image) …");
    let mut builder = Computer::builder().profile(profile).keep_on_drop(true);
    if args.iter().any(|arg| arg == "--dock") {
        builder = builder.dock();
    }
    if let Some(directory) = image_dir {
        builder = builder.image_dir(directory);
    }
    let computer = builder.launch().await?;

    if let Some(url) = url {
        computer.open_url(url).await?;
    }

    let display = computer.support().display.expect("a screen");
    println!("\n  box       {}", computer.name());
    println!(
        "  screen    {}x{} on {:?}",
        display.width, display.height, display.server
    );

    match computer.viewer_url() {
        Some(url) => println!("\n  watch it  {url}"),
        None => println!("\n  no viewer port was published"),
    }
    if let Some(endpoint) = computer.devtools() {
        println!("  devtools  {}", endpoint.http_url);
    }

    println!("\n  keyboard  cargo run --example takeover");
    println!("  stop it   docker rm -f {}", computer.name());

    Ok(())
}
