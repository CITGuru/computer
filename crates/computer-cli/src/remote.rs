use crate::{USAGE, bare, flag, framing, positional, present, wheel};
use computer_api::{
    Action, ActionBatch, Arrange, BatchResult, ConsoleRead, CookieSet, Evaluate, Find, ForkMode,
    ForkRequest, Held, LoadState, OnElement, OnNode, OpenIn, Out, PagePdf, PageShot, Picture,
    Reading, RecordOp, Rect, SaveState, SetCookies, Shot, SnapshotOptions, StateView, Where,
    Window, WindowOp,
};
use computer_client::{Client, captured_image, cookies_from_curl, frame_png, printed_pdf};
use computer_types::{Button, Motion, NodeQuery, Point, Search, Selection};
use std::time::Duration;

type Done = Result<(), String>;

fn wanted(what: &str) -> String {
    format!("expected {what}\n\n{USAGE}")
}

pub async fn new(client: &Client, args: &[String]) -> Done {
    let asked = crate::spec::asked(args)?;

    if flag(args, "--name").is_some() {
        return Err("a server names its own boxes: --name is for --local".to_string());
    }

    eprintln!("opening a box (the first one builds the image) …");
    let created = client
        .create(&asked.spec, &asked.placement, None)
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
                    label: None,
                },
            )
            .await
            .map_err(|error| error.to_string())?;
    }

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

pub async fn cdp(client: &Client, args: &[String]) -> Done {
    let rest = bare(args, &["--ttl"]);
    let id = positional(&rest, 0, "a box").map_err(|e| e.to_string())?;

    if present(args, "--direct") {
        let found = client.get(id).await.map_err(|e| e.to_string())?;
        let url = found
            .devtools_url
            .ok_or_else(|| "this box publishes no DevTools port".to_string())?;
        println!("{url}");
        return Ok(());
    }

    let minutes: Option<u64> = counted(args, "--ttl", "a number of minutes")?;
    let token = client
        .cdp(id, minutes.map(|minutes| minutes * 60))
        .await
        .map_err(|e| e.to_string())?;

    println!(
        "{}",
        match present(args, "--ws") {
            true => &token.ws_url,
            false => &token.url,
        }
    );
    eprintln!("  good until  {}", stamped(token.expires_at_ms));
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
            label: flag(args, "--label").map(str::to_string),
        },
    )
    .await?;

    for tab in &result.tabs {
        match &tab.label {
            Some(label) => println!("{} {label}", tab.id),
            None => println!("{}", tab.id),
        }
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

pub async fn state(client: &Client, args: &[String]) -> Done {
    match args.first().map(String::as_str) {
        Some("list") => {
            let names = client.states().await.map_err(|e| e.to_string())?;
            if names.is_empty() {
                println!("no state is saved on this server");
            }
            for name in names {
                println!("{name}");
            }
            return Ok(());
        }
        Some("rm") => {
            let name = positional(args, 1, "a state name").map_err(|e| e.to_string())?;
            client.forget_state(name).await.map_err(|e| e.to_string())?;
            eprintln!("{name} is gone");
            return Ok(());
        }
        _ => {}
    }

    let id = positional(args, 0, "a box, list or rm").map_err(|e| e.to_string())?;
    let op = positional(args, 1, "save or load").map_err(|e| e.to_string())?;
    let rest = bare(args, &["--name", "--origin"]);
    let name = flag(args, "--name").map(str::to_string);
    let file = rest.get(2);

    match (op, &name, file) {
        ("save" | "load", Some(_), Some(_)) => {
            Err("a file or --name, one of them: --name keeps it on the server".to_string())
        }
        ("save" | "load", None, None) => Err(format!(
            "{op} needs a file, or --name for a state the server keeps"
        )),
        ("save", _, _) => {
            let view = client
                .save_state(
                    id,
                    &SaveState {
                        origins: every(args, "--origin"),
                        name: name.clone(),
                        session_storage: present(args, "--session-storage"),
                        indexed_db: present(args, "--indexed-db"),
                        no_local_storage: present(args, "--no-local-storage"),
                    },
                )
                .await
                .map_err(|e| e.to_string())?;

            if let Some(file) = file {
                write_private(file, view.session_json.as_deref().unwrap_or_default())?;
            }
            println!("saved {}", said_state(&view));
            match (&view.name, file) {
                (Some(name), _) => eprintln!("the server keeps it as {name} until it restarts"),
                (None, Some(file)) => eprintln!("written to {file}; it holds live logins"),
                (None, None) => {}
            }
            for gap in &view.incomplete {
                eprintln!("not saved: {gap}");
            }
            Ok(())
        }
        ("load", _, _) => {
            let what = match file {
                Some(file) => LoadState {
                    session_json: Some(
                        std::fs::read_to_string(file)
                            .map_err(|error| format!("{file}: {error}"))?,
                    ),
                    ..LoadState::default()
                },
                None => LoadState {
                    name,
                    ..LoadState::default()
                },
            };
            let view = client
                .load_state(id, &what)
                .await
                .map_err(|e| e.to_string())?;
            println!("loaded {}", said_state(&view));
            Ok(())
        }
        (other, _, _) => Err(format!("no such op: {other}")),
    }
}

fn said_state(view: &StateView) -> String {
    format!(
        "{} cookies and the storage of {} origin(s), for {}",
        view.cookies,
        view.stored,
        view.origins.join(", ")
    )
}

fn every(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == name)
        .flat_map(|pair| pair[1].split(','))
        .map(str::trim)
        .filter(|one| !one.is_empty())
        .map(str::to_string)
        .collect()
}

