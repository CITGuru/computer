//! What happened to a box, in order.
//!
//! Every action this API performs is recorded with the actor that asked for
//! it. A person's input is not: it arrives over VNC and goes straight into the
//! box, so nothing out here sees a keystroke. What the trace records instead is
//! custody — when a screen was handed over and when it came back. A person's
//! work is attributable to an interval, not to an action, and a reader that
//! assumes otherwise is reading a claim this server cannot make.
//!
//! Frames carry the weaker claim still: the actor on one is whoever held the
//! screen when it was captured, not whoever changed it.
//!
//! Frames are held by content, so a desktop that sits still costs one copy
//! however long it sits, and an entry is written only when the screen actually
//! changed.
//!
//! A trace outlives the box it describes: removing a box must not remove the
//! record of what was done in it.

use computer_api::{Actor, TraceEntry, TraceEvent};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Entries kept per box. Past this the oldest go, because a trace is a record
/// and not a database.
const MAX_ENTRIES: usize = 10_000;
/// Distinct frames kept per box. An entry whose frame has been evicted still
/// names its hash, and asking for it answers that it is no longer held.
const MAX_FRAMES: usize = 256;
/// Boxes whose traces are kept after they are gone.
const MAX_TRACES: usize = 256;

#[derive(Default)]
pub struct Trace {
    next: AtomicU64,
    entries: Mutex<VecDeque<TraceEntry>>,
    frames: Mutex<Frames>,
    /// The last frame written per screen, so an idle screen polled in a loop
    /// adds nothing.
    last: Mutex<HashMap<u32, String>>,
}

#[derive(Default)]
struct Frames {
    by_hash: HashMap<String, Arc<Vec<u8>>>,
    order: VecDeque<String>,
}

impl Trace {
    pub fn record(&self, actor: Actor, event: TraceEvent) -> u64 {
        self.write(actor, event, None)
    }

    /// Keep a frame, and write an entry only where the screen moved.
    ///
    /// Returns whether it was worth recording, so a caller polling a still
    /// screen can tell it changed nothing.
    pub fn note_frame(&self, actor: Actor, screen: u32, hash: &str, png: &[u8]) -> bool {
        self.put_frame(hash, png);

        {
            let Ok(mut last) = self.last.lock() else {
                return false;
            };
            if last.get(&screen).map(String::as_str) == Some(hash) {
                return false;
            }
            last.insert(screen, hash.to_string());
        }

        self.write(actor, TraceEvent::Frame { screen }, Some(hash.to_string()));
        true
    }

    pub fn frame(&self, hash: &str) -> Option<Arc<Vec<u8>>> {
        self.frames.lock().ok()?.by_hash.get(hash).cloned()
    }

    /// Entries after `after`, oldest first.
    pub fn entries(&self, after: Option<u64>, limit: usize) -> Vec<TraceEntry> {
        let Ok(entries) = self.entries.lock() else {
            return Vec::new();
        };

        entries
            .iter()
            .filter(|entry| after.is_none_or(|seq| entry.seq > seq))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn write(&self, actor: Actor, event: TraceEvent, frame: Option<String>) -> u64 {
        let seq = self.next.fetch_add(1, Ordering::Relaxed);
        let entry = TraceEntry {
            seq,
            at_ms: now_ms(),
            actor,
            event,
            frame,
        };

        if let Ok(mut entries) = self.entries.lock() {
            entries.push_back(entry);
            while entries.len() > MAX_ENTRIES {
                entries.pop_front();
            }
        }

        seq
    }

    fn put_frame(&self, hash: &str, png: &[u8]) {
        let Ok(mut frames) = self.frames.lock() else {
            return;
        };

        if frames.by_hash.contains_key(hash) {
            return;
        }

        frames
            .by_hash
            .insert(hash.to_string(), Arc::new(png.to_vec()));
        frames.order.push_back(hash.to_string());

        while frames.order.len() > MAX_FRAMES {
            let Some(oldest) = frames.order.pop_front() else {
                break;
            };
            frames.by_hash.remove(&oldest);
        }
    }
}

/// Every box's trace, including boxes that are gone.
#[derive(Default)]
pub struct Traces {
    by_box: Mutex<HashMap<String, Arc<Trace>>>,
    order: Mutex<VecDeque<String>>,
}

impl Traces {
    /// The trace for this box, started if this is the first thing it did.
    pub fn of(&self, id: &str) -> Arc<Trace> {
        if let Ok(traces) = self.by_box.lock()
            && let Some(trace) = traces.get(id)
        {
            return Arc::clone(trace);
        }

        let trace = Arc::new(Trace::default());

        if let Ok(mut traces) = self.by_box.lock() {
            let trace = Arc::clone(traces.entry(id.to_string()).or_insert(trace));

            if let Ok(mut order) = self.order.lock() {
                order.push_back(id.to_string());
                while order.len() > MAX_TRACES {
                    let Some(oldest) = order.pop_front() else {
                        break;
                    };
                    traces.remove(&oldest);
                }
            }

            return trace;
        }

        trace
    }

