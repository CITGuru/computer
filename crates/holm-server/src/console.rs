use crate::caller::{Caller, ConsoleKey, Kind, Token};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use holm_storage::Store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

pub const URL: &str = "HOLM_CONSOLE_URL";
pub const SECRET: &str = "HOLM_CONSOLE_SECRET";
pub const API_KEY_PREFIX: &str = "holm_sk_";

pub const KEYS_PATH: &str = "/api/holmd/keys.json";
pub const REVOKED_PATH: &str = "/api/holmd/revoked";
pub const USAGE_PATH: &str = "/api/holmd/usage";
pub const FUNDS_PATH: &str = "/api/holmd/funds";
pub const PUSH_PATH: &str = "/v1/console/revoked";

pub const PUSH_ID: &str = "webhook-id";
pub const PUSH_TIMESTAMP: &str = "webhook-timestamp";
pub const PUSH_SIGNATURE: &str = "webhook-signature";

const KIND: &str = "revoked";
const PUSH_WINDOW_SECS: u64 = 300;
const PUSHED_FOR: Duration = Duration::from_secs(5);
const REVOKED_EVERY_PUSHED: Duration = Duration::from_secs(300);

const REVOKED_EVERY: Duration = Duration::from_secs(30);
const KEYS_EVERY: Duration = Duration::from_secs(300);
const USAGE_EVERY: Duration = Duration::from_secs(60);
const UNKNOWN_KEY_GAP: Duration = Duration::from_secs(30);
const ASKING: Duration = Duration::from_secs(10);
const FUNDED_FOR: Duration = Duration::from_secs(10);

pub struct Console {
    pinned: Option<ConsoleKey>,
    link: Option<Link>,
    keys: RwLock<HashMap<String, ConsoleKey>>,
    revoked: RwLock<Option<Revoked>>,
    used: Mutex<BTreeMap<String, u64>>,
    keys_asked: Mutex<Option<Instant>>,
    store: Option<Arc<dyn Store>>,
    pushed: RwLock<Option<(Instant, Revoked)>>,
    push_seen: AtomicBool,
    funded: Mutex<HashMap<String, Instant>>,
}

#[derive(Deserialize)]
struct Funds {
    allowed: bool,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Unfunded(String),
    Unknown(String),
}

#[derive(Debug, Deserialize)]
pub struct Push {
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub workspaces: Vec<String>,
    #[serde(default)]
    pub until_ms: Option<u64>,
}

struct Link {
    base: String,
    secret: Option<String>,
    http: reqwest::Client,
}

