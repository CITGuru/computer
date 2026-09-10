//! What every backend must do, asserted once.

use crate::now_ms;
use crate::{Blobs, BoxRecord, Frames, Store};
use computer_api::{Actor, Placement, Spec, TraceEvent};

fn record(id: &str, width: u32) -> BoxRecord {
    BoxRecord {
        id: id.to_string(),
        spec: Spec::default(),
        placement: Placement::default(),
        width,
        height: 800,
        screens: 1,
        created_at_ms: 1_700_000_000_000,
        expires_at_ms: None,
    }
}

fn wrote() -> TraceEvent {
    TraceEvent::BoxDeleted
}

pub async fn store(store: &dyn Store) {
    let missing = store.get_box("never_stored").await.expect("asked");
    assert!(
        missing.is_none(),
        "a box with no record is absent, not an error"
    );

    store.put_box(&record("box_1", 1280)).await.expect("stored");

    let held = store.get_box("box_1").await.expect("asked");
    assert_eq!(
        held.as_ref().map(|held| held.width),
        Some(1280),
        "the record came back as it went in"
    );

    store.put_box(&record("box_1", 640)).await.expect("stored");
    let held = store.get_box("box_1").await.expect("asked");
    assert_eq!(
        held.map(|held| held.width),
        Some(640),
        "writing an id again replaces what it held"
    );

    store.put_box(&record("box_2", 1280)).await.expect("stored");

    let once = store.list_boxes().await.expect("listed");
    let twice = store.list_boxes().await.expect("listed");
    assert_eq!(once.len(), 2, "both records are listed");
    assert_eq!(
        once.iter().map(|held| &held.id).collect::<Vec<_>>(),
        twice.iter().map(|held| &held.id).collect::<Vec<_>>(),
        "and listing twice answers the same way"
    );

    let empty = store.entries("box_1", None, 10).await.expect("asked");
    assert!(
        empty.is_empty(),
        "a box nothing has been written about has no entries"
    );

    let first = store
        .append("box_1", Actor::Agent, wrote(), None)
        .await
        .expect("appended");
    let second = store
        .append("box_1", Actor::Person, wrote(), Some("aaa".to_string()))
        .await
        .expect("appended");
    assert!(second > first, "a sequence only goes up");

    let entries = store.entries("box_1", None, 10).await.expect("asked");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].seq, first, "oldest first");
    assert_eq!(entries[1].seq, second);
    assert_eq!(
        entries[1].frame.as_deref(),
        Some("aaa"),
        "the frame an entry left behind is named in it"
    );
    assert!(
        entries[0].at_ms > 0,
        "an entry says when, so a trace can be read as a timeline"
    );
    assert!(
        matches!(entries[1].actor, Actor::Person),
        "and who asked for it"
    );

    let after = store
        .entries("box_1", Some(first), 10)
        .await
        .expect("asked");
    assert_eq!(after.len(), 1, "reading from a sequence excludes it");
    assert_eq!(after[0].seq, second);

    let limited = store.entries("box_1", None, 1).await.expect("asked");
    assert_eq!(limited.len(), 1, "a limit is a limit");
    assert_eq!(limited[0].seq, first, "and it takes the oldest");

    let other = store
        .append("box_2", Actor::Agent, wrote(), None)
        .await
        .expect("appended");
    assert_eq!(
        other, first,
        "sequences are per box: a route asks for one box's trace and pages it \
         by number, so a shared counter would leave gaps a caller reads as loss"
    );

    store
        .append("never_stored", Actor::System, wrote(), None)
        .await
        .expect("appended without a record");
    let orphan = store
        .entries("never_stored", None, 10)
        .await
        .expect("asked");
    assert_eq!(
        orphan.len(),
        1,
        "a box is written about before anything is stored for it: an adopted \
         box is found running and has no record yet"
    );

    store.forget_box("box_1").await.expect("forgotten");
    assert!(
        store.get_box("box_1").await.expect("asked").is_none(),
        "forgetting drops the record"
    );
    assert!(
        store
            .entries("box_1", None, 10)
            .await
            .expect("asked")
            .is_empty(),
        "and the trace with it"
    );
    assert!(
        store.get_box("box_2").await.expect("asked").is_some(),
        "and nothing else"
    );
}

/// What a sweep must be able to do.
pub async fn pruning(store: &dyn Store) {
    for _ in 0..3 {
        store
            .append("box_p", Actor::Agent, wrote(), Some("aaa".to_string()))
            .await
            .expect("appended");
    }

    let ahead = now_ms() + 60_000;
    let behind = now_ms() - 60_000;

    assert!(
        store
            .frames_before("box_p", behind)
            .await
            .expect("asked")
            .is_empty(),
        "nothing written a minute ago is older than a minute ago"
    );
    assert_eq!(
        store
            .frames_before("box_p", ahead)
            .await
            .expect("asked")
            .len(),
        3,
        "and every entry naming a frame is named back"
    );

    assert_eq!(
        store.prune_entries("box_p", behind).await.expect("pruned"),
        0,
        "a cutoff before everything drops nothing"
    );
    assert_eq!(
        store.entries("box_p", None, 10).await.expect("asked").len(),
        3
    );

    assert_eq!(
        store.prune_entries("box_p", ahead).await.expect("pruned"),
        3,
        "and a cutoff after everything drops it all"
    );
    assert!(
        store
            .entries("box_p", None, 10)
            .await
            .expect("asked")
            .is_empty(),
        "which is what leaves a box with nothing left to keep"
    );

    let after = store
        .append("box_p", Actor::Agent, wrote(), None)
        .await
        .expect("appended");
    assert!(
        after >= 3,
        "a pruned trace does not reissue the sequences it dropped: an entry \
         numbered behind one a caller already read looks like loss"
    );

    store.forget_box("box_p").await.expect("forgotten");
}

