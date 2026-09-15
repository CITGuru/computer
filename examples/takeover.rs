//! ```text
//! cargo run --example takeover                    # a new box, yours for 60 seconds
//! cargo run --example takeover -- <box-name>      # a box that is already up
//! cargo run --example takeover -- <box-name> hold # leave control open and exit
//! cargo run --example takeover -- <box-name> release # take a held screen back
//! ```

use computer::{Button, Computer, Point};
use std::time::Duration;

#[tokio::main]
async fn main() -> computer::Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next();
    let mode = args.next();
    let hold = mode.as_deref() == Some("hold");
    let release = mode.as_deref() == Some("release");

    let computer = match &name {
        Some(name) => Computer::attach(name).await?,
        None => {
            let computer = Computer::builder().keep_on_drop(true).launch().await?;
            computer.open_url("https://example.com").await?;
            tokio::time::sleep(Duration::from_secs(3)).await;
            computer
        }
    };
    println!("{} is up", computer.name());

    if release {
        // The gate that opened it went with its program, and the server did not.
        let before = computer.viewers().await?;
        println!("  before: {before:?}");

        computer.reclaim().await?;
        computer
            .click(Point::new(640, 400), Button::Left)
            .await
            .expect("the owner drives again");

        println!("  reclaimed: {:?}", computer.viewers().await?);
        return Ok(());
    }

    if let Some(url) = computer.viewer_url() {
        println!("  watching, read-only:       {url}");
    }

    let takeover = computer.hand_over().await?;
    match takeover.url() {
        Some(url) => println!("  driving, take the keyboard: {url}"),
        None => println!("  the control viewer is up inside the box"),
    }

    let frame = computer.screenshot().await?;
    println!("\n  still watching: {} bytes", frame.len());

    match computer.click(Point::new(10, 10), Button::Left).await {
        Err(error) => println!("  input refused, as it should be: {error}"),
        Ok(()) => println!("  the gate let a click through, which is a bug"),
    }

    if hold {
        // The owner's gate goes with this process; the person's server stays in the box.
        println!("\n  the screen is yours until you release it:");
        println!(
            "    docker exec {} computer-screen release 0",
            computer.name()
        );
        return Ok(());
    }

    println!("\n  the screen is yours for 60 seconds …");
    tokio::time::sleep(Duration::from_secs(60)).await;

    takeover.end().await?;
    println!("  taken back");

    computer.click(Point::new(640, 400), Button::Left).await?;
    println!("  the owner is driving again");

    Ok(())
}
