//! `computerd`: the server that outlives a command.
//!
//! Here rather than in `computer-server` so one install hands over both
//! binaries. The crate beside this is a library; what a service manager starts
//! is this.
//!
//! What it adds to the server the `computer` command starts for itself is a
//! life longer than one command: it holds the trace a fork reads, it sweeps
//! boxes past their deadline, it prunes what has aged out, and it can be
//! reached from off this host — which is what the token is for.

use computer_server::{AppState, routes};
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "computerd=info,computer_server=info,computer=info".into()),
        )
        .init();

    let address: SocketAddr = std::env::var("COMPUTER_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()?;

    let token = match std::env::var("COMPUTER_SERVER_TOKEN") {
        Ok(value) => Some(computer::Secret::new(value)?),
        Err(_) => None,
    };

    // Before anything is stored or taken back: an open API beyond loopback is
    // refused rather than served for the time it takes to notice.
    if let Err(why) = computer_server::auth::allowed(&address, token.as_ref()) {
        return Err(why.into());
    }

    let state = Arc::new(AppState::from_env().await?.gated(token));

    let runtimes = computer_server::recover::runtimes();
    let sandboxes = computer_server::recover::sandboxes();
    let taken = computer_server::recover::adopt(&state, &runtimes, &sandboxes).await;
    if taken > 0 {
        tracing::info!(taken, "took back boxes left running by an earlier server");
    }

    let every = std::env::var("COMPUTER_SERVER_REAP_SECS")
        .ok()
        .and_then(|secs| secs.parse().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(computer_server::reap::EVERY);

    computer_server::reap::spawn(Arc::clone(&state), runtimes, every);
    computer_server::prune::spawn(Arc::clone(&state), computer_server::prune::every());

    let listener = tokio::net::TcpListener::bind(address).await?;

    tracing::info!(
        %address,
        gated = state.token.is_some(),
        "computerd is listening"
    );
    axum::serve(listener, routes::router(Arc::clone(&state)))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    // A store that batches is holding entries a reader has already been shown.
    // Ctrl-C is the one ending where they can still be put down.
    if let Err(why) = state.store.flush().await {
        tracing::warn!(%why, "what the store was still holding did not go down");
    }

    Ok(())
}
