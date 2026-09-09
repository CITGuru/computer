//! A store shaped like a directory, over a backend that need not be one.
//!
//! ```text
//! boxes/{id}/main.json                     the record
//! boxes/{id}/traces/{first}-{last}.jsonl   the history, in segments
//! boxes/{id}/screens/{hash}.png            the frames
//! ```
//!
//! The layout is the index. There is no query underneath, so everything a
//! reader needs is in a key: a segment is named by the range it holds, and the
//! numbers are zero-padded because listing is lexical and `10` sorts before
//! `2`.
//!
//! # Segments rather than a log
//!
//! Object storage has no append. A trace therefore goes down as whole objects
//! that are never revisited, which is also what keeps the layout the same on a
//! disk and in a bucket: a directory copied to a bucket is a store, and back
//! again.
//!
//! Several entries per segment, which is why the name carries a range rather
//! than a number. One object per click would be one round trip per click, which
//! a disk absorbs and a bucket bills for.
//!
//! # What is not batched
//!
//! An entry saying who held the screen goes down before [`Store::append`]
//! answers. Those are the claims that would ever have to be defended, and a
//! crash losing the moment a person took a keyboard is not a trade worth
//! making. Clicks, frames and reads buffer: losing the tail of what an agent
//! did costs a reader detail and costs nobody a claim.
//!
//! A buffered entry is visible through [`Store::entries`] as soon as it is
//! written, so a caller that appends and reads back does not see a hole. What
//! it is not is durable, and the cost of that is worth stating: a process that
//! dies with a buffer loses those entries, and a reader that had already seen
//! their sequences will watch the next process hand the same numbers out again.
//! The window is one batch, because a box's first entry is a custody one and
//! every later one advances the watermark, but it is not nothing. Call
//! [`Store::flush`] before a process ends.
//!
//! # One writer per box
//!
//! A backend has no counter, so the next sequence is read once from the highest
//! segment and held here after. That is sound because the daemon holding a
//! box's live handle is the only thing that writes its trace. Two processes
//! writing one box would hand out the same number twice.

use crate::error::poisoned;
use crate::{Blobs, BoxRecord, Error, Frames, Result, Store, now_ms};
use async_trait::async_trait;
use computer_api::{Actor, TraceEntry, TraceEvent};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Digits in a sequence inside a key. Twelve holds every entry a box could be
/// given before the padding runs out and the order goes wrong.
const WIDTH: usize = 12;

/// Entries a segment may hold before it goes down.
///
/// A batch is one round trip either way, so this is how much of an agent's run
/// a crash may cost rather than how much a write can carry.
const BATCH: usize = 64;

/// How long the oldest buffered entry may wait.
///
/// Checked when the next entry arrives rather than on a timer, so a busy box is
/// bounded and a quiet one holds only what it has written.
const LINGER: Duration = Duration::from_secs(5);

pub struct Files<B> {
    blobs: B,
    /// The next sequence per box, primed from the highest segment on first
    /// touch. See the note on writers above.
    next: Mutex<HashMap<String, u64>>,
    /// Written and not yet put down, per box.
    held: Mutex<HashMap<String, Waiting>>,
}

struct Waiting {
    entries: Vec<TraceEntry>,
    /// When the oldest of them was written.
    since: Instant,
}

impl<B: Blobs> Files<B> {
    pub fn over(blobs: B) -> Self {
        Self {
            blobs,
            next: Mutex::new(HashMap::new()),
            held: Mutex::new(HashMap::new()),
        }
    }

