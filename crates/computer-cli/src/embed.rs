//! A server for the length of one command.

use computer_server::{AppState, recover, routes};
use std::sync::{Arc, OnceLock};

/// The state of the server this process started, if it started one.
static SERVING: OnceLock<Arc<AppState>> = OnceLock::new();

pub async fn start() -> Result<String, String> {
    let state = Arc::new(AppState::from_env().await?);

    let runtimes = recover::runtimes();
    recover::adopt(&state, &runtimes, &recover::sandboxes()).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("no loopback port to serve on: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();

    let _ = SERVING.set(Arc::clone(&state));

    tokio::spawn(async move {
        let _ = axum::serve(listener, routes::router(state)).await;
    });

    Ok(format!("http://127.0.0.1:{port}"))
}

/// Puts down what the server this process started is still holding.
pub async fn flush() {
    let Some(state) = SERVING.get() else {
        return;
    };

    if let Err(why) = state.store.flush().await {
        eprintln!("what was written did not reach the store: {why}");
    }
}
