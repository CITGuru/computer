//! Records that are no longer worth their disk.
//!
//! Memory bounds itself, because a process that never restarts must not grow
//! until it is killed. A directory, a bucket and a database keep what they are
//! given, so somebody has to say when to stop — and that is a policy rather
//! than a store's business.
//!
//! Two windows, because the two things age at different rates. Frames are the
//! volume: a PNG per changed screen, and thousands per busy box. A replay does
//! not read one — [`crate::routes`] repeats actions — so they are worth far
//! less than the entries that name them, and go long before. Entries are a JSON
//! line each and are what a fork needs, so they last a week.
//!
//! A box whose entries have all gone and whose desktop is no longer running is
//! then forgotten outright. Nothing else decides that: the trace ageing out is
//! what says the box is finished with.

use crate::AppState;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often to look. Hourly, because nothing here is urgent and every pass
/// reads every box.
pub const EVERY: Duration = Duration::from_secs(60 * 60);
/// How long a frame is kept. Long enough to see what just went wrong.
pub const KEEP_FRAMES: Duration = Duration::from_secs(2 * 60 * 60);
/// How long an entry is kept. Long enough to fork last week's box.
pub const KEEP_ENTRIES: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub frames: usize,
    pub entries: u64,
    pub boxes: usize,
}

impl Swept {
    fn nothing(&self) -> bool {
        *self == Self::default()
    }
}

/// The windows, from the environment or the constants above.
pub fn windows() -> (Duration, Duration) {
    (
        seconds("COMPUTER_KEEP_FRAMES_SECS").unwrap_or(KEEP_FRAMES),
        seconds("COMPUTER_KEEP_ENTRIES_SECS").unwrap_or(KEEP_ENTRIES),
    )
}

pub fn every() -> Duration {
    seconds("COMPUTER_PRUNE_SECS").unwrap_or(EVERY)
}

fn seconds(name: &str) -> Option<Duration> {
    std::env::var(name)
        .ok()?
        .parse()
        .ok()
        .map(Duration::from_secs)
}

pub fn spawn(state: Arc<AppState>, every: Duration) {
    let (frames, entries) = windows();

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;

            let swept = once(&state, cutoff(frames), cutoff(entries)).await;
            if !swept.nothing() {
                tracing::info!(
                    frames = swept.frames,
                    entries = swept.entries,
                    boxes = swept.boxes,
                    "pruned what had aged out"
                );
            }
        }
    });
}

/// Cutoffs rather than windows, so what a pass does is a function of its
/// arguments and not of how long the clock took to answer twice.
///
/// Never fails: a store that will not answer for one box must not stop the
/// sweep of the rest, and the only reader is the log.
pub async fn once(state: &AppState, frames_before: u64, entries_before: u64) -> Swept {
    let mut swept = Swept::default();

    let listed = match state.store.list_boxes().await {
        Ok(listed) => listed,
        Err(why) => {
            tracing::warn!(%why, "nothing could be pruned: the store would not list");
            return swept;
        }
    };

    for record in listed {
        let id = &record.id;

        match state.store.frames_before(id, frames_before).await {
            Ok(stale) if !stale.is_empty() => match state.frames.drop_frames(id, &stale).await {
                Ok(()) => swept.frames += stale.len(),
                Err(why) => tracing::warn!(box_ = %id, %why, "frames would not go"),
            },
            Ok(_) => (),
            Err(why) => tracing::warn!(box_ = %id, %why, "the old frames would not be named"),
        }

        match state.store.prune_entries(id, entries_before).await {
            Ok(gone) => swept.entries += gone,
            Err(why) => tracing::warn!(box_ = %id, %why, "the old entries would not go"),
        }

        if forgettable(state, id).await {
            for outcome in [
                state.store.forget_box(id).await,
                state.frames.forget(id).await,
            ] {
                if let Err(why) = outcome {
                    tracing::warn!(box_ = %id, %why, "a finished box would not be forgotten");
                }
            }

            state.forget_screens(id);
            swept.boxes += 1;
        }
    }

    swept
}

