use crate::now_ms;
use crate::{Blobs, BoxRecord, Frames, ImageRecord, RuntimeRecord, Sealed, Store};
use holm_api::{Actor, Placement, Spec, TraceEvent};

fn record(id: &str, width: u32) -> BoxRecord {
    BoxRecord {
        owner: None,
        runtime: "docker".to_string(),
        id: id.to_string(),
        spec: Spec::default(),
        placement: Placement::default(),
        width,
        height: 800,
        screens: 1,
        created_at_ms: 1_700_000_000_000,
        expires_at_ms: None,
        deleted_at_ms: None,
    }
}

fn runtime(name: &str, key: &str) -> RuntimeRecord {
    RuntimeRecord {
        owner: None,
        name: name.to_string(),
        provider: "e2b".to_string(),
        fields: serde_json::json!({ "region": "eu" }),
        secrets: std::collections::BTreeMap::from([("api_key".to_string(), Sealed::of(key))]),
        created_at_ms: 1_700_000_000_000,
        updated_at_ms: 1_700_000_000_000,
    }
}

fn wrote() -> TraceEvent {
    TraceEvent::BoxDeleted
}

pub async fn images(store: &dyn Store) {
    let made = |runtime: &str, digest: &str, reference: &str| ImageRecord {
        runtime: runtime.to_string(),
        spec_digest: digest.to_string(),
        reference: reference.to_string(),
        built_at_ms: 1_700_000_000_000,
        bytes: Some(400 * 1024 * 1024),
    };

    assert!(
        store
            .get_image("smolvm", "never-built")
            .await
            .expect("asked")
            .is_none(),
        "an image nobody built is not an error"
    );

    store
        .put_image(&made("smolvm", "abc", "/images/abc.tar"))
        .await
        .expect("kept");
    store
        .put_image(&made("cloud", "abc", "tmpl-1"))
        .await
        .expect("kept");

    let held = store
        .get_image("smolvm", "abc")
        .await
        .expect("asked")
        .expect("a record");
    assert_eq!(
        held.reference, "/images/abc.tar",
        "one spec has a different image on every runtime, so the runtime is part of the key"
    );
    assert_eq!(
        store
            .get_image("cloud", "abc")
            .await
            .expect("asked")
            .map(|held| held.reference),
        Some("tmpl-1".to_string())
    );

    store
        .put_image(&made("cloud", "abc", "tmpl-2"))
        .await
        .expect("built again");
    assert_eq!(
        store
            .get_image("cloud", "abc")
            .await
            .expect("asked")
            .map(|held| held.reference),
        Some("tmpl-2".to_string()),
        "a rebuild replaces what was there rather than sitting beside it"
    );

    assert_eq!(store.list_images().await.expect("listed").len(), 2);

    store.forget_image("cloud", "abc").await.expect("forgotten");
    assert!(
        store
            .get_image("cloud", "abc")
            .await
            .expect("asked")
            .is_none()
    );
    assert_eq!(store.list_images().await.expect("listed").len(), 1);
}

pub async fn locks(store: std::sync::Arc<dyn Store>) {
    use std::sync::atomic::{AtomicBool, Ordering};

    let first = store.lock("box_1/take").await.expect("locked");
    let other = store
        .lock("box_2/take")
        .await
        .expect("another name is free");
    drop(other);

    let entered = std::sync::Arc::new(AtomicBool::new(false));
    let waiting = {
        let (store, entered) = (
            std::sync::Arc::clone(&store),
            std::sync::Arc::clone(&entered),
        );
        tokio::spawn(async move {
            let _held = store.lock("box_1/take").await.expect("locked");
            entered.store(true, Ordering::SeqCst);
        })
    };

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !entered.load(Ordering::SeqCst),
        "a second holder waits while the first holds the name"
    );

    drop(first);
    tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .expect("the second holder gets the lock once the first lets go")
        .expect("joined");
    assert!(entered.load(Ordering::SeqCst));
}

