//! Launch a box from a spec file, with no server in between.
//!
//! ```text
//! cargo run --example from_spec -- examples/box.json
//! ```
//!
//! The same spec the REST API takes. `computer-server` adds a name, a label and
//! a lifetime that outlives the request; the desktop it describes is this.

use computer::{Builder, Placement, ScreenId, Spec};

#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
struct File {
    spec: Spec,
    placement: Placement,
}

#[tokio::main]
async fn main() -> computer::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: from_spec <spec.json>");
    let text = std::fs::read_to_string(&path).expect("a readable spec file");
    let file: File = serde_json::from_str(&text).expect("a spec this version understands");

    let resolved = computer::spec::resolve(&file.spec)?;
    println!(
        "{path}: {}x{}, {} screen(s), digest {}",
        resolved.width,
        resolved.height,
        resolved.screens,
        file.spec.digest()
    );

    let computer = Builder::from_spec(&file.spec)?
        .place(&file.placement)?
        .launch()
        .await?;

    println!("{} is up", computer.name());
    if let Some(url) = computer.screen(ScreenId(0)).await?.viewer_url() {
        println!("  viewer {url}");
    }

    let frame = computer.screenshot().await?;
    println!("  a screenshot came back at {} bytes", frame.len());

    Ok(())
}
