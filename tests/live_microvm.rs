//! The same box, on a hypervisor. Ignored by default.
//!
//! ```text
//! cargo test --test live_microvm -- --ignored --nocapture
//! ```
//!
//! Needs microsandbox installed and the desktop image handed over to it, which
//! this does for itself the first time and which costs a gigabyte through the
//! disk. Everything after the boot is the same code the container test runs,
//! and that is the point: `Machine` is the only part that knows where the box
//! is.

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

    // A real pointer in a real kernel of its own.
    screen.click(Point::new(640, 400), Button::Left).await?;
    assert_eq!(screen.cursor().await?, Point::new(640, 400));

    screen.type_text("on a microVM").await?;
    screen.key("ctrl+a").await?;
    screen.scroll(Point::new(640, 400), Delta::down(2)).await?;

    // The viewer is forwarded, so a person can watch a machine that has its
    // own kernel exactly the way they watch a container.
    assert!(
        computer.viewer_url().is_some(),
        "the hypervisor was given the pairs to forward"
    );

    // Files, both ways.
    computer.write_file("/tmp/live/frame.png", &frame).await?;
    let back = computer.read_file("/tmp/live/frame.png").await?;
    assert_eq!(back.len(), frame.len(), "the bytes changed on the way");
    println!("  {} bytes went in and came back", back.len());

    // The browser, over the protocol rather than the screen.
    let browser = computer.browser().expect("a forwarded DevTools port");
    // Retried, because a machine this young is still settling: the guest's
    // interface comes up after chromium has started, and the browser answers
    // the first navigation with ERR_NETWORK_CHANGED. That is the machine
    // rather than the page, so it is worth asking again.
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

    // A second screen, with its own display and its own pointer.
    let second = computer.screen(ScreenId(1)).await?;
    assert_eq!(second.display(), ":2");
    second.move_to(Point::new(20, 30)).await?;
    assert_eq!(second.cursor().await?, Point::new(20, 30));
    assert_eq!(computer.primary().cursor().await?, Point::new(640, 400));
    computer.close_screen(ScreenId(1)).await?;
    println!("  screen 1 is its own display");

    // The same audit the container test ends with, against a machine that has
    // its own kernel.
    let audit = computer::audit::audit_strictly(computer, Duration::from_secs(60)).await?;
    println!("  audit: {audit}");

    Ok(())
}
