//! A REST API over `computer` boxes.
//!
//! Two halves. Lifecycle — create, list, remove — is ordinary REST. Driving
//! is the batch at `POST /v1/boxes/{id}/screens/{n}/actions`, which takes a
//! run of actions and hands back the frame they produced, because an agent's
//! step is several actions and one look, and one request per click spends a
//! round trip on each.

pub mod auth;
pub mod error;
pub mod extract;
pub mod idempotency;
pub mod prune;
pub mod reap;
pub mod recover;
pub mod registry;
pub mod routes;
pub mod spec;

use computer::ContainerCli;
use computer_api::{Actor, TraceEvent};
use computer_storage::files::Files;
use computer_storage::local::LocalDir;
use computer_storage::memory::Memory;
use computer_storage::{Frames, Store};
use idempotency::Replies;
use registry::Registry;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub registry: Registry,
    pub replies: Replies,
    /// `None` leaves the API open, which is only allowed on loopback — see
    /// [`auth::allowed`].
    pub token: Option<computer::Secret>,
    /// Kept beside the registry rather than on a box, because removing a box
    /// must not remove the record of what was done in it.
    pub store: Arc<dyn Store>,
    pub frames: Arc<dyn Frames>,
    /// The last frame written per screen, so an idle screen polled in a loop
    /// adds nothing.
    ///
    /// Here rather than in a backend: it is news about this process, not about
    /// the record, and four backends would otherwise each keep a copy. A
    /// restart writing the first frame it sees is right — after an adoption
    /// that frame is news.
    seen: Mutex<HashMap<(String, u32), String>>,
    /// What to reach the container runtime through.
    ///
    /// `None` is this host's own. A test supplies a double here, because
    /// otherwise every request this server accepts starts a real container —
    /// which a test asserting a refusal finds out about only when the
    /// refusal stops happening.
    pub cli: Option<Arc<dyn ContainerCli>>,
}

impl Default for AppState {
    /// In memory. A daemon that is to keep its state somewhere says where.
    fn default() -> Self {
        Self::on(Arc::new(Memory::default()))
    }
}

impl AppState {
    /// One backend answering for both, which is the ordinary case: a record and
    /// the frames it names belong in the same place.
    pub fn on<B: Store + Frames + 'static>(backend: Arc<B>) -> Self {
        let store = Arc::clone(&backend) as Arc<dyn Store>;

        Self::split(store, backend)
    }

    /// Rows in one place and pictures in another, for a fleet whose frames
    /// outgrow whatever holds its records.
    pub fn split(store: Arc<dyn Store>, frames: Arc<dyn Frames>) -> Self {
        Self {
            registry: Registry::default(),
            replies: Replies::default(),
            token: None,
            store,
            frames,
            seen: Mutex::new(HashMap::new()),
            cli: None,
        }
    }

    /// Where the environment says, or in memory.
    ///
    /// Whatever `COMPUTER_STORAGE_BACKEND` names, and memory where it names
    /// nothing.
    ///
    /// Named rather than worked out from whichever setting happens to be in the
    /// environment: a server with a stale `COMPUTER_STATE_DIR` beside a new
    /// database URL should not have to be guessed about, and one asked for a
    /// backend it was not built with should say so rather than keep records
    /// somewhere else.
    ///
    /// | Named      | Reads                                            |
    /// | ---------- | ------------------------------------------------ |
    /// | `memory`   | nothing                                          |
    /// | `local`    | `COMPUTER_STATE_DIR`                             |
    /// | `sqlite`   | `COMPUTER_STATE_URL`                             |
    /// | `postgres` | `COMPUTER_STATE_URL`                             |
    /// | `s3`       | `COMPUTER_S3_*`, `AWS_ACCESS_KEY_ID` and its pair |
    pub async fn from_env() -> Result<Self, String> {
        let named = std::env::var("COMPUTER_STORAGE_BACKEND").unwrap_or_default();

        match named.trim() {
            "" | "memory" => Ok(Self::default()),
            "local" => Self::on_dir(),
            dialect @ ("sqlite" | "postgres") => Self::on_url(dialect).await,
            "s3" => Self::on_bucket().await,
            other => Err(format!(
                "COMPUTER_STORAGE_BACKEND={other} names no backend here: use \
                 memory, local, sqlite, postgres or s3"
            )),
        }
    }

    fn on_dir() -> Result<Self, String> {
        let root = need("COMPUTER_STATE_DIR")?;

        tracing::info!(%root, "keeping box records, traces and frames under");
        Ok(Self::on(Arc::new(Files::over(LocalDir::at(root)))))
    }

    #[cfg(feature = "sql")]
    async fn on_url(dialect: &str) -> Result<Self, String> {
        let url = need("COMPUTER_STATE_URL")?;

        if !url.starts_with(dialect) {
            return Err(format!(
                "COMPUTER_STORAGE_BACKEND={dialect} and COMPUTER_STATE_URL is not \
                 a {dialect} URL"
            ));
        }

        let store = computer_storage::sql::Sql::open(&url)
            .await
            .map_err(|why| format!("{dialect}: {why}"))?;

        // Not the URL: it carries a password.
        tracing::info!(%dialect, "keeping box records, traces and frames in");
        Ok(Self::on(Arc::new(store)))
    }

    #[cfg(not(feature = "sql"))]
    async fn on_url(dialect: &str) -> Result<Self, String> {
        Err(format!(
            "COMPUTER_STORAGE_BACKEND={dialect} and this server was built without \
             it: rebuild with --features {dialect}"
        ))
    }

    /// The same layout a directory holds, so a state directory can be copied
    /// into a bucket and read back.
    #[cfg(feature = "s3")]
    async fn on_bucket() -> Result<Self, String> {
        let store = computer_storage::s3::S3::from_env().map_err(|why| why.to_string())?;
        let bucket = need("COMPUTER_S3_BUCKET")?;

        tracing::info!(%bucket, "keeping box records, traces and frames in");
        Ok(Self::on(Arc::new(Files::over(store))))
    }

    #[cfg(not(feature = "s3"))]
    async fn on_bucket() -> Result<Self, String> {
        Err(
            "COMPUTER_STORAGE_BACKEND=s3 and this server was built without it: \
             rebuild with --features s3"
                .to_string(),
        )
    }

    /// Gated, or left open where `None` — see [`auth::allowed`].
    pub fn gated(mut self, token: Option<computer::Secret>) -> Self {
        self.token = token;
        self
    }

    /// What to reach the container runtime through. `None` is this host's own.
    pub fn through(mut self, cli: Option<Arc<dyn ContainerCli>>) -> Self {
        self.cli = cli;
        self
    }

    /// Records, and logs rather than fails.
    ///
    /// A trace entry that could not be written must not turn a click that
    /// worked into an error: the record sits beside the work and is not part of
    /// it.
    pub async fn record(&self, id: &str, actor: Actor, event: TraceEvent) {
        if let Err(why) = self.store.append(id, actor, event, None).await {
            tracing::warn!(box_ = %id, %why, "a trace entry was not written");
        }
    }

    /// Records a frame only where the screen moved, and answers whether it did,
    /// so a caller polling a still screen can tell it changed nothing.
    pub async fn note_frame(
        &self,
        id: &str,
        actor: Actor,
        screen: u32,
        hash: &str,
        png: &[u8],
    ) -> bool {
        if let Err(why) = self.frames.put(id, hash, png).await {
            tracing::warn!(box_ = %id, %why, "a frame was not kept");
        }

        {
            let Ok(mut seen) = self.seen.lock() else {
                return false;
            };
            let at = (id.to_string(), screen);
            if seen.get(&at).map(String::as_str) == Some(hash) {
                return false;
            }
            seen.insert(at, hash.to_string());
        }

        let named = Some(hash.to_string());
        if let Err(why) = self
            .store
            .append(id, actor, TraceEvent::Frame { screen }, named)
            .await
        {
            tracing::warn!(box_ = %id, %why, "a frame entry was not written");
        }

        true
    }

    /// Whether anything has been written about this box.
    ///
    /// A read of one entry rather than a flag, because that is the question
    /// every backend can answer without holding a second index.
    pub async fn traced(&self, id: &str) -> bool {
        self.store
            .entries(id, None, 1)
            .await
            .map(|entries| !entries.is_empty())
            .unwrap_or_default()
    }

    /// Drops what this process remembers about a box's screens. The record
    /// stays: a fork reads it after the box has gone.
    pub fn forget_screens(&self, id: &str) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.retain(|(held, _), _| held != id);
        }
    }
}

