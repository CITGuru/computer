use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::registry::{Entry, Made};
use axum::http::StatusCode;
use holm_api::{Actor, ErrorCode, ReplayReport, TraceEvent};
use holm_storage::{BoxRecord, JobRecord, Store};
use holm_types::{Placement, Spec};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

pub const MODE: &str = "HOLM_SERVER_JOBS";

const PHASES: &str = "phases";
const LAUNCH: &str = "launch";
const BUILD: &str = "build";
const BUILDS: &str = "builds";
const FAILED_BUILD_KEPT_MS: u64 = 60 * 60 * 1000;
const LEASE_MS: u64 = 60_000;
const RENEW: Duration = Duration::from_secs(20);
const IDLE: Duration = Duration::from_secs(1);
const ATTEMPTS: u32 = 2;
const AT_ONCE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Inline,
    Queue,
}

impl Mode {
    pub fn from_env() -> Result<Self, String> {
        match std::env::var(MODE).ok().as_deref() {
            None | Some("") | Some("inline") => Ok(Self::Inline),
            Some("queue") => Ok(Self::Queue),
            Some(other) => Err(format!("{MODE} is inline or queue, not {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Failed { reason: String },
}

pub async fn phase(store: &dyn Store, id: &str) -> Option<Phase> {
    let held = store.get_note(PHASES, id).await.ok()??;
    serde_json::from_str(&held).ok()
}

pub async fn phases(store: &dyn Store) -> Vec<(String, Phase)> {
    store
        .list_notes(PHASES, "")
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, held)| Some((id, serde_json::from_str(&held).ok()?)))
        .collect()
}

async fn set_phase(store: &dyn Store, id: &str, phase: &Phase) -> ApiResult<()> {
    let value = serde_json::to_string(phase)
        .map_err(|error| ApiError::internal(format!("a phase would not serialise: {error}")))?;

    Ok(store.put_note(PHASES, id, &value, None).await?)
}

pub async fn forget_phase(store: &dyn Store, id: &str) {
    if let Err(why) = store.forget_note(PHASES, id).await {
        tracing::warn!(box_ = %id, %why, "the phase of a box was not forgotten");
    }
}

pub fn not_ready(id: &str, phase: &Phase) -> ApiError {
    let message = match phase {
        Phase::Starting => format!("box {id} is still starting"),
        Phase::Failed { reason } => format!("box {id} did not start: {reason}"),
    };

    ApiError::new(StatusCode::CONFLICT, ErrorCode::Unavailable, message)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Launch {
    pub id: String,
    pub runtime: String,
    pub spec: Spec,
    pub placement: Placement,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub fork: Option<Forked>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Forked {
    pub source: String,
    #[serde(default)]
    pub up_to: Option<u64>,
}

pub async fn launch(
    state: &AppState,
    job: &Launch,
) -> ApiResult<(Arc<Entry>, Option<ReplayReport>)> {
    let runtime = state
        .runtimes
        .get(&job.runtime)
        .ok_or_else(|| ApiError::not_found(format!("no runtime named {} here", job.runtime)))?;

    tracing::info!(
        id = %job.id,
        digest = %job.spec.digest(),
        runtime = %runtime.name,
        "launching a box"
    );
    let (computer, resolved) = crate::images::started(
        state,
        &job.spec,
        &job.placement,
        &job.id,
        &runtime,
        job.owner.as_deref(),
    )
    .await?;

    let entry = state
        .registry
        .insert(
            job.id.clone(),
            runtime.name.clone(),
            job.spec.clone(),
            resolved,
            computer,
            Made {
                placement: job.placement.clone(),
                owner: job.owner.clone(),
                created_at: None,
            },
        )
        .await;

    crate::routes::kept(state, &entry).await;
    state
        .record(
            &entry.id,
            Actor::Agent,
            TraceEvent::BoxCreated {
                spec_digest: entry.spec_digest(),
                spec: Box::new(job.spec.clone()),
                placement: Box::new(job.placement.clone()),
                width: resolved.width,
                height: resolved.height,
                screens: resolved.screens,
            },
        )
        .await;

    let report = match &job.fork {
        Some(fork) => Some(crate::routes::forked(state, &entry, &fork.source, fork.up_to).await?),
        None => None,
    };

    Ok((entry, report))
}

pub async fn queue_launch(state: &AppState, job: &Launch) -> ApiResult<BoxRecord> {
    let resolved = crate::spec::resolve(&job.spec)?;
    let record = BoxRecord {
        id: job.id.clone(),
        runtime: job.runtime.clone(),
        spec: job.spec.clone(),
        placement: job.placement.clone(),
        width: resolved.width,
        height: resolved.height,
        screens: resolved.screens,
        created_at_ms: crate::routes::ms_of(std::time::SystemTime::now()),
        expires_at_ms: None,
        owner: job.owner.clone(),
        deleted_at_ms: None,
    };
    let body = serde_json::to_string(job)
        .map_err(|error| ApiError::internal(format!("a job would not serialise: {error}")))?;

    state.store.put_box(&record).await?;
    set_phase(state.store.as_ref(), &job.id, &Phase::Starting).await?;
    state
        .store
        .push_job(&JobRecord::new(format!("job_{}", job.id), LAUNCH, body))
        .await?;
    state.cron.kick();

    Ok(record)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    pub runtime: String,
    pub spec: Spec,
}

fn build_key(runtime: &str, digest: &str) -> String {
    format!("{runtime}/{digest}")
}

pub async fn build(store: &dyn Store, runtime: &str, digest: &str) -> Option<Phase> {
    let held = store
        .get_note(BUILDS, &build_key(runtime, digest))
        .await
        .ok()??;
    serde_json::from_str(&held).ok()
}

async fn set_build(
    store: &dyn Store,
    key: &str,
    phase: &Phase,
    until_ms: Option<u64>,
) -> ApiResult<()> {
    let value = serde_json::to_string(phase)
        .map_err(|error| ApiError::internal(format!("a phase would not serialise: {error}")))?;

    Ok(store.put_note(BUILDS, key, &value, until_ms).await?)
}

pub async fn queue_build(state: &AppState, runtime: &str, spec: &Spec) -> ApiResult<()> {
    let store = state.store.as_ref();
    let digest = spec.digest();
    if build(store, runtime, &digest).await == Some(Phase::Starting) {
        return Ok(());
    }

    let job = Build {
        runtime: runtime.to_string(),
        spec: spec.clone(),
    };
    let body = serde_json::to_string(&job)
        .map_err(|error| ApiError::internal(format!("a job would not serialise: {error}")))?;

    set_build(store, &build_key(runtime, &digest), &Phase::Starting, None).await?;
    store
        .push_job(&JobRecord::new(
            format!("job_build_{}", worker_name()),
            BUILD,
            body,
        ))
        .await?;
    state.cron.kick();

    Ok(())
}

async fn built(state: &AppState, job: &JobRecord) -> ApiResult<()> {
    let build: Build = serde_json::from_str(&job.body)
        .map_err(|error| ApiError::internal(format!("the job does not parse: {error}")))?;
    let store = state.store.as_ref();
    let key = build_key(&build.runtime, &build.spec.digest());

    let done = match state.runtimes.get(&build.runtime) {
        Some(runtime) => crate::images::prepare(state, &runtime, &build.spec)
            .await
            .map(|_| ()),
        None => Err(ApiError::not_found(format!(
            "no runtime named {} here",
            build.runtime
        ))),
    };

    match done {
        Ok(()) => Ok(store.forget_note(BUILDS, &key).await?),
        Err(why) => {
            let reason = why.body.message.clone();
            let until = crate::routes::ms_of(std::time::SystemTime::now()) + FAILED_BUILD_KEPT_MS;
            set_build(store, &key, &Phase::Failed { reason }, Some(until)).await
        }
    }
}

async fn launched(state: &AppState, job: &JobRecord) -> ApiResult<()> {
    let launch: Launch = serde_json::from_str(&job.body)
        .map_err(|error| ApiError::internal(format!("the job does not parse: {error}")))?;
    let store = state.store.as_ref();

    if job.attempts > ATTEMPTS {
        let reason = "the server that was starting it stopped before it was ready".to_string();
        return set_phase(store, &launch.id, &Phase::Failed { reason }).await;
    }

    match self::launch(state, &launch).await {
        Ok((entry, _)) => {
            if store.get_box(&launch.id).await?.is_none()
                || phase(store, &launch.id).await.is_none()
            {
                let _ = state.registry.remove(&entry.id).await;
                let _ = store.forget_box(&entry.id).await;
                forget_phase(store, &launch.id).await;
                return Ok(());
            }
            forget_phase(store, &launch.id).await;
            Ok(())
        }
        Err(why) => {
            tracing::warn!(box_ = %launch.id, why = %why.body.message, "a queued box did not start");
            let reason = why.body.message.clone();
            state
                .box_event(
                    &launch.id,
                    "box.failed",
                    serde_json::json!({ "why": reason }),
                )
                .await;
            set_phase(store, &launch.id, &Phase::Failed { reason }).await
        }
    }
}

pub async fn run_one(state: &Arc<AppState>, worker: &str) -> bool {
    let job = match state.store.claim_job(worker, LEASE_MS).await {
        Ok(Some(job)) => job,
        Ok(None) => return false,
        Err(why) => {
            tracing::warn!(%why, "the job queue could not be read");
            return false;
        }
    };

    let renewing = {
        let (state, id, worker) = (Arc::clone(state), job.id.clone(), worker.to_string());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(RENEW).await;
                let _ = state.store.renew_job(&id, &worker, LEASE_MS).await;
            }
        })
    };

    let done = match job.kind.as_str() {
        LAUNCH => launched(state, &job).await,
        BUILD => built(state, &job).await,
        other => Err(ApiError::internal(format!("no job of kind {other}"))),
    };
    renewing.abort();

    if let Err(why) = &done {
        tracing::warn!(job = %job.id, why = %why.body.message, "a job did not finish");
    }
    if let Err(why) = state.store.finish_job(&job.id).await {
        tracing::warn!(job = %job.id, %why, "a finished job was not removed");
    }
    true
}

pub fn spawn(state: Arc<AppState>) {
    if state.jobs != Mode::Queue {
        return;
    }

    for n in 0..AT_ONCE {
        let state = Arc::clone(&state);
        let worker = format!("{}-{n}", worker_name());
        tokio::spawn(async move {
            loop {
                if !run_one(&state, &worker).await {
                    tokio::time::sleep(IDLE).await;
                }
            }
        });
    }
}

pub(crate) fn worker_name() -> String {
    let mut bytes = [0u8; 6];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
