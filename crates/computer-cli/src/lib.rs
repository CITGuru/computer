//! `computer` — a desktop in a box, from the command line.

mod daemon;
mod embed;
mod local;
mod remote;

pub use daemon::serve as daemon;

use computer_client::Client;

pub const USAGE: &str = "\
computer — a desktop in a box

  up [--size WxH] [--url URL] [--ttl MINUTES] [--wide-fonts]
     [--accessibility] [--video] [--wayland]
                              --accessibility reads native windows by widget
                              name, for the widget command below. --video puts
                              ffmpeg in the box, for record. --wayland runs
                              sway in place of X11. none can be turned on
                              afterwards
                              open a box and print where to watch it
  ls                          boxes that are running
  box <box>                   everything the server knows about one
  pause <box>                 freeze it: it keeps its memory and its ports and
                              costs no processor until resumed
  resume <box>                wake it, as the box it was
  stop <box>                  end everything in it, keeping its files. cheaper
                              than pause, which holds the memory
  start <box>                 start a stopped box. the desktop starts again,
                              so nothing that was open is open, and the viewer
                              URL is a new one
  screenshot <box> [file.png] [--window ID | --at X,Y --size WxH]
             [--scale PERCENT] [--pointer] [--tab ID]
                              capture the screen, one window, or a rectangle
                              of it. --scale answers smaller, which is most of
                              a megabyte an agent would otherwise pay per step.
                              --pointer draws the pointer, which a capture
                              leaves out. --tab raises that page first, and
                              still captures the desktop around it
  open <box> <url> [--target blank|current]
                              open a URL in the box's browser, and say which tab
                              it landed in. a new tab unless told `current`
  app <box> <name> [args…]    open an application, and wait until it has drawn
  apps                        the application names a box can be given
  window <box> list           what is on the screen
  window <box> active         which window the keyboard reaches
  window <box> wait <class> [--within SECONDS]
                              wait for a window to appear and hold still
  window <box> <id> focus | close
                              raise a window, or ask it to go away
  window <box> <id> move <x> <y> | size <w> <h> | max | min | restore
                              move a window, resize it, or change its state
  widget <box> find <query> [--role R] [--exact] [--app NAME] [--limit N]
  widget <box> tree [--app NAME] [--depth N]
  widget <box> press <query> [--action NAME]
  widget <box> fill <query> <value>
  widget <box> focus <query>
                              drive a native window by the names of its widgets
                              rather than by its pixels. a query matches the
                              label beside a field as well as the widget's own
                              name, and press sends no pointer event at all.
                              needs a box built with accessibility
  browser <box> read [--format text|raw] [--limit N] [--tab ID]
  browser <box> find [<query>] [--role R] [--exact] [--scroll] [--limit N]
  browser <box> click <query> [--double] [--button right] [--tab ID]
  browser <box> fill <query> <value>
  browser <box> select <query> <option> | options <query>
  browser <box> upload <query> <file…> [--in-box]
                              hand files to a file input. a path is one out
                              here, read and written into the box first;
                              --in-box names paths already there
  browser <box> wait <query> [--gone] [--or TEXT,TEXT] [--within MS]
  browser <box> hover <query>
  browser <box> eval <expression> [--timeout MS] [--limit N]
  browser <box> screenshot [file] [--full] [--format png|jpeg] [--quality N]
  browser <box> tabs | switch <tab> | close <tab>
  browser <box> back | forward | reload
                              drive the web page by what is on it rather than
                              by its pixels: a query is words, a name, an id or
                              a selector, and find answers with one that names
                              exactly the element it found. eval runs javascript
                              in the page, which a box is isolated enough for.
                              screenshot is what the browser drew, with no
                              window frame and no address bar; --full reaches
                              past the viewport to the whole scrollable page,
                              which is as tall as the page is and so answers
                              jpeg unless told otherwise

  keyboard <box> type <text>  type into the focused window
  keyboard <box> key <chord>  send a chord, such as ctrl+l or cmd+enter

  mouse <box> move <x> <y>    put the pointer somewhere, in device pixels
  mouse <box> click <x> <y> [button] [--double] [--held shift,ctrl]
                              --double clicks twice, as a page counts it.
                              --held keeps modifiers down around it, which
                              pressing a key first cannot: that press ends
                              with its own command
  mouse <box> drag <x> <y> <x> <y> [button] [--held shift,ctrl]
                              press at the first point, release at the second
  mouse <box> scroll [<x> <y>] up|down|left|right [notches]
  mouse <box> scroll <x> <y> <dy> [dx]
                              turn the wheel, in notches. a direction goes 3
                              unless told otherwise; the signed form takes both
                              axes at once, positive being down and right.
                              without a point, the middle of the screen
  mouse <box> at              where the pointer is

  record <box> start [--fps N]
  record <box> stop [file.mp4]
  record <box> status         record the screen to a file. ffmpeg writes it
                              inside the box, so the frames never cross the
                              wire; stop brings the file out. needs a box
                              opened with --video
  still <box> [--settle MS] [--within MS]
                              wait until the screen stops changing
  clip <box> [text] [--primary]
                              read a selection, or set it
  takeover <box>              open the input viewer and print its URL
  release <box>               close it and take the screen back
  exec <box> -- <command…>    run a command inside the box
  rm <box>                    take the box away
  fork <box> [--up-to SEQ]    build another by doing again what was done
  trace <box> [--after SEQ]   what has been done to it, and by whom
  sweep                       remove every box whose deadline has passed
  mcp [--stdio]               serve the Model Context Protocol, for an agent

  --server URL                a server to use, over $COMPUTER_SERVER_URL
  --local                     drive the box from here, with no server at all.
                              fork and trace need one, and say so.

