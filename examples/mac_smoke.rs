//! Prove the machine layer against a guest, with no macOS image in sight.

use computer::mac::MacMachine;
use computer::machine::Machine;
use computer::runtime::Config;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

const NAME: &str = "computer-smoke";

/// What the guest's sshd says when something connects through the relay.
async fn ssh_banner(port: u16) -> String {
    use tokio::io::AsyncReadExt;

    // Retried, because `tart ip` answers as soon as the guest has an address
    // and well before anything in it is listening. The relay connects on
    // demand, so an early attempt closes with nothing said.
    for attempt in 1..=30 {
        let connect = tokio::net::TcpStream::connect(("127.0.0.1", port));
        let Ok(Ok(mut stream)) = tokio::time::timeout(Duration::from_secs(5), connect).await else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };

        let mut banner = [0u8; 64];
        if let Ok(Ok(read)) =
            tokio::time::timeout(Duration::from_secs(5), stream.read(&mut banner)).await
            && read > 0
        {
            let said = String::from_utf8_lossy(&banner[..read]).trim().to_string();
            return format!("{said:?} after {attempt} attempt(s)");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    "the guest never answered through the relay".to_string()
}

#[tokio::main]
async fn main() -> computer::Result<()> {
    let image = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ghcr.io/cirruslabs/ubuntu:latest".to_string());

    // No viewer: the viewer is Screen Sharing inside the guest, and this one
    // is Linux. Everything else here is guest-agnostic.
    let machine = MacMachine::default().without_viewer();
    let config = Config {
        image: image.clone(),
        // Nothing is ever built for a box that is a virtual machine, and
        // `ensure_image` refuses a configuration that asks.
        bundle: None,
        image_dir: None,
        publish: vec![22],
        ..Config::default()
    };

    machine.preflight().await?;
    println!("preflight: {} answers on Apple silicon", machine.runtime());

    machine.ensure_image(&config).await?;
    println!("image:     {image}");

    let ports = machine.start(NAME, &config).await?;
    println!("started:   ports {ports:?}");

    // The relay is the piece nothing above it can check: everything upstream
    // is written against a PortMap and simply trusts what start() returned.
    match ports.get(&22) {
        Some(port) => println!("relay:     22 -> {port}, {}", ssh_banner(*port).await),
        None => println!("relay:     nothing was published"),
    }

    println!(
        "viewer:    not asked for -- a Linux guest has no Screen Sharing{}",
        match ports.get(&computer::mac::VIEWER_PORT) {
            Some(_) => ", yet one was published, which is wrong",
            None => "",
        }
    );

    let uname = machine
        .exec(
            NAME,
            &["uname".to_string(), "-sm".to_string()],
            &BTreeMap::new(),
        )
        .await?;
    println!(
        "agent:     {} (exit {})",
        uname.stdout_utf8().trim(),
        uname.code
    );

    let env = BTreeMap::from([("COMPUTER_PROBE".to_string(), "it's set".to_string())]);
    let echoed = machine
        .exec(
            NAME,
            &[
                "sh".to_string(),
                "-c".to_string(),
                "echo $COMPUTER_PROBE".to_string(),
            ],
            &env,
        )
        .await?;
    println!("env:       {:?}", echoed.stdout_utf8().trim());

    let probe = Path::new("/tmp/computer-probe");
    machine.write_file(NAME, probe, b"round trip").await?;
    let back = machine.read_file(NAME, probe).await?;
    println!("files:     {:?}", String::from_utf8_lossy(&back));

    println!("running:   {}", machine.running(NAME).await?);

    machine.stop(NAME).await?;
    println!("stopped:   and the clone is deleted");

    Ok(())
}
