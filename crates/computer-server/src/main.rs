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

    let state = Arc::new(AppState::default());

    let runtimes = computer_server::recover::runtimes();
    let taken = computer_server::recover::adopt(&state.registry, &state.traces, &runtimes).await;
    if taken > 0 {
        tracing::info!(taken, "took back boxes left running by an earlier server");
    }

    let listener = tokio::net::TcpListener::bind(address).await?;

    tracing::info!(%address, "computer-server is listening");
    axum::serve(listener, routes::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    Ok(())
}