pub async fn jobs(store: &dyn Store) {
    use crate::JobRecord;

    assert_eq!(store.claim_job("w1", 60_000).await.expect("asked"), None);

    let made = |id: &str, at: u64| JobRecord {
        created_at_ms: at,
        ..JobRecord::new(id, "launch", format!("{{\"box\":\"{id}\"}}"))
    };
    store.push_job(&made("job_b", 2_000)).await.expect("pushed");
    store.push_job(&made("job_a", 1_000)).await.expect("pushed");

    let first = store
        .claim_job("w1", 60_000)
        .await
        .expect("asked")
        .expect("a job");
    assert_eq!(first.id, "job_a", "the oldest job goes first");
    assert_eq!(first.kind, "launch");
    assert_eq!(first.body, r#"{"box":"job_a"}"#);
    assert_eq!(first.attempts, 1);

    let second = store
        .claim_job("w2", 60_000)
        .await
        .expect("asked")
        .expect("a job");
    assert_eq!(
        second.id, "job_b",
        "a claimed job is not given to a second worker"
    );
    assert_eq!(store.claim_job("w3", 60_000).await.expect("asked"), None);

    assert!(
        store
            .renew_job("job_a", "w1", 60_000)
            .await
            .expect("renewed")
    );
    assert!(
        !store.renew_job("job_a", "w2", 60_000).await.expect("asked"),
        "only the worker that holds a job can keep it"
    );

    store.finish_job("job_a").await.expect("finished");
    assert!(!store.renew_job("job_a", "w1", 60_000).await.expect("asked"));

    assert!(store.renew_job("job_b", "w2", 0).await.expect("renewed"));
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let again = store
        .claim_job("w3", 60_000)
        .await
        .expect("asked")
        .expect("a job whose worker went away is given to another");
    assert_eq!((again.id.as_str(), again.attempts), ("job_b", 2));

    store.finish_job("job_b").await.expect("finished");
    assert_eq!(store.claim_job("w1", 60_000).await.expect("asked"), None);
}

pub async fn events(store: &dyn Store) {
    use crate::EventRecord;

    let made = |kind: &str, at: u64, owner: Option<&str>| EventRecord {
        seq: 0,
        at_ms: at,
        kind: kind.to_string(),
        owner: owner.map(str::to_string),
        box_id: Some("box_1".to_string()),
        runtime: Some("docker".to_string()),
        data: serde_json::json!({ "state": "ready" }),
    };

    assert!(
        store
            .events_after(0, u64::MAX / 2, 10)
            .await
            .expect("asked")
            .is_empty()
    );

    let first = store
        .append_event(&made("box.created", 1_000, Some("ws_a")))
        .await
        .expect("kept");
    let second = store
        .append_event(&made("box.ready", 2_000, Some("ws_a")))
        .await
        .expect("kept");
    let third = store
        .append_event(&made("box.removed", 3_000, None))
        .await
        .expect("kept");
    assert!(
        first < second && second < third,
        "each event has the next number"
    );

    let all = store.events_after(0, u64::MAX / 2, 10).await.expect("read");
    assert_eq!(
        all.iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        ["box.created", "box.ready", "box.removed"]
    );
    assert_eq!(all[0].seq, first);
    assert_eq!(all[0].owner.as_deref(), Some("ws_a"));
    assert_eq!(all[2].owner, None);
    assert_eq!(all[1].data["state"], "ready");

    let later = store
        .events_after(first, u64::MAX / 2, 1)
        .await
        .expect("read");
    assert_eq!(later.len(), 1, "a page holds at most the limit");
    assert_eq!(later[0].seq, second, "and starts after the cursor");

    assert_eq!(
        store.events_after(0, 2_000, 10).await.expect("read").len(),
        2,
        "an event newer than the cut is left for the next read"
    );

    assert_eq!(store.prune_events(2_500).await.expect("pruned"), 2);
    let left = store.events_after(0, u64::MAX / 2, 10).await.expect("read");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].seq, third, "what is left keeps its number");
}

