//! cargo run --example microvm

use computer::SystemEngine;
use computer::microvm::import_image;
use computer::sandboxes::microsandbox::msb;
use computer::{Button, Computer, Engine, Point, bundle};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> computer::Result<()> {
    let hypervisor = msb::Msb::found();
    let docker: Arc<dyn Engine> = Arc::new(SystemEngine::default());
    let tag = bundle::DESKTOP.tag();

    // Once per image: about a gigabyte moves through the disk.
    if !computer::microvm::MicroVmApi::has_image(&hypervisor, &tag).await? {
        println!("building the image and handing it to the hypervisor …");
        bundle::ensure(docker.as_ref(), &tag).await?;
        import_image(docker.as_ref(), &hypervisor, &tag).await?;
    }

    println!("booting a microVM …");
    let computer = Computer::builder()
        .machine(Arc::new(msb::machine()))
        .image(&tag)
        .memory("2g")
        .cpus("2")
        .launch()
        .await?;

    println!("  runtime  {}", computer.runtime());
    if let Some(url) = computer.viewer_url() {
        println!("  watch it {url}");
    }

    computer.open_url("https://example.com").await?;
    tokio::time::sleep(Duration::from_secs(4)).await;

    let frame = computer.screenshot().await?;
    std::fs::write("microvm.png", &frame).ok();
    println!("  screenshot: {} bytes -> microvm.png", frame.len());

    computer.click(Point::new(640, 400), Button::Left).await?;
    println!("  cursor: {:?}", computer.cursor().await?);

    let geometry = computer.primary().geometry().await?;
    println!("  geometry: {geometry:?}");

    computer.shutdown().await
}