    /// Puts down what this box is holding, as one segment.
    ///
    /// The entries are taken out from under the lock before the write, so an
    /// append arriving during it buffers behind rather than waiting. A write
    /// that fails puts them back, in front of anything that arrived meanwhile.
    async fn put_down(&self, id: &str) -> Result<()> {
        let taken = match self.held.lock().map_err(poisoned)?.remove(id) {
            Some(waiting) => waiting.entries,
            None => return Ok(()),
        };

        let Some((first, last)) = taken
            .first()
            .map(|entry| entry.seq)
            .zip(taken.last().map(|entry| entry.seq))
        else {
            return Ok(());
        };

        let key = segment(&traces(id)?, first, last);
        let body = lines(&taken)?;

        if let Err(why) = self.blobs.put(&key, &body).await {
            if let Ok(mut held) = self.held.lock() {
                let waiting = held.entry(id.to_string()).or_insert_with(|| Waiting {
                    entries: Vec::new(),
                    since: Instant::now(),
                });
                let arrived = std::mem::take(&mut waiting.entries);
                waiting.entries = taken;
                waiting.entries.extend(arrived);
            }

            return Err(why);
        }

        Ok(())
    }

    /// Whether this box has waited long enough or written enough.
    fn full(&self, id: &str) -> Result<bool> {
        let held = self.held.lock().map_err(poisoned)?;

        Ok(match held.get(id) {
            Some(waiting) => waiting.entries.len() >= BATCH || waiting.since.elapsed() >= LINGER,
            None => false,
        })
    }

    fn waiting(&self, id: &str) -> Result<Vec<TraceEntry>> {
        Ok(self
            .held
            .lock()
            .map_err(poisoned)?
            .get(id)
            .map(|waiting| waiting.entries.clone())
            .unwrap_or_default())
    }

    async fn claim(&self, id: &str) -> Result<u64> {
        {
            let mut held = self.next.lock().map_err(poisoned)?;
            if let Some(next) = held.get_mut(id) {
                let seq = *next;
                *next = seq + 1;
                return Ok(seq);
            }
        }

        let primed = self.highest(id).await?;

        let mut held = self.next.lock().map_err(poisoned)?;
        // A second caller may have primed this while the listing was in
        // flight. Its answer came from the same keys, so take whichever is
        // further on rather than handing out a sequence already given away.
        let next = held.entry(id.to_string()).or_insert(primed);
        let seq = (*next).max(primed);
        *next = seq + 1;

        Ok(seq)
    }

    async fn read(&self, key: &str) -> Result<Vec<TraceEntry>> {
        read_segment(&self.blobs, key).await
    }

    /// One past the last sequence any segment holds, or zero where none do.
    async fn highest(&self, id: &str) -> Result<u64> {
        let mut highest = None;

        let prefix = traces(id)?;

        for key in self.blobs.list(&prefix, None).await? {
            let Some((_, last)) = range(&prefix, &key) else {
                continue;
            };
            highest = Some(highest.map_or(last, |held: u64| held.max(last)));
        }

        Ok(highest.map_or(0, |last| last + 1))
    }
}

