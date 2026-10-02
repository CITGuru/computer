use holm_server::{AppState, routes};
use std::net::SocketAddr;
use std::sync::Arc;

pub async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "holmd=info,holm=info".into()),
        )
        .init();

    let address: SocketAddr = std::env::var("HOLM_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()?;

    let token = match std::env::var("HOLM_SERVER_TOKEN") {
        Ok(value) => Some(holm::Secret::new(value)?),
        Err(_) => None,
    };

    let console = holm_server::console::Console::configured();
    if let Err(why) = holm_server::auth::allowed(&address, token.as_ref(), console) {
        return Err(why.into());
    }

    let state = Arc::new(AppState::from_env().await?.gated(token));
    if let Some(console) = &state.console {
        console.follow();
    }

    let taken = holm_server::recover::adopt(&state).await;
    if taken > 0 {
        tracing::info!(taken, "took back boxes left running by an earlier server");
    }

    let every = std::env::var("HOLM_SERVER_REAP_SECS")
        .ok()
        .and_then(|secs| secs.parse().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(holm_server::reap::EVERY);

    holm_server::schedule::spawn(Arc::clone(&state), every, holm_server::prune::every());

    let listener = tokio::net::TcpListener::bind(address).await?;
    let api = format!("http://{}", own(listener.local_addr()?));
    let public = std::env::var("HOLM_PUBLIC_URL").ok();

    tracing::info!(
        %address,
        gated = state.token.is_some() || state.console.is_some(),
        mcp = "/mcp",
        runtimes = %state.runtimes.names(),
        "holmd is listening"
    );
    let app = routes::router(Arc::clone(&state))
        .merge(holm_server::mcp::router(Arc::clone(&state), api, public))
        .layer(axum::middleware::from_fn(holm_server::oidc::offered));
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
