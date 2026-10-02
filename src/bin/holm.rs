fn main() {
    // Rust ignores SIGPIPE so a server survives a closed socket; a tool wants the
    // Unix rule back, or `holm … | head` panics in println! once head has gone.
    #[cfg(unix)]
    // SAFETY: setting a signal disposition before any thread but this one exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    holm_cli::adopt_earlier_names();

    let done = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(holm_cli::run()),
        Err(error) => Err(error.to_string()),
    };
    if let Err(error) = done {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