impl Link {
    async fn asked(
        &self,
        path: &str,
        make: impl Fn(String) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, String> {
        make(format!("{}{path}", self.base))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revoked {
    #[serde(default)]
    pub keys: HashSet<String>,
    #[serde(default)]
    pub workspaces: HashSet<String>,
}

#[derive(Deserialize)]
struct KeySet {
    keys: Vec<Published>,
}

#[derive(Deserialize)]
struct Published {
    kid: String,
    #[serde(default)]
    crv: Option<String>,
    x: String,
}

#[derive(Serialize)]
struct Usage<'a> {
    keys: &'a BTreeMap<String, u64>,
}

impl Console {
    pub fn pinned(key: ConsoleKey) -> Self {
        Self::new(Some(key), None)
    }

    pub fn linked(
        base: &str,
        secret: Option<String>,
        pinned: Option<ConsoleKey>,
    ) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(ASKING)
            .build()
            .map_err(|error| format!("a client for the console: {error}"))?;
        Ok(Self::new(
            pinned,
            Some(Link {
                base: base.trim_end_matches('/').to_string(),
                secret,
                http,
            }),
        ))
    }

    fn new(pinned: Option<ConsoleKey>, link: Option<Link>) -> Self {
        Self {
            pinned,
            link,
            keys: RwLock::new(HashMap::new()),
            revoked: RwLock::new(None),
            used: Mutex::new(BTreeMap::new()),
            keys_asked: Mutex::new(None),
            store: None,
            pushed: RwLock::new(None),
            push_seen: AtomicBool::new(false),
            funded: Mutex::new(HashMap::new()),
        }
    }

    pub fn watching(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    pub fn admits_push(&self, id: &str, timestamp: &str, signature: &str, body: &[u8]) -> bool {
        let Some(secret) = self.link.as_ref().and_then(|link| link.secret.as_ref()) else {
            return false;
        };
        let Ok(at) = timestamp.parse::<u64>() else {
            return false;
        };
        if crate::caller::now_secs().abs_diff(at) > PUSH_WINDOW_SECS {
            return false;
        }
        let Some(tag) = signature
            .strip_prefix("v1,")
            .and_then(|tag| base64::engine::general_purpose::STANDARD.decode(tag).ok())
        else {
            return false;
        };

        let mut signed = format!("{id}.{timestamp}.").into_bytes();
        signed.extend_from_slice(body);
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
        ring::hmac::verify(&key, &signed, &tag).is_ok()
    }

    pub async fn take_push(&self, push: &Push) -> Result<(), String> {
        let store = self.store.as_ref().ok_or("no store to keep a push in")?;

        for (prefix, ids) in [("key/", &push.keys), ("ws/", &push.workspaces)] {
            for id in ids {
                store
                    .put_note(KIND, &format!("{prefix}{id}"), "", push.until_ms)
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }

        self.push_seen.store(true, Ordering::Relaxed);
        if let Ok(mut pushed) = self.pushed.write() {
            *pushed = None;
        }
        Ok(())
    }

    async fn pushed(&self) -> Revoked {
        if let Ok(held) = self.pushed.read()
            && let Some((at, list)) = held.as_ref()
            && at.elapsed() < PUSHED_FOR
        {
            return list.clone();
        }
        let Some(store) = &self.store else {
            return Revoked::default();
        };

        let mut list = Revoked::default();
        match store.list_notes(KIND, "").await {
            Ok(notes) => {
                for (key, _) in notes {
                    if let Some(id) = key.strip_prefix("key/") {
                        list.keys.insert(id.to_string());
                    } else if let Some(id) = key.strip_prefix("ws/") {
                        list.workspaces.insert(id.to_string());
                    }
                }
            }
            Err(why) => {
                tracing::warn!(%why, "the pushed revocations could not be read");
                return self
                    .pushed
                    .read()
                    .ok()
                    .and_then(|held| held.as_ref().map(|(_, list)| list.clone()))
                    .unwrap_or_default();
            }
        }

        if !list.keys.is_empty() || !list.workspaces.is_empty() {
            self.push_seen.store(true, Ordering::Relaxed);
        }
        if let Ok(mut pushed) = self.pushed.write() {
            *pushed = Some((Instant::now(), list.clone()));
        }
        list
    }

    pub fn from_env() -> Result<Option<Self>, String> {
        let pinned = ConsoleKey::from_env()?;
        let secret = std::env::var(SECRET)
            .ok()
            .filter(|secret| !secret.is_empty());

        match std::env::var(URL).ok().filter(|url| !url.is_empty()) {
            Some(base) => Self::linked(&base, secret, pinned).map(Some),
            None => Ok(pinned.map(Self::pinned)),
        }
    }

    pub fn configured() -> bool {
        [
            URL,
            crate::caller::CONSOLE_KEY,
            crate::caller::CONSOLE_KEY_FILE,
        ]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
    }

    pub async fn verify(&self, bearer: &str, now_secs: u64) -> Option<Caller> {
        match bearer.strip_prefix(API_KEY_PREFIX) {
            Some(key) => self.verify_key(key, now_secs).await,
            None => {
                let token = Token::parse(bearer)?;
                self.signed(&token)
                    .await
                    .then(|| token.caller(Kind::Call, now_secs))?
            }
        }
    }

    async fn verify_key(&self, key: &str, now_secs: u64) -> Option<Caller> {
        let token = Token::parse(key)?;
        if !self.signed(&token).await {
            return None;
        }
        let caller = token.caller(Kind::Key, now_secs)?;
        let id = token.key_id()?.to_string();

        if self.link.is_some() {
            {
                let revoked = self.revoked.read().ok()?;
                let list = revoked.as_ref()?;
                if list.keys.contains(&id) || list.workspaces.contains(token.workspace()) {
                    return None;
                }
            }
            let pushed = self.pushed().await;
            if pushed.keys.contains(&id) || pushed.workspaces.contains(token.workspace()) {
                return None;
            }
        }

        if let Ok(mut used) = self.used.lock() {
            used.insert(id, now_secs.saturating_mul(1000));
        }
        Some(caller)
    }

    async fn signed(&self, token: &Token) -> bool {
        if let Some(kid) = &token.kid {
            if self.known(kid, token) {
                return true;
            }
            if self.link.is_some() && self.may_ask_for_keys() {
                if let Err(why) = self.refresh_keys().await {
                    tracing::warn!(%why, "the console's keys could not be read");
                }
                if self.known(kid, token) {
                    return true;
                }
            }
        }
        self.pinned
            .as_ref()
            .is_some_and(|pinned| pinned.verifies(token))
    }

    fn known(&self, kid: &str, token: &Token) -> bool {
        self.keys
            .read()
            .ok()
            .and_then(|keys| keys.get(kid).map(|key| key.verifies(token)))
            .unwrap_or(false)
    }

    fn may_ask_for_keys(&self) -> bool {
        let Ok(mut asked) = self.keys_asked.lock() else {
            return false;
        };
        match *asked {
            Some(at) if at.elapsed() < UNKNOWN_KEY_GAP => false,
            _ => {
                *asked = Some(Instant::now());
                true
            }
        }
    }

    pub async fn refresh_keys(&self) -> Result<usize, String> {
        let link = self.link.as_ref().ok_or("no console to ask")?;
        let set: KeySet = link
            .asked(KEYS_PATH, |url| link.http.get(url))
            .await?
            .json()
            .await
            .map_err(|error| error.to_string())?;

        let mut fresh = HashMap::new();
        for published in set.keys {
            if published.crv.as_deref().is_some_and(|crv| crv != "Ed25519") {
                continue;
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(published.x.trim_end_matches('='))
                .map_err(|_| format!("key {} is not base64url", published.kid))?;
            fresh.insert(published.kid, ConsoleKey::raw(bytes)?);
        }

        let count = fresh.len();
        if let Ok(mut keys) = self.keys.write() {
            *keys = fresh;
        }
        Ok(count)
    }

    pub async fn refresh_revoked(&self) -> Result<(), String> {
        let link = self.link.as_ref().ok_or("no console to ask")?;
        let list: Revoked = link
            .asked(REVOKED_PATH, |url| {
                let request = link.http.get(url);
                match &link.secret {
                    Some(secret) => request.bearer_auth(secret),
                    None => request,
                }
            })
            .await?
            .json()
            .await
            .map_err(|error| error.to_string())?;

        if let Ok(mut revoked) = self.revoked.write() {
            *revoked = Some(list);
        }
        Ok(())
    }

    pub async fn funds(&self, workspace: &str) -> Result<(), Refusal> {
        let Some(link) = &self.link else {
            return Ok(());
        };
        let known = self
            .funded
            .lock()
            .ok()
            .and_then(|funded| funded.get(workspace).copied());
        if known.is_some_and(|at| at.elapsed() < FUNDED_FOR) {
            return Ok(());
        }

        let asked = link
            .asked(FUNDS_PATH, |url| {
                let request = link.http.get(url).query(&[("workspace", workspace)]);
                match &link.secret {
                    Some(secret) => request.bearer_auth(secret),
                    None => request,
                }
            })
            .await;
        let answer = match asked {
            Ok(answer) => answer
                .json::<Funds>()
                .await
                .map_err(|error| error.to_string()),
            Err(error) => Err(error),
        };

        match answer {
            Ok(Funds { allowed: true, .. }) => {
                if let Ok(mut funded) = self.funded.lock() {
                    funded.insert(workspace.to_string(), Instant::now());
                }
                Ok(())
            }
            Ok(Funds { reason, .. }) => {
                if let Ok(mut funded) = self.funded.lock() {
                    funded.remove(workspace);
                }
                Err(Refusal::Unfunded(reason.unwrap_or_else(|| {
                    "this workspace has no credit for a shared runtime".to_string()
                })))
            }
            Err(_) if known.is_some() => Ok(()),
            Err(why) => Err(Refusal::Unknown(why.to_string())),
        }
    }

    pub async fn report_usage(&self) -> Result<(), String> {
        let link = self.link.as_ref().ok_or("no console to tell")?;
        let used = match self.used.lock() {
            Ok(mut used) => std::mem::take(&mut *used),
            Err(_) => return Ok(()),
        };
        if used.is_empty() {
            return Ok(());
        }

        link.asked(USAGE_PATH, |url| {
            let request = link.http.post(url).json(&Usage { keys: &used });
            match &link.secret {
                Some(secret) => request.bearer_auth(secret),
                None => request,
            }
        })
        .await
        .map(|_| ())
    }

    pub fn follow(self: &Arc<Self>) {
        if self.link.is_none() {
            return;
        }
        let console = Arc::clone(self);
        tokio::spawn(async move {
            let mut keys_at: Option<Instant> = None;
            let mut usage_at = Instant::now();
            loop {
                if keys_at.is_none_or(|at| at.elapsed() >= KEYS_EVERY) {
                    match console.refresh_keys().await {
                        Ok(_) => keys_at = Some(Instant::now()),
                        Err(why) => tracing::warn!(%why, "the console's keys could not be read"),
                    }
                }
                if let Err(why) = console.refresh_revoked().await {
                    tracing::warn!(%why, "the console's revoked keys could not be read");
                }
                if usage_at.elapsed() >= USAGE_EVERY {
                    if let Err(why) = console.report_usage().await {
                        tracing::warn!(%why, "key use could not be reported to the console");
                    }
                    usage_at = Instant::now();
                }
                tokio::time::sleep(match console.push_seen.load(Ordering::Relaxed) {
                    true => REVOKED_EVERY_PUSHED,
                    false => REVOKED_EVERY,
                })
                .await;
            }
        });
    }
}
