//! That a record outlives the process that wrote it.

use computer_api::{Actor, TraceEvent};
use computer_server::AppState;
use computer_storage::BoxRecord;
use computer_storage::files::Files;
use computer_storage::local::LocalDir;
use computer_types::{Placement, Spec};
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

        Self(std::env::temp_dir().join(format!("computer-server-{nanos}-{mine}")))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn served_from(root: &PathBuf) -> AppState {
    AppState::on(Arc::new(Files::over(LocalDir::at(root))))
}

fn record() -> BoxRecord {
    BoxRecord {
        id: "box_1".to_string(),
        spec: Spec::default(),
        placement: Placement::default(),
        width: 1280,
        height: 800,
        screens: 1,
        created_at_ms: 1_700_000_000_000,
        expires_at_ms: None,
    }
}

#[tokio::test]
async fn test_a_record_and_its_trace_outlive_the_server() {
    let scratch = Scratch::new();

    {
        let state = served_from(&scratch.0);
        state.store.put_box(&record()).await.expect("recorded");
        state
            .record("box_1", Actor::Agent, TraceEvent::BoxDeleted)
            .await;
        state
            .note_frame("box_1", Actor::Agent, 0, "aaa", b"png")
            .await;
    }

    let restarted = served_from(&scratch.0);

    let held = restarted.store.get_box("box_1").await.expect("asked");
    assert_eq!(
        held.map(|held| held.width),
        Some(1280),
        "the box a gone server made is still described"
    );
    assert!(
        restarted.traced("box_1").await,
        "and what was done to it is still readable"
    );

    let frame = restarted.frames.get("box_1", "aaa").await.expect("asked");
    assert_eq!(
        frame.as_deref().map(Vec::as_slice),
        Some(b"png".as_slice()),
        "and the picture it left behind is still there"
    );
}

#[tokio::test]
async fn test_a_restart_carries_on_where_the_sequence_left_off() {
    let scratch = Scratch::new();

    {
        let state = served_from(&scratch.0);
        state
            .record("box_1", Actor::Agent, TraceEvent::BoxDeleted)
            .await;
        state
            .record("box_1", Actor::Agent, TraceEvent::BoxDeleted)
            .await;
    }

    let restarted = served_from(&scratch.0);
    restarted
        .record("box_1", Actor::System, TraceEvent::BoxDeleted)
        .await;

    let entries = restarted
        .store
        .entries("box_1", None, 10)
        .await
        .expect("read back");

    assert_eq!(
        entries.len(),
        3,
        "the second run added to the trace rather than writing over it"
    );

    let mut sequences: Vec<u64> = entries.iter().map(|entry| entry.seq).collect();
    sequences.dedup();
    assert_eq!(sequences.len(), 3, "and no sequence was handed out twice");
}
