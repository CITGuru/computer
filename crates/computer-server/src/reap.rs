//! Boxes that should not still be here.
//!
//! The engine arms a deadline task at launch, but it belongs to whichever
//! process launched the box, so one taken back after a restart has nobody
//! counting for it. Something else may also have removed a box this registry
//! still holds. Either way `GET /v1/boxes` lists a box that is not there and
//! driving it fails somewhere confusing, so the deadline is swept from the label
//! the engine wrote and anything the runtime no longer holds is let go of.

use crate::AppState;
use computer::{DockerMachine, Machine, SystemDocker};
use computer_api::{Actor, TraceEvent};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// How often to look. A box goes within one of these of its deadline, which is
/// close enough for something billed by the hour and cheap enough to run
/// forever.
pub const EVERY: Duration = Duration::from_secs(30);

pub fn spawn(state: Arc<AppState>, runtimes: Vec<String>, every: Duration) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            once(&state, &runtimes).await;
        }
    });
}

pub async fn once(state: &AppState, runtimes: &[String]) -> usize {
    let mut gone = 0;

    for runtime in runtimes {
        let machine: Arc<dyn Machine> = Arc::new(DockerMachine::new(Arc::new(SystemDocker::new(
            runtime.clone(),
        ))));

        match computer::sweep_expired(machine.as_ref(), SystemTime::now()).await {
            Ok(swept) => {
                for name in swept {
                    tracing::info!(box_ = %name, %runtime, "a box outlived its deadline");
                    forget(state, &name, "its deadline passed").await;
                    gone += 1;
                }
            }
            Err(error) => tracing::debug!(%runtime, %error, "nothing to sweep here"),
        }
    }

    gone + reconcile(state).await
}

/// A box removed by a sweep on another server, by a person with `docker rm`, or
/// by the engine's own timer is gone whatever this process still believes.
async fn reconcile(state: &AppState) -> usize {
    let mut gone = 0;

    for entry in state.registry.list().await {
        // Only a definite "no". A runtime that could not answer is not
        // evidence, and forgetting a box on a hiccup loses a live desktop.
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
    state.traces.of(id).record(
        Actor::System,
        TraceEvent::Gone {
            why: why.to_string(),
        },
    );
}
