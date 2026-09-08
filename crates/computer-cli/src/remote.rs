//! Driving boxes through a server.

use crate::{USAGE, flag, framing, positional, present, wheel};
use computer_api::{
    Action, ActionBatch, Arrange, ForkMode, ForkRequest, Held, Region, Shot, Window,
};
use computer_client::{Client, frame_png};
use computer_types::{Button, Desktop, Feature, Placement, Point, Selection, Spec};
use std::time::Duration;

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
    let out = named(args).unwrap_or("screen.png");
    let shot = asked(args)?;

    let frame = client
        .capture(id, 0, &shot, None)
        .await
        .map_err(|e| e.to_string())?;
    let png = frame_png(&frame)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the server sent no picture".to_string())?;

    tokio::fs::write(out, &png)
        .await
        .map_err(|error| format!("{out}: {error}"))?;

    eprintln!("{} bytes → {out}", png.len());
    Ok(())
}

/// The file to write, which is the first argument that is not part of a flag.
pub fn named(args: &[String]) -> Option<&str> {
    let flags = ["--window", "--at", "--size", "--scale"];
    let mut rest = args.iter().skip(1);

    while let Some(arg) = rest.next() {
        if flags.contains(&arg.as_str()) {
            rest.next();
            continue;
        }
        return Some(arg);
    }
    None
}

fn asked(args: &[String]) -> Result<Shot, String> {
    let shot = framing(args).map_err(|e| e.to_string())?;

    Ok(Shot {
        window: match &shot.of {
            computer::Of::Window(id) => Some(id.clone()),
            _ => None,
        },
        region: match &shot.of {
            computer::Of::Region(area) => Some(Region {
                at: Point {
                    x: area.at.x,
                    y: area.at.y,
                },
                width: area.width,
                height: area.height,
            }),
            _ => None,
        },
        scale: shot.scale,
    })
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

/// Open an app by name, and wait until it has drawn.
pub async fn app(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let app = positional(args, 1, "an app").map_err(|e| e.to_string())?;

    act(
        client,
        id,
        Action::Launch {
            app: app.to_string(),
            args: args.get(2..).unwrap_or_default().to_vec(),
        },
    )
    .await
}

/// The app names this server can open.
pub async fn apps(client: &Client) -> Done {
    for name in client.catalog().await.map_err(|e| e.to_string())? {
        println!("{name}");
    }
    Ok(())
}

/// What is on a screen, whoever opened it.
pub async fn windows(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;

    for window in client.windows(id, 0).await.map_err(|e| e.to_string())? {
        println!("{}", shown(&window));
    }
    Ok(())
}

pub async fn window(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let what = positional(args, 1, "active, wait or a window id").map_err(|e| e.to_string())?;

    let window = match what {
        "active" => match client
            .active_window(id, 0)
            .await
            .map_err(|e| e.to_string())?
        {
            Some(window) => window,
            None => {
                println!("nothing has the keyboard");
                return Ok(());
            }
        },
        "wait" => {
            let class = positional(args, 2, "a window class").map_err(|e| e.to_string())?;
            let within = match flag(args, "--within") {
                Some(secs) => {
                    Some(Duration::from_secs(secs.parse().map_err(|_| {
                        "--within takes a number of seconds".to_string()
                    })?))
                }
                None => None,
            };

            client
                .wait_for_window(id, 0, class, within)
                .await
                .map_err(|e| e.to_string())?
        }
        window => match positional(args, 2, "focus, close, move, size, max, min or restore")
            .map_err(|e| e.to_string())?
        {
            "focus" => {
                client
                    .focus_window(id, 0, window)
                    .await
                    .map_err(|e| e.to_string())?;

                // Answered with whatever holds the keyboard afterwards: a
                // window manager is free to refuse a raise, and this is the
                // only thing that says whether it did.
                match client
                    .active_window(id, 0)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    Some(window) => window,
                    None => return Ok(()),
                }
            }
            // Nothing to answer with: the window it named is gone.
            "close" => {
                return client
                    .close_window(id, 0, window)
                    .await
                    .map_err(|e| e.to_string());
            }
            verb => client
                .arrange_window(id, 0, window, arrangement(verb, args)?)
                .await
                .map_err(|e| e.to_string())?,
        },
    };

    println!("{}", shown(&window));
    Ok(())
}

