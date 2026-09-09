//! That a database holds what a directory does.
//!
//! The same assertions as `persistence.rs`, against the other backend, because
//! the wiring is what is being checked rather than the store: `AppState::on`
//! takes anything that is both, and a backend that cannot be handed to it is
//! not usable however well it behaves on its own.

#![cfg(feature = "sqlite")]

use computer_api::{Actor, TraceEvent};
use computer_server::AppState;
use computer_storage::sql::Sql;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default();
        let mine = NEXT.fetch_add(1, Ordering::Relaxed);

        Self(std::env::temp_dir().join(format!("computer-server-{nanos}-{mine}.db")))
    }

    fn url(&self) -> String {
        format!("sqlite:{}?mode=rwc", self.0.display())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

async fn served_from(url: &str) -> AppState {
    AppState::on(Arc::new(Sql::open(url).await.expect("opened")))
}

#[tokio::test]
async fn test_a_trace_and_its_frames_outlive_the_server() {
    let scratch = Scratch::new();

    {
        let state = served_from(&scratch.url()).await;
        state
            .record("box_1", Actor::Agent, TraceEvent::BoxDeleted)
            .await;
        state
            .note_frame("box_1", Actor::Agent, 0, "aaa", b"png")
            .await;
    }

    let restarted = served_from(&scratch.url()).await;

    assert!(
        restarted.traced("box_1").await,
        "what was done to the box is still readable"
    );
    assert_eq!(
        restarted
            .frames
            .get("box_1", "aaa")
            .await
            .expect("asked")
            .as_deref()
            .map(Vec::as_slice),
        Some(b"png".as_slice()),
        "and the picture it left behind is still there"
    );
}

#[tokio::test]
async fn test_two_servers_on_one_database_do_not_share_a_sequence() {
    let scratch = Scratch::new();

    let first = served_from(&scratch.url()).await;
    let second = served_from(&scratch.url()).await;

    first
        .record("box_1", Actor::Agent, TraceEvent::BoxDeleted)
        .await;
    second
        .record("box_1", Actor::System, TraceEvent::BoxDeleted)
        .await;

    let entries = first
        .store
        .entries("box_1", None, 10)
        .await
        .expect("read back");

    assert_eq!(entries.len(), 2, "both writers' entries are held");
    assert_ne!(
        entries[0].seq, entries[1].seq,
        "the database assigns the sequence, so a second server does not write \
         over the first — which is the whole reason to run one"
    );
}
