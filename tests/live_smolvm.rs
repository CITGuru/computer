//! On a hypervisor, with smolvm installed: `cargo test --test live_smolvm -- --ignored`.

use computer::SystemEngine;
use computer::microvm::{MicroVm, MicroVmApi, import_image};
use computer::sandboxes::smolvm::SmolVm;
use computer::{Button, Computer, Delta, Engine, Point, bundle};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
#[ignore = "needs smolvm and a hypervisor"]
async fn a_real_smolvm_runs_the_same_desktop() {
    let hypervisor = SmolVm::found();
    if hypervisor.available().await.is_err() {
        eprintln!("smolvm is not installed; nothing to test against");
        return;
    }

    let tag = bundle::DESKTOP.tag();
    let docker: Arc<dyn Engine> = Arc::new(SystemEngine::default());

    if !hypervisor.has_image(&tag).await.expect("asked") {
        println!("  handing {tag} to the hypervisor …");
        bundle::ensure(docker.as_ref(), &tag)
            .await
            .expect("an image");
        import_image(docker.as_ref(), &hypervisor, &tag)
            .await
            .expect("the image goes over");
    }

    let computer = Computer::builder()
        .machine(Arc::new(
            MicroVm::new(Arc::new(SmolVm::found())).named("smolvm"),
        ))
        .image(&tag)
        .memory("2g")
        .cpus("2")
        .launch()
        .await
        .expect("a microVM");

    println!("  {} on {}", computer.name(), computer.provider());
    let outcome = exercise(&computer).await;

    computer.shutdown().await.expect("it goes away");
    outcome.expect("every step");
}

async fn exercise(computer: &Computer) -> computer::Result<()> {
    assert_eq!(computer.provider(), "smolvm");
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
    println!(
        "  watch it at {}",
        computer.viewer_url().unwrap_or_default()
    );

    computer.write_file("/tmp/live/frame.png", &frame).await?;
    let back = computer.read_file("/tmp/live/frame.png").await?;
    assert_eq!(back.len(), frame.len(), "the bytes changed on the way");
    println!("  {} bytes went in and came back", back.len());

    let browser = computer.browser().expect("a forwarded DevTools port");
    let page = browser
        .open_page("https://example.com", Duration::from_secs(30))
        .await?;
    println!("  the browser drove itself: {}", page.target().title);

    let taken = computer
        .machine()
        .labelled("computer-rs")
        .await
        .unwrap_or_default();
    assert!(
        taken.iter().any(|(name, _)| name == computer.name()),
        "a restart finds this box by its label: {taken:?}"
    );

    Ok(())
}
