//! Everything in this process, and nothing after it.

use crate::error::poisoned;
use crate::{Blobs, BoxRecord, Frames, Result, Store, now_ms};
use async_trait::async_trait;
use computer_api::{Actor, TraceEntry, TraceEvent};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Entries kept per box.
const MAX_ENTRIES: usize = 10_000;
const MAX_FRAMES: usize = 256;
/// Boxes whose traces are kept after they are gone.
const MAX_TRACES: usize = 256;

#[derive(Default)]
pub struct Memory {
    boxes: Mutex<BTreeMap<String, BoxRecord>>,
    traces: Mutex<HashMap<String, Arc<Trace>>>,
    /// First-touch order, for the cap on traces.
    order: Mutex<VecDeque<String>>,
    frames: Mutex<HashMap<String, Held>>,
}

#[derive(Default)]
struct Trace {
    next: AtomicU64,
    entries: Mutex<VecDeque<TraceEntry>>,
}

#[derive(Default)]
struct Held {
    by_hash: HashMap<String, Arc<Vec<u8>>>,
    order: VecDeque<String>,
}

impl Memory {
    fn trace(&self, id: &str) -> Result<Arc<Trace>> {
        if let Some(trace) = self.traces.lock().map_err(poisoned)?.get(id) {
            return Ok(Arc::clone(trace));
        }

        let mut traces = self.traces.lock().map_err(poisoned)?;

        let mine = !traces.contains_key(id);
        let trace = Arc::clone(traces.entry(id.to_string()).or_default());

        if mine {
            let mut order = self.order.lock().map_err(poisoned)?;
            order.push_back(id.to_string());
            while order.len() > MAX_TRACES {
                let Some(oldest) = order.pop_front() else {
                    break;
                };
                traces.remove(&oldest);
            }
        }

        Ok(trace)
    }
}

#[async_trait]
impl Store for Memory {
    async fn put_box(&self, record: &BoxRecord) -> Result<()> {
        self.boxes
            .lock()
            .map_err(poisoned)?
            .insert(record.id.clone(), record.clone());

        Ok(())
    }

    async fn get_box(&self, id: &str) -> Result<Option<BoxRecord>> {
        Ok(self.boxes.lock().map_err(poisoned)?.get(id).cloned())
    }

    async fn list_boxes(&self) -> Result<Vec<BoxRecord>> {
        Ok(self
            .boxes
            .lock()
            .map_err(poisoned)?
            .values()
            .cloned()
            .collect())
    }

    async fn forget_box(&self, id: &str) -> Result<()> {
        self.boxes.lock().map_err(poisoned)?.remove(id);
        self.traces.lock().map_err(poisoned)?.remove(id);
        self.order
            .lock()
            .map_err(poisoned)?
            .retain(|held| held != id);

        Ok(())
    }

    async fn append(
        &self,
        id: &str,
        actor: Actor,
        event: TraceEvent,
        frame: Option<String>,
    ) -> Result<u64> {
        let trace = self.trace(id)?;
        let seq = trace.next.fetch_add(1, Ordering::Relaxed);

        let mut entries = trace.entries.lock().map_err(poisoned)?;
        entries.push_back(TraceEntry {
            seq,
            at_ms: now_ms(),
            actor,
            event,
            frame,
        });

        while entries.len() > MAX_ENTRIES {
            entries.pop_front();
        }

        Ok(seq)
    }

    async fn entries(&self, id: &str, after: Option<u64>, limit: usize) -> Result<Vec<TraceEntry>> {
        let held = self.traces.lock().map_err(poisoned)?.get(id).cloned();
        let Some(trace) = held else {
            return Ok(Vec::new());
        };

        let entries = trace.entries.lock().map_err(poisoned)?;

        Ok(entries
            .iter()
            .filter(|entry| after.is_none_or(|seq| entry.seq > seq))
            .take(limit)
            .cloned()
            .collect())
    }

    async fn frames_before(&self, id: &str, before_ms: u64) -> Result<Vec<String>> {
        let held = self.traces.lock().map_err(poisoned)?.get(id).cloned();
        let Some(trace) = held else {
            return Ok(Vec::new());
        };

        let entries = trace.entries.lock().map_err(poisoned)?;

        Ok(entries
            .iter()
            .filter(|entry| entry.at_ms < before_ms)
            .filter_map(|entry| entry.frame.clone())
            .collect())
    }

