//! Hand the screen to a person, and take it back.
//!
//! ```text
//! cargo run --example takeover                    # a new box, yours for 60 seconds
//! cargo run --example takeover -- <box-name>      # a box that is already up
//! cargo run --example takeover -- <box-name> hold # leave control open and exit
//! cargo run --example takeover -- <box-name> release # take a held screen back
//! ```
//!
//! Two servers, not one: the read-only viewer runs from the moment the screen
//! starts, and the input-accepting one is opened only when somebody is handed
//! the screen. A person already watching the first is therefore never silently
//! given the keyboard — they have to be told where to connect.
//!
//! While the takeover is running the owner's input is refused and its reads
//! are not: the work is not paused, it just may not touch anything.

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
        // Ends a takeover this process never started, which is the whole
        // reason `reclaim` exists: the gate that opened it went with the
        // program that opened it, and the server did not.
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

    // The owner may still look …
    let frame = computer.screenshot().await?;
    println!("\n  still watching: {} bytes", frame.len());

    // … and may not touch.
    match computer.click(Point::new(10, 10), Button::Left).await {
        Err(error) => println!("  input refused, as it should be: {error}"),
        Ok(()) => println!("  the gate let a click through, which is a bug"),
    }

    if hold {
        // Left open on purpose. The gate that refuses the owner's input lives
        // in this process and goes when it does; the server that accepts the
        // person's input is in the box and stays until it is released.
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
