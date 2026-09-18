use crate::{USAGE, bare, flag, framing, positional, present, wheel};
use computer_api::{
    Action, ActionBatch, Arrange, BatchResult, Evaluate, Find, ForkMode, ForkRequest, Held,
    OnElement, OnNode, OpenIn, PageShot, Picture, Reading, Rect, Shot, SnapshotOptions, Where,
    Window,
};
use computer_client::{Client, captured_image, frame_png};
use computer_types::{
    Button, Desktop, DisplayServer, Feature, Motion, NodeQuery, Placement, Point, Search,
    Selection, Spec,
};
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
    if present(args, "--accessibility") {
        desktop.features.push(Feature::Accessibility);
    }
    if present(args, "--video") {
        desktop.features.push(Feature::Video);
    }
    if present(args, "--wayland") {
        desktop.server = DisplayServer::Wayland;
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
                    target: OpenIn::Blank,
                },
            )
            .await
            .map_err(|error| error.to_string())?;
    }

    // Only the id goes to stdout, so `computer screenshot $(computer up)` works.
    println!("{}", created.id);
    if let Some(url) = &created.viewer_url {
        eprintln!("  watch it  {url}");
    }
    eprintln!("  stop it   computer rm {}", created.id);
    Ok(())
}

pub async fn list(client: &Client) -> Done {
    for found in client.list().await.map_err(|e| e.to_string())? {
        let state = match found.state {
            computer_api::BoxState::Ready => String::new(),
            other => format!("\t{other:?}"),
        };

        println!(
            "{}\t{}x{}\t{} screen(s){state}",
            found.id, found.width, found.height, found.screens
        );
    }
    Ok(())
}

pub async fn describe(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let found = client.get(id).await.map_err(|e| e.to_string())?;

    println!("id        {}", found.id);
    println!("state     {:?}", found.state);
    println!("screens   {}", found.screens);
    println!("size      {}x{}", found.width, found.height);
    println!("spec      {}", found.spec_digest);
    println!("created   {}", stamped(found.created_at_ms));
    match found.expires_at_ms {
        Some(at) => println!("expires   {}", stamped(at)),
        None => println!("expires   never"),
    }
    if let Some(url) = &found.viewer_url {
        println!("viewer    {url}");
    }
    if let Some(url) = &found.devtools_url {
        println!("devtools  {url}");
    }

    Ok(())
}

fn stamped(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    match chrono_free(secs) {
        Some(when) => when,
        None => format!("{ms}ms"),
    }
}

fn chrono_free(secs: i64) -> Option<String> {
    if secs < 0 {
        return None;
    }

    let days = secs / 86_400;
    let rest = secs % 86_400;
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);

    // Days since 1970 to a civil date, by Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = era * 400 + yoe + i64::from(month <= 2);

    Some(format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z"
    ))
}

pub async fn stop(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    client.stop(id).await.map_err(|e| e.to_string())?;

    eprintln!("stopped; its files are kept. bring it back with: computer resume {id}");
    Ok(())
}

pub async fn pause(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    client.pause(id).await.map_err(|e| e.to_string())?;

    eprintln!("frozen; wake it with: computer resume {id}");
    Ok(())
}

pub async fn resume(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;

    let was = client.get(id).await.map_err(|e| e.to_string())?;
    let found = client.resume(id).await.map_err(|e| e.to_string())?;

    if was.state == computer_api::BoxState::Stopped {
        match &found.viewer_url {
            Some(url) => eprintln!("  watch it  {url}"),
            None => eprintln!("  no viewer port is published"),
        }
        eprintln!("  it was stopped, so the desktop started again with nothing open");
    }

    Ok(())
}

