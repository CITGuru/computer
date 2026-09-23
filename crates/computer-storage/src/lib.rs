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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoxRecord {
    pub id: String,
    #[serde(default = "on_docker")]
    pub runtime: String,
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
    async fn put_box(&self, record: &BoxRecord) -> Result<()>;

    async fn get_box(&self, id: &str) -> Result<Option<BoxRecord>>;

    /// In a stable order, gone boxes included.
    async fn list_boxes(&self) -> Result<Vec<BoxRecord>>;

    async fn forget_box(&self, id: &str) -> Result<()>;

    async fn append(
        &self,
        id: &str,
        actor: Actor,
        event: TraceEvent,
        frame: Option<String>,
    ) -> Result<u64>;

    /// Oldest first.
    async fn entries(&self, id: &str, after: Option<u64>, limit: usize) -> Result<Vec<TraceEntry>>;

    async fn frames_before(&self, id: &str, before_ms: u64) -> Result<Vec<String>>;

    async fn prune_entries(&self, id: &str, before_ms: u64) -> Result<u64>;

    async fn flush(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
pub trait Frames: Send + Sync {
    async fn put(&self, id: &str, hash: &str, png: &[u8]) -> Result<()>;

    async fn get(&self, id: &str, hash: &str) -> Result<Option<Arc<Vec<u8>>>>;

    async fn drop_frames(&self, id: &str, hashes: &[String]) -> Result<()>;

    async fn forget(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait Blobs: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<()>;

    /// In lexical order.
    async fn list(&self, prefix: &str, start_after: Option<&str>) -> Result<Vec<String>>;

    async fn delete_prefix(&self, prefix: &str) -> Result<()>;
}

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

fn on_docker() -> String {
    "docker".to_string()
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}