#[async_trait]
impl<B: Blobs + 'static> Store for Files<B> {
    async fn put_box(&self, record: &BoxRecord) -> Result<()> {
        let body = serde_json::to_vec(record)
            .map_err(|error| Error::Internal(format!("a record would not serialise: {error}")))?;

        self.blobs.put(&main(&record.id)?, &body).await
    }

    async fn get_box(&self, id: &str) -> Result<Option<BoxRecord>> {
        let Some(body) = self.blobs.get(&main(id)?).await? else {
            return Ok(None);
        };

        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|error| Error::Corrupt(format!("the record for {id} does not parse: {error}")))
    }

    async fn list_boxes(&self) -> Result<Vec<BoxRecord>> {
        // A listing names keys and nothing else, so each record is fetched. A
        // delimiter would give the ids alone and still not the records.
        let mut records = Vec::new();

        for key in self.blobs.list("boxes/", None).await? {
            if !key.ends_with("/main.json") {
                continue;
            }

            let Some(body) = self.blobs.get(&key).await? else {
                continue;
            };

            records.push(serde_json::from_slice(&body).map_err(|error| {
                Error::Corrupt(format!("the record at {key} does not parse: {error}"))
            })?);
        }

        Ok(records)
    }

    async fn forget_box(&self, id: &str) -> Result<()> {
        self.held.lock().map_err(poisoned)?.remove(id);
        self.blobs.delete_prefix(&main(id)?).await?;
        self.blobs.delete_prefix(&traces(id)?).await?;
        self.next.lock().map_err(poisoned)?.remove(id);

        Ok(())
    }

    async fn append(
        &self,
        id: &str,
        actor: Actor,
        event: TraceEvent,
        frame: Option<String>,
    ) -> Result<u64> {
        let seq = self.claim(id).await?;
        let must_land = custody(&event);

        let entry = TraceEntry {
            seq,
            at_ms: now_ms(),
            actor,
            event,
            frame,
        };

        {
            let mut held = self.held.lock().map_err(poisoned)?;
            held.entry(id.to_string())
                .or_insert_with(|| Waiting {
                    entries: Vec::new(),
                    since: Instant::now(),
                })
                .entries
                .push(entry);
        }

        // Everything buffered goes with it, so a segment's range stays whole and
        // the order on disk is the order it happened in.
        if must_land || self.full(id)? {
            self.put_down(id).await?;
        }

        Ok(seq)
    }

    async fn flush(&self) -> Result<()> {
        let holding: Vec<String> = self
            .held
            .lock()
            .map_err(poisoned)?
            .keys()
            .cloned()
            .collect();

        for id in holding {
            self.put_down(&id).await?;
        }

        Ok(())
    }

    async fn entries(&self, id: &str, after: Option<u64>, limit: usize) -> Result<Vec<TraceEntry>> {
        let prefix = traces(id)?;
        let mut keys = Vec::new();

        for key in self.blobs.list(&prefix, None).await? {
            let Some((first, last)) = range(&prefix, &key) else {
                continue;
            };
            // A segment holding nothing past `after` is not fetched. The one
            // that straddles it is, because its later half is wanted.
            if after.is_none_or(|seq| last > seq) {
                keys.push((first, key));
            }
        }

        keys.sort();

        let mut entries = Vec::new();

        for (_, key) in keys {
            if entries.len() >= limit {
                break;
            }

            let Some(body) = self.blobs.get(&key).await? else {
                continue;
            };

            for line in body.split(|byte| *byte == b'\n') {
                if entries.len() >= limit {
                    break;
                }
                if line.is_empty() {
                    continue;
                }

                let entry: TraceEntry = serde_json::from_slice(line).map_err(|error| {
                    Error::Corrupt(format!("an entry in {key} does not parse: {error}"))
                })?;

                if after.is_none_or(|seq| entry.seq > seq) {
                    entries.push(entry);
                }
            }
        }

        // What is still buffered carries the highest sequences there are, so it
        // goes on the end and the order holds. Without this a caller that
        // appends and reads back sees a hole where its own write was.
        for entry in self.waiting(id)? {
            if entries.len() >= limit {
                break;
            }
            if after.is_none_or(|seq| entry.seq > seq) {
                entries.push(entry);
            }
        }

        Ok(entries)
    }
    async fn frames_before(&self, id: &str, before_ms: u64) -> Result<Vec<String>> {
        // Put down first: a cutoff is a time, and a buffered entry has one.
        self.put_down(id).await?;

        let prefix = traces(id)?;
        let mut named = Vec::new();

        // Every segment, because a segment's name carries sequences and not
        // times. Bounded by a sweep's own cadence rather than by a request.
        for key in self.blobs.list(&prefix, None).await? {
            for entry in self.read(&key).await? {
                if entry.at_ms < before_ms
                    && let Some(hash) = entry.frame
                {
                    named.push(hash);
                }
            }
        }

        Ok(named)
    }

    async fn prune_entries(&self, id: &str, before_ms: u64) -> Result<u64> {
        self.put_down(id).await?;

        let prefix = traces(id)?;
        let keys = self.blobs.list(&prefix, None).await?;

        // The highest segment's name is this box's watermark: `highest` reads
        // the sequence out of it, and a process that starts after a trace was
        // pruned away would otherwise begin again at zero and hand out numbers
        // a caller has already read.
        let watermark = keys
            .iter()
            .filter_map(|key| range(&prefix, key).map(|(_, last)| (last, key.clone())))
            .max()
            .map(|(_, key)| key);

        let mut gone = 0;

        for key in keys {
            let held = self.read(&key).await?;
            let kept: Vec<TraceEntry> = held
                .iter()
                .filter(|entry| entry.at_ms >= before_ms)
                .cloned()
                .collect();

            if kept.len() == held.len() {
                continue;
            }

            gone += (held.len() - kept.len()) as u64;

            if kept.is_empty() {
                match watermark.as_deref() == Some(key.as_str()) {
                    // Emptied rather than removed, so the range in its name
                    // survives as the watermark. One object per box.
                    true => self.blobs.put(&key, &[]).await?,
                    false => self.blobs.delete_prefix(&key).await?,
                }
                continue;
            }

            // Written back under the name it had. The range in a name is what
            // a reader filters on before it reads, and a narrower one would
            // hide the entries still inside.
            self.blobs.put(&key, &lines(&kept)?).await?;
        }

        Ok(gone)
    }
}