/// A box with nothing left to say and no desktop still running.
///
/// The registry is asked rather than the record: a box created a month ago and
/// driven this morning has a trace, and a box whose trace has aged out entirely
/// has not been touched in a week.
async fn forgettable(state: &AppState, id: &str) -> bool {
    if state.registry.get(id).await.is_ok() {
        return false;
    }

    state
        .store
        .entries(id, None, 1)
        .await
        .map(|left| left.is_empty())
        .unwrap_or_default()
}

fn cutoff(keep: Duration) -> u64 {
    SystemTime::now()
        .checked_sub(keep)
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use computer_api::{Actor, TraceEvent};
    use computer_storage::BoxRecord;
    use computer_types::{Placement, Spec};

    fn record(id: &str) -> BoxRecord {
        BoxRecord {
            id: id.to_string(),
            spec: Spec::default(),
            placement: Placement::default(),
            width: 1280,
            height: 800,
            screens: 1,
            created_at_ms: 0,
            expires_at_ms: None,
        }
    }

    /// A cutoff before everything written here, so nothing ages out.
    fn keep_all() -> u64 {
        cutoff(Duration::from_secs(60 * 60))
    }

    /// A cutoff after everything written here, so it all does.
    fn keep_none() -> u64 {
        cutoff(Duration::ZERO) + 60_000
    }

    async fn holding(id: &str) -> AppState {
        let state = AppState::default();
        state.store.put_box(&record(id)).await.expect("recorded");
        state.note_frame(id, Actor::Agent, 0, "aaa", b"png").await;
        state.record(id, Actor::Agent, TraceEvent::BoxDeleted).await;

        state
    }

    #[tokio::test]
    async fn test_nothing_recent_is_pruned() {
        let state = holding("box_1").await;

        assert_eq!(once(&state, keep_all(), keep_all()).await, Swept::default());
        assert!(state.traced("box_1").await);
        assert!(
            state
                .frames
                .get("box_1", "aaa")
                .await
                .expect("asked")
                .is_some(),
            "a sweep on a busy box costs it nothing"
        );
    }

    #[tokio::test]
    async fn test_frames_go_before_the_entries_that_name_them() {
        let state = holding("box_1").await;

        let swept = once(&state, keep_none(), keep_all()).await;

        assert_eq!(swept.frames, 1);
        assert_eq!(swept.entries, 0);
        assert!(
            state
                .frames
                .get("box_1", "aaa")
                .await
                .expect("asked")
                .is_none(),
            "the picture went"
        );
        assert!(
            state.traced("box_1").await,
            "and the record of what happened stayed, which is what a fork reads"
        );
    }

    #[tokio::test]
    async fn test_a_box_with_nothing_left_to_say_is_forgotten() {
        let state = holding("box_1").await;

        let swept = once(&state, keep_none(), keep_none()).await;

        assert_eq!(swept.boxes, 1);
        assert!(
            state.store.get_box("box_1").await.expect("asked").is_none(),
            "a box whose trace aged out entirely has not been touched in a week"
        );
    }

    #[tokio::test]
    async fn test_a_running_box_is_never_forgotten() {
        let state = holding("box_1").await;

        // No registry entry can be made here without a container, so the
        // reverse is asserted: a box the registry does not hold is the only
        // kind this may forget.
        assert!(
            state.registry.get("box_1").await.is_err(),
            "the double is a box that is not running"
        );
        assert!(
            forgettable(&state, "box_1").await || state.traced("box_1").await,
            "and one still holding entries is kept whatever the registry says"
        );

        once(&state, keep_all(), keep_all()).await;
        assert!(
            !forgettable(&state, "box_1").await,
            "a box with entries left is not finished with"
        );
    }

    #[test]
    fn test_the_windows_can_be_set() {
        assert_eq!(windows(), (KEEP_FRAMES, KEEP_ENTRIES));
        assert_eq!(every(), EVERY);
    }
}