The first `up` builds the image, which takes a few minutes. Every one after it
starts in seconds.

Without a server these commands start one for their own length. Run `computerd`
to keep one: it holds the trace a fork reads, sweeps boxes past their deadline,
and can be reached from off this host.
";

/// Runs one command and answers with what to print if it failed.
pub async fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let local = take(&mut args, "--local");
    let named = value(&mut args, "--server").or_else(|| std::env::var("COMPUTER_SERVER_URL").ok());

    let command = args
        .first()
        .map(String::as_str)
        .unwrap_or("help")
        .to_string();
    let rest = args.get(1..).unwrap_or_default().to_vec();

    if matches!(command.as_str(), "help" | "--help" | "-h") {
        print!("{USAGE}");
        return Ok(());
    }

    let outcome = match command.as_str() {
        "mcp" => mcp(named, &rest).await,
        _ => match local {
            true => here(&command, &rest).await,
            false => match connect(named).await {
                Ok(client) => there(&client, &command, &rest).await,
                Err(why) => Err(why),
            },
        },
    };

    embed::flush().await;

    outcome
}

/// Serve the Model Context Protocol on this process's own streams.
async fn mcp(named: Option<String>, args: &[String]) -> Result<(), String> {
    if let Some(odd) = args.iter().find(|arg| *arg != "--stdio") {
        return Err(format!("unknown option for mcp: {odd}\n\n{USAGE}"));
    }

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "computer_mcp=info,computer=info".into()),
        )
        .init();

    let client = connect(named).await?;
    tracing::info!(server = %client.base(), "computer mcp is serving boxes from");

    computer_mcp::stdio(&client)
        .await
        .map_err(|error| error.to_string())
}

async fn connect(named: Option<String>) -> Result<Client, String> {
    let token = std::env::var("COMPUTER_SERVER_TOKEN").ok();

    if let Some(url) = named {
        return Ok(carrying(url, token));
    }

    let listening = carrying("http://127.0.0.1:8080".to_string(), token.clone());
    if listening.health().await.is_ok() {
        return Ok(listening);
    }

    Ok(carrying(embed::start().await?, token))
}

fn carrying(base: String, token: Option<String>) -> Client {
    match token {
        Some(token) => Client::new(base).with_token(token),
        None => Client::new(base),
    }
}

