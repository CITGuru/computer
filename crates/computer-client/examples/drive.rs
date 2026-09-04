//! Launch a box, drive it, fork it, read what happened, and take it away.
//!
//! ```bash
//! cargo run -p computer-server
//! cargo run -p computer-client --example drive
//! ```

use computer_api::{Action, ActionBatch, ForkMode, ForkRequest, Want};
use computer_client::{Client, frame_png};
use computer_types::{Placement, Point, Selection, Spec};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    let mut client = Client::new(base);
    if let Ok(token) = std::env::var("COMPUTER_SERVER_TOKEN") {
        client = client.with_token(token);
    }

    client.health().await?;

    let box_ = client
        .create(
            &Spec::default(),
            &Placement::default(),
            Some("drive-example"),
        )
        .await?;
    println!("launched {} at {}x{}", box_.id, box_.width, box_.height);
    if let Some(url) = &box_.viewer_url {
        println!("  watch it at {url}");
    }

    let result = client
        .act(
            &box_.id,
            0,
            &ActionBatch {
                actions: vec![
                    Action::OpenUrl {
                        url: "https://example.com".to_string(),
                    },
                    Action::Move {
                        to: Point { x: 640, y: 400 },
                    },
                ],
                settle_ms: Some(3000),
                want: vec![Want::Frame, Want::Cursor],
                have_frame: None,
            },
            None,
        )
        .await?;

    println!(
        "acted: {:?}, cursor {:?}",
        result.results.iter().map(|r| r.ok).collect::<Vec<_>>(),
        result.cursor
    );

    if let Some(frame) = &result.frame {
        let png = frame_png(frame)?.unwrap_or_default();
        println!(
            "  frame {} is {} bytes",
            frame.hash.get(..12).unwrap_or(&frame.hash),
            png.len()
        );

        // The same hash back means nothing moved, and no picture travels.
        let again = client.frame(&box_.id, 0, Some(&frame.hash)).await?;
        println!(
            "  asking again with the hash we hold: unchanged={}",
            again.unchanged
        );
    }

    client
        .set_clipboard(&box_.id, 0, "through the client", Selection::Clipboard)
        .await?;
    println!(
        "clipboard: {:?}",
        client.clipboard(&box_.id, 0, Selection::Clipboard).await?
    );

    let ran = client
        .exec(&box_.id, &["uname".to_string(), "-s".to_string()], None)
        .await?;
    println!("exec: {} -> {:?}", ran.code, ran.stdout.trim());

    let forked = client
        .fork(
            &box_.id,
            &ForkRequest {
                mode: ForkMode::Replay,
                up_to: None,
                placement: None,
            },
            None,
        )
        .await?;
    println!(
        "forked into {}: {} of {} replayed",
        forked.created.id, forked.replay.ok, forked.replay.attempted
    );

    let trace = client.trace(&box_.id, None, Some(20)).await?;
    println!("trace:");
    for entry in &trace.entries {
        println!("  {:>3} {:?} {:?}", entry.seq, entry.actor, entry.event);
    }

    // A refusal arrives as one, rather than as a status code to interpret.
    match client
        .act_once(&box_.id, 9, Action::Key { chord: "a".into() })
        .await
    {
        Err(error) => println!("screen 9: {error}"),
        Ok(_) => println!("screen 9 answered, which it should not have"),
    }

    for id in [&forked.created.id, &box_.id] {
        client.delete(id).await?;
    }
    println!("removed both");
    Ok(())
}
