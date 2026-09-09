//! Where a daemon keeps what it must not lose.
//!
//! A daemon holds two kinds of state. One is process-local and cannot be
//! stored at all: the live `Computer` handle and the per-screen locks beside it
//! are sockets and mutexes, and a restart rebuilds them from whatever still
//! holds the box. The other is a record — what a box was asked to be, and what
//! has been done to it — and that outlives both the box and the process that
//! made it.
//!
//! [`Store`] holds the record. [`Frames`] holds the pictures, which are blobs
//! and want a different home once there are enough of them. [`Blobs`] is the
//! backend a file-shaped store is written over, so one implementation reaches
//! a local directory and a bucket.
//!
//! # Existence is not kept here
//!
//! A row claiming a box is running after the runtime let it go is the one
//! answer this crate must not give. So a caller lists live boxes from whatever
//! holds them and joins this for their history. [`Store::list_boxes`] lists
//! what has a record, which includes boxes that are gone — that is what lets a
//! fork rebuild one.
//!
//! # What is not in the traits
//!
//! Writing a frame only where the screen moved is policy, not storage: it
//! needs the last hash per screen, which is news about this process rather than
//! about the record. It stays above the seam, so four backends do not carry
//! four copies of it.
//!
//! Caps and eviction are each implementation's own. [`memory`] drops the oldest
//! to stay bounded; a file-shaped store keeps what it is given. Nothing above
//! the trait may assume either.

mod error;

pub mod files;
pub mod memory;

#[cfg(feature = "local")]
pub mod local;

#[cfg(feature = "s3")]
pub mod s3;

#[cfg(feature = "sql")]
pub mod sql;

#[cfg(any(test, feature = "conformance"))]
pub mod conformance;

pub use error::{Error, Result};

use async_trait::async_trait;
use computer_api::{Actor, Placement, Spec, TraceEntry, TraceEvent};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// What survives a box.
///
/// No viewer or DevTools URL, because those name host-local ports that a
/// restart reassigns: a stored one points at whatever took the port next. No
/// state either — see the note on existence above.
///
/// Unknown fields are allowed, unlike the wire types. A record is read back by
/// whatever binary is running later, which may be older than the one that
/// wrote it, and refusing a field it does not know would lose the box rather
/// than the field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoxRecord {
    pub id: String,
    /// Whole rather than its digest: a fork rebuilds from this once the box and
    /// the labels it carried are gone.
    pub spec: Spec,
    pub placement: Placement,
    pub width: u32,
    pub height: u32,
    pub screens: u32,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
}

#[async_trait]
pub trait Store: Send + Sync {
    /// Writes the record, replacing any held under the same id.
    async fn put_box(&self, record: &BoxRecord) -> Result<()>;

    async fn get_box(&self, id: &str) -> Result<Option<BoxRecord>>;

    /// Every box with a record, in a stable order, including boxes that are
    /// gone.
    async fn list_boxes(&self) -> Result<Vec<BoxRecord>>;

    /// Drops the record and the trace. Frames are a separate store and go with
    /// [`Frames::forget`].
    async fn forget_box(&self, id: &str) -> Result<()>;

    /// Appends one entry and answers with the sequence it was given.
    ///
    /// The store assigns the sequence, never the caller: a counter living in
    /// one process hands the same number out twice as soon as a second process
    /// writes the same box.
    ///
    /// A box needs no record first. A trace outlives the box it describes, and
    /// an adopted box is written about before anything has been stored for it.
    async fn append(
        &self,
        id: &str,
        actor: Actor,
        event: TraceEvent,
        frame: Option<String>,
    ) -> Result<u64>;

    /// Entries after `after`, oldest first, at most `limit` of them. A box
    /// nothing has been written about answers with none.
    async fn entries(&self, id: &str, after: Option<u64>, limit: usize) -> Result<Vec<TraceEntry>>;

    /// The frames named by entries older than `before_ms`.
    ///
    /// Frames are the volume and a fork does not read one — a replay repeats
    /// actions — so they are dropped long before the entries that name them.
    /// The hashes come back rather than going straight out because [`Frames`] is
    /// a separate store and may not be in the same place.
    ///
    /// A hash a surviving entry also names can come back here: a screen
    /// returning to a state it held before writes the same hash twice, and
    /// finding out costs a read of everything kept. The API already answers
    /// that an old entry names a frame nothing holds, so this is a frame going
    /// early rather than a wrong answer.
    async fn frames_before(&self, id: &str, before_ms: u64) -> Result<Vec<String>>;

    /// Drops entries older than `before_ms` and answers with how many went.
    ///
    /// A box whose entries have all gone and whose desktop is no longer running
    /// is what a caller then forgets outright.
    async fn prune_entries(&self, id: &str, before_ms: u64) -> Result<u64>;

    /// Puts down anything a store is still holding.
    ///
    /// Nothing for a store that writes as it goes, which is why this is a
    /// default rather than a method every backend has to answer. Call it before
    /// a process ends: a store that batches has entries in it that a reader
    /// through [`Store::entries`] can already see and a restart cannot.
    async fn flush(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
pub trait Frames: Send + Sync {
    /// Held by content, so a desktop that sits still costs one copy however
    /// long it sits. Writing a hash already held is not an error.
    ///
    /// Kept per box rather than in one pool. Two boxes showing the same pixels
    /// store them twice, which is cheap; a shared pool would need every frame
    /// reference counted before any of them could be dropped.
    async fn put(&self, id: &str, hash: &str, png: &[u8]) -> Result<()>;

    /// `Arc` rather than bytes, so a store holding the frame in memory answers
    /// without copying it.
    async fn get(&self, id: &str, hash: &str) -> Result<Option<Arc<Vec<u8>>>>;

    /// Drops these hashes from this box. One nothing holds is not an error.
    async fn drop_frames(&self, id: &str, hashes: &[String]) -> Result<()>;

    async fn forget(&self, id: &str) -> Result<()>;
}

/// The backend a file-shaped store is written over.
///
/// Four verbs, because that is what object storage offers. There is no append
/// and no rename, so a trace goes down as segments named by the range they
/// hold, and order comes from zero-padded keys rather than from a counter the
/// backend does not have.
#[async_trait]
pub trait Blobs: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Replaces whatever the key held.
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()>;

    /// Keys under `prefix` in lexical order, those after `start_after` where
    /// one is given.
    async fn list(&self, prefix: &str, start_after: Option<&str>) -> Result<Vec<String>>;

    async fn delete_prefix(&self, prefix: &str) -> Result<()>;
}

/// So a backend can be shared: a store takes its backend by value, and two
/// stores over one bucket is how a frame store and a record store meet.
#[async_trait]
impl<B: Blobs + ?Sized> Blobs for Arc<B> {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        (**self).get(key).await
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        (**self).put(key, bytes).await
    }

    async fn list(&self, prefix: &str, start_after: Option<&str>) -> Result<Vec<String>> {
        (**self).list(prefix, start_after).await
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        (**self).delete_prefix(prefix).await
    }
}

#[async_trait]
impl<B: Blobs + ?Sized> Blobs for &B {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        (**self).get(key).await
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        (**self).put(key, bytes).await
    }

    async fn list(&self, prefix: &str, start_after: Option<&str>) -> Result<Vec<String>> {
        (**self).list(prefix, start_after).await
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        (**self).delete_prefix(prefix).await
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}
