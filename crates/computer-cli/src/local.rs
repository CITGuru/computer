use crate::{bare, flag, framing, positional, present, wheel};
use computer::{Button, Computer, Delta, Point};
use std::time::Duration;

pub async fn attach(args: &[String]) -> computer::Result<Computer> {
    Computer::attach(positional(args, 0, "a box name")?).await
}

pub async fn new(args: &[String]) -> computer::Result<()> {
    let asked = crate::spec::asked(args).map_err(computer::Error::denied)?;

    let mut builder = computer::Builder::from_spec(&asked.spec)?
        .place(&asked.placement)?
        .keep_on_drop(true);

    if let Some(name) = flag(args, "--name") {
        builder = builder.name(name);
    }

    eprintln!("opening a box (the first one builds the image) …");
    let computer = builder.launch().await?;

    if let Some(url) = flag(args, "--url") {
        computer.open_url(url).await?;
    }

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
    // Through the runtime: the boxes worth listing outlived whatever opened them.
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

pub async fn screenshot(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let out = crate::remote::named(args).unwrap_or("screen.png");

    let frame = computer.capture(&framing(args)?).await?;
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

pub async fn mouse(args: &[String]) -> computer::Result<()> {
    let op = positional(args, 1, "move, click, drag, path, scroll or at")?.to_string();

    let mut rest = args.to_vec();
    rest.remove(1);

    match op.as_str() {
        "click" => click(&rest).await,
        "scroll" => scroll(&rest).await,
        "move" => {
            let computer = attach(&rest).await?;
            computer.move_to(point(&rest, 1)?).await
        }
        "drag" => {
            let computer = attach(&rest).await?;
            computer
                .drag_with(
                    point(&rest, 1)?,
                    point(&rest, 3)?,
                    button(rest.get(5)),
                    &modifiers(&rest)?,
                )
                .await
        }
        "path" => {
            let (through, button) = crate::remote::stroke(&bare(&rest, &["--held", "--seed"]))
                .map_err(computer::Error::denied)?;
            let steps: Vec<computer::motion::Step> = through[1..]
                .iter()
                .map(|at| computer::motion::Step {
                    at: *at,
                    pause: Duration::ZERO,
                })
                .collect();

            let computer = attach(&rest).await?;
            computer::Desktop::drag_along(&computer, through[0], &steps, button, &modifiers(&rest)?)
                .await
        }
        "at" => {
            let computer = attach(&rest).await?;
            let at = computer.cursor().await?;
            println!("{},{}", at.x, at.y);
            Ok(())
        }
        "down" | "up" => Err(computer::Error::denied(
            "a button held across commands needs a server, which lets it go when its time \
             runs out. Run it without --local.",
        )),
        other => Err(computer::Error::denied(format!("no such op: {other}"))),
    }
}

pub async fn keyboard(args: &[String]) -> computer::Result<()> {
    let op = positional(args, 1, "type or press")?.to_string();

    let mut rest = args.to_vec();
    rest.remove(1);

    match op.as_str() {
        "type" => type_text(&rest).await,
        "press" => press(&rest).await,
        "down" | "up" => Err(computer::Error::denied(
            "a key held across commands needs a server, which lets it go when its time runs \
             out. Run it without --local.",
        )),
        other => Err(computer::Error::denied(format!("no such op: {other}"))),
    }
}

fn point(args: &[String], at: usize) -> computer::Result<Point> {
    let read = |at: usize, what: &str| -> computer::Result<u32> {
        positional(args, at, what)?
            .parse()
            .map_err(|_| computer::Error::denied(format!("{what} must be a whole number")))
    };

    Ok(Point::new(
        read(at, "an x coordinate")?,
        read(at + 1, "a y coordinate")?,
    ))
}

fn button(named: Option<&String>) -> Button {
    match named.map(String::as_str) {
        Some("right") => Button::Right,
        Some("middle") => Button::Middle,
        _ => Button::Left,
    }
}

const TYPED: [&str; 2] = ["--delay", "--held"];

pub async fn type_text(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let rest = bare(args, &TYPED);

    let delay = match flag(args, "--delay") {
        None => None,
        Some(given) => Some(Duration::from_millis(given.parse().map_err(|_| {
            computer::Error::denied(format!("--delay takes milliseconds: {given}"))
        })?)),
    };

    let typing = computer.type_text(rest[1..].join(" "));
    match delay {
        Some(delay) => typing.every(delay).await,
        None => typing.await,
    }
}

pub async fn press(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let rest = bare(args, &TYPED);

    let keys = rest.get(1..).unwrap_or_default();
    if keys.is_empty() {
        return Err(computer::Error::denied("expected a key to press"));
    }

    computer.press(keys).holding(modifiers(args)?).await
}

pub async fn click(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let x = positional(args, 1, "an x coordinate")?
        .parse()
        .map_err(|_| computer::Error::denied("x must be a whole number of pixels"))?;
    let y = positional(args, 2, "a y coordinate")?
        .parse()
        .map_err(|_| computer::Error::denied("y must be a whole number of pixels"))?;

    let button = button(args.get(3));
    let at = Point::new(x, y);

    match args.iter().any(|arg| arg == "--double") {
        true => computer.double_click(at, button).await,
        false => computer.click_with(at, button, &modifiers(args)?).await,
    }
}

fn modifiers(args: &[String]) -> computer::Result<Vec<computer::Held>> {
    let Some(given) = flag(args, "--held") else {
        return Ok(Vec::new());
    };

    given
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(|word| {
            computer::Held::named(word)
                .ok_or_else(|| computer::Error::denied(format!("no such modifier: {word}")))
        })
        .collect()
}

pub async fn scroll(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let turn = wheel(args.get(1..).unwrap_or_default())?;

    let at = match turn.at {
        Some((x, y)) => Point::new(x, y),
        None => {
            let (width, height) = computer.primary().geometry().await?;
            Point::new(width / 2, height / 2)
        }
    };

    computer
        .scroll(
            at,
            Delta {
                dx: turn.dx,
                dy: turn.dy,
            },
        )
        .await
}

pub async fn wait(args: &[String]) -> computer::Result<()> {
    let computer = attach(args).await?;
    let ms = |name, fallback| -> computer::Result<Duration> {
        match flag(args, name) {
            None => Ok(Duration::from_millis(fallback)),
            Some(given) => given.parse().map(Duration::from_millis).map_err(|_| {
                computer::Error::denied(format!("{name} takes milliseconds: {given}"))
            }),
        }
    };

    computer
        .wait_until_still(ms("--settle", 400)?, ms("--within", 10_000)?)
        .await
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
    let machine = computer::EngineMachine::default();
    let swept = computer::sweep_expired(&machine, std::time::SystemTime::now()).await?;

    for name in &swept {
        println!("{name}");
    }
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
