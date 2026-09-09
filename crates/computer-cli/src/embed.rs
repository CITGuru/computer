//! A server for the length of one command.
//!
//! Works because a box carries its own spec in a label and is taken back on
//! startup: a server that lives for one command is not amnesiac, it rediscovers
//! what is running each time.
//!
//! What a label cannot carry is a trace. Where `COMPUTER_STATE_DIR` names a
//! directory the record is kept there and outlives the command, so `fork` and
//! `trace` work with no daemon. Without one it goes when the process does.

use computer_server::{AppState, recover, routes};
use std::sync::{Arc, OnceLock};

/// The state of the server this process started, if it started one.
///
/// Held so [`flush`] can reach it: a store that batches is holding entries this
/// command has already been shown, and the process is about to end.
static SERVING: OnceLock<Arc<AppState>> = OnceLock::new();

pub async fn start() -> Result<String, String> {
    let state = Arc::new(AppState::from_env().await?);

    // Silent: taking boxes back is how this works, not news. A command that
    // announced it on every run would be shouting its own plumbing.
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
///
/// Nothing where no server was started here: a command against a daemon left
/// what it wrote with that daemon.
pub async fn flush() {
    let Some(state) = SERVING.get() else {
        return;
    };

    // To the person, not to a log: this command's own trace is what did not go
    // down, and a CLI with no subscriber would say it to nobody.
    if let Err(why) = state.store.flush().await {
        eprintln!("what was written did not reach the store: {why}");
    }
}