    async fn prune_entries(&self, id: &str, before_ms: u64) -> Result<u64> {
        let held = self.traces.lock().map_err(poisoned)?.get(id).cloned();
        let Some(trace) = held else {
            return Ok(0);
        };

        let mut entries = trace.entries.lock().map_err(poisoned)?;
        let before = entries.len();
        entries.retain(|entry| entry.at_ms >= before_ms);

        Ok((before - entries.len()) as u64)
    }
}

#[async_trait]
impl Frames for Memory {
    async fn put(&self, id: &str, hash: &str, png: &[u8]) -> Result<()> {
        let mut frames = self.frames.lock().map_err(poisoned)?;
        let held = frames.entry(id.to_string()).or_default();

        if held.by_hash.contains_key(hash) {
            return Ok(());
        }

        held.by_hash
            .insert(hash.to_string(), Arc::new(png.to_vec()));
        held.order.push_back(hash.to_string());

        while held.order.len() > MAX_FRAMES {
            let Some(oldest) = held.order.pop_front() else {
                break;
            };
            held.by_hash.remove(&oldest);
        }

        Ok(())
    }

    async fn get(&self, id: &str, hash: &str) -> Result<Option<Arc<Vec<u8>>>> {
        Ok(self
            .frames
            .lock()
            .map_err(poisoned)?
            .get(id)
            .and_then(|held| held.by_hash.get(hash).cloned()))
    }

    async fn drop_frames(&self, id: &str, hashes: &[String]) -> Result<()> {
        let mut frames = self.frames.lock().map_err(poisoned)?;
        let Some(held) = frames.get_mut(id) else {
            return Ok(());
        };

        for hash in hashes {
            held.by_hash.remove(hash);
        }
        held.order.retain(|hash| held.by_hash.contains_key(hash));

        Ok(())
    }

    async fn forget(&self, id: &str) -> Result<()> {
        self.frames.lock().map_err(poisoned)?.remove(id);

        Ok(())
    }
}

#[derive(Default)]
pub struct Bucket {
    keys: Mutex<BTreeMap<String, Vec<u8>>>,
}

#[async_trait]
impl Blobs for Bucket {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.keys.lock().map_err(poisoned)?.get(key).cloned())
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.keys
            .lock()
            .map_err(poisoned)?
            .insert(key.to_string(), bytes.to_vec());

        Ok(())
    }

    async fn list(&self, prefix: &str, start_after: Option<&str>) -> Result<Vec<String>> {
        Ok(self
            .keys
            .lock()
            .map_err(poisoned)?
            .keys()
            .filter(|key| key.starts_with(prefix))
            .filter(|key| start_after.is_none_or(|after| key.as_str() > after))
            .cloned()
            .collect())
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        self.keys
            .lock()
            .map_err(poisoned)?
            .retain(|key, _| !key.starts_with(prefix));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;

    #[tokio::test]
    async fn test_memory_behaves_like_a_store() {
        conformance::store(&Memory::default()).await;
    }

    #[tokio::test]
    async fn test_memory_behaves_like_a_frame_store() {
        conformance::frames(&Memory::default()).await;
    }

    #[tokio::test]
    async fn test_a_bucket_behaves_like_a_blob_backend() {
        conformance::blobs(&Bucket::default()).await;
    }

    #[tokio::test]
    async fn test_memory_prunes() {
        conformance::pruning(&Memory::default()).await;
    }

    #[tokio::test]
    async fn test_memory_drops_named_frames() {
        conformance::dropping(&Memory::default()).await;
    }

    #[tokio::test]
    async fn test_a_trace_stays_bounded() {
        let memory = Memory::default();

        for _ in 0..MAX_ENTRIES + 10 {
            memory
                .append("box_1", Actor::Agent, TraceEvent::BoxDeleted, None)
                .await
                .expect("appended");
        }

        let entries = memory
            .entries("box_1", None, MAX_ENTRIES * 2)
            .await
            .expect("read back");

        assert_eq!(entries.len(), MAX_ENTRIES, "the oldest were dropped");
        assert_eq!(entries[0].seq, 10, "and the ones dropped were the oldest");
    }

    #[tokio::test]
    async fn test_frames_stay_bounded_per_box() {
        let memory = Memory::default();

        for frame in 0..MAX_FRAMES + 1 {
            Frames::put(&memory, "box_1", &format!("hash_{frame}"), b"png")
                .await
                .expect("held");
        }

        assert!(
            Frames::get(&memory, "box_1", "hash_0")
                .await
                .expect("asked")
                .is_none(),
            "the first frame aged out"
        );
        assert!(
            Frames::get(&memory, "box_1", &format!("hash_{MAX_FRAMES}"))
                .await
                .expect("asked")
                .is_some(),
            "the newest is still held"
        );
    }
}