async fn there(client: &Client, command: &str, args: &[String]) -> Result<(), String> {
    match command {
        "up" => remote::up(client, args).await,
        "ls" => remote::list(client).await,
        "box" => remote::describe(client, args).await,
        "stop" => remote::stop(client, args).await,
        "start" => remote::start(client, args).await,
        "pause" => remote::pause(client, args).await,
        "resume" => remote::resume(client, args).await,
        "screenshot" => remote::screenshot(client, args).await,
        "open" => remote::open(client, args).await,
        "app" => remote::app(client, args).await,
        "apps" => remote::apps(client).await,
        "window" => remote::window(client, args).await,
        "browser" => remote::browser(client, args).await,
        "widget" => remote::widget(client, args).await,
        "mouse" => remote::mouse(client, args).await,
        "keyboard" => remote::keyboard(client, args).await,
        "record" => remote::record(client, args).await,
        "still" => remote::still(client, args).await,
        "clip" => remote::clip(client, args).await,
        "takeover" => remote::takeover(client, args).await,
        "release" => remote::release(client, args).await,
        "exec" => remote::exec(client, args).await,
        "rm" => remote::remove(client, args).await,
        "fork" => remote::fork(client, args).await,
        "trace" => remote::trace(client, args).await,
        "sweep" => local::sweep().await.map_err(|error| error.to_string()),
        other => Err(format!("unknown command: {other}\n\n{USAGE}")),
    }
}

async fn here(command: &str, args: &[String]) -> Result<(), String> {
    let outcome = match command {
        "up" => local::up(args).await,
        "ls" => local::list().await,
        "screenshot" => local::screenshot(args).await,
        "open" => local::open(args).await,
        "app" | "apps" | "window" | "widget" | "browser" | "record" | "box" | "pause"
        | "resume" | "stop" | "start" => Err(computer::Error::invalid(format!(
            "`{command}` needs a server; drop --local"
        ))),
        "mouse" => local::mouse(args).await,
        "keyboard" => local::keyboard(args).await,
        "still" => local::still(args).await,
        "clip" => local::clip(args).await,
        "takeover" => local::takeover(args).await,
        "release" => local::release(args).await,
        "exec" => local::exec(args).await,
        "rm" => local::remove(args).await,
        "sweep" => local::sweep().await,
        "fork" | "trace" => {
            return Err(format!(
                "{command} needs a server to remember what was done, and --local has none. \
                 Run it without --local."
            ));
        }
        other => return Err(format!("unknown command: {other}\n\n{USAGE}")),
    };

    outcome.map_err(|error| error.to_string())
}

/// Take a flag out, so what is left is positional.
fn mine(args: &[String]) -> usize {
    args.iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len())
}

fn take(args: &mut Vec<String>, name: &str) -> bool {
    match args[..mine(args)].iter().position(|arg| arg == name) {
        Some(at) => {
            args.remove(at);
            true
        }
        None => false,
    }
}

fn value(args: &mut Vec<String>, name: &str) -> Option<String> {
    let at = args[..mine(args)].iter().position(|arg| arg == name)?;
    if at + 1 >= args.len() {
        return None;
    }

    args.remove(at);
    Some(args.remove(at))
}

/// A flag's value, where flags are `--name value`.
pub fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

pub fn present(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

pub fn framing(args: &[String]) -> computer::Result<computer::Shot> {
    let at = pair(flag(args, "--at"), ',', "--at takes X,Y")?;
    let size = pair(flag(args, "--size"), 'x', "--size takes WIDTHxHEIGHT")?;
    let window = flag(args, "--window");

    let of = match (window, at, size) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err(computer::Error::denied(
                "a capture is of a window or of a region, not both",
            ));
        }
        (Some(id), None, None) => computer::Of::Window(id.to_string()),
        (None, Some(at), Some((width, height))) => computer::Of::Region(computer::Rect::new(
            computer::Point::new(at.0, at.1),
            width,
            height,
        )),
        (None, None, None) => computer::Of::Screen,
        _ => {
            return Err(computer::Error::denied(
                "a region takes --at and --size together",
            ));
        }
    };

    let scale = match flag(args, "--scale") {
        None => None,
        Some(percent) => Some(percent.parse().map_err(|_| {
            computer::Error::denied(format!("--scale takes a percentage: {percent}"))
        })?),
    };

    Ok(computer::Shot {
        of,
        scale,
        pointer: present(args, "--pointer"),
    })
}