fn write_private(path: &str, text: &str) -> Done {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        if std::path::Path::new(path).exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("{path}: {error}"))?;
        }
    }

    options
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| format!("{path}: {error}"))
}

pub async fn cookies(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let rest = bare(
        args,
        &["--url", "--curl", "--domain", "--path", "--expires"],
    );
    let url = flag(args, "--url");

    match rest.get(1).map(String::as_str) {
        None | Some("list") => {
            let cookies = client.cookies(id, url).await.map_err(|e| e.to_string())?;
            if cookies.is_empty() {
                println!("no cookies");
            }
            for cookie in &cookies {
                let mut said = format!(
                    "{}={}  {}{}",
                    cookie.name, cookie.value, cookie.domain, cookie.path
                );
                if cookie.secure {
                    said.push_str("  secure");
                }
                if cookie.http_only {
                    said.push_str("  http-only");
                }
                if let Some(expires) = cookie.expires {
                    said.push_str(&format!("  until {}", stamped((expires * 1000.0) as u64)));
                }
                println!("{said}");
            }
            Ok(())
        }
        Some("set") => {
            let (url, mut cookies) = match flag(args, "--curl") {
                Some(command) => {
                    let (url, cookies) = cookies_from_curl(command)?;
                    (Some(url), cookies)
                }
                None => {
                    let cookies = rest[2..]
                        .iter()
                        .map(|pair| {
                            pair.split_once('=')
                                .map(|(name, value)| CookieSet {
                                    name: name.to_string(),
                                    value: value.to_string(),
                                    ..CookieSet::default()
                                })
                                .ok_or_else(|| format!("{pair} is not NAME=VALUE"))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    (url.map(str::to_string), cookies)
                }
            };
            if cookies.is_empty() {
                return Err("set takes NAME=VALUE, one or more, or --curl".to_string());
            }

            let expires =
                counted::<u64>(args, "--expires", "a number of seconds")?.map(|seconds| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|now| now.as_secs_f64())
                        .unwrap_or_default()
                        + seconds as f64
                });
            for cookie in &mut cookies {
                cookie.domain = flag(args, "--domain").map(str::to_string);
                cookie.path = flag(args, "--path").map(str::to_string);
                cookie.expires = expires;
                cookie.http_only = present(args, "--http-only");
                if present(args, "--secure") {
                    cookie.secure = Some(true);
                }
            }

            let set = client
                .set_cookies(id, &SetCookies { url, cookies })
                .await
                .map_err(|e| e.to_string())?;
            for cookie in &set {
                println!("set {} for {}{}", cookie.name, cookie.domain, cookie.path);
            }
            Ok(())
        }
        Some("clear") => {
            if url.is_none() && !present(args, "--all") {
                return Err(
                    "clear takes --url for one site's cookies, or --all for every one".to_string(),
                );
            }
            let cleared = client
                .clear_cookies(id, url)
                .await
                .map_err(|e| e.to_string())?;
            println!("cleared {} cookies", cleared.cleared);
            Ok(())
        }
        Some(other) => Err(format!("no such op: {other}")),
    }
}

