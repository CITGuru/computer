//! cargo run --example custom_image

use computer::bundle::{DESKTOP, Extras};
use computer::{Computer, ImageSource, ProfileBuilder, X11Profile};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

const TAG: &str = "acme-desktop:1";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("making sure the base image is here …");
    let base = Computer::launch().await?;
    let base_tag = DESKTOP.tag_with(&Extras::none());
    base.shutdown().await?;

    let context = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/images/acme");
    println!("building {TAG} on {base_tag} …");

    let built = Command::new("docker")
        .args(["build", "--build-arg"])
        .arg(format!("BASE={base_tag}"))
        .args(["--tag", TAG])
        .arg(&context)
        .status()?;

    if !built.success() {
        return Err("the image did not build".into());
    }

    let profile = ProfileBuilder::new(X11Profile)
        .image(ImageSource::Registry(TAG.to_string()))
        .build();

    let computer = Computer::builder()
        .profile(Arc::new(profile))
        .launch()
        .await?;

    println!("{} is up", computer.name());
    if let Some(url) = computer.viewer_url() {
        println!("  watch it {url}");
    }

    let carried = computer.exec(["cat", "/opt/acme/README"]).await?;
    println!("  /opt/acme/README: {}", carried.stdout_utf8().trim());

    let tools = computer
        .exec([
            "sh",
            "-c",
            "for t in htop jq; do printf '%s=%s ' \"$t\" \"$(command -v $t || echo missing)\"; done",
        ])
        .await?;
    println!("  tools: {}", tools.stdout_utf8().trim());

    let frame = computer.screenshot().await?;
    println!("  screenshot: {} bytes", frame.len());

    computer.shutdown().await?;
    println!("gone");
    Ok(())
}