fn pair(given: Option<&str>, between: char, wanted: &str) -> computer::Result<Option<(u32, u32)>> {
    let Some(given) = given else {
        return Ok(None);
    };

    given
        .split_once(between)
        .and_then(|(one, two)| Some((one.trim().parse().ok()?, two.trim().parse().ok()?)))
        .map(Some)
        .ok_or_else(|| computer::Error::denied(format!("{wanted}: {given}")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wheel {
    pub at: Option<(u32, u32)>,
    pub dx: i32,
    pub dy: i32,
}

fn heading(word: &str, notches: i32) -> Option<(i32, i32)> {
    match word {
        "up" => Some((0, -notches)),
        "down" => Some((0, notches)),
        "left" => Some((-notches, 0)),
        "right" => Some((notches, 0)),
        _ => None,
    }
}

/// What was scrolled, however it was spelled.
pub fn wheel(args: &[String]) -> computer::Result<Wheel> {
    let word = |at: usize| args.get(at).map(String::as_str).unwrap_or_default();
    let count = |at: usize| -> computer::Result<i32> {
        match args.get(at) {
            None => Ok(NOTCHES),
            Some(given) => given
                .parse()
                .map_err(|_| computer::Error::denied(format!("{given} is not a distance"))),
        }
    };

    if let Some((dx, dy)) = heading(word(0), count(1)?) {
        return Ok(Wheel { at: None, dx, dy });
    }

    positional(args, 0, "a direction, or a point to turn the wheel at")?;
    let x = word(0)
        .parse()
        .map_err(|_| instead(word(0), "an x coordinate"))?;
    let y = pixels(args, 1, "a y coordinate")?;
    let at = Some((x, y));

    if let Some((dx, dy)) = heading(word(2), count(3)?) {
        return Ok(Wheel { at, dx, dy });
    }

    positional(args, 2, "a direction, or a number of notches down")?;
    Ok(Wheel {
        at,
        dy: word(2)
            .parse()
            .map_err(|_| instead(word(2), "a number of notches down"))?,
        dx: match args.get(3) {
            Some(_) => signed(args, 3, "a number of notches right")?,
            None => 0,
        },
    })
}

fn instead(word: &str, or: &str) -> computer::Error {
    computer::Error::denied(format!(
        "expected up, down, left or right, or {or}: {word}\n\n{USAGE}"
    ))
}

/// What a direction with no distance means.
const NOTCHES: i32 = 3;

fn pixels(args: &[String], at: usize, what: &str) -> computer::Result<u32> {
    positional(args, at, what)?
        .parse()
        .map_err(|_| computer::Error::denied(format!("{what} must be a whole number of pixels")))
}

/// A count that can go the other way, which a coordinate cannot.
fn signed(args: &[String], at: usize, what: &str) -> computer::Result<i32> {
    positional(args, at, what)?.parse().map_err(|_| {
        computer::Error::denied(format!(
            "{what} must be a whole number, and may be negative"
        ))
    })
}

/// A positional argument, or a usage failure that names what was wanted.
pub fn positional<'a>(args: &'a [String], at: usize, what: &str) -> computer::Result<&'a str> {
    args.get(at)
        .map(String::as_str)
        .ok_or_else(|| computer::Error::denied(format!("expected {what}\n\n{USAGE}")))
}

/// The positional arguments alone, with the flags and the values they take
/// removed, so a flag may stand anywhere on the line rather than only after
/// the last positional. `valued` names the flags that take a value.
pub fn bare(args: &[String], valued: &[&str]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut rest = args.iter();

    while let Some(arg) = rest.next() {
        if arg.starts_with("--") {
            if valued.contains(&arg.as_str()) {
                rest.next();
            }
            continue;
        }
        kept.push(arg.clone());
    }

    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(listed: &[&str]) -> Vec<String> {
        listed.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn test_a_shot_with_no_flags_is_the_whole_screen() {
        let shot = framing(&args(&["mybox"])).expect("a plain shot");

        assert_eq!(shot.of, computer::Of::Screen);
        assert_eq!(shot.scale, None);
    }

    #[test]
    fn test_a_shot_takes_a_window_or_a_rectangle_and_a_size() {
        let one = framing(&args(&["mybox", "--window", "42", "--scale", "50"])).expect("a window");
        assert_eq!(one.of, computer::Of::Window("42".to_string()));
        assert_eq!(one.scale, Some(50));

        let area =
            framing(&args(&["mybox", "--at", "10,20", "--size", "400x300"])).expect("a rectangle");
        assert_eq!(
            area.of,
            computer::Of::Region(computer::Rect::new(computer::Point::new(10, 20), 400, 300))
        );
    }

    #[test]
    fn test_half_a_rectangle_is_refused_rather_than_guessed_at() {
        let error = framing(&args(&["mybox", "--at", "10,20"])).expect_err("half a rectangle");
        assert!(error.to_string().contains("--at and --size"), "{error}");

        let error = framing(&args(&["mybox", "--window", "42", "--size", "10x10"]))
            .expect_err("both at once");
        assert!(error.to_string().contains("not both"), "{error}");

        let error = framing(&args(&["mybox", "--at", "10", "--size", "1x1"]))
            .expect_err("a point with one number");
        assert!(error.to_string().contains("--at takes X,Y"), "{error}");
    }

    #[test]
    fn test_the_file_to_write_is_not_a_flags_value() {
        assert_eq!(remote::named(&args(&["mybox", "--window", "42"])), None);
        assert_eq!(
            remote::named(&args(&["mybox", "out.png", "--scale", "50"])),
            Some("out.png")
        );
        assert_eq!(
            remote::named(&args(&["mybox", "--scale", "50", "out.png"])),
            Some("out.png")
        );
    }

    #[test]
    fn test_a_direction_is_a_scroll_with_no_point_in_it() {
        assert_eq!(
            wheel(&args(&["down"])).expect("a direction"),
            Wheel {
                at: None,
                dx: 0,
                dy: NOTCHES
            }
        );
        assert_eq!(
            wheel(&args(&["left", "8"])).expect("a direction and a distance"),
            Wheel {
                at: None,
                dx: -8,
                dy: 0
            }
        );
    }

    #[test]
    fn test_a_direction_may_still_name_the_point_it_turns_at() {
        assert_eq!(
            wheel(&args(&["640", "400", "right"])).expect("a point and a direction"),
            Wheel {
                at: Some((640, 400)),
                dx: NOTCHES,
                dy: 0
            }
        );
        assert_eq!(
            wheel(&args(&["640", "400", "up", "2"])).expect("all of it"),
            Wheel {
                at: Some((640, 400)),
                dx: 0,
                dy: -2
            }
        );
    }

    #[test]
    fn test_the_signed_form_still_takes_both_axes() {
        assert_eq!(
            wheel(&args(&["640", "400", "-3", "4"])).expect("two axes"),
            Wheel {
                at: Some((640, 400)),
                dx: 4,
                dy: -3
            }
        );
        assert_eq!(
            wheel(&args(&["640", "400", "5"])).expect("one axis"),
            Wheel {
                at: Some((640, 400)),
                dx: 0,
                dy: 5
            }
        );
    }

    #[test]
    fn test_a_word_that_is_neither_is_refused_where_it_stands() {
        for (given, wrong) in [
            (args(&["dwon"]), "dwon"),
            (args(&["640", "400", "sideways"]), "sideways"),
        ] {
            let said = wheel(&given).expect_err("no such direction").to_string();

            assert!(said.contains(wrong), "the word that was wrong: {said}");
            assert!(said.contains("up, down, left or right"), "{said}");
        }
    }

    #[test]
    fn test_a_global_flag_is_taken_out_of_the_positionals() {
        let mut given = args(&["--local", "screenshot", "mybox", "out.png"]);

        assert!(take(&mut given, "--local"));
        assert_eq!(given, args(&["screenshot", "mybox", "out.png"]));
    }

    #[test]
    fn test_a_flag_with_a_value_takes_both() {
        let mut given = args(&["--server", "http://elsewhere", "ls"]);

        assert_eq!(
            value(&mut given, "--server").as_deref(),
            Some("http://elsewhere")
        );
        assert_eq!(given, args(&["ls"]));
    }

    #[test]
    fn test_a_flag_with_nothing_after_it_takes_neither() {
        let mut given = args(&["ls", "--server"]);

        assert_eq!(value(&mut given, "--server"), None);
        assert_eq!(
            given,
            args(&["ls", "--server"]),
            "the argument list is left alone"
        );
    }

    #[test]
    fn test_a_flag_that_is_not_there_changes_nothing() {
        let mut given = args(&["ls"]);

        assert!(!take(&mut given, "--local"));
        assert_eq!(given, args(&["ls"]));
    }
}