/// A setting the named backend cannot do without.
fn need(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} is not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_a_screen_that_did_not_move_writes_nothing() {
        let state = AppState::default();

        assert!(
            state
                .note_frame("box_1", Actor::Agent, 0, "aaa", b"png")
                .await
        );
        assert!(
            !state
                .note_frame("box_1", Actor::Agent, 0, "aaa", b"png")
                .await
        );
        assert!(
            state
                .note_frame("box_1", Actor::Agent, 0, "bbb", b"other")
                .await
        );

        let entries = state.store.entries("box_1", None, 10).await.expect("read");
        assert_eq!(entries.len(), 2, "the unchanged frame was not recorded");
    }

    #[tokio::test]
    async fn test_two_screens_are_tracked_apart() {
        let state = AppState::default();

        assert!(
            state
                .note_frame("box_1", Actor::Agent, 0, "aaa", b"png")
                .await
        );
        assert!(
            state
                .note_frame("box_1", Actor::Agent, 1, "aaa", b"png")
                .await,
            "screen 1 showing what screen 0 shows is still news about screen 1"
        );
    }

    #[tokio::test]
    async fn test_two_boxes_are_tracked_apart() {
        let state = AppState::default();

        assert!(
            state
                .note_frame("box_1", Actor::Agent, 0, "aaa", b"png")
                .await
        );
        assert!(
            state
                .note_frame("box_2", Actor::Agent, 0, "aaa", b"png")
                .await,
            "one box's screen says nothing about another's"
        );
    }

    #[tokio::test]
    async fn test_a_frame_is_held_once_however_often_it_is_seen() {
        let state = AppState::default();

        state
            .note_frame("box_1", Actor::Agent, 0, "aaa", b"png")
            .await;
        state
            .note_frame("box_1", Actor::Agent, 1, "aaa", b"png")
            .await;

        let held = state.frames.get("box_1", "aaa").await.expect("asked");
        assert_eq!(held.as_deref().map(Vec::as_slice), Some(b"png".as_slice()));
    }

    #[tokio::test]
    async fn test_a_forgotten_box_leaves_its_record_behind() {
        let state = AppState::default();

        state
            .note_frame("box_1", Actor::Agent, 0, "aaa", b"png")
            .await;
        state.forget_screens("box_1");

        assert!(
            state.traced("box_1").await,
            "what this process remembers goes; what a fork reads stays"
        );
        assert!(
            state
                .note_frame("box_1", Actor::Agent, 0, "aaa", b"png")
                .await,
            "and the screen is news again, which after an adoption it is"
        );
    }
}
