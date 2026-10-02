use crate::AppState;
use axum::http::HeaderMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MODE: &str = "COMPUTER_SERVER_SCHEDULE";
pub const SECRET: &str = "CRON_SECRET";
pub const PUBLIC_URL: &str = "COMPUTER_PUBLIC_URL";

pub const PATHS: &str = "/v1/jobs/";
pub const RUN_PATH: &str = "/v1/jobs/run";

pub const RUN_BUDGET: Duration = Duration::from_secs(50);
const KICK_WAIT: Duration = Duration::from_secs(800);
const LEADER_RETRY: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Schedule {
    #[default]
    Internal,
    External,
}

impl Schedule {
    pub fn from_env() -> Result<Self, String> {
        match std::env::var(MODE).ok().as_deref() {
            None | Some("") | Some("internal") => Ok(Self::Internal),
            Some("external") => Ok(Self::External),
            Some(other) => Err(format!("{MODE} is internal or external, not {other}")),
        }
    }
}

#[derive(Default)]
pub struct Cron {
    pub schedule: Schedule,
    pub secret: Option<computer::Secret>,
    pub public_url: Option<String>,
}

impl Cron {
    pub fn from_env() -> Result<Self, String> {
        let secret = match std::env::var(SECRET).ok().filter(|held| !held.is_empty()) {
            Some(held) => Some(computer::Secret::new(held).map_err(|error| error.to_string())?),
            None => None,
        };

        Ok(Self {
            schedule: Schedule::from_env()?,
            secret,
            public_url: std::env::var(PUBLIC_URL)
                .ok()
                .filter(|held| !held.is_empty())
                .map(|held| held.trim_end_matches('/').to_string()),
        })
    }

    pub fn admits(&self, headers: &HeaderMap) -> bool {
        let Some(secret) = &self.secret else {
            return false;
        };
        let offered = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or_default();

        crate::auth::same(offered.as_bytes(), secret.expose().as_bytes())
    }

    pub fn kick(&self) {
        if self.schedule != Schedule::External {
            return;
        }
        let (Some(base), Some(secret)) = (&self.public_url, &self.secret) else {
            return;
        };

        let (url, bearer) = (format!("{base}{RUN_PATH}"), secret.expose().to_string());
        tokio::spawn(async move {
            let sent = reqwest::Client::new()
                .post(&url)
                .bearer_auth(bearer)
                .timeout(KICK_WAIT)
                .send()
                .await;
            if let Err(why) = sent {
                tracing::debug!(%why, "the job runner was not reached; the next scheduled run takes the job");
            }
        });
    }
}

pub fn spawn(state: Arc<AppState>, reap_every: Duration, prune_every: Duration) {
    if state.cron.schedule == Schedule::External {
        tracing::info!("periodic work and jobs wait for calls to {PATHS}");
        return;
    }

    crate::jobs::spawn(Arc::clone(&state));

    tokio::spawn(async move {
        let _leader = loop {
            match state.store.lock("leader").await {
                Ok(held) => break held,
                Err(why) => {
                    tracing::warn!(%why, "the leader lock could not be taken");
                    tokio::time::sleep(LEADER_RETRY).await;
                }
            }
        };
        tracing::info!("this server runs the periodic work");

        let mut pruned = Instant::now();
        loop {
            tokio::time::sleep(reap_every).await;
            crate::reap::once(&state).await;

            if pruned.elapsed() >= prune_every {
                crate::prune::sweep(&state).await;
                pruned = Instant::now();
            }
        }
    });
}

pub async fn run_jobs(state: &Arc<AppState>, budget: Duration) -> usize {
    let started = Instant::now();
    let worker = crate::jobs::worker_name();
    let mut ran = 0;

    while started.elapsed() < budget && crate::jobs::run_one(state, &worker).await {
        ran += 1;
    }
    ran
}
