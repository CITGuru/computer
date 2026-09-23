use crate::AppState;
use computer_api::{Actor, TraceEvent};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub const EVERY: Duration = Duration::from_secs(30);

pub fn spawn(state: Arc<AppState>, every: Duration) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            once(&state).await;
        }
    });
}

pub async fn once(state: &AppState) -> usize {
    let mut gone = 0;

    for runtime in state.runtimes.all() {
        if !runtime.ready() {
            continue;
        }

        let machine = runtime.scanning();
        let name = &runtime.name;

        match computer::sweep_expired(machine.as_ref(), SystemTime::now()).await {
            Ok(swept) => {
                for box_ in swept {
                    tracing::info!(box_ = %box_, runtime = %name, "a box outlived its deadline");
                    forget(state, &box_, "its deadline passed").await;
                    gone += 1;
                }
            }
            Err(error) => tracing::debug!(runtime = %name, %error, "nothing to sweep here"),
        }
    }

    gone + reconcile(state).await
}

async fn reconcile(state: &AppState) -> usize {
    let mut gone = 0;

    for entry in state.registry.list().await {
        // Only a definite no: forgetting a box on a runtime hiccup loses a live desktop.
        if matches!(entry.computer.machine().running(&entry.id).await, Ok(false)) {
            tracing::info!(box_ = %entry.id, "the runtime no longer holds this box");
            forget(state, &entry.id, "the runtime no longer has it").await;
            gone += 1;
        }
    }

    gone
}

async fn forget(state: &AppState, id: &str, why: &str) {
    state.registry.forget(id).await;

    if state.traced(id).await {
        state
            .record(
                id,
                Actor::System,
                TraceEvent::Gone {
                    why: why.to_string(),
                },
            )
            .await;
    }

    state.forget_screens(id);
}