pub async fn notes(store: &dyn Store) {
    let far = now_ms() + 3_600_000;

    assert_eq!(
        store.get_note("states", "ws_a/login").await.expect("asked"),
        None
    );

    store
        .put_note("states", "ws_a/login", "one", None)
        .await
        .expect("kept");
    store
        .put_note("states", "ws_a/shop cart", "two", Some(far))
        .await
        .expect("kept");
    store
        .put_note("states", "ws_b/login", "three", None)
        .await
        .expect("kept");
    store
        .put_note("labels", "ws_a/login", "another kind", None)
        .await
        .expect("kept");
    store
        .put_note("states", "ws_a/old", "gone", Some(1))
        .await
        .expect("kept");

    assert_eq!(
        store.get_note("states", "ws_a/login").await.expect("asked"),
        Some("one".to_string())
    );
    assert_eq!(
        store.get_note("states", "ws_a/old").await.expect("asked"),
        None,
        "a note past its time reads as absent before anything prunes it"
    );

    store
        .put_note("states", "ws_a/login", "one again", None)
        .await
        .expect("replaced");
    assert_eq!(
        store.list_notes("states", "ws_a/").await.expect("listed"),
        vec![
            ("ws_a/login".to_string(), "one again".to_string()),
            ("ws_a/shop cart".to_string(), "two".to_string()),
        ],
        "one kind, one prefix, in key order, and only what is still live"
    );
    assert_eq!(
        store.list_notes("states", "").await.expect("listed").len(),
        3
    );

    assert_eq!(
        store.prune_notes(now_ms()).await.expect("pruned"),
        1,
        "only the note past its time goes"
    );

    store
        .put_note("states", "ws_a/login2", "a longer key", None)
        .await
        .expect("kept");
    store
        .forget_note("states", "ws_a/login")
        .await
        .expect("forgotten");
    assert_eq!(
        store
            .get_note("states", "ws_a/login2")
            .await
            .expect("asked"),
        Some("a longer key".to_string()),
        "forgetting one key leaves a key that starts with it"
    );

    store
        .forget_notes("states", "ws_a/")
        .await
        .expect("forgotten");
    assert_eq!(
        store.list_notes("states", "").await.expect("listed"),
        vec![("ws_b/login".to_string(), "three".to_string())]
    );
    assert_eq!(
        store.get_note("labels", "ws_a/login").await.expect("asked"),
        Some("another kind".to_string()),
        "forgetting one kind leaves the others"
    );
}

pub async fn runtimes(store: &dyn Store) {
    assert!(
        store
            .get_runtime("never_stored")
            .await
            .expect("asked")
            .is_none(),
        "a runtime nobody added is not an error"
    );

    store
        .put_runtime(&runtime("cloud", "sealed-1"))
        .await
        .expect("kept");
    store
        .put_runtime(&runtime("eu", "sealed-2"))
        .await
        .expect("kept");

    let held = store
        .get_runtime("cloud")
        .await
        .expect("asked")
        .expect("a runtime");
    assert_eq!(held.provider, "e2b");
    assert_eq!(held.fields["region"], "eu");
    assert_eq!(
        held.secrets["api_key"].as_str(),
        "sealed-1",
        "what the server sealed comes back as it went down"
    );

    store
        .put_runtime(&runtime("cloud", "sealed-3"))
        .await
        .expect("kept again");
    assert_eq!(
        store
            .get_runtime("cloud")
            .await
            .expect("asked")
            .expect("a runtime")
            .secrets["api_key"]
            .as_str(),
        "sealed-3",
        "a new key replaces the old one rather than sitting beside it"
    );

    let listed = store.list_runtimes().await.expect("listed");
    assert_eq!(listed.len(), 2);

    store.forget_runtime("cloud").await.expect("forgotten");
    assert!(store.get_runtime("cloud").await.expect("asked").is_none());
    assert_eq!(store.list_runtimes().await.expect("listed").len(), 1);
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
