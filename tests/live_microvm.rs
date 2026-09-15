//! On a hypervisor, with microsandbox installed: `cargo test --test live_microvm -- --ignored`.

use computer::microvm::{MicroVmApi, import_image};
use computer::runtime::SystemDocker;
use computer::sandboxes::microsandbox::msb;
use computer::{Button, Computer, ContainerCli, Delta, Point, ScreenId, bundle};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
#[ignore = "needs microsandbox and a hypervisor"]
async fn a_real_microvm_runs_the_same_desktop() {
    let hypervisor = msb::Msb::found();
    if hypervisor.available().await.is_err() {
        eprintln!("microsandbox is not installed; nothing to test against");
        return;
    }

    let tag = bundle::DESKTOP.tag();
    let docker: Arc<dyn ContainerCli> = Arc::new(SystemDocker::default());

    if !hypervisor.has_image(&tag).await.expect("an image list") {
        println!("  handing {tag} to the hypervisor …");
        bundle::ensure(docker.as_ref(), &tag)
            .await
            .expect("an image");
        import_image(docker.as_ref(), &hypervisor, &tag)
            .await
            .expect("the image goes over");
    }

    let computer = Computer::builder()
        .machine(Arc::new(msb::machine()))
        .image(&tag)
        .memory("2g")
        .cpus("2")
        .launch()
        .await
        .expect("a microVM");

    println!("  {} on {}", computer.name(), computer.runtime());
    let outcome = exercise(&computer).await;

    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn exercise(computer: &Computer) -> computer::Result<()> {
    assert_eq!(computer.runtime(), "microsandbox");
    assert!(
        computer.probe().await.ready(),
        "launch waited for the screen and the browser"
    );

    let screen = computer.primary();
    assert_eq!(screen.geometry().await?, (1280, 800));

    let frame = screen.screenshot().await?;
    assert_eq!(
        frame.first_chunk::<4>(),
        Some(&[0x89, b'P', b'N', b'G']),
        "the capture is not a PNG"
    );
    println!("  screenshot: {} bytes", frame.len());

    screen.click(Point::new(640, 400), Button::Left).await?;
    assert_eq!(screen.cursor().await?, Point::new(640, 400));

    screen.type_text("on a microVM").await?;
    screen.press("ctrl+a").await?;
    screen.scroll(Point::new(640, 400), Delta::down(2)).await?;

    assert!(
        computer.viewer_url().is_some(),
        "the hypervisor was given the pairs to forward"
    );

    computer.write_file("/tmp/live/frame.png", &frame).await?;
    let back = computer.read_file("/tmp/live/frame.png").await?;
    assert_eq!(back.len(), frame.len(), "the bytes changed on the way");
    println!("  {} bytes went in and came back", back.len());

    let browser = computer.browser().expect("a forwarded DevTools port");
    // A young guest's interface comes up after chromium, which answers ERR_NETWORK_CHANGED.
    let mut page = None;
    for attempt in 1..=3 {
        match browser
            .open_page("https://example.com", Duration::from_secs(30))
            .await
        {
            Ok(opened) => {
                page = Some(opened);
                break;
            }
            Err(error) if attempt < 3 => {
                println!("  the network was still settling: {error}");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Err(error) => return Err(error),
        }
    }

    let mut page = page.expect("a page");
    assert_eq!(page.title().await?, "Example Domain");

    let id = page.target().id.clone();
    browser.close(&id).await?;
    println!("  the browser drove itself");

    let second = computer.screen(ScreenId(1)).await?;
    assert_eq!(second.display(), ":2");
    second.move_to(Point::new(20, 30)).await?;
    assert_eq!(second.cursor().await?, Point::new(20, 30));
    assert_eq!(computer.primary().cursor().await?, Point::new(640, 400));
    computer.close_screen(ScreenId(1)).await?;
    println!("  screen 1 is its own display");

    let audit = computer::audit::audit_strictly(computer, Duration::from_secs(60)).await?;
    println!("  audit: {audit}");

    Ok(())
}
