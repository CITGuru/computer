#[tokio::main]
async fn main() {
    // Rust ignores SIGPIPE so a server survives a closed socket; a tool wants the
    // Unix rule back, or `computer … | head` panics in println! once head has gone.
    #[cfg(unix)]
    // SAFETY: setting a signal disposition before any thread but this one exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    if let Err(error) = computer_cli::run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
