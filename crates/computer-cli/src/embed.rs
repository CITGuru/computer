//! A server for the length of one command.
//!
//! Works because a box carries its own spec in a label and is taken back on
//! startup: a server that lives for one command is not amnesiac, it rediscovers
//! what is running each time. What it cannot rediscover is a trace, which lived
//! in the memory of whatever server was there before.

use computer_server::{AppState, recover, routes};
use std::sync::Arc;

pub async fn start() -> Result<String, String> {
    let state = Arc::new(AppState::default());

    // Silent: taking boxes back is how this works, not news. A command that
    // announced it on every run would be shouting its own plumbing.
    let runtimes = recover::runtimes();
    recover::adopt(&state.registry, &state.traces, &runtimes).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("no loopback port to serve on: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();

    tokio::spawn(async move {
        let _ = axum::serve(listener, routes::router(state)).await;
    });

    Ok(format!("http://127.0.0.1:{port}"))
}