pub async fn mouse(client: &Client, args: &[String]) -> Done {
    let id = positional(args, 0, "a box").map_err(|e| e.to_string())?;
    let op = positional(args, 1, "move, click, down, up, drag, path, scroll or at")
        .map_err(|e| e.to_string())?;

    let mut rest = args.to_vec();
    rest.remove(1);

    match op {
        "click" => click(client, &rest).await,
        "scroll" => scroll(client, &rest).await,
        "move" => {
            let to = point(&rest, 1)?;
            let (motion, seed) = motion(args)?;
            act(
                client,
                id,
                Action::Move {
                    to,
                    motion,
                    seed,
                    pause_ms: None,
                },
            )
            .await
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
        "path" => {
            let (through, button) = stroke(&bare(&rest, &["--held", "--seed"]))?;
            let (motion, seed) = motion(args)?;
            act(
                client,
                id,
                Action::Path {
                    through,
                    button,
                    held: modifiers(&rest)?,
                    motion,
                    seed,
                },
            )
            .await
        }
        "down" => {
            let (at, button) = spot(&rest)?;
            let seconds: Option<u64> = counted(args, "--hold", "a number of seconds")?;
            let (motion, seed) = motion(args)?;

            let result = acted(
                client,
                id,
                Action::MouseDown {
                    at,
                    button,
                    motion,
                    seed,
                    hold_ms: Some(seconds.map_or(HOLD_MS, |seconds| seconds * 1000)),
                },
            )
            .await?;

            for held in &result.holding {
                eprintln!(
                    "the {} button is down until {}, or until: computer mouse {id} up",
                    format!("{:?}", held.button).to_lowercase(),
                    stamped(held.until_ms)
                );
            }
            Ok(())
        }
        "up" => {
            let (at, button) = spot(&rest)?;
            act(client, id, Action::MouseUp { at, button }).await
        }
        "at" => {
            let at = client.cursor(id, 0).await.map_err(|e| e.to_string())?;
            println!("{},{}", at.x, at.y);
            Ok(())
        }
        other => Err(format!("no such op: {other}")),
    }
}

const HOLD_MS: u64 = 10_000;

pub fn stroke(named: &[String]) -> Result<(Vec<Point>, Button), String> {
    let words = named.get(1..).unwrap_or_default();
    let (numbers, button) = match words.split_last() {
        Some((last, before)) if last.parse::<u32>().is_err() => match last.as_str() {
            "left" => (before, Button::Left),
            "right" => (before, Button::Right),
            "middle" => (before, Button::Middle),
            other => return Err(format!("{other} is not a coordinate or a button")),
        },
        _ => (words, Button::Left),
    };

    if numbers.len() % 2 != 0 {
        return Err(format!(
            "{} is half a point: a path is x y x y …",
            numbers.last().map(String::as_str).unwrap_or_default()
        ));
    }
    let through = numbers
        .chunks(2)
        .map(|pair| match (pair[0].parse(), pair[1].parse()) {
            (Ok(x), Ok(y)) => Ok(Point { x, y }),
            _ => Err(format!(
                "{} {} is not a point in whole pixels",
                pair[0], pair[1]
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;

    if through.len() < 2 {
        return Err("a path needs two points at least: x y x y …".to_string());
    }
    Ok((through, button))
}

fn spot(rest: &[String]) -> Result<(Option<Point>, Button), String> {
    let named = bare(rest, &["--hold", "--seed"]);

    match named.get(1).is_some_and(|word| word.parse::<u32>().is_ok()) {
        true => Ok((Some(point(&named, 1)?), button(named.get(3)))),
        false => Ok((None, button(named.get(1)))),
    }
}

pub async fn keyboard(client: &Client, args: &[String]) -> Done {
    let op = positional(args, 1, "type, press, down or up").map_err(|e| e.to_string())?;

    let mut rest = args.to_vec();
    rest.remove(1);

    match op {
        "type" => type_text(client, &rest).await,
        "press" => press(client, &rest).await,
        "down" => {
            let id = positional(&rest, 0, "a box").map_err(|e| e.to_string())?;
            let key = one_key(&rest)?;
            let seconds: Option<u64> = counted(args, "--hold", "a number of seconds")?;

            let result = acted(
                client,
                id,
                Action::KeyDown {
                    key,
                    hold_ms: Some(seconds.map_or(HOLD_MS, |seconds| seconds * 1000)),
                },
            )
            .await?;

            for held in &result.holding_keys {
                eprintln!(
                    "{} is down until {}, or until: computer keyboard {id} up {}",
                    held.key,
                    stamped(held.until_ms),
                    held.key
                );
            }
            Ok(())
        }
        "up" => {
            let id = positional(&rest, 0, "a box").map_err(|e| e.to_string())?;
            let key = one_key(&rest)?;
            act(client, id, Action::KeyUp { key }).await
        }
        other => Err(format!("no such op: {other}")),
    }
}

fn one_key(rest: &[String]) -> Result<String, String> {
    let named = bare(rest, &["--hold"]);
    match named.get(1..).unwrap_or_default() {
        [key] => Ok(key.clone()),
        [] => Err("a key is needed: shift, space, a".to_string()),
        several => Err(format!(
            "{} is {} keys: down and up take one, and press takes several",
            several.join(" "),
            several.len()
        )),
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
        O::Choose { drop, .. } => match drop {
            true => "deselect",
            false => "select",
        },
        O::Upload { .. } => "upload",
        O::WaitFor { .. } => "wait",
        O::Hover { .. } => "hover",
        O::Highlight { .. } => "highlight",
        O::Focus { .. } => "focus",
        O::Check { on, .. } => match on {
            true => "check",
            false => "uncheck",
        },
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

/// The actions as JSON, from a file or from stdin: a shell line cannot carry
/// a list of points, and the same JSON is what `trace` writes out.
pub async fn batch(client: &Client, args: &[String]) -> Done {
    let rest = bare(args, &VALUED);
    let id = positional(&rest, 0, "a box").map_err(|e| e.to_string())?;

    let raw = match rest.get(1).map(String::as_str) {
        Some("-") | None => {
            let mut read = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut read)
                .map_err(|error| format!("could not read the batch: {error}"))?;
            read
        }
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| format!("could not read {path}: {error}"))?,
    };

    let actions: Vec<Action> = match serde_json::from_str::<serde_json::Value>(raw.trim()) {
        Ok(serde_json::Value::Array(_)) => serde_json::from_str(raw.trim()),
        _ => serde_json::from_str::<ActionBatch>(raw.trim()).map(|batch| batch.actions),
    }
    .map_err(|error| format!("the batch would not parse: {error}"))?;

    if actions.is_empty() {
        return Err("the batch has no actions".to_string());
    }

    let told = actions.iter().map(name_of).collect::<Vec<_>>();
    let result = client
        .act(
            id,
            0,
            &ActionBatch {
                actions,
                settle_ms: flag(args, "--settle").and_then(|ms| ms.parse().ok()),
                want: Vec::new(),
                have_frame: None,
                keep_going: present(args, "--keep-going"),
            },
            None,
        )
        .await
        .map_err(|e| e.to_string())?;

    for (step, ran) in told.iter().zip(&result.results) {
        match (&ran.ok, &ran.out) {
            (false, _) => println!(
                "{:>3}  {step}: {}",
                ran.index,
                ran.error
                    .as_ref()
                    .map(|error| error.message.as_str())
                    .unwrap_or("refused")
            ),
            (true, None) => println!("{:>3}  {step}", ran.index),
            (true, Some(out)) => {
                println!("{:>3}  {step}", ran.index);
                for line in out_lines(out) {
                    println!("       {line}");
                }
            }
        }
    }

    for button in &result.released {
        eprintln!(
            "the {} button was still down when the batch ended, and was let go",
            format!("{button:?}").to_lowercase()
        );
    }
    for key in &result.released_keys {
        eprintln!("{key} was still down when the batch ended, and was let go");
    }

    let refused = result.results.iter().filter(|one| !one.ok).count();

    match (result.stopped_at, refused) {
        (Some(at), _) => Err(format!("{} step(s) did not run", told.len() - at - 1)),
        (None, 0) => Ok(()),
        (None, refused) => Err(format!("{refused} step(s) were refused")),
    }
}

fn out_lines(out: &Out) -> Vec<String> {
    match out {
        Out::Value(value) => vec![value.json.clone()],
        Out::Elements(found) => found.iter().map(shown_element).collect(),
        Out::Snapshot(taken) => taken
            .elements
            .iter()
            .map(|one| one.brief(false))
            .chain([format!("{} of {} shown", taken.elements.len(), taken.total)])
            .collect(),
        Out::Text(read) => vec![
            read.title.clone(),
            format!("{} characters", read.text.len()),
        ],
        Out::Picture(shot) => vec![format!("{} bytes", shot.bytes)],
        Out::Frame(frame) => vec![format!("frame {}", frame.hash)],
        Out::At(at) => vec![format!("{},{}", at.x, at.y)],
        Out::Windows(windows) => windows
            .iter()
            .map(|one| format!("{} {:?}", one.id, one.title))
            .collect(),
        Out::Window(window) => match window {
            Some(window) => vec![format!("{} {:?}", window.id, window.title)],
            None => vec!["no window".to_string()],
        },
        Out::Tabs(tabs) => tabs
            .iter()
            .map(|tab| match &tab.label {
                Some(label) => format!("{} [{label}] {:?} {}", tab.id, tab.title, tab.url),
                None => format!("{} {:?} {}", tab.id, tab.title, tab.url),
            })
            .collect(),
        Out::Ran(ran) => {
            let mut said = vec![format!("exit {}", ran.code)];
            said.extend(ran.stdout.lines().map(str::to_string));
            said.extend(ran.stderr.lines().map(str::to_string));
            said
        }
        Out::File(file) => vec![format!("{} read", file.path)],
        Out::Listing(listing) => match listing.entries.is_empty() {
            true => vec![format!("{} is empty", listing.path)],
            false => listing
                .entries
                .iter()
                .map(|entry| match entry.dir {
                    true => format!("{}/", entry.name),
                    false => format!("{} ({} bytes)", entry.name, entry.bytes),
                })
                .collect(),
        },
        Out::Found(found) => found
            .matches
            .iter()
            .map(|one| format!("{}:{}:{}", one.path, one.line, one.text))
            .chain(found.cut.then(|| "(cut here)".to_string()))
            .collect(),
        Out::Globbed(found) => found
            .paths
            .iter()
            .cloned()
            .chain(found.cut.then(|| "(cut here)".to_string()))
            .collect(),
        Out::Clipboard(held) => vec![held.text.clone()],
        Out::Recording(state) => vec![match (&state.recording, &state.path) {
            (true, Some(path)) => format!("recording to {path}"),
            (true, None) => "recording".to_string(),
            (false, Some(path)) => format!("written to {path}"),
            (false, None) => "not recording".to_string(),
        }],
        Out::Apps(names) => names.clone(),
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
        Action::MouseDown { at, button, .. } => match at {
            Some(at) => format!("{button:?} button down at {},{}", at.x, at.y).to_lowercase(),
            None => format!("{button:?} button down").to_lowercase(),
        },
        Action::MouseUp { at, button } => match at {
            Some(at) => format!("{button:?} button up at {},{}", at.x, at.y).to_lowercase(),
            None => format!("{button:?} button up").to_lowercase(),
        },
        Action::Dialog { accept: true, .. } => "accept the dialog".to_string(),
        Action::Dialog { accept: false, .. } => "dismiss the dialog".to_string(),
        Action::KeyDown { key, .. } => format!("{key} down"),
        Action::KeyUp { key } => format!("{key} up"),
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
        Action::Path { through, .. } => format!("draw through {} points", through.len()),
        Action::Evaluate { what } => format!("evaluate {:?}", what.expression),
        Action::Look { what } => format!("find {:?}", what.query),
        Action::Snapshot { .. } => "snapshot the page".to_string(),
        Action::Read { .. } => "read the page".to_string(),
        Action::PageShot { .. } => "capture the page".to_string(),
        Action::Capture { .. } => "capture the screen".to_string(),
        Action::Cursor => "where the pointer is".to_string(),
        Action::Windows { active } => match active {
            true => "the active window".to_string(),
            false => "the windows".to_string(),
        },
        Action::AwaitWindow { what } => format!("wait for a {} window", what.class),
        Action::OnWindow { window, what } => format!("window {window} {}", window_op_of(what)),
        Action::Tabs => "the tabs".to_string(),
        Action::OnTab { tab, close } => match close {
            true => format!("close tab {tab}"),
            false => format!("raise tab {tab}"),
        },
        Action::Exec { what } => format!("run {}", what.argv.join(" ")),
        Action::ReadFile { path } => format!("read {path}"),
        Action::WriteFile { what } => format!("write {}", what.path),
        Action::ListFiles { path } => format!("list {path}"),
        Action::Grep { what } => format!("grep {:?} in {}", what.pattern, what.path),
        Action::Glob { what } => format!("glob {:?}", what.pattern),
        Action::Clipboard { selection, text } => match text {
            Some(_) => format!("set the {} selection", selection.name()),
            None => format!("read the {} selection", selection.name()),
        },
        Action::Record { what } => match what {
            RecordOp::Start { .. } => "start recording".to_string(),
            RecordOp::Stop => "stop recording".to_string(),
            RecordOp::Status => "is it recording".to_string(),
        },
        Action::Apps => "the apps".to_string(),
    }
}

fn window_op_of(what: &WindowOp) -> &'static str {
    match what {
        WindowOp::Focus => "focus",
        WindowOp::Close => "close",
        WindowOp::Arrange { .. } => "arrange",
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
                keep_going: false,
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

const VALUED: [&str; 14] = [
    "--for",
    "--fn",
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
        "read, snapshot, find, click, fill, focus, check, uncheck, select, \
         deselect, options, upload, wait, hover, highlight, drag, eval, screenshot, pdf, tabs, \
         switch, close, back, forward, reload, dialog, console or errors",
    )
    .map_err(|e| e.to_string())?;

    let tab = flag(args, "--tab");
    let query = |at: usize| -> Result<String, String> {
        Ok(positional(&rest, at, "a query")
            .map_err(|e| e.to_string())?
            .to_string())
    };

    match op {
        "read" | "snapshot" | "find" | "eval" | "console" | "errors" => {
            let bounded =
                present(args, "--content-boundaries") || computer_mcp::boundaries::asked();
            let (opening, closing) = match bounded {
                true => computer_mcp::boundaries::markers(),
                false => (String::new(), String::new()),
            };

            if bounded {
                println!("{opening}");
            }
            let done = match op {
                "read" => read_page(client, id, args).await,
                "snapshot" => snapshot_page(client, id, args).await,
                "find" => find_on_page(client, id, args, &rest).await,
                "console" | "errors" => {
                    read_console(
                        client,
                        id,
                        args,
                        op == "errors" || present(args, "--errors"),
                    )
                    .await
                }
                _ => evaluate_on_page(client, id, args, &rest).await,
            };
            if bounded {
                println!("{closing}");
            }
            return done;
        }
        "screenshot" => return capture_page(client, id, args, &rest).await,
        "pdf" => {
            let printed = client
                .page_pdf(
                    id,
                    &PagePdf {
                        landscape: present(args, "--landscape"),
                        no_background: present(args, "--no-background"),
                        tab: tab.map(str::to_string),
                        path: None,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;

            let document = printed_pdf(&printed).map_err(|e| e.to_string())?;
            let out = rest
                .get(2)
                .cloned()
                .unwrap_or_else(|| "page.pdf".to_string());
            std::fs::write(&out, &document).map_err(|error| format!("{out}: {error}"))?;

            eprintln!("{} bytes → {out}", document.len());
            return Ok(());
        }
        "tabs" => return list_tabs(client, id).await,
        "dialog" => {
            let accept =
                match positional(&rest, 2, "accept or dismiss").map_err(|e| e.to_string())? {
                    "accept" => true,
                    "dismiss" => false,
                    other => return Err(format!("{other} is not accept or dismiss")),
                };
            let text = rest
                .get(3..)
                .filter(|words| !words.is_empty())
                .map(|words| words.join(" "));
            if let (false, Some(_)) = (accept, &text) {
                return Err(
                    "text goes into a prompt that is accepted, not one dismissed".to_string(),
                );
            }
            return act(client, id, Action::Dialog { accept, text }).await;
        }
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
            new_tab: present(args, "--new-tab"),
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
            options: match rest.get(3..) {
                Some([]) | None => return Err("expected an option".to_string()),
                Some(named) => named.to_vec(),
            },
            drop: false,
        },
        "deselect" => OnElement::Choose {
            query: query(2)?,
            options: rest.get(3..).unwrap_or_default().to_vec(),
            drop: true,
        },
        "options" => OnElement::Options { query: query(2)? },
        "focus" => OnElement::Focus { query: query(2)? },
        "check" => OnElement::Check {
            query: query(2)?,
            on: true,
        },
        "uncheck" => OnElement::Check {
            query: query(2)?,
            on: false,
        },
        "upload" => OnElement::Upload {
            query: query(2)?,
            paths: handed(client, id, args, &rest).await?,
        },
        "wait" => OnElement::WaitFor {
            query: match present(args, "--quiet")
                || present(args, "--load")
                || present(args, "--fn")
            {
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
            enabled: present(args, "--enabled"),
            load: present(args, "--load"),
            until: flag(args, "--fn").map(str::to_string),
        },
        "hover" => OnElement::Hover {
            query: query(2)?,
            motion,
            seed,
        },
        "highlight" => OnElement::Highlight {
            query: query(2)?,
            ms: counted::<u64>(args, "--for", "a number of seconds")?
                .map(|seconds| seconds.saturating_mul(1000)),
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

    for said in &result.alerts {
        println!("an alert said {said:?}, and was accepted");
    }
    if let Some(tab) = &result.tab {
        println!("opened tab {} {:?}, now on screen", tab.id, tab.title);
    }
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

async fn read_console(client: &Client, id: &str, args: &[String], errors: bool) -> Done {
    let view = client
        .console(
            id,
            &ConsoleRead {
                errors,
                clear: present(args, "--clear"),
                limit: counted(args, "--limit", "a number of lines")?,
                tab: flag(args, "--tab").map(str::to_string),
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    if view.earlier > 0 {
        println!("({} earlier lines left out)", view.earlier);
    }
    if view.lines.is_empty() {
        println!(
            "{}",
            match errors {
                true => "no errors since the page loaded",
                false => "nothing logged since the page loaded",
            }
        );
    }
    for line in &view.lines {
        match &line.at {
            Some(at) => println!("{:<9} {}  ({at})", line.level, line.text),
            None => println!("{:<9} {}", line.level, line.text),
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
                annotate: present(args, "--annotate"),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(drawn) = taken.annotated {
        eprintln!("{drawn} controls numbered as the last snapshot numbered them");
    }

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
            "{}{}{} {:?} {}",
            tab.id,
            match &tab.label {
                Some(label) => format!(" [{label}]"),
                None => String::new(),
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(listed: &[&str]) -> Vec<String> {
        listed.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn test_a_button_goes_down_where_the_pointer_is_unless_given_a_point() {
        assert_eq!(spot(&args(&["mybox"])), Ok((None, Button::Left)));
        assert_eq!(spot(&args(&["mybox", "right"])), Ok((None, Button::Right)));
        assert_eq!(
            spot(&args(&["mybox", "400", "300"])),
            Ok((Some(Point { x: 400, y: 300 }), Button::Left))
        );
        assert_eq!(
            spot(&args(&["mybox", "400", "300", "middle", "--hold", "5"])),
            Ok((Some(Point { x: 400, y: 300 }), Button::Middle))
        );
        assert_eq!(
            spot(&args(&["mybox", "--hold", "5", "400", "300"])),
            Ok((Some(Point { x: 400, y: 300 }), Button::Left)),
            "the seconds a button is held are not a coordinate"
        );
        assert!(
            spot(&args(&["mybox", "400"])).is_err(),
            "half a point is a mistake, and pressing where the pointer is would hide it"
        );
    }

    #[test]
    fn test_a_path_is_points_in_pairs_and_then_a_button() {
        let line = |listed: &[&str]| stroke(&bare(&args(listed), &["--held", "--seed"]));

        assert_eq!(
            line(&["mybox", "10", "20", "30", "40", "50", "60"]),
            Ok((
                vec![
                    Point { x: 10, y: 20 },
                    Point { x: 30, y: 40 },
                    Point { x: 50, y: 60 }
                ],
                Button::Left
            ))
        );
        assert_eq!(
            line(&[
                "mybox", "10", "20", "30", "40", "right", "--held", "shift", "--seed", "9"
            ]),
            Ok((
                vec![Point { x: 10, y: 20 }, Point { x: 30, y: 40 }],
                Button::Right
            )),
            "the seed of a glide is not a coordinate, and the button comes last"
        );

        let half = line(&["mybox", "10", "20", "30"]).expect_err("an odd count");
        assert!(half.contains("30 is half a point"), "{half}");
        assert!(
            line(&["mybox", "10", "20"]).is_err(),
            "one point is a click, and a path that drew nothing would say it worked"
        );
        let wrong = line(&["mybox", "10", "20", "30", "forty"]).expect_err("a word");
        assert!(
            wrong.contains("forty is not a coordinate or a button"),
            "{wrong}"
        );
        assert!(line(&["mybox", "10", "-5", "30", "40"]).is_err());
    }
}