#[async_trait]
impl<B: Blobs + 'static> Frames for Files<B> {
    async fn put(&self, id: &str, hash: &str, png: &[u8]) -> Result<()> {
        let key = frame(id, hash)?;

        // Held by content, so a second write under one hash is the same
        // picture. A listing rather than a fetch: the answer is whether
        // anything is there, not what.
        if !self.blobs.list(&key, None).await?.is_empty() {
            return Ok(());
        }

        self.blobs.put(&key, png).await
    }

    async fn get(&self, id: &str, hash: &str) -> Result<Option<Arc<Vec<u8>>>> {
        Ok(self.blobs.get(&frame(id, hash)?).await?.map(Arc::new))
    }

    /// One key at a time, through the prefix delete: a whole key is a prefix
    /// of itself and nothing else lives under it.
    async fn drop_frames(&self, id: &str, hashes: &[String]) -> Result<()> {
        for hash in hashes {
            self.blobs.delete_prefix(&frame(id, hash)?).await?;
        }

        Ok(())
    }

    async fn forget(&self, id: &str) -> Result<()> {
        self.blobs.delete_prefix(&screens(id)?).await
    }
}

/// Whether losing this entry would lose a claim rather than a detail.
///
/// Custody and lifecycle: when a box began, when it ended, and when a person
/// held a screen. Everything else is what was done in between, which is worth
/// having and not worth a round trip each.
fn custody(event: &TraceEvent) -> bool {
    matches!(
        event,
        TraceEvent::BoxCreated { .. }
            | TraceEvent::BoxDeleted
            | TraceEvent::Gone { .. }
            | TraceEvent::Adopted { .. }
            | TraceEvent::ForkedFrom { .. }
            | TraceEvent::TakeoverStarted { .. }
            | TraceEvent::TakeoverEnded { .. }
    )
}

/// One segment's entries, in the order they were written.
async fn read_segment<B: Blobs>(blobs: &B, key: &str) -> Result<Vec<TraceEntry>> {
    let Some(body) = blobs.get(key).await? else {
        return Ok(Vec::new());
    };

    let mut held = Vec::new();
    for line in body.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        held.push(serde_json::from_slice(line).map_err(|error| {
            Error::Corrupt(format!("an entry in {key} does not parse: {error}"))
        })?);
    }

    Ok(held)
}

fn lines(entries: &[TraceEntry]) -> Result<Vec<u8>> {
    let mut body = Vec::new();

    for entry in entries {
        let mut line = serde_json::to_vec(entry)
            .map_err(|error| Error::Internal(format!("an entry would not serialise: {error}")))?;
        line.push(b'\n');
        body.extend(line);
    }

    Ok(body)
}

fn main(id: &str) -> Result<String> {
    Ok(format!("boxes/{}/main.json", part(id)?))
}

fn traces(id: &str) -> Result<String> {
    Ok(format!("boxes/{}/traces/", part(id)?))
}

fn screens(id: &str) -> Result<String> {
    Ok(format!("boxes/{}/screens/", part(id)?))
}

fn frame(id: &str, hash: &str) -> Result<String> {
    Ok(format!("{}{}.png", screens(id)?, part(hash)?))
}

