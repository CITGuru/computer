pub mod auth;
pub mod caller;
pub mod cdp;
pub mod config;
pub mod console;
pub mod error;
pub mod events;
pub mod extract;
pub mod idempotency;
pub mod images;
pub mod jobs;
pub mod labels;
pub mod mcp;
pub mod oidc;
pub mod presses;
pub mod prune;
pub mod reap;
pub mod recover;
pub mod registry;
pub mod routes;
pub mod runtimes;
pub mod schedule;
pub mod secrets;
pub mod spec;
pub mod states;
pub mod viewer;

use holm::{Engine, EngineMachine};
use holm_api::{Actor, TraceEvent};
use holm_storage::files::Files;
use holm_storage::local::LocalDir;
use holm_storage::memory::Memory;
use holm_storage::{Frames, Store};
use registry::Registry;
use runtimes::Runtimes;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub registry: Registry,
    pub token: Option<holm::Secret>,
    pub store: Arc<dyn Store>,
    pub frames: Arc<dyn Frames>,
    seen: Mutex<HashMap<(String, u32), String>>,
    out_of_reach: Mutex<BTreeMap<String, String>>,
    pub runtimes: Runtimes,
    pub secrets: secrets::Keeper,
    pub vendors: Arc<dyn runtimes::Vendors>,
    pub tickets: viewer::Tickets,
    pub cdp_tokens: viewer::Tickets,
    pub doors: viewer::Doors,
    pub console: Option<Arc<console::Console>>,
    pub jobs: jobs::Mode,
    pub cron: schedule::Cron,
}

impl Default for AppState {
    fn default() -> Self {
        Self::on(Arc::new(Memory::default()))
    }
}

impl AppState {
    pub fn on<B: Store + Frames + 'static>(backend: Arc<B>) -> Self {
        let store = Arc::clone(&backend) as Arc<dyn Store>;

