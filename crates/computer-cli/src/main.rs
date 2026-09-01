//! `computer` — a desktop in a box, from the command line.
//!
//! For the times when writing a program is more ceremony than the job needs:
//! open a box, look at it, point it at a page, hand it to somebody, take it
//! away again.
//!
//! Commands go through a server, which is what lets the same ones reach a box
//! on this machine and a box in somebody's fleet. When no server is named and
//! none is listening, one is started here for the length of the command — so
//! nothing has to be running first.
//!
//! `--local` skips all that and drives the box from this process. Faster, and
//! a smaller set: what a server remembers, a one-shot process does not.

mod embed;
mod local;
mod remote;

use computer_client::Client;

pub const USAGE: &str = "\
computer — a desktop in a box

  up [--size WxH] [--url URL] [--ttl MINUTES] [--wide-fonts]
                              open a box and print where to watch it
  ls                          boxes that are running
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
  fork <box> [--up-to SEQ]    build another by doing again what was done
  trace <box> [--after SEQ]   what has been done to it, and by whom
  sweep                       remove every box whose deadline has passed

  --server URL                a server to use, over $COMPUTER_SERVER_URL
  --local                     drive the box from here, with no server at all.
                              fork and trace need one, and say so.

The first `up` builds the image, which takes a few minutes. Every one after it
starts in seconds.
";

#[tokio::main]
async fn main() {
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
        return;
    }

    let outcome = match local {
        true => here(&command, &rest).await,
        false => match connect(named).await {
            Ok(client) => there(&client, &command, &rest).await,
            Err(why) => Err(why),
        },
    };

    if let Err(error) = outcome {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// A server to talk to: the one named, the one already listening, or one
/// started here and thrown away with the process.
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
        "shot" => remote::shot(client, args).await,
        "open" => remote::open(client, args).await,
        "type" => remote::type_text(client, args).await,
        "key" => remote::key(client, args).await,
        "click" => remote::click(client, args).await,
        "clip" => remote::clip(client, args).await,
        "takeover" => remote::takeover(client, args).await,
        "release" => remote::release(client, args).await,
        "exec" => remote::exec(client, args).await,
        "rm" => remote::remove(client, args).await,
        "fork" => remote::fork(client, args).await,
        "trace" => remote::trace(client, args).await,
        // The server sweeps on its own cadence; this is the local operator's
        // version of the same thing.
        "sweep" => local::sweep().await.map_err(|error| error.to_string()),
        other => Err(format!("unknown command: {other}\n\n{USAGE}")),
    }
}

async fn here(command: &str, args: &[String]) -> Result<(), String> {
    let outcome = match command {
        "up" => local::up(args).await,
        "ls" => local::list().await,
        "shot" => local::shot(args).await,
        "open" => local::open(args).await,
        "type" => local::type_text(args).await,
        "key" => local::key(args).await,
        "click" => local::click(args).await,
        "clip" => local::clip(args).await,
        "takeover" => local::takeover(args).await,
        "release" => local::release(args).await,
        "exec" => local::exec(args).await,
        "rm" => local::remove(args).await,
        "sweep" => local::sweep().await,
        // Both are built on a trace, and a trace is something a server keeps.
        // A process that exits when the command does has nowhere to keep one.
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
fn take(args: &mut Vec<String>, name: &str) -> bool {
    match args.iter().position(|arg| arg == name) {
        Some(at) => {
            args.remove(at);
            true
        }
        None => false,
    }
}

fn value(args: &mut Vec<String>, name: &str) -> Option<String> {
    let at = args.iter().position(|arg| arg == name)?;
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

/// A positional argument, or a usage failure that names what was wanted.
pub fn positional<'a>(args: &'a [String], at: usize, what: &str) -> computer::Result<&'a str> {
    args.get(at)
        .map(String::as_str)
        .ok_or_else(|| computer::Error::denied(format!("expected {what}\n\n{USAGE}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(listed: &[&str]) -> Vec<String> {
        listed.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn test_a_global_flag_is_taken_out_of_the_positionals() {
        let mut given = args(&["--local", "shot", "mybox", "out.png"]);

        assert!(take(&mut given, "--local"));
        assert_eq!(given, args(&["shot", "mybox", "out.png"]));
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
