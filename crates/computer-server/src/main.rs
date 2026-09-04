use computer_server::{AppState, routes};
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "computer_server=info,computer=info".into()),
        )
        .init();

    let address: SocketAddr = std::env::var("COMPUTER_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()?;

    let token = match std::env::var("COMPUTER_SERVER_TOKEN") {
        Ok(value) => Some(computer::Secret::new(value)?),
        Err(_) => None,
    };

    if let Err(why) = computer_server::auth::allowed(&address, token.as_ref()) {
        return Err(why.into());
    }

    let state = Arc::new(AppState {
        token,
        ..AppState::default()
    });

    let runtimes = computer_server::recover::runtimes();
    let taken = computer_server::recover::adopt(&state.registry, &state.traces, &runtimes).await;
    if taken > 0 {
        tracing::info!(taken, "took back boxes left running by an earlier server");
    }

    let every = std::env::var("COMPUTER_SERVER_REAP_SECS")
        .ok()
        .and_then(|secs| secs.parse().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(computer_server::reap::EVERY);

    computer_server::reap::spawn(Arc::clone(&state), runtimes, every);

    let listener = tokio::net::TcpListener::bind(address).await?;

    tracing::info!(
        %address,
        gated = state.token.is_some(),
        "computer-server is listening"
    );
    axum::serve(listener, routes::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    Ok(())
}