        Self::split(store, backend)
    }

    pub fn split(store: Arc<dyn Store>, frames: Arc<dyn Frames>) -> Self {
        Self {
            registry: Registry::default(),
            token: None,
            store,
            frames,
            seen: Mutex::new(HashMap::new()),
            out_of_reach: Mutex::new(BTreeMap::new()),
            runtimes: Runtimes::default(),
            secrets: secrets::Keeper::default(),
            vendors: Arc::new(runtimes::Builtin),
            tickets: viewer::Tickets::default(),
            cdp_tokens: viewer::Tickets::default(),
            doors: viewer::Doors::default(),
            console: None,
            jobs: jobs::Mode::default(),
            cron: schedule::Cron::default(),
        }
    }

    pub async fn from_env() -> Result<Self, String> {
        let mut state = Self::stored().await?;
        state = state.keeping(secrets::Keeper::from_env()?);
        state.jobs = jobs::Mode::from_env()?;
        state.cron = schedule::Cron::from_env()?;
        state.console = console::Console::from_env()?
            .map(|console| Arc::new(console.watching(Arc::clone(&state.store))));

        let config = config::ServerConfig::from_env()?;
        let found = runtimes::discover(
            &config,
            &runtimes::offered(),
            &runtimes::sandboxes(),
            state.vendors.as_ref(),
        )
        .await?;

        let taken = runtimes::from_store(
            &found,
            state.store.as_ref(),
            &state.secrets,
            state.vendors.as_ref(),
        )
        .await;
        if taken > 0 {
            tracing::info!(taken, "runtimes this server was given earlier");
        }
        found.settle();

        Ok(state.with(found))
    }

    async fn stored() -> Result<Self, String> {
        let named = std::env::var("HOLM_STORAGE_BACKEND").unwrap_or_default();

        match named.trim() {
            "" | "memory" => Ok(Self::default()),
            "local" => Self::on_dir(),
            dialect @ ("sqlite" | "postgres") => Self::on_url(dialect).await,
            "s3" => Self::on_bucket().await,
            other => Err(format!(
                "HOLM_STORAGE_BACKEND={other} names no backend here: use \
                 memory, local, sqlite, postgres or s3"
            )),
        }
    }

    fn on_dir() -> Result<Self, String> {
        let root = need("HOLM_STATE_DIR")?;

        tracing::info!(%root, "keeping box records, traces and frames under");
        Ok(Self::on(Arc::new(Files::over(LocalDir::at(root)))))
    }

    #[cfg(feature = "sql")]
    async fn on_url(dialect: &str) -> Result<Self, String> {
        let url = need("HOLM_STATE_URL")?;

        if !url.starts_with(dialect) {
            return Err(format!(
                "HOLM_STORAGE_BACKEND={dialect} and HOLM_STATE_URL is not \
                 a {dialect} URL"
            ));
        }

        let connections = match std::env::var("HOLM_STATE_POOL") {
            Ok(value) => Some(
                value
                    .parse()
                    .map_err(|_| format!("HOLM_STATE_POOL={value} is not a number"))?,
            ),
            Err(_) => None,
        };
        let store = holm_storage::sql::Sql::open_sized(&url, connections)
            .await
            .map_err(|why| format!("{dialect}: {why}"))?;

        tracing::info!(%dialect, "keeping box records, traces and frames in");
        Ok(Self::on(Arc::new(store)))
    }

    #[cfg(not(feature = "sql"))]
    async fn on_url(dialect: &str) -> Result<Self, String> {
        Err(format!(
            "HOLM_STORAGE_BACKEND={dialect} and this server was built without \
             it: rebuild with --features {dialect}"
        ))
    }

    #[cfg(feature = "s3")]
    async fn on_bucket() -> Result<Self, String> {
        let store = holm_storage::s3::S3::from_env().map_err(|why| why.to_string())?;
        let bucket = need("HOLM_S3_BUCKET")?;

        tracing::info!(%bucket, "keeping box records, traces and frames in");
        Ok(Self::on(Arc::new(Files::over(store))))
    }

    #[cfg(not(feature = "s3"))]
    async fn on_bucket() -> Result<Self, String> {
        Err(
            "HOLM_STORAGE_BACKEND=s3 and this server was built without it: \
             rebuild with --features s3"
                .to_string(),
        )
    }

    pub fn gated(mut self, token: Option<holm::Secret>) -> Self {
        self.token = token;
        self
    }

    pub fn trusting(mut self, key: caller::ConsoleKey) -> Self {
        self.console = Some(Arc::new(console::Console::pinned(key)));
        self
    }

    pub fn linking(mut self, console: console::Console) -> Self {
        self.console = Some(Arc::new(console.watching(Arc::clone(&self.store))));
        self
    }

    pub async fn owner_of(&self, id: &str) -> Option<Option<String>> {
        if let Ok(entry) = self.registry.get(id).await {
            return Some(entry.owner.clone());
        }
        match self.store.get_box(id).await {
            Ok(Some(record)) => Some(record.owner),
            _ => None,
        }
    }

    pub fn scheduled(mut self, cron: schedule::Cron) -> Self {
        self.cron = cron;
        self
    }

    pub fn queueing(mut self) -> Self {
        self.jobs = jobs::Mode::Queue;
        self
    }

    pub fn keeping(mut self, secrets: secrets::Keeper) -> Self {
        if let Some(key) = secrets.derive("tickets") {
            self.tickets = viewer::Tickets::keyed(&key);
        }
        if let Some(key) = secrets.derive("cdp-tokens") {
            self.cdp_tokens = viewer::Tickets::keyed(&key);
        }
        if let Some(key) = secrets.derive("viewer-keys") {
            self.doors = viewer::Doors::keyed(&key);
        }
        self.secrets = secrets;
        self
    }

    pub fn serving(mut self, vendors: Arc<dyn runtimes::Vendors>) -> Self {
        self.vendors = vendors;
        self
    }

    pub fn with(mut self, runtimes: Runtimes) -> Self {
        self.runtimes = runtimes;
        self
    }

    pub fn through(self, cli: Option<Arc<dyn Engine>>) -> Self {
        if let Some(cli) = cli {
            self.runtimes.add(runtimes::engine(
                "docker",
                Arc::new(EngineMachine::new(cli)),
            ));
            self.runtimes.settle();
        }

        self
    }

    pub async fn record(&self, id: &str, actor: Actor, event: TraceEvent) {
        let said = events::of(&event);

        if let Err(why) = self.store.append(id, actor, event, None).await {
            tracing::warn!(box_ = %id, %why, "a trace entry was not written");
        }
        for (kind, data) in said {
            if !self.news(id, kind).await {
                continue;
            }
            self.box_event(id, kind, data).await;
        }
    }

    async fn news(&self, id: &str, kind: &str) -> bool {
        const KIND: &str = "unreachable";

        match kind {
            "box.unreachable" => {
                if matches!(self.store.get_note(KIND, id).await, Ok(Some(_))) {
                    return false;
                }
                let _ = self.store.put_note(KIND, id, "", None).await;
                true
            }
            "box.ready" | "box.removed" => {
                let _ = self.store.forget_note(KIND, id).await;
                true
            }
            _ => true,
        }
    }

    pub async fn box_event(&self, id: &str, kind: &str, data: serde_json::Value) {
        let (owner, runtime) = match self.store.get_box(id).await {
            Ok(Some(record)) => (record.owner, Some(record.runtime)),
            _ => match self.registry.get(id).await {
                Ok(entry) => (entry.owner.clone(), Some(entry.runtime.clone())),
                Err(_) => (None, None),
            },
        };

        self.event(kind, owner, Some(id.to_string()), runtime, data)
            .await;
    }

    pub async fn event(
        &self,
        kind: &str,
        owner: Option<String>,
        box_id: Option<String>,
        runtime: Option<String>,
        data: serde_json::Value,
    ) {
        let event = holm_storage::EventRecord {
            seq: 0,
            at_ms: routes::ms_of(std::time::SystemTime::now()),
            kind: kind.to_string(),
            owner,
            box_id,
            runtime,
            data,
        };

        if let Err(why) = self.store.append_event(&event).await {
            tracing::warn!(%kind, %why, "an event was not written");
        }
    }

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

    pub async fn traced(&self, id: &str) -> bool {
        self.store
            .entries(id, None, 1)
            .await
            .map(|entries| !entries.is_empty())
            .unwrap_or_default()
    }

    pub async fn entry(&self, id: &str) -> error::ApiResult<Arc<registry::Entry>> {
        if let Ok(entry) = self.registry.get(id).await {
            return Ok(entry);
        }

        if let Some(phase) = jobs::phase(self.store.as_ref(), id).await {
            return Err(jobs::not_ready(id, &phase));
        }
        if self.deleted(id).await {
            return Err(removed(id));
        }

        recover::one(self, id).await;
        self.registry.get(id).await
    }

    pub fn signs(&self, entry: &registry::Entry) -> bool {
        entry.computer.viewer_auth() == holm::Auth::Signed
            || (self.doors.holds_a_key()
                && self
                    .runtimes
                    .get(&entry.runtime)
                    .is_some_and(|runtime| runtime.tokens_the_viewer(&entry.spec)))
    }

    pub async fn deleted(&self, id: &str) -> bool {
        matches!(
            self.store.get_box(id).await,
            Ok(Some(record)) if record.deleted_at_ms.is_some()
        )
    }

    pub async fn mark_deleted(&self, id: &str) -> error::ApiResult<bool> {
        let Some(mut record) = self.store.get_box(id).await? else {
            return Ok(false);
        };
        if record.deleted_at_ms.is_none() {
            record.deleted_at_ms = Some(routes::ms_of(std::time::SystemTime::now()));
            self.store.put_box(&record).await?;
        }
        self.registry.forget(id).await;

        Ok(true)
    }

    pub fn out_of_reach(&self, id: &str, why: String) {
        if let Ok(mut held) = self.out_of_reach.lock() {
            held.insert(id.to_string(), why);
        }
    }

    pub fn why_out_of_reach(&self, id: &str) -> Option<String> {
        self.out_of_reach.lock().ok()?.get(id).cloned()
    }

    pub fn all_out_of_reach(&self) -> BTreeMap<String, String> {
        self.out_of_reach
            .lock()
            .map(|held| held.clone())
            .unwrap_or_default()
    }

    pub fn within_reach(&self, id: &str) {
        if let Ok(mut held) = self.out_of_reach.lock() {
            held.remove(id);
        }
    }

    pub fn forget_screens(&self, id: &str) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.retain(|(held, _), _| held != id);
        }
    }
}

pub fn removed(id: &str) -> error::ApiError {
    error::ApiError::new(
        axum::http::StatusCode::GONE,
        holm_api::ErrorCode::Gone,
        format!("box {id} was removed"),
    )
}

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