/// Dropping named frames rather than a whole box's.
pub async fn dropping(frames: &dyn Frames) {
    frames.put("box_d", "aaa", b"first").await.expect("held");
    frames.put("box_d", "bbb", b"second").await.expect("held");

    frames
        .drop_frames("box_d", &["aaa".to_string(), "never_held".to_string()])
        .await
        .expect("a hash nothing holds is not an error");

    assert!(
        frames.get("box_d", "aaa").await.expect("asked").is_none(),
        "the named frame went"
    );
    assert!(
        frames.get("box_d", "bbb").await.expect("asked").is_some(),
        "and the one beside it stayed, which is the whole point of naming them"
    );

    frames.forget("box_d").await.expect("forgotten");
}

pub async fn frames(frames: &dyn Frames) {
    let missing = frames.get("box_1", "never_held").await.expect("asked");
    assert!(
        missing.is_none(),
        "a hash nothing holds is absent, not an error"
    );

    frames.put("box_1", "aaa", b"first").await.expect("held");
    assert_eq!(
        frames
            .get("box_1", "aaa")
            .await
            .expect("asked")
            .as_deref()
            .map(Vec::as_slice),
        Some(b"first".as_slice()),
        "the frame came back as it went in"
    );

    frames.put("box_1", "aaa", b"second").await.expect("held");
    assert_eq!(
        frames
            .get("box_1", "aaa")
            .await
            .expect("asked")
            .as_deref()
            .map(Vec::as_slice),
        Some(b"first".as_slice()),
        "a hash already held is left alone: content addresses content, so a \
         second write under one hash is the same picture or a caller's bug"
    );

    frames
        .put("box_2", "aaa", b"elsewhere")
        .await
        .expect("held");
    frames.forget("box_1").await.expect("forgotten");

    assert!(
        frames.get("box_1", "aaa").await.expect("asked").is_none(),
        "forgetting a box drops its frames"
    );
    assert!(
        frames.get("box_2", "aaa").await.expect("asked").is_some(),
        "and leaves another box holding the same hash alone"
    );
}

pub async fn blobs(blobs: &dyn Blobs) {
    assert!(
        blobs
            .get("boxes/box_1/main.json")
            .await
            .expect("asked")
            .is_none(),
        "a key nothing holds is absent, not an error"
    );

    blobs
        .put("boxes/box_1/main.json", b"first")
        .await
        .expect("written");
    assert_eq!(
        blobs.get("boxes/box_1/main.json").await.expect("asked"),
        Some(b"first".to_vec())
    );

    blobs
        .put("boxes/box_1/main.json", b"second")
        .await
        .expect("written");
    assert_eq!(
        blobs.get("boxes/box_1/main.json").await.expect("asked"),
        Some(b"second".to_vec()),
        "a key is replaced, not appended to"
    );

    for segment in ["000000000001", "000000000002", "000000000003"] {
        blobs
            .put(&format!("boxes/box_1/traces/{segment}.jsonl"), b"{}")
            .await
            .expect("written");
    }
    blobs
        .put("boxes/box_2/traces/000000000001.jsonl", b"{}")
        .await
        .expect("written");

    let listed = blobs
        .list("boxes/box_1/traces/", None)
        .await
        .expect("listed");
    assert_eq!(
        listed,
        vec![
            "boxes/box_1/traces/000000000001.jsonl".to_string(),
            "boxes/box_1/traces/000000000002.jsonl".to_string(),
            "boxes/box_1/traces/000000000003.jsonl".to_string(),
        ],
        "a prefix lists only its own keys, in lexical order: that order is \
         where a segmented trace gets its sequence back from"
    );

    let rest = blobs
        .list(
            "boxes/box_1/traces/",
            Some("boxes/box_1/traces/000000000001.jsonl"),
        )
        .await
        .expect("listed");
    assert_eq!(
        rest,
        vec![
            "boxes/box_1/traces/000000000002.jsonl".to_string(),
            "boxes/box_1/traces/000000000003.jsonl".to_string(),
        ],
        "starting after a key excludes it"
    );

    blobs.delete_prefix("boxes/box_1/").await.expect("deleted");

    assert!(
        blobs
            .list("boxes/box_1/", None)
            .await
            .expect("listed")
            .is_empty(),
        "deleting a prefix takes the subtree"
    );
    assert_eq!(
        blobs
            .list("boxes/box_2/", None)
            .await
            .expect("listed")
            .len(),
        1,
        "and stops there"
    );
}
