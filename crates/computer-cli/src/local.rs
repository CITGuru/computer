//! Driving boxes with the SDK, in this process.
//!
//! The escape hatch behind `--local`: no server, no socket, nothing to start.
//! It reaches a box on this machine and nothing else, so the commands that need
//! a server to remember anything — a trace, and the fork built on one — are not
//! here.

use crate::{flag, positional, present};
use computer::{Button, Computer, Point};
use std::time::Duration;

pub async fn attach(args: &[String]) -> computer::Result<Computer> {
    Computer::attach(positional(args, 0, "a box name")?).await
}

pub async fn up(args: &[String]) -> computer::Result<()> {
    let mut builder = Computer::builder().keep_on_drop(true);

    if let Some(size) = flag(args, "--size") {
        let (width, height) = size
            .split_once(['x', 'X'])
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .ok_or_else(|| {
                computer::Error::denied("--size takes WIDTHxHEIGHT, such as 1920x1080")
            })?;
        builder = builder.size(width, height);
    }
    if let Some(name) = flag(args, "--name") {
        builder = builder.name(name);
    }
    if let Some(minutes) = flag(args, "--ttl") {
        let minutes: u64 = minutes
            .parse()
            .map_err(|_| computer::Error::denied("--ttl takes a number of minutes"))?;
        builder = builder.expires_after(Duration::from_secs(minutes * 60));
    }
    if present(args, "--wide-fonts") {
        builder = builder.wide_fonts();
    }

    eprintln!("opening a box (the first one builds the image) …");
    let computer = builder.launch().await?;

    if let Some(url) = flag(args, "--url") {
        computer.open_url(url).await?;
    }

    // The name on standard output, so `computer shot $(computer up)` works.
    // Everything a person reads goes to standard error.
    println!("{}", computer.name());
    if let Some(url) = computer.viewer_url() {
        eprintln!("  watch it  {url}");
    }
    if let Some(at) = computer.expires_at() {
        eprintln!("  expires   {at:?}");
    }
    eprintln!("  stop it   computer rm {}", computer.name());
    Ok(())
}

pub async fn list() -> computer::Result<()> {
    // Through the runtime rather than through this crate: the boxes worth
    // listing are the ones that outlived whatever opened them.
    let output = tokio::process::Command::new("docker")
        .args([
            "ps",
            "--filter",
            "label=computer-rs=1",
            "--format",
            "{{.Names}}\t{{.Status}}",
        ])
        .output()
        .await
        .map_err(|error| computer::Error::transport_public(error.to_string()))?;

    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

pub async fn shot(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let out = args.get(1).map(String::as_str).unwrap_or("screen.png");

    let frame = computer.screenshot().await?;
    tokio::fs::write(out, &frame)
        .await
        .map_err(|error| computer::Error::denied(format!("{out}: {error}")))?;

    eprintln!("{} bytes → {out}", frame.len());
    Ok(())
}

pub async fn open(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    computer.open_url(positional(args, 1, "a URL")?).await
}

pub async fn type_text(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let text = args[1..].join(" ");
    computer.type_text(&text).await
}

pub async fn key(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    computer.key(positional(args, 1, "a chord")?).await
}

pub async fn click(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let x = positional(args, 1, "an x coordinate")?
        .parse()
        .map_err(|_| computer::Error::denied("x must be a whole number of pixels"))?;
    let y = positional(args, 2, "a y coordinate")?
        .parse()
        .map_err(|_| computer::Error::denied("y must be a whole number of pixels"))?;

    let button = match args.get(3).map(String::as_str) {
        Some("right") => Button::Right,
        Some("middle") => Button::Middle,
        _ => Button::Left,
    };

    computer.click(Point::new(x, y), button).await
}

pub async fn clip(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;

    let selection = match present(args, "--primary") {
        true => computer::Selection::Primary,
        false => computer::Selection::Clipboard,
    };

    let text: Vec<&String> = args[1..].iter().filter(|arg| *arg != "--primary").collect();

    match text.is_empty() {
        true => {
            print!("{}", computer.selection(selection).await?);
            Ok(())
        }
        false => {
            let joined: Vec<&str> = text.into_iter().map(String::as_str).collect();
            computer.set_selection(selection, &joined.join(" ")).await
        }
    }
}

pub async fn takeover(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let takeover = computer.hand_over().await?;

    match takeover.url() {
        Some(url) => println!("{url}"),
        None => eprintln!("the control viewer is up inside the box, and no port is published"),
    }
    eprintln!("  give it back with: computer release {}", computer.name());
    Ok(())
}

pub async fn release(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    computer.reclaim().await?;
    eprintln!("{:?}", computer.viewers().await?);
    Ok(())
}

pub async fn exec(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;

    // Everything after `--`, so the box's command keeps its own flags.
    let argv: Vec<&String> = args
        .iter()
        .position(|arg| arg == "--")
        .map(|at| args[at + 1..].iter().collect())
        .unwrap_or_else(|| args[1..].iter().collect());

    if argv.is_empty() {
        return Err(computer::Error::denied("expected a command\n\n"));
    }

    let result = computer.exec(argv.into_iter().cloned()).await?;
    print!("{}", result.stdout_utf8());
    eprint!("{}", result.stderr_utf8());

    if !result.ok() {
        std::process::exit(result.code);
    }
    Ok(())
}

pub async fn sweep() -> computer::Result<()> {
    // The deadline is on the box itself, so this finds the ones whose program
    // died before it could clean up — which no timer is watching any more.
    let machine = computer::DockerMachine::default();
    let swept = computer::sweep_expired(&machine, std::time::SystemTime::now()).await?;

    for name in &swept {
        println!("{name}");
    }
    // Named, because `sweep` is the one command that stays here when
    // --server points somewhere else: an operator who just aimed at a fleet
    // should not read this line as the fleet having been swept.
    eprintln!("{} removed from this host's own runtime", swept.len());
    Ok(())
}

pub async fn remove(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let name = computer.name().to_string();
    computer.shutdown().await?;
    eprintln!("{name} is gone");
    Ok(())
}