    pub fn get(&self, id: &str) -> Option<Arc<Trace>> {
        self.by_box.lock().ok()?.get(id).cloned()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entries_are_numbered_in_the_order_they_happened() {
        let trace = Trace::default();
        trace.record(Actor::Agent, TraceEvent::BoxDeleted);
        trace.record(Actor::System, TraceEvent::BoxDeleted);

        let entries = trace.entries(None, 10);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].seq < entries[1].seq);
    }

    #[test]
    fn test_reading_from_a_sequence_returns_only_what_came_after() {
        let trace = Trace::default();
        let first = trace.record(Actor::Agent, TraceEvent::BoxDeleted);
        trace.record(Actor::Agent, TraceEvent::BoxDeleted);

        let entries = trace.entries(Some(first), 10);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].seq > first);
    }

    #[test]
    fn test_a_screen_that_did_not_move_writes_nothing() {
        let trace = Trace::default();

        assert!(trace.note_frame(Actor::Agent, 0, "aaa", b"png"));
        assert!(!trace.note_frame(Actor::Agent, 0, "aaa", b"png"));
        assert!(trace.note_frame(Actor::Agent, 0, "bbb", b"other"));

        assert_eq!(trace.len(), 2, "the unchanged frame was not recorded");
    }

    #[test]
    fn test_two_screens_are_tracked_apart() {
        let trace = Trace::default();

        assert!(trace.note_frame(Actor::Agent, 0, "aaa", b"png"));
        assert!(
            trace.note_frame(Actor::Agent, 1, "aaa", b"png"),
            "screen 1 showing what screen 0 shows is still news about screen 1"
        );
    }

    #[test]
    fn test_a_frame_is_held_once_however_often_it_is_seen() {
        let trace = Trace::default();
        trace.note_frame(Actor::Agent, 0, "aaa", b"png");
        trace.note_frame(Actor::Agent, 1, "aaa", b"png");

        assert_eq!(trace.frame("aaa").as_deref(), Some(&b"png".to_vec()));
    }

    #[test]
    fn test_a_frame_nobody_kept_is_absent_rather_than_wrong() {
        assert!(Trace::default().frame("never-seen").is_none());
    }

    #[test]
    fn test_a_trace_outlives_the_box() {
        let traces = Traces::default();
        traces
            .of("box_1")
            .record(Actor::Agent, TraceEvent::BoxDeleted);

        let kept = traces.get("box_1").expect("the record survives the box");
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn test_asking_twice_gets_the_same_trace() {
        let traces = Traces::default();
        traces
            .of("box_1")
            .record(Actor::Agent, TraceEvent::BoxDeleted);
        traces
            .of("box_1")
            .record(Actor::Agent, TraceEvent::BoxDeleted);

        assert_eq!(traces.of("box_1").len(), 2);
    }
}