pub async fn screenshot(client: &Client, args: &[String]) -> Done {
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

pub fn named(args: &[String]) -> Option<&str> {
    let flags = ["--window", "--at", "--size", "--scale", "--tab"];
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
            computer::Of::Region(area) => Some(Rect {
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
        pointer: present(args, "--pointer"),
        tab: flag(args, "--tab").map(str::to_string),
    })
}

pub async fn open(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let url = positional(args, 1, "a URL").map_err(|e| e.to_string())?;
    let target = match flag(args, "--target") {
        Some("current") => OpenIn::Current,
        Some("blank") | None => OpenIn::Blank,
        Some(other) => return Err(format!("no such target: {other}")),
    };

    let result = acted(
        client,
        id,
        Action::OpenUrl {
            url: url.to_string(),
            target,
        },
    )
    .await?;

    for tab in &result.tabs {
        println!("{}", tab.id);
    }

    Ok(())
}

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

pub async fn apps(client: &Client) -> Done {
    for name in client.catalog().await.map_err(|e| e.to_string())? {
        println!("{name}");
    }
    Ok(())
}

fn counted<T: std::str::FromStr>(
    args: &[String],
    name: &str,
    what: &str,
) -> Result<Option<T>, String> {
    match flag(args, name) {
        Some(given) => given
            .parse()
            .map(Some)
            .map_err(|_| format!("{name} takes {what}")),
        None => Ok(None),
    }
}

fn shown_node(node: &computer_api::Node) -> String {
    let named = match (&node.labelled, node.name.is_empty()) {
        (Some(label), _) => format!("labelled {label:?}"),
        (None, false) => format!("{:?}", node.name),
        (None, true) => "unnamed".to_string(),
    };
    let where_ = match node.at {
        Some(at) => format!("{},{} {}x{}", at.x, at.y, node.width, node.height),
        None => "not drawn".to_string(),
    };
    let does = match node.actions.is_empty() {
        true => String::new(),
        false => format!("  {}", node.actions.join("/")),
    };

    format!("{:14} {named:24} {where_:18} {}{does}", node.role, node.app)
}

pub async fn widget(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let op = positional(args, 1, "find, tree, press, fill or focus").map_err(|e| e.to_string())?;

    let query = |at: usize| -> Result<NodeQuery, String> {
        Ok(NodeQuery {
            query: positional(args, at, "the words on the widget")
                .map_err(|e| e.to_string())?
                .to_string(),
            role: flag(args, "--role").map(str::to_string),
            exact: present(args, "--exact"),
            app: flag(args, "--app").map(str::to_string),
        })
    };

    let what = match op {
        "find" => OnNode::Find {
            node: query(2)?,
            limit: counted(args, "--limit", "a number of matches")?,
        },
        "tree" => OnNode::Tree {
            app: flag(args, "--app").map(str::to_string),
            depth: counted(args, "--depth", "a number of levels")?,
        },
        "press" => OnNode::Invoke {
            node: query(2)?,
            action: flag(args, "--action").map(str::to_string),
        },
        "fill" => OnNode::Set {
            node: query(2)?,
            value: positional(args, 3, "a value")
                .map_err(|e| e.to_string())?
                .to_string(),
        },
        "focus" => OnNode::Focus { node: query(2)? },
        other => return Err(format!("no such op: {other}")),
    };

    let result = client
        .on_node(id, 0, &what)
        .await
        .map_err(|e| e.to_string())?;

    for node in &result.nodes {
        println!("{}", shown_node(node));
    }
    if result.nodes.is_empty() && result.node.is_none() {
        println!("nothing in the tree matched");
    }
    if let Some(node) = &result.node {
        match &result.action {
            Some(action) => println!("{action}: {}", shown_node(node)),
            None => println!("{}", shown_node(node)),
        }
    }

    Ok(())
}

pub async fn window(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let what =
        positional(args, 1, "list, active, wait or a window id").map_err(|e| e.to_string())?;

    let window = match what {
        "list" => {
            for window in client.windows(id, 0).await.map_err(|e| e.to_string())? {
                println!("{}", shown(&window));
            }
            return Ok(());
        }
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

                // A window manager may refuse a raise, so answer with what holds focus after.
                match client
                    .active_window(id, 0)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    Some(window) => window,
                    None => return Ok(()),
                }
            }
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

const TYPED: [&str; 2] = ["--delay", "--held"];

pub async fn type_text(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let rest = bare(args, &TYPED);

    act(
        client,
        id,
        Action::Type {
            text: rest[1..].join(" "),
            delay_ms: counted(args, "--delay", "a number of milliseconds")?,
        },
    )
    .await
}

pub async fn press(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let rest = bare(args, &TYPED);

    let mut keys = rest.get(1..).unwrap_or_default().iter();
    let chord = keys
        .next()
        .ok_or_else(|| "expected a key to press".to_string())?
        .clone();

    act(
        client,
        id,
        Action::Press {
            chord,
            then: keys.cloned().collect(),
            held: modifiers(args)?,
        },
    )
    .await
}

pub async fn mouse(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let op = positional(args, 1, "move, click, drag, scroll or at").map_err(|e| e.to_string())?;

    let mut rest = args.to_vec();
    rest.remove(1);

    match op {
        "click" => click(client, &rest).await,
        "scroll" => scroll(client, &rest).await,
        "move" => {
            let to = point(&rest, 1)?;
            let (motion, seed) = motion(args)?;
            act(client, id, Action::Move { to, motion, seed }).await
        }
        "drag" => {
            let (motion, seed) = motion(args)?;
            act(
                client,
                id,
                Action::Drag {
                    from: point(&rest, 1)?,
                    to: point(&rest, 3)?,
                    button: button(rest.get(5)),
                    held: modifiers(&rest)?,
                    motion,
                    seed,
                },
            )
            .await
        }
        "at" => {
            let at = client.cursor(id, 0).await.map_err(|e| e.to_string())?;
            println!("{},{}", at.x, at.y);
            Ok(())
        }
        other => Err(format!("no such op: {other}")),
    }
}

pub async fn keyboard(client: &Client, args: &[String]) -> Done {
    let op = positional(args, 1, "type or press").map_err(|e| e.to_string())?;

    let mut rest = args.to_vec();
    rest.remove(1);

    match op {
        "type" => type_text(client, &rest).await,
        "press" => press(client, &rest).await,
        other => Err(format!("no such op: {other}")),
    }
}

fn point(args: &[String], at: usize) -> Result<Point, String> {
    Ok(Point {
        x: number(args, at, "an x coordinate")?,
        y: number(args, at + 1, "a y coordinate")?,
    })
}

fn motion(args: &[String]) -> Result<(Motion, Option<u64>), String> {
    let motion = match (present(args, "--human"), present(args, "--smooth")) {
        (true, _) => Motion::Human,
        (_, true) => Motion::Smooth,
        _ => Motion::Instant,
    };
    Ok((motion, counted(args, "--seed", "a number")?))
}

fn button(named: Option<&String>) -> Button {
    match named.map(String::as_str) {
        Some("right") => Button::Right,
        Some("middle") => Button::Middle,
        _ => Button::Left,
    }
}

pub async fn click(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let x = number(args, 1, "an x coordinate")?;
    let y = number(args, 2, "a y coordinate")?;

    let button = button(args.get(3));
    let held = modifiers(args)?;
    let at = Some(Point { x, y });
    let (motion, seed) = motion(args)?;

    let action = match args.iter().any(|arg| arg == "--double") {
        true => Action::DoubleClick {
            at,
            button,
            motion,
            seed,
        },
        false => Action::Click {
            at,
            button,
            held,
            motion,
            seed,
        },
    };

    act(client, id, action).await
}

pub async fn scroll(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let turn = wheel(args.get(1..).unwrap_or_default()).map_err(|e| e.to_string())?;

    let at = match turn.at {
        Some((x, y)) => Point { x, y },
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

pub async fn record(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let op = positional(args, 1, "start, stop or status").map_err(|e| e.to_string())?;
    let rest = bare(args, &["--fps"]);

    match op {
        "start" => {
            let view = client
                .start_recording(
                    id,
                    0,
                    counted(args, "--fps", "a number of frames a second")?,
                )
                .await
                .map_err(|e| e.to_string())?;

            eprintln!("recording; stop it with `computer record {id} stop`");
            println!("{}", view.path.unwrap_or_default());
            Ok(())
        }
        "stop" => {
            let view = client
                .stop_recording(id, 0)
                .await
                .map_err(|e| e.to_string())?;

            let Some(inside) = view.path else {
                return Err("the box did not say what it had recorded".to_string());
            };

            let out = rest
                .get(2)
                .cloned()
                .unwrap_or_else(|| "recording.mp4".to_string());

            let bytes = client
                .read_file(id, &inside)
                .await
                .map_err(|e| e.to_string())?;
            std::fs::write(&out, &bytes).map_err(|why| format!("{out}: {why}"))?;

            println!("{} bytes → {out}", bytes.len());
            Ok(())
        }
        "status" => {
            let view = client.recording(id, 0).await.map_err(|e| e.to_string())?;

            match view.path {
                Some(path) => println!("recording {path}"),
                None => println!("idle"),
            }
            Ok(())
        }
        other => Err(format!("no such op: {other}")),
    }
}

pub async fn wait(client: &Client, args: &[String]) -> Done {
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

pub async fn file(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let op = positional(args, 1, "ls, get, put, grep or glob").map_err(|e| e.to_string())?;
    let at = |n| positional(args, n, "a path").map_err(|e| e.to_string());

    match op {
        "get" => {
            let there = at(2)?;
            let bytes = client
                .read_file(id, there)
                .await
                .map_err(|e| e.to_string())?;

            // To standard output where no file is named, so a caller can pipe
            // it. The byte count goes to standard error either way.
            match args.get(3) {
                Some(here) => {
                    std::fs::write(here, &bytes).map_err(|why| format!("{here}: {why}"))?;
                    eprintln!("{} bytes → {here}", bytes.len());
                }
                None => {
                    use std::io::Write;
                    std::io::stdout()
                        .write_all(&bytes)
                        .map_err(|why| why.to_string())?;
                    eprintln!("{} bytes", bytes.len());
                }
            }
            Ok(())
        }
        "put" => {
            let here = at(2)?;
            let bytes = std::fs::read(here).map_err(|why| format!("{here}: {why}"))?;

            // The same name in the box where only a directory was given, so
            // `file box put ./notes.txt /tmp` lands where a person expects.
            let there = match args.get(3) {
                Some(given) if given.ends_with('/') => format!("{given}{}", named_part(here)),
                Some(given) => given.clone(),
                None => format!("/tmp/{}", named_part(here)),
            };

            client
                .write_file(id, &there, &bytes)
                .await
                .map_err(|e| e.to_string())?;

            eprintln!("{} bytes → {there}", bytes.len());
            Ok(())
        }
        "ls" => {
            let there = args.get(2).map(String::as_str).unwrap_or("/");
            let listing = client
                .list_dir(id, there)
                .await
                .map_err(|e| e.to_string())?;

            if listing.entries.is_empty() {
                println!("nothing in {there}");
            }
            for entry in &listing.entries {
                match entry.dir {
                    true => println!("{:>10}  {}/", "-", entry.name),
                    false => println!("{:>10}  {}", entry.bytes, entry.name),
                }
            }
            Ok(())
        }
        "grep" => {
            let pattern = at(2)?;
            let where_ = bare(args, &SEARCHED);

            let found = client
                .grep(
                    id,
                    &Search {
                        pattern: pattern.to_string(),
                        // Required: a search of the whole filesystem answers
                        // in megabytes, and nobody means to ask for that.
                        path: where_
                            .get(3)
                            .cloned()
                            .ok_or_else(|| "expected a directory to search".to_string())?,
                        include: flag(args, "--include").map(str::to_string),
                        ignore_case: present(args, "--ignore-case"),
                        limit: counted(args, "--limit", "a number of matches")?,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;

            if found.matches.is_empty() {
                println!("nothing matched");
            }
            for one in &found.matches {
                println!("{}:{}:{}", one.path, one.line, one.text);
            }
            if found.cut {
                eprintln!("(cut at {} matches)", found.matches.len());
            }
            Ok(())
        }
        "glob" => {
            let pattern = at(2)?;
            let where_ = bare(args, &SEARCHED);

            let found = client
                .glob(
                    id,
                    pattern,
                    where_.get(3).map(String::as_str),
                    counted(args, "--limit", "a number of paths")?,
                )
                .await
                .map_err(|e| e.to_string())?;

            if found.paths.is_empty() {
                println!("nothing matched");
            }
            for path in &found.paths {
                println!("{path}");
            }
            if found.cut {
                eprintln!("(cut at {} paths)", found.paths.len());
            }
            Ok(())
        }
        other => Err(format!("no such op: {other}")),
    }
}

/// The `file` flags that take a value.
const SEARCHED: [&str; 2] = ["--include", "--limit"];

/// The file's own name, for a path that named a directory to put it in.
fn named_part(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file")
        .to_string()
}

pub async fn exec(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;

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
        E::BoxPaused => "frozen".to_string(),
        E::BoxResumed => "woken".to_string(),
        E::BoxStopped => "stopped".to_string(),
        E::BoxStarted => "started again, on new ports".to_string(),
        E::PageCaptured { full, bytes } => match full {
            true => format!("captured the whole page, {bytes} bytes"),
            false => format!("captured the page in view, {bytes} bytes"),
        },
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
        O::Drag { .. } => "drag",
        O::History { .. } => "history",
        O::Scroll { .. } => "scroll",
    }
}

fn node_op_of(what: &OnNode) -> &'static str {
    match what {
        OnNode::Tree { .. } => "tree",
        OnNode::Find { .. } => "find",
        OnNode::Focus { .. } => "focus",
        OnNode::Invoke { .. } => "invoke",
        OnNode::Set { .. } => "set",
    }
}

fn name_of(action: &Action) -> String {
    match action {
        Action::Move { to, .. } => format!("move to {},{}", to.x, to.y),
        Action::Click { at, .. } => match at {
            Some(at) => format!("click at {},{}", at.x, at.y),
            None => "click".to_string(),
        },
        Action::DoubleClick { .. } => "double click".to_string(),
        Action::Drag { from, to, .. } => {
            format!("drag {},{} → {},{}", from.x, from.y, to.x, to.y)
        }
        Action::Type { text, .. } => format!("type {text:?}"),
        Action::Press { chord, then, .. } => match then.is_empty() {
            true => format!("press {chord}"),
            false => format!("press {chord} and {} more", then.len()),
        },
        Action::Scroll { dy, .. } => format!("scroll {dy}"),
        Action::OpenUrl { url, .. } => format!("open {url}"),
        Action::OnPage { what } => format!("page {}", op_of(what)),
        Action::OnNode { what } => format!("widget {}", node_op_of(what)),
        Action::Launch { app, args } => match args.is_empty() {
            true => format!("open {app}"),
            false => format!("open {app} {}", args.join(" ")),
        },
        Action::Wait { ms } => format!("wait {ms}ms"),
        Action::WaitStill { .. } => "wait until still".to_string(),
    }
}

async fn act(client: &Client, id: &str, action: Action) -> Done {
    acted(client, id, action).await.map(|_| ())
}

async fn acted(client: &Client, id: &str, action: Action) -> Result<BatchResult, String> {
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
        None => Ok(result),
    }
}

fn number(args: &[String], at: usize, what: &str) -> Result<u32, String> {
    positional(args, at, what)
        .map_err(|e| e.to_string())?
        .parse()
        .map_err(|_| format!("{what} must be a whole number of pixels"))
}

const UPLOADS: &str = "/tmp/computer/uploads";

const SETTLE_MS: u64 = 600;

const VALUED: [&str; 12] = [
    "--quality",
    "--scope",
    "--seed",
    "--tab",
    "--button",
    "--role",
    "--limit",
    "--within",
    "--or",
    "--format",
    "--timeout",
    "--quiet",
];

pub async fn browser(client: &Client, args: &[String]) -> Done {
    let rest = bare(args, &VALUED);
    let id = positional(&rest, 0, "a box").map_err(|e| e.to_string())?;
    let op = positional(
        &rest,
        1,
        "read, snapshot, find, click, fill, select, options, upload, wait, hover, \
         drag, eval, screenshot, tabs, switch, close, back, forward or reload",
    )
    .map_err(|e| e.to_string())?;

    let tab = flag(args, "--tab");
    let query = |at: usize| -> Result<String, String> {
        Ok(positional(&rest, at, "a query")
            .map_err(|e| e.to_string())?
            .to_string())
    };

    match op {
        "read" => return read_page(client, id, args).await,
        "snapshot" => return snapshot_page(client, id, args).await,
        "find" => return find_on_page(client, id, args, &rest).await,
        "eval" => return evaluate_on_page(client, id, args, &rest).await,
        "screenshot" => return capture_page(client, id, args, &rest).await,
        "tabs" => return list_tabs(client, id).await,
        "switch" => {
            let which = query(2)?;
            client
                .focus_tab(id, &which)
                .await
                .map_err(|e| e.to_string())?;
            println!("switched to {which}");
            return Ok(());
        }
        "close" => {
            let which = query(2)?;
            client
                .close_tab(id, &which)
                .await
                .map_err(|e| e.to_string())?;
            println!("closed {which}");
            return Ok(());
        }
        _ => {}
    }

    let (motion, seed) = motion(args)?;
    let button = match flag(args, "--button") {
        Some("right") => Button::Right,
        Some("middle") => Button::Middle,
        _ => Button::Left,
    };

    let what = match op {
        "click" => OnElement::Click {
            query: query(2)?,
            button,
            double: present(args, "--double"),
            motion,
            seed,
        },
        "drag" => OnElement::Drag {
            from: query(2)?,
            to: positional(&rest, 3, "a target")
                .map_err(|e| e.to_string())?
                .to_string(),
            button,
            motion,
            seed,
        },
        "fill" => OnElement::Fill {
            query: query(2)?,
            text: positional(&rest, 3, "a value")
                .map_err(|e| e.to_string())?
                .to_string(),
        },
        "select" => OnElement::Choose {
            query: query(2)?,
            option: positional(&rest, 3, "an option")
                .map_err(|e| e.to_string())?
                .to_string(),
        },
        "options" => OnElement::Options { query: query(2)? },
        "upload" => OnElement::Upload {
            query: query(2)?,
            paths: handed(client, id, args, &rest).await?,
        },
        "wait" => OnElement::WaitFor {
            // Under --quiet the words are optional: the wait is for the page to settle.
            query: match present(args, "--quiet") {
                true => query(2).unwrap_or_default(),
                false => query(2)?,
            },
            gone: present(args, "--gone"),
            within_ms: counted(args, "--within", "a number of milliseconds")?,
            or: flag(args, "--or")
                .map(|given| {
                    given
                        .split(',')
                        .map(str::trim)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            exact: present(args, "--exact"),
            quiet_ms: counted(args, "--quiet", "a number of milliseconds")?,
        },
        "hover" => OnElement::Hover {
            query: query(2)?,
            motion,
            seed,
        },
        "back" => OnElement::History { go: Where::Back },
        "forward" => OnElement::History { go: Where::Forward },
        "reload" => OnElement::History { go: Where::Reload },
        other => return Err(format!("no such op: {other}")),
    };

    let result = client
        .on_element(id, &what, SETTLE_MS, tab)
        .await
        .map_err(|e| e.to_string())?;

    for option in &result.options {
        println!("{option}");
    }
    if let Some(element) = &result.element {
        println!("{}", shown_element(element));
    }
    if let Some(matched) = &result.matched {
        println!("matched {matched:?}");
    }
    match (&result.url, result.changed) {
        (Some(url), _) if result.navigated || op == "reload" => println!("{url}"),
        (_, Some(true)) => println!("the page changed"),
        (_, Some(false)) => println!("nothing on the page changed within {SETTLE_MS} ms"),
        _ => {}
    }
    if let Some(delta) = &result.delta {
        for line in delta.lines(false) {
            println!("{line}");
        }
    }

    Ok(())
}

async fn read_page(client: &Client, id: &str, args: &[String]) -> Done {
    let format = match flag(args, "--format") {
        Some("text") => Reading::Text,
        Some("raw") => Reading::Raw,
        Some("markdown") | None => Reading::Markdown,
        Some(other) => return Err(format!("no such format: {other}")),
    };

    let read = client
        .page(
            id,
            format,
            counted(args, "--limit", "a number of characters")?,
            None,
            flag(args, "--tab"),
        )
        .await
        .map_err(|e| e.to_string())?;

    println!("{}", read.text);
    if read.truncated {
        eprintln!("(truncated)");
    }

    Ok(())
}

async fn snapshot_page(client: &Client, id: &str, args: &[String]) -> Done {
    let taken = client
        .snapshot(
            id,
            &SnapshotOptions {
                scope: flag(args, "--scope").map(str::to_string),
                limit: counted(args, "--limit", "a number of elements")?,
                tab: flag(args, "--tab").map(str::to_string),
                delta: present(args, "--delta"),
                quiet_ms: counted(args, "--quiet", "a number of milliseconds")?,
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    for line in taken.lines(present(args, "--urls")) {
        println!("{line}");
    }

    Ok(())
}

async fn find_on_page(client: &Client, id: &str, args: &[String], rest: &[String]) -> Done {
    let found = client
        .find(
            id,
            &Find {
                query: rest.get(2).cloned().unwrap_or_default(),
                limit: counted(args, "--limit", "a number of matches")?,
                scroll: Some(present(args, "--scroll")),
                exact: Some(present(args, "--exact")),
                role: flag(args, "--role").map(str::to_string),
                tab: flag(args, "--tab").map(str::to_string),
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    if found.is_empty() {
        println!("nothing in view matched");
    }
    for element in &found {
        println!("{}", shown_element(element));
    }

    Ok(())
}

async fn handed(
    client: &Client,
    id: &str,
    args: &[String],
    rest: &[String],
) -> Result<Vec<String>, String> {
    let named = rest.get(3..).unwrap_or_default();
    if named.is_empty() {
        return Err("expected a file to upload".to_string());
    }

    if present(args, "--in-box") {
        return Ok(named.to_vec());
    }

    let mut inside = Vec::with_capacity(named.len());
    for path in named {
        let bytes = std::fs::read(path).map_err(|why| format!("{path}: {why}"))?;

        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{path} has no file name"))?;
        let there = format!("{UPLOADS}/{name}");

        client
            .write_file(id, &there, &bytes)
            .await
            .map_err(|e| e.to_string())?;
        inside.push(there);
    }

    Ok(inside)
}

async fn capture_page(client: &Client, id: &str, args: &[String], rest: &[String]) -> Done {
    let format = match flag(args, "--format") {
        Some("png") => Some(Picture::Png),
        Some("jpeg" | "jpg") => Some(Picture::Jpeg),
        None => None,
        Some(other) => return Err(format!("no such format: {other}")),
    };

    let taken = client
        .page_screenshot(
            id,
            &PageShot {
                full: present(args, "--full"),
                format,
                quality: counted(args, "--quality", "a quality between 1 and 100")?,
                tab: flag(args, "--tab").map(str::to_string),
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    let image = captured_image(&taken).map_err(|e| e.to_string())?;

    // --full answers JPEG by default, so the extension follows the format.
    let out = rest.get(2).cloned().unwrap_or_else(|| {
        match taken.format {
            Picture::Jpeg => "page.jpg",
            Picture::Png => "page.png",
        }
        .to_string()
    });

    std::fs::write(&out, &image).map_err(|error| format!("{out}: {error}"))?;

    eprintln!("{} bytes → {out}", image.len());
    Ok(())
}

async fn evaluate_on_page(client: &Client, id: &str, args: &[String], rest: &[String]) -> Done {
    let what = Evaluate {
        expression: positional(rest, 2, "an expression")
            .map_err(|e| e.to_string())?
            .to_string(),
        timeout_ms: counted(args, "--timeout", "a number of milliseconds")?,
        limit: counted(args, "--limit", "a number of characters")?,
    };

    let answered = client
        .evaluate(id, &what, flag(args, "--tab"))
        .await
        .map_err(|e| e.to_string())?;

    println!("{}", answered.json);
    if answered.truncated {
        eprintln!("(truncated)");
    }

    Ok(())
}

async fn list_tabs(client: &Client, id: &str) -> Done {
    let tabs = client.tabs(id).await.map_err(|e| e.to_string())?;

    if tabs.is_empty() {
        println!("no pages are open");
    }
    for tab in &tabs {
        println!(
            "{}{} {:?} {}",
            tab.id,
            match tab.visible {
                true => " (on screen)",
                false => "",
            },
            tab.title,
            tab.url
        );
    }

    Ok(())
}

fn shown_element(element: &computer_api::Element) -> String {
    let where_ = match element.at {
        Some(at) => format!("{},{}", at.x, at.y),
        None => "out of view".to_string(),
    };

    format!(
        "{}{}{} {:?}{} {} {}x{}{}{}",
        match element.r#ref.as_deref() {
            Some(numbered) => format!("@{numbered} "),
            None => String::new(),
        },
        element.tag,
        element.kind.as_deref().unwrap_or(""),
        element.text,
        match element.role.as_deref() {
            Some(role) => format!(" role={role}"),
            None => String::new(),
        },
        where_,
        element.width,
        element.height,
        match element.states.is_empty() {
            true => String::new(),
            false => format!(" [{}]", element.states.join(" ")),
        },
        match element.selector.as_deref() {
            Some(selector) => format!("  {selector}"),
            None => String::new(),
        }
    )
}