fn segment(prefix: &str, first: u64, last: u64) -> String {
    format!(
        "{prefix}{first:0width$}-{last:0width$}.jsonl",
        width = WIDTH
    )
}

/// The sequences a segment key claims to hold.
fn range(prefix: &str, key: &str) -> Option<(u64, u64)> {
    let name = key.strip_prefix(prefix)?.strip_suffix(".jsonl")?;
    let (first, last) = name.split_once('-')?;

    Some((first.parse().ok()?, last.parse().ok()?))
}

/// One path segment, refused if it could reach outside its own.
///
/// Ids and hashes are minted above this crate, so a bad one is a bug here
/// rather than a caller's request — but a key that escapes its prefix would
/// write over another box, and `delete_prefix` would then take more than it was
/// given.
fn part(part: &str) -> Result<&str> {
    let bad =
        part.is_empty() || part == "." || part == ".." || part.contains('/') || part.contains('\\');

    match bad {
        true => Err(Error::Internal(format!(
            "{part:?} is not usable as one part of a key"
        ))),
        false => Ok(part),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;
    use crate::memory::Bucket;
    use std::sync::Arc;

    fn acted() -> TraceEvent {
        TraceEvent::Frame { screen: 0 }
    }

    /// A backend that will not take a write, so the buffer's own promise can be
    /// checked: what it could not put down it still holds.
    struct Refusing;

    #[async_trait]
    impl Blobs for Refusing {
        async fn get(&self, _key: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }

        async fn put(&self, key: &str, _bytes: &[u8]) -> Result<()> {
            Err(Error::Unavailable(format!("{key}: refused")))
        }

        async fn list(&self, _prefix: &str, _start_after: Option<&str>) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        async fn delete_prefix(&self, _prefix: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_files_behaves_like_a_store() {
        conformance::store(&Files::over(Bucket::default())).await;
    }

    #[tokio::test]
    async fn test_files_behaves_like_a_frame_store() {
        conformance::frames(&Files::over(Bucket::default())).await;
    }

    #[tokio::test]
    async fn test_files_prunes() {
        conformance::pruning(&Files::over(Bucket::default())).await;
    }

    #[tokio::test]
    async fn test_files_drops_named_frames() {
        conformance::dropping(&Files::over(Bucket::default())).await;
    }

    #[tokio::test]
    async fn test_a_sequence_carries_on_where_the_keys_left_off() {
        let bucket = Arc::new(Bucket::default());

        let first = Files::over(Arc::clone(&bucket));
        first
            .append("box_1", Actor::Agent, TraceEvent::BoxDeleted, None)
            .await
            .expect("appended");
        first
            .append("box_1", Actor::Agent, TraceEvent::BoxDeleted, None)
            .await
            .expect("appended");

        let restarted = Files::over(Arc::clone(&bucket));
        let seq = restarted
            .append("box_1", Actor::Agent, TraceEvent::BoxDeleted, None)
            .await
            .expect("appended");

        assert_eq!(
            seq, 2,
            "a store with no memory of the last run reads the sequence out of \
             the keys, or a restart writes over what the first run wrote"
        );
        assert_eq!(
            restarted
                .entries("box_1", None, 10)
                .await
                .expect("read back")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn test_segments_are_named_so_they_sort_in_order() {
        let bucket = Arc::new(Bucket::default());
        let files = Files::over(Arc::clone(&bucket));

        for _ in 0..11 {
            files
                .append("box_1", Actor::Agent, TraceEvent::BoxDeleted, None)
                .await
                .expect("appended");
        }

        let prefix = traces("box_1").expect("a prefix");
        let keys = bucket
            .list(&prefix, None)
            .await
            .expect("listed in lexical order");
        let mut sequences = Vec::new();
        for key in &keys {
            sequences.push(range(&prefix, key).expect("a segment key").0);
        }

        let mut ordered = sequences.clone();
        ordered.sort();
        assert_eq!(
            sequences, ordered,
            "lexical order is sequence order, or entry 10 is read before entry 2"
        );
    }

    #[tokio::test]
    async fn test_many_entries_go_down_as_one_segment() {
        let bucket = Arc::new(Bucket::default());
        let files = Files::over(Arc::clone(&bucket));

        for _ in 0..5 {
            files
                .append("box_1", Actor::Agent, acted(), None)
                .await
                .expect("appended");
        }
        files.flush().await.expect("put down");

        let prefix = traces("box_1").expect("a prefix");
        assert_eq!(
            bucket.list(&prefix, None).await.expect("listed").len(),
            1,
            "five clicks are one object, not five: a bucket bills per call"
        );
        assert_eq!(
            files.entries("box_1", None, 10).await.expect("read").len(),
            5
        );
    }

    #[tokio::test]
    async fn test_an_entry_is_readable_before_it_lands() {
        let bucket = Arc::new(Bucket::default());
        let files = Files::over(Arc::clone(&bucket));

        let seq = files
            .append("box_1", Actor::Agent, acted(), None)
            .await
            .expect("appended");

        let prefix = traces("box_1").expect("a prefix");
        assert!(
            bucket.list(&prefix, None).await.expect("listed").is_empty(),
            "nothing has gone down yet"
        );

        let read = files.entries("box_1", None, 10).await.expect("read");
        assert_eq!(read.len(), 1, "and a caller reading back sees no hole");
        assert_eq!(read[0].seq, seq);
        assert!(
            files
                .entries("box_1", Some(seq), 10)
                .await
                .expect("read")
                .is_empty(),
            "and paging past it still ends"
        );
    }

    #[tokio::test]
    async fn test_who_held_the_screen_does_not_wait_in_a_buffer() {
        let bucket = Arc::new(Bucket::default());
        let files = Files::over(Arc::clone(&bucket));

        files
            .append("box_1", Actor::Agent, acted(), None)
            .await
            .expect("appended");
        files
            .append(
                "box_1",
                Actor::Person,
                TraceEvent::TakeoverStarted {
                    screen: 0,
                    exclusive: true,
                },
                None,
            )
            .await
            .expect("appended");

        let prefix = traces("box_1").expect("a prefix");
        let keys = bucket.list(&prefix, None).await.expect("listed");

        assert_eq!(
            keys.len(),
            1,
            "a takeover puts itself down, and takes what was waiting with it"
        );
        assert_eq!(
            range(&prefix, &keys[0]),
            Some((0, 1)),
            "so the segment holds both, in the order they happened"
        );
    }

    #[tokio::test]
    async fn test_a_write_that_fails_keeps_what_it_was_given() {
        let files = Files::over(Refusing);

        files
            .append("box_1", Actor::Agent, acted(), None)
            .await
            .expect("buffered rather than written");

        assert!(
            files.flush().await.is_err(),
            "the bucket refused, and the caller is told"
        );
        assert_eq!(
            files.entries("box_1", None, 10).await.expect("read").len(),
            1,
            "and the entry is still held rather than dropped on the floor"
        );
    }

    #[tokio::test]
    async fn test_a_pruned_trace_does_not_start_again_after_a_restart() {
        let bucket = Arc::new(Bucket::default());

        {
            let files = Files::over(Arc::clone(&bucket));
            for _ in 0..3 {
                files
                    .append("box_1", Actor::Agent, TraceEvent::BoxDeleted, None)
                    .await
                    .expect("appended");
            }
            files
                .prune_entries("box_1", now_ms() + 60_000)
                .await
                .expect("pruned");
        }

        let restarted = Files::over(Arc::clone(&bucket));
        let seq = restarted
            .append("box_1", Actor::Agent, TraceEvent::BoxDeleted, None)
            .await
            .expect("appended");

        assert_eq!(
            seq, 3,
            "the watermark outlived the entries it counted, so a fresh process \
             does not reissue a sequence a caller already read"
        );
    }

    #[tokio::test]
    async fn test_a_key_cannot_reach_out_of_its_own_box() {
        let files = Files::over(Bucket::default());

        assert!(
            files.get_box("../other").await.is_err(),
            "an id that climbs out of its prefix is refused"
        );
        assert!(
            Frames::get(&files, "box_1", "a/b").await.is_err(),
            "and so is a hash that does"
        );
    }
}