fn arrangement(verb: &str, args: &[String]) -> Result<Arrange, String> {
    match verb {
        "move" => Ok(Arrange::At {
            to: Point {
                x: number(args, 3, "an x coordinate")?,
                y: number(args, 4, "a y coordinate")?,
            },
        }),
        "size" => Ok(Arrange::Size {
            width: number(args, 3, "a width")?,
            height: number(args, 4, "a height")?,
        }),
        "max" => Ok(Arrange::Maximise),
        "min" => Ok(Arrange::Minimise),
        "restore" => Ok(Arrange::Restore),
        other => Err(wanted(&format!(
            "focus, close, move, size, max, min or restore, not {other}"
        ))),
    }
}

fn shown(window: &Window) -> String {
    format!(
        "{}\t{}x{}+{}+{}\t{}\t{}",
        window.id,
        window.width,
        window.height,
        window.at.x,
        window.at.y,
        window.class,
        window.title
    )
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
            held: modifiers(args)?,
        },
    )
    .await
}

pub async fn scroll(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let turn = wheel(args.get(1..).unwrap_or_default()).map_err(|e| e.to_string())?;

    let at = match turn.at {
        Some((x, y)) => Point { x, y },
        // Asked for rather than assumed: a box can be any size, and a wheel
        // turned outside the screen reaches nothing.
        None => {
            let found = client.get(id).await.map_err(|e| e.to_string())?;
            Point {
                x: found.width / 2,
                y: found.height / 2,
            }
        }
    };

    act(
        client,
        id,
        Action::Scroll {
            at,
            dx: turn.dx,
            dy: turn.dy,
        },
    )
    .await
}

pub async fn still(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let ms = |name| -> Result<Option<u64>, String> {
        match flag(args, name) {
            None => Ok(None),
            Some(given) => given
                .parse()
                .map(Some)
                .map_err(|_| format!("{name} takes milliseconds: {given}")),
        }
    };

    act(
        client,
        id,
        Action::WaitStill {
            settle_ms: ms("--settle")?,
            within_ms: ms("--within")?,
        },
    )
    .await
}

/// `--held shift,ctrl`, in the same spellings a chord takes.
fn modifiers(args: &[String]) -> Result<Vec<Held>, String> {
    let Some(given) = flag(args, "--held") else {
        return Ok(Vec::new());
    };

    given
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(|word| match computer::Held::named(word) {
            Some(computer::Held::Shift) => Ok(Held::Shift),
            Some(computer::Held::Ctrl) => Ok(Held::Ctrl),
            Some(computer::Held::Alt) => Ok(Held::Alt),
            Some(computer::Held::Super) => Ok(Held::Super),
            None => Err(format!("no such modifier: {word}")),
        })
        .collect()
}

pub async fn clip(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;

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
        E::AppLaunched { screen, app, .. } => format!("screen {screen}  opened {app}"),
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

fn op_of(what: &computer_api::OnElement) -> &'static str {
    use computer_api::OnElement as O;

    match what {
        O::Click { .. } => "click",
        O::Fill { .. } => "fill",
        O::Options { .. } => "options",
        O::Choose { .. } => "choose",
        O::Upload { .. } => "upload",
        O::WaitFor { .. } => "wait",
        O::Hover { .. } => "hover",
        O::History { .. } => "history",
        O::Scroll { .. } => "scroll",
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
        Action::OnPage { what } => format!("page {}", op_of(what)),
        Action::Launch { app, args } => match args.is_empty() {
            true => format!("open {app}"),
            false => format!("open {app} {}", args.join(" ")),
        },
        Action::Wait { ms } => format!("wait {ms}ms"),
        Action::WaitStill { .. } => "wait until still".to_string(),
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
