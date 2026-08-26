//! The viewer gate, against a real box. Ignored by default.
//!
//! ```text
//! cargo test --test live_auth -- --ignored --nocapture
//! ```
//!
//! The offline suite proves the crate writes the gate and the scripts read it.
//! Only this proves the gate *refuses*: that a wrong credential does not open a
//! desktop, and that the credential for one door does not open the other. A
//! refusal nobody has watched happen is the failure this whole design exists to
//! prevent.

use computer::{Auth, Computer, WaylandProfile};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

fn authority(url: &str) -> String {
    url.split("//")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .expect("a URL with an authority")
        .to_string()
}

fn curl(args: &[String]) -> String {
    let output = Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "15",
        ])
        .args(args)
        .output()
        .expect("curl is on the path");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A websocket handshake, as the browser would open one.
///
/// The status is what says whether the gate let it through: 101 is the upgrade,
/// and anything else — including `000` for a connection closed without a
/// reply — is a refusal.
fn upgrade(url: &str, credentials: Option<&str>) -> String {
    let mut args: Vec<String> = [
        "--http1.1",
        "-H",
        "Connection: Upgrade",
        "-H",
        "Upgrade: websocket",
        "-H",
        "Sec-WebSocket-Version: 13",
        "-H",
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
        "-H",
        "Sec-WebSocket-Protocol: binary",
    ]
    .iter()
    .map(|word| word.to_string())
    .collect();

    if let Some(credentials) = credentials {
        args.push("-u".to_string());
        args.push(credentials.to_string());
    }
    args.push(url.to_string());
    curl(&args)
}

fn page(url: &str, credentials: Option<&str>) -> String {
    let mut args = Vec::new();
    if let Some(credentials) = credentials {
        args.push("-u".to_string());
        args.push(credentials.to_string());
    }
    args.push(url.to_string());
    curl(&args)
}

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn a_token_gate_opens_for_its_ticket_and_for_nothing_else() {
    let computer = Computer::builder()
        .auth(Auth::Token)
        .launch()
        .await
        .expect("a gated box");

    let outcome = token_checks(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn token_checks(computer: &Computer) -> Result<(), String> {
    let credentials = computer.credentials().expect("a gated box holds a pair");
    let view = credentials.view.expose().to_string();
    let control_secret = credentials.control.expose().to_string();

    let viewer = computer.viewer_url().expect("a viewer");
    let at = authority(&viewer);
    println!("  viewer at {at}");

    let socket = |token: Option<&str>| match token {
        Some(token) => format!("http://{at}/websockify?token={token}"),
        None => format!("http://{at}/websockify"),
    };

    let check = |what: &str, got: String, want: &str| -> Result<(), String> {
        println!("  {what}: {got}");
        match got == want {
            true => Ok(()),
            false => Err(format!("{what}: got {got}, wanted {want}")),
        }
    };

    // The page is static noVNC and carries no desktop, so it is not what the
    // token gates. The socket behind it is.
    check(
        "the page is served",
        page(&format!("http://{at}/vnc.html"), None),
        "200",
    )?;
    check(
        "the right ticket",
        upgrade(&socket(Some(&view)), None),
        "101",
    )?;

    let refused = upgrade(&socket(Some("not-the-ticket")), None);
    println!("  a wrong ticket: {refused}");
    if refused == "101" {
        return Err("a wrong ticket opened the viewer".to_string());
    }

    let refused = upgrade(&socket(None), None);
    println!("  no ticket at all: {refused}");
    if refused == "101" {
        return Err("no ticket at all opened the viewer".to_string());
    }

    // The claim two credentials exist to make: a watch link does not become a
    // control link by changing the port.
    let takeover = computer
        .hand_over()
        .await
        .map_err(|error| format!("hand over: {error}"))?;
    let control = takeover.url().expect("a control URL");
    let control_at = authority(control);
    println!("  control at {control_at}");

    let crossed = upgrade(
        &format!("http://{control_at}/websockify?token={view}"),
        None,
    );
    println!("  the view ticket on the control door: {crossed}");
    if crossed == "101" {
        return Err("the view ticket opened the control door".to_string());
    }

    check(
        "the control ticket on the control door",
        upgrade(
            &format!("http://{control_at}/websockify?token={control_secret}"),
            None,
        ),
        "101",
    )?;

    Ok(())
}

#[tokio::test]
#[ignore = "needs a container runtime"]
async fn a_password_gate_covers_the_page_as_well_as_the_socket() {
    let computer = Computer::builder()
        .auth(Auth::Password)
        .launch()
        .await
        .expect("a gated box");

    let outcome = password_checks(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn password_checks(computer: &Computer) -> Result<(), String> {
    let credentials = computer.credentials().expect("a gated box holds a pair");
    let right = format!("{}:{}", computer::VIEWER_USER, credentials.view.expose());
    let wrong = format!("{}:not-the-password", computer::VIEWER_USER);

    let viewer = computer.viewer_url().expect("a viewer");
    let at = authority(&viewer);
    println!("  viewer at {at}");

    if viewer.contains(credentials.view.expose()) {
        return Err("the password reached the URL, which is what this mode avoids".to_string());
    }

    let html = format!("http://{at}/vnc.html");

    // `--web-auth` is what makes this cover the page. Without it the HTML is
    // served to anyone and only the socket is gated.
    let unasked = page(&html, None);
    println!("  the page with no credentials: {unasked}");
    if unasked != "401" {
        return Err(format!("the page was served unasked: {unasked}"));
    }

    let refused = page(&html, Some(&wrong));
    println!("  the page with a wrong password: {refused}");
    if refused == "200" {
        return Err("a wrong password was served the page".to_string());
    }

    let served = page(&html, Some(&right));
    println!("  the page with the right password: {served}");
    if served != "200" {
        return Err(format!("the right password was refused the page: {served}"));
    }

    let socket = format!("http://{at}/websockify");
    let refused = upgrade(&socket, Some(&wrong));
    println!("  the socket with a wrong password: {refused}");
    if refused == "101" {
        return Err("a wrong password opened the socket".to_string());
    }

    let opened = upgrade(&socket, Some(&right));
    println!("  the socket with the right password: {opened}");
    if opened != "101" {
        return Err(format!(
            "the right password was refused the socket: {opened}"
        ));
    }

    Ok(())
}

/// The gate is one block shared by all three images, and the Wayland one puts a
/// different compositor and a different VNC server behind the same websockify.
/// Nothing about that should reach the gate, which is exactly why it is worth
/// watching once rather than assumed.
#[tokio::test]
#[ignore = "needs a container runtime, and builds the Wayland image"]
async fn the_wayland_image_gates_its_viewer_the_same_way() {
    let computer = Computer::builder()
        .profile(Arc::new(WaylandProfile))
        .auth(Auth::Token)
        .launch()
        .await
        .expect("a gated Wayland box");

    let outcome = token_checks(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

/// The Ubuntu image builds the same scripts from a local directory, and its
/// `screen.sh` is byte-identical to the desktop one. Watched anyway, because
/// "identical" is a claim about two files rather than about two images.
#[tokio::test]
#[ignore = "needs a container runtime, and builds the Ubuntu image"]
async fn the_ubuntu_image_gates_its_viewer_the_same_way() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("images/ubuntu");
    let computer = Computer::builder()
        .image_dir(directory)
        .auth(Auth::Token)
        .launch()
        .await
        .expect("a gated Ubuntu box");

    let outcome = token_checks(&computer).await;
    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}
