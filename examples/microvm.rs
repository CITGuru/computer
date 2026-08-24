//! The same desktop, in a microVM instead of a container.
//!
//! ```text
//! cargo run --example microvm
//! ```
//!
//! Two things differ, and neither is the driving:
//!
//! 1. The image has to be handed over. A container runtime keeps its
//!    images in a store only it can read, so the image this crate builds is
//!    saved once and imported into the hypervisor's own store. An OCI
//!    reference the hypervisor can pull works too; pass it instead.
//! 2. Ports are chosen on this side. A hypervisor forwards the pairs it is
//!    given, so free host ports are picked before the machine is created.
//!
//! Everything after that is the same code as the container path, because
//! `Machine` is the only thing that knows where the box is.

use computer::microvm::import_image;
use computer::runtime::SystemDocker;
use computer::sandboxes::microsandbox::msb;
use computer::{Button, Computer, ContainerCli, Point, bundle};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> computer::Result<()> {
    let hypervisor = msb::Msb::found();
    let docker: Arc<dyn ContainerCli> = Arc::new(SystemDocker::default());
    let tag = bundle::DESKTOP.tag();

    // Once per image, not once per box: about a gigabyte moves through the
    // disk on the way over.
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
