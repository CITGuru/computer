//! Driving boxes through a server.
//!
//! The way round every command works unless `--local` says otherwise, whether
//! the server is a fleet somewhere else or one this process started a moment
//! ago for itself.

use crate::{USAGE, flag, positional, present};
use computer_api::{Action, ActionBatch, ForkMode, ForkRequest};
use computer_client::{Client, frame_png};
use computer_types::{Button, Desktop, Feature, Placement, Point, Selection, Spec};

type Done = Result<(), String>;

fn wanted(what: &str) -> String {
    format!("expected {what}\n\n{USAGE}")
}

pub async fn up(client: &Client, args: &[String]) -> Done {
    let mut desktop = Desktop::default();

    if let Some(size) = flag(args, "--size") {
        let (width, height) = size
            .split_once(['x', 'X'])
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .ok_or_else(|| "--size takes WIDTHxHEIGHT, such as 1920x1080".to_string())?;
        desktop.width = Some(width);
        desktop.height = Some(height);
    }
    if present(args, "--wide-fonts") {
        desktop.features.push(Feature::WideFonts);
    }

    let mut placement = Placement::default();
    if let Some(minutes) = flag(args, "--ttl") {
        let minutes: u64 = minutes
            .parse()
            .map_err(|_| "--ttl takes a number of minutes".to_string())?;
        placement.expires_after_secs = Some(minutes * 60);
    }

    let spec = Spec {
        desktop,
        ..Spec::default()
    };

    eprintln!("opening a box (the first one builds the image) …");
    let created = client
        .create(&spec, &placement, None)
        .await
        .map_err(|error| error.to_string())?;

    if let Some(url) = flag(args, "--url") {
        client
            .act_once(
                &created.id,
                0,
                Action::OpenUrl {
                    url: url.to_string(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
    }

    // The id on standard output, so `computer shot $(computer up)` works.
    // Everything a person reads goes to standard error.
    println!("{}", created.id);
    if let Some(url) = &created.viewer_url {
        eprintln!("  watch it  {url}");
    }
    eprintln!("  stop it   computer rm {}", created.id);
    Ok(())
}

pub async fn list(client: &Client) -> Done {
    for found in client.list().await.map_err(|e| e.to_string())? {
        println!(
            "{}\t{}x{}\t{} screen(s)",
            found.id, found.width, found.height, found.screens
        );
    }
    Ok(())
}

pub async fn shot(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let out = args.get(1).map(String::as_str).unwrap_or("screen.png");

    let frame = client.frame(id, 0, None).await.map_err(|e| e.to_string())?;
    let png = frame_png(&frame)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the server sent no picture".to_string())?;

    tokio::fs::write(out, &png)
        .await
        .map_err(|error| format!("{out}: {error}"))?;

    eprintln!("{} bytes → {out}", png.len());
    Ok(())
}

pub async fn open(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let url = positional(args, 1, "a URL").map_err(|e| e.to_string())?;
    act(
        client,
        id,
        Action::OpenUrl {
            url: url.to_string(),
        },
    )
    .await
}

pub async fn type_text(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    act(
        client,
        id,
        Action::Type {
            text: args[1..].join(" "),
        },
    )
    .await
}

pub async fn key(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let chord = positional(args, 1, "a chord").map_err(|e| e.to_string())?;
    act(
        client,
        id,
        Action::Key {
            chord: chord.to_string(),
        },
    )
    .await
}

pub async fn click(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let x = number(args, 1, "an x coordinate")?;
    let y = number(args, 2, "a y coordinate")?;

    let button = match args.get(3).map(String::as_str) {
        Some("right") => Button::Right,
        Some("middle") => Button::Middle,
        _ => Button::Left,
    };

    act(
        client,
        id,
        Action::Click {
            at: Some(Point { x, y }),
            button,
        },
    )
    .await
}

pub async fn clip(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;

    // PRIMARY is what dragging the mouse over text fills, and a middle click
    // pastes. It is a different selection from the one copy and paste uses.
    let selection = match present(args, "--primary") {
        true => Selection::Primary,
        false => Selection::Clipboard,
    };

    let text: Vec<&str> = args[1..]
        .iter()
        .filter(|arg| *arg != "--primary")
        .map(String::as_str)
        .collect();

    if text.is_empty() {
        print!(
            "{}",
            client
                .clipboard(id, 0, selection)
                .await
                .map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    client
        .set_clipboard(id, 0, &text.join(" "), selection)
        .await
        .map_err(|e| e.to_string())
}

pub async fn takeover(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let view = client
        .takeover(id, 0, false)
        .await
        .map_err(|e| e.to_string())?;

    match view.url {
        Some(url) => println!("{url}"),
        None => eprintln!("the control viewer is up inside the box, and no port is published"),
    }
    eprintln!("  give it back with: computer release {id}");
    Ok(())
}

pub async fn release(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    client
        .end_takeover(id, 0)
        .await
        .map_err(|e| e.to_string())?;

    let watching = client.viewers(id, 0).await.map_err(|e| e.to_string())?;
    eprintln!(
        "watching {} driving {}",
        watching.watching, watching.driving
    );
    Ok(())
}

pub async fn exec(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;

    // Everything after `--`, so the box's command keeps its own flags.
    let argv: Vec<String> = args
        .iter()
        .position(|arg| arg == "--")
        .map(|at| args[at + 1..].to_vec())
        .unwrap_or_else(|| args[1..].to_vec());

    if argv.is_empty() {
        return Err(wanted("a command"));
    }

    let ran = client
        .exec(id, &argv, None)
        .await
        .map_err(|e| e.to_string())?;
    print!("{}", ran.stdout);
    eprint!("{}", ran.stderr);

    if ran.code != 0 {
        std::process::exit(ran.code);
    }
    Ok(())
}

pub async fn remove(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    client.delete(id).await.map_err(|e| e.to_string())?;
    eprintln!("{id} is gone");
    Ok(())
}

pub async fn fork(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let up_to = flag(args, "--up-to").and_then(|seq| seq.parse().ok());

    let forked = client
        .fork(
            id,
            &ForkRequest {
                mode: ForkMode::Replay,
                up_to,
                placement: None,
            },
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

    println!("{}", forked.created.id);
    eprintln!(
        "  {} of {} actions replayed{}",
        forked.replay.ok,
        forked.replay.attempted,
        if forked.replay.truncated {
            ", and it ran out of time"
        } else {
            ""
        }
    );
    Ok(())
}

pub async fn trace(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let after = flag(args, "--after").and_then(|seq| seq.parse().ok());

    let read = client
        .trace(id, after, None)
        .await
        .map_err(|e| e.to_string())?;

    for entry in read.entries {
        println!(
            "{:>4}  {:<7}  {}",
            entry.seq,
            format!("{:?}", entry.actor).to_lowercase(),
            summarise(&entry.event)
        );
    }
    Ok(())
}

fn summarise(event: &computer_api::TraceEvent) -> String {
    use computer_api::TraceEvent as E;

    match event {
        E::BoxCreated { width, height, .. } => format!("created {width}x{height}"),
        E::Acted {
            action, ok, screen, ..
        } => format!(
            "screen {screen}  {}{}",
            name_of(action),
            if *ok { "" } else { "  refused" }
        ),
        E::Frame { screen } => format!("screen {screen}  the screen changed"),
        E::Executed { argv, code, .. } => format!("ran {} → {code}", argv.join(" ")),
        E::FileWritten { path, bytes } => format!("wrote {bytes} bytes to {path}"),
        E::FileRead { path, bytes } => format!("read {bytes} bytes from {path}"),
        E::ClipboardSet { selection, .. } => format!("set the {selection:?} selection"),
        E::ClipboardRead { selection, .. } => format!("read the {selection:?} selection"),
        E::TakeoverStarted { screen, .. } => format!("screen {screen} handed to a person"),
        E::TakeoverEnded { screen } => format!("screen {screen} taken back"),
        E::Gone { why } => format!("gone: {why}"),
        E::Adopted { runtime } => format!("found running on {runtime}"),
        E::ForkedFrom { source, .. } => format!("forked from {source}"),
        E::BoxDeleted => "removed".to_string(),
    }
}

fn name_of(action: &Action) -> String {
    match action {
        Action::Move { to } => format!("move to {},{}", to.x, to.y),
        Action::Click { at, .. } => match at {
            Some(at) => format!("click at {},{}", at.x, at.y),
            None => "click".to_string(),
        },
        Action::DoubleClick { .. } => "double click".to_string(),
        Action::Drag { from, to, .. } => {
            format!("drag {},{} → {},{}", from.x, from.y, to.x, to.y)
        }
        Action::Type { text } => format!("type {text:?}"),
        Action::Key { chord } => format!("key {chord}"),
        Action::Scroll { dy, .. } => format!("scroll {dy}"),
        Action::OpenUrl { url } => format!("open {url}"),
        Action::Wait { ms } => format!("wait {ms}ms"),
    }
}

/// Do it, and say nothing when it worked.
async fn act(client: &Client, id: &str, action: Action) -> Done {
    let result = client
        .act(
            id,
            0,
            &ActionBatch {
                actions: vec![action],
                settle_ms: None,
                want: Vec::new(),
                have_frame: None,
            },
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

    match result.results.iter().find(|one| !one.ok) {
        Some(refused) => Err(refused
            .error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "it was refused".to_string())),
        None => Ok(()),
    }
}

fn number(args: &[String], at: usize, what: &str) -> Result<u32, String> {
    positional(args, at, what)
        .map_err(|e| e.to_string())?
        .parse()
        .map_err(|_| format!("{what} must be a whole number of pixels"))
}
