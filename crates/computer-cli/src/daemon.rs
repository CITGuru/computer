use computer_server::{AppState, routes};
use std::net::SocketAddr;
use std::sync::Arc;

pub async fn serve() -> Result<(), Box<dyn std::error::Error>> {
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

    if let Err(why) = computer_server::auth::allowed(&address, token.as_ref()) {
        return Err(why.into());
    }

    let state = Arc::new(AppState::from_env().await?.gated(token));

    let taken = computer_server::recover::adopt(&state).await;
    if taken > 0 {
        tracing::info!(taken, "took back boxes left running by an earlier server");
    }

    let every = std::env::var("COMPUTER_SERVER_REAP_SECS")
        .ok()
        .and_then(|secs| secs.parse().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(computer_server::reap::EVERY);

    computer_server::reap::spawn(Arc::clone(&state), every);
    computer_server::prune::spawn(Arc::clone(&state), computer_server::prune::every());

    let listener = tokio::net::TcpListener::bind(address).await?;
    let api = format!("http://{}", own(listener.local_addr()?));
    let public = std::env::var("COMPUTER_PUBLIC_URL").ok();

    tracing::info!(
        %address,
        gated = state.token.is_some(),
        mcp = "/mcp",
        runtimes = %state.runtimes.names(),
        "computerd is listening"
    );
    let app = routes::router(Arc::clone(&state)).merge(computer_server::mcp::router(
        Arc::clone(&state),
        api,
        public,
    ));
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    if let Err(why) = state.store.flush().await {
        tracing::warn!(%why, "what the store was still holding did not go down");
    }

    Ok(())
}

/// Where this process reaches itself: the bound address, or the loopback when it took them all.
fn own(bound: SocketAddr) -> SocketAddr {
    if bound.ip().is_unspecified() {
        SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), bound.port())
    } else {
        bound
    }
}
