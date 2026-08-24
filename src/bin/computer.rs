//! `computer` — a desktop in a box, from the command line.
//!
//! For the times when writing a program is more ceremony than the job needs:
//! open a box, look at it, point it at a page, hand it to somebody, take it
//! away again.
//!
//! ```text
//! computer up [--size WxH] [--wide-fonts] [--url URL] [--name NAME] [--ttl MINUTES]
//! computer ls
//! computer shot <box> [file.png]
//! computer open <box> <url>
//! computer type <box> <text>
//! computer key  <box> <chord>
//! computer click <box> <x> <y> [left|right|middle]
//! computer clip <box> [text]
//! computer takeover <box>
//! computer release <box>
//! computer exec <box> -- <command> [args…]
//! computer rm <box>
//! ```

use computer::{Button, Computer, Point};
use std::time::Duration;

const USAGE: &str = "\
computer — a desktop in a box

  up [--size WxH] [--url URL] [--name NAME] [--ttl MINUTES] [--wide-fonts]
                              open a box and print where to watch it
  ls                          boxes this tool has opened
  shot <box> [file.png]       capture the screen
  open <box> <url>            open a URL in the box's browser
  type <box> <text>           type into the focused window
  key <box> <chord>           send a chord, such as ctrl+l or cmd+enter
  click <box> <x> <y> [button] click at a point, in device pixels
  clip <box> [text] [--primary]
                              read a selection, or set it
  takeover <box>              open the input viewer and print its URL
  release <box>               close it and take the screen back
  exec <box> -- <command…>    run a command inside the box
  rm <box>                    take the box away
  sweep                       remove every desktop whose deadline has passed

The first `up` builds the image, which takes a few minutes. Every one after it
starts in seconds.
";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("help");

    let outcome = match command {
        "up" => up(&args[1..]).await,
        "ls" => list().await,
        "shot" => shot(&args[1..]).await,
        "open" => open(&args[1..]).await,
        "type" => type_text(&args[1..]).await,
        "key" => key(&args[1..]).await,
        "click" => click(&args[1..]).await,
        "clip" => clip(&args[1..]).await,
        "takeover" => takeover(&args[1..]).await,
        "release" => release(&args[1..]).await,
        "exec" => exec(&args[1..]).await,
        "rm" => remove(&args[1..]).await,
        "sweep" => sweep().await,
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            return;
        }
        other => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    if let Err(error) = outcome {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// A flag's value, where flags are `--name value`.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

fn present(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

/// A positional argument, or a usage failure that names what was wanted.
fn positional<'a>(args: &'a [String], at: usize, what: &str) -> computer::Result<&'a str> {
    args.get(at)
        .map(String::as_str)
        .ok_or_else(|| computer::Error::denied(format!("expected {what}\n\n{USAGE}")))
}

async fn attach(args: &[String]) -> computer::Result<Computer> {
    Computer::attach(positional(args, 0, "a box name")?).await
}

async fn up(args: &[String]) -> computer::Result<()> {
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

async fn list() -> computer::Result<()> {
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

async fn shot(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let out = args.get(1).map(String::as_str).unwrap_or("screen.png");

    let frame = computer.screenshot().await?;
    tokio::fs::write(out, &frame)
        .await
        .map_err(|error| computer::Error::denied(format!("{out}: {error}")))?;

    eprintln!("{} bytes → {out}", frame.len());
    Ok(())
}

async fn open(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    computer.open_url(positional(args, 1, "a URL")?).await
}

async fn type_text(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let text = args[1..].join(" ");
    computer.type_text(&text).await
}

async fn key(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    computer.key(positional(args, 1, "a chord")?).await
}

async fn click(args: &[String]) -> computer::Result<()> {
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

async fn clip(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;

    // PRIMARY is what dragging the mouse over text fills, and a middle click
    // pastes. It is a different selection from the one copy and paste uses.
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

async fn takeover(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let takeover = computer.hand_over().await?;

    match takeover.url() {
        Some(url) => println!("{url}"),
        None => eprintln!("the control viewer is up inside the box, and no port is published"),
    }
    eprintln!("  give it back with: computer release {}", computer.name());
    Ok(())
}

async fn release(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    computer.reclaim().await?;
    eprintln!("{:?}", computer.viewers().await?);
    Ok(())
}

async fn exec(args: &[String]) -> computer::Result<()> {
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

async fn sweep() -> computer::Result<()> {
    // The deadline is on the box itself, so this finds the ones whose program
    // died before it could clean up — which no timer is watching any more.
    let machine = computer::DockerMachine::default();
    let swept = computer::sweep_expired(&machine, std::time::SystemTime::now()).await?;

    for name in &swept {
        println!("{name}");
    }
    eprintln!("{} removed", swept.len());
    Ok(())
}

async fn remove(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let name = computer.name().to_string();
    computer.shutdown().await?;
    eprintln!("{name} is gone");
    Ok(())
}
