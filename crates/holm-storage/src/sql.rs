use crate::{BoxRecord, Error, Frames, ImageRecord, Result, RuntimeRecord, Store, now_ms};
use async_trait::async_trait;
use holm_api::{Actor, TraceEntry, TraceEvent};
use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, AssertSqlSafe, Row, error::DatabaseError};
use std::sync::Arc;

const ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    Sqlite,
    Postgres,
}

pub struct Sql {
    pool: AnyPool,
    dialect: Dialect,
    locks: crate::Locks,
    leasing: bool,
}

const LOCKS: &str = "locks";
const LEASE_MS: u64 = 30_000;
const LEASE_RENEW: std::time::Duration = std::time::Duration::from_secs(10);
const LEASE_POLL: std::time::Duration = std::time::Duration::from_millis(100);

const CLAIM: &str = "INSERT INTO notes (kind, key, value, until_ms)
     VALUES (?, ?, ?, ?)
     ON CONFLICT (kind, key) DO UPDATE SET
         value = excluded.value,
         until_ms = excluded.until_ms
     WHERE notes.until_ms <= ?";

struct Lease {
    pool: AnyPool,
    dialect: Dialect,
    name: String,
    token: String,
    renewing: tokio::task::JoinHandle<()>,
    _local: tokio::sync::OwnedMutexGuard<()>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.renewing.abort();

        let (pool, name, token) = (self.pool.clone(), self.name.clone(), self.token.clone());
        let written = numbered(
            self.dialect,
            "DELETE FROM notes WHERE kind = ? AND key = ? AND value = ?",
        );
        tokio::spawn(async move {
            let _ = sqlx::query(AssertSqlSafe(written))
                .bind(LOCKS)
                .bind(name)
                .bind(token)
                .execute(&pool)
                .await;
        });
    }
}

const IDLE: std::time::Duration = std::time::Duration::from_secs(30);

fn shown(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let Some((user, _)) = authority.rsplit_once('@') else {
        return url.to_string();
    };
    let (Some((name, _)), Some(after)) = (user.split_once(':'), rest.strip_prefix(user)) else {
        return url.to_string();
    };

    format!("{scheme}://{name}:***{after}")
}

fn token() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let turn = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();

    format!("{}-{nanos}-{turn}", std::process::id())
}

impl Sql {
    pub async fn open(url: &str) -> Result<Self> {
        Self::open_sized(url, None).await
    }

    pub async fn open_sized(url: &str, connections: Option<u32>) -> Result<Self> {
        sqlx::any::install_default_drivers();

        let dialect = match url {
            _ if url.starts_with("sqlite:") => Dialect::Sqlite,
            _ if url.starts_with("postgres:") || url.starts_with("postgresql:") => {
                Dialect::Postgres
            }
            _ => {
                return Err(Error::Unavailable(format!(
                    "{} names no dialect this store has: use sqlite:// or postgres://",
                    shown(url)
                )));
            }
        };

        let mut options = AnyPoolOptions::new().idle_timeout(IDLE);
        if let Some(connections) = connections {
            options = options.max_connections(connections.max(1));
        }
        let pool = options
            .connect(url)
            .await
            .map_err(|error| Error::Unavailable(format!("{}: {error}", shown(url))))?;

        let store = Self {
            pool,
            dialect,
            locks: crate::Locks::default(),
            leasing: dialect == Dialect::Postgres,
        };
        store.migrate().await?;

        Ok(store)
    }

    pub fn leasing(mut self) -> Self {
        self.leasing = true;
        self
    }

    async fn claim(&self, name: &str, token: &str) -> Result<bool> {
        let now = now_ms();
        let done = sqlx::query(self.q(CLAIM))
            .bind(LOCKS)
            .bind(name)
            .bind(token)
            .bind((now + LEASE_MS) as i64)
            .bind(now as i64)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("a lock would not be taken", error))?;

        Ok(done.rows_affected() == 1)
    }

    async fn migrate(&self) -> Result<()> {
        let blob = match self.dialect {
            Dialect::Sqlite => "BLOB",
            Dialect::Postgres => "BYTEA",
        };

        let serial = match self.dialect {
            Dialect::Sqlite => "INTEGER PRIMARY KEY AUTOINCREMENT",
            Dialect::Postgres => "BIGSERIAL PRIMARY KEY",
        };

        let schema = [
            "CREATE TABLE IF NOT EXISTS boxes (
                 id TEXT PRIMARY KEY,
                 created_at_ms BIGINT NOT NULL,
                 expires_at_ms BIGINT,
                 record TEXT NOT NULL
             )"
            .to_string(),
            "CREATE TABLE IF NOT EXISTS entries (
                 box_id TEXT NOT NULL,
                 seq BIGINT NOT NULL,
                 at_ms BIGINT NOT NULL,
                 kind TEXT NOT NULL,
                 entry TEXT NOT NULL,
                 PRIMARY KEY (box_id, seq)
             )"
            .to_string(),
            format!(
                "CREATE TABLE IF NOT EXISTS frames (
                     box_id TEXT NOT NULL,
                     hash TEXT NOT NULL,
                     png {blob} NOT NULL,
                     PRIMARY KEY (box_id, hash)
                 )"
            ),
            "CREATE TABLE IF NOT EXISTS runtimes (
                 name TEXT PRIMARY KEY,
                 created_at_ms BIGINT NOT NULL,
                 updated_at_ms BIGINT NOT NULL,
                 record TEXT NOT NULL
             )"
            .to_string(),
            "CREATE TABLE IF NOT EXISTS images (
                 runtime TEXT NOT NULL,
                 spec_digest TEXT NOT NULL,
                 built_at_ms BIGINT NOT NULL,
                 record TEXT NOT NULL,
                 PRIMARY KEY (runtime, spec_digest)
             )"
            .to_string(),
            "CREATE TABLE IF NOT EXISTS notes (
                 kind TEXT NOT NULL,
                 key TEXT NOT NULL,
                 value TEXT NOT NULL,
                 until_ms BIGINT,
                 PRIMARY KEY (kind, key)
             )"
            .to_string(),
            "CREATE TABLE IF NOT EXISTS jobs (
                 id TEXT PRIMARY KEY,
                 kind TEXT NOT NULL,
                 body TEXT NOT NULL,
                 created_at_ms BIGINT NOT NULL,
                 attempts BIGINT NOT NULL,
                 claimed_by TEXT,
                 claimed_until_ms BIGINT
             )"
            .to_string(),
            format!(
                "CREATE TABLE IF NOT EXISTS events (
                     seq {serial},
                     at_ms BIGINT NOT NULL,
                     kind TEXT NOT NULL,
                     owner TEXT,
                     box_id TEXT,
                     runtime TEXT,
                     data TEXT NOT NULL
                 )"
            ),
            "CREATE TABLE IF NOT EXISTS sequences (
                 box_id TEXT PRIMARY KEY,
                 next BIGINT NOT NULL
             )"
            .to_string(),
            "CREATE INDEX IF NOT EXISTS entries_by_box ON entries (box_id, seq)".to_string(),
            "CREATE INDEX IF NOT EXISTS frames_by_box ON frames (box_id)".to_string(),
        ];

        for statement in schema {
            sqlx::query(AssertSqlSafe(statement))
                .execute(&self.pool)
                .await
                .map_err(|error| failed("the schema would not go on", error))?;
        }

        Ok(())
    }

    fn q(&self, written: &str) -> AssertSqlSafe<String> {
        AssertSqlSafe(numbered(self.dialect, written))
    }
}

/// These statements are written with `?`; Postgres counts instead.
fn numbered(dialect: Dialect, written: &str) -> String {
    if dialect == Dialect::Sqlite {
        return written.to_string();
    }

    let mut out = String::with_capacity(written.len() + 8);
    for (index, part) in written.split('?').enumerate() {
        if index > 0 {
            out.push('$');
            out.push_str(&index.to_string());
        }
        out.push_str(part);
    }

    out
}

#[async_trait]
impl Store for Sql {
    async fn put_box(&self, record: &BoxRecord) -> Result<()> {
        let body = serde_json::to_string(record)
            .map_err(|error| Error::Internal(format!("a record would not serialise: {error}")))?;

        sqlx::query(self.q(
            "INSERT INTO boxes (id, created_at_ms, expires_at_ms, record)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                 created_at_ms = excluded.created_at_ms,
                 expires_at_ms = excluded.expires_at_ms,
                 record = excluded.record",
        ))
        .bind(&record.id)
        .bind(record.created_at_ms as i64)
        .bind(record.expires_at_ms.map(|at| at as i64))
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(|error| failed("a record would not go down", error))?;

        Ok(())
    }

    async fn get_box(&self, id: &str) -> Result<Option<BoxRecord>> {
        let held: Option<String> =
            sqlx::query_scalar(self.q("SELECT record FROM boxes WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| failed("a record would not come back", error))?;

        held.map(|body| {
            serde_json::from_str(&body).map_err(|error| {
                Error::Corrupt(format!("the record for {id} does not parse: {error}"))
            })
        })
        .transpose()
    }

    async fn list_boxes(&self) -> Result<Vec<BoxRecord>> {
        let rows: Vec<String> = sqlx::query_scalar(self.q("SELECT record FROM boxes ORDER BY id"))
            .fetch_all(&self.pool)
            .await
            .map_err(|error| failed("the records would not come back", error))?;

        rows.iter()
            .map(|body| {
                serde_json::from_str(body)
                    .map_err(|error| Error::Corrupt(format!("a record does not parse: {error}")))
            })
            .collect()
    }

    async fn forget_box(&self, id: &str) -> Result<()> {
        for statement in [
            "DELETE FROM boxes WHERE id = ?",
            "DELETE FROM entries WHERE box_id = ?",
            "DELETE FROM sequences WHERE box_id = ?",
        ] {
            sqlx::query(self.q(statement))
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|error| failed("a box would not be forgotten", error))?;
        }

        Ok(())
    }

    async fn put_runtime(&self, record: &RuntimeRecord) -> Result<()> {
        let body = serde_json::to_string(record)
            .map_err(|error| Error::Internal(format!("a record would not serialise: {error}")))?;

        sqlx::query(self.q(
            "INSERT INTO runtimes (name, created_at_ms, updated_at_ms, record)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (name) DO UPDATE SET
                 updated_at_ms = excluded.updated_at_ms,
                 record = excluded.record",
        ))
        .bind(&record.name)
        .bind(record.created_at_ms as i64)
        .bind(record.updated_at_ms as i64)
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(|error| failed("a runtime would not go down", error))?;

        Ok(())
    }

    async fn get_runtime(&self, name: &str) -> Result<Option<RuntimeRecord>> {
        let held: Option<String> =
            sqlx::query_scalar(self.q("SELECT record FROM runtimes WHERE name = ?"))
                .bind(name)
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| failed("a runtime would not be read", error))?;

        held.map(|body| {
            serde_json::from_str(&body).map_err(|error| {
                Error::Corrupt(format!("the runtime {name} does not parse: {error}"))
            })
        })
        .transpose()
    }

    async fn list_runtimes(&self) -> Result<Vec<RuntimeRecord>> {
        let rows: Vec<String> =
            sqlx::query_scalar(self.q("SELECT record FROM runtimes ORDER BY name"))
                .fetch_all(&self.pool)
                .await
                .map_err(|error| failed("the runtimes would not be read", error))?;

        rows.into_iter()
            .map(|body| {
                serde_json::from_str(&body)
                    .map_err(|error| Error::Corrupt(format!("a runtime does not parse: {error}")))
            })
            .collect()
    }

    async fn forget_runtime(&self, name: &str) -> Result<()> {
        sqlx::query(self.q("DELETE FROM runtimes WHERE name = ?"))
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("a runtime would not go away", error))?;

        Ok(())
    }

    async fn put_image(&self, record: &ImageRecord) -> Result<()> {
        let body = serde_json::to_string(record)
            .map_err(|error| Error::Internal(format!("a record would not serialise: {error}")))?;

        sqlx::query(self.q(
            "INSERT INTO images (runtime, spec_digest, built_at_ms, record)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (runtime, spec_digest) DO UPDATE SET
                 built_at_ms = excluded.built_at_ms,
                 record = excluded.record",
        ))
        .bind(&record.runtime)
        .bind(&record.spec_digest)
        .bind(record.built_at_ms as i64)
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(|error| failed("an image would not go down", error))?;

        Ok(())
    }

    async fn get_image(&self, runtime: &str, spec_digest: &str) -> Result<Option<ImageRecord>> {
        let held: Option<String> = sqlx::query_scalar(
            self.q("SELECT record FROM images WHERE runtime = ? AND spec_digest = ?"),
        )
        .bind(runtime)
        .bind(spec_digest)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| failed("an image would not be read", error))?;

        held.map(|body| {
            serde_json::from_str(&body)
                .map_err(|error| Error::Corrupt(format!("an image does not parse: {error}")))
        })
        .transpose()
    }

    async fn list_images(&self) -> Result<Vec<ImageRecord>> {
        let rows: Vec<String> =
            sqlx::query_scalar(self.q("SELECT record FROM images ORDER BY runtime, spec_digest"))
                .fetch_all(&self.pool)
                .await
                .map_err(|error| failed("the images would not be read", error))?;

        rows.into_iter()
            .map(|body| {
                serde_json::from_str(&body)
                    .map_err(|error| Error::Corrupt(format!("an image does not parse: {error}")))
            })
            .collect()
    }

    async fn forget_image(&self, runtime: &str, spec_digest: &str) -> Result<()> {
        sqlx::query(self.q("DELETE FROM images WHERE runtime = ? AND spec_digest = ?"))
            .bind(runtime)
            .bind(spec_digest)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("an image would not go away", error))?;

        Ok(())
    }

    async fn lock(&self, name: &str) -> Result<crate::Lock> {
        let local = self.locks.hold(name).await;
        if !self.leasing {
            return Ok(crate::Lock::of(local));
        }

        let token = token();
        while !self.claim(name, &token).await? {
            tokio::time::sleep(LEASE_POLL).await;
        }

        let (pool, held, mine) = (self.pool.clone(), name.to_string(), token.clone());
        let written = numbered(
            self.dialect,
            "UPDATE notes SET until_ms = ? WHERE kind = ? AND key = ? AND value = ?",
        );
        let renewing = tokio::spawn(async move {
            loop {
                tokio::time::sleep(LEASE_RENEW).await;
                let _ = sqlx::query(AssertSqlSafe(written.clone()))
                    .bind((now_ms() + LEASE_MS) as i64)
                    .bind(LOCKS)
                    .bind(&held)
                    .bind(&mine)
                    .execute(&pool)
                    .await;
            }
        });

        Ok(crate::Lock::of(Lease {
            pool: self.pool.clone(),
            dialect: self.dialect,
            name: name.to_string(),
            token,
            renewing,
            _local: local,
        }))
    }

    async fn push_job(&self, job: &crate::JobRecord) -> Result<()> {
        sqlx::query(self.q(
            "INSERT INTO jobs (id, kind, body, created_at_ms, attempts) VALUES (?, ?, ?, ?, ?)",
        ))
        .bind(&job.id)
        .bind(&job.kind)
        .bind(&job.body)
        .bind(job.created_at_ms as i64)
        .bind(i64::from(job.attempts))
        .execute(&self.pool)
        .await
        .map_err(|error| failed("a job would not go down", error))?;

        Ok(())
    }

    async fn claim_job(&self, worker: &str, lease_ms: u64) -> Result<Option<crate::JobRecord>> {
        let skipping = match self.dialect {
            Dialect::Sqlite => "",
            Dialect::Postgres => "FOR UPDATE SKIP LOCKED",
        };
        let now = now_ms();
        let until = now + lease_ms;

        let row = sqlx::query(self.q(&format!(
            "UPDATE jobs SET claimed_by = ?, claimed_until_ms = ?, attempts = attempts + 1
             WHERE (claimed_until_ms IS NULL OR claimed_until_ms < ?) AND id = (
                 SELECT id FROM jobs
                 WHERE claimed_until_ms IS NULL OR claimed_until_ms < ?
                 ORDER BY created_at_ms, id LIMIT 1 {skipping}
             )
             RETURNING id, kind, body, created_at_ms, attempts"
        )))
        .bind(worker)
        .bind(until as i64)
        .bind(now as i64)
        .bind(now as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| failed("a job would not be claimed", error))?;

        let Some(row) = row else {
            return Ok(None);
        };
        let read = |error| failed("a job would not be read", error);
        let created: i64 = row.try_get(3).map_err(read)?;
        let attempts: i64 = row.try_get(4).map_err(read)?;

        Ok(Some(crate::JobRecord {
            id: row.try_get(0).map_err(read)?,
            kind: row.try_get(1).map_err(read)?,
            body: row.try_get(2).map_err(read)?,
            created_at_ms: created as u64,
            attempts: attempts as u32,
            claimed_by: Some(worker.to_string()),
            claimed_until_ms: Some(until),
        }))
    }

    async fn renew_job(&self, id: &str, worker: &str, lease_ms: u64) -> Result<bool> {
        let done = sqlx::query(
            self.q("UPDATE jobs SET claimed_until_ms = ? WHERE id = ? AND claimed_by = ?"),
        )
        .bind((now_ms() + lease_ms) as i64)
        .bind(id)
        .bind(worker)
        .execute(&self.pool)
        .await
        .map_err(|error| failed("a job would not be renewed", error))?;

        Ok(done.rows_affected() == 1)
    }

    async fn finish_job(&self, id: &str) -> Result<()> {
        sqlx::query(self.q("DELETE FROM jobs WHERE id = ?"))
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("a job would not go away", error))?;

        Ok(())
    }

    async fn append_event(&self, event: &crate::EventRecord) -> Result<u64> {
        let seq: i64 = sqlx::query_scalar(self.q(
            "INSERT INTO events (at_ms, kind, owner, box_id, runtime, data)
             VALUES (?, ?, ?, ?, ?, ?) RETURNING seq",
        ))
        .bind(event.at_ms as i64)
        .bind(&event.kind)
        .bind(&event.owner)
        .bind(&event.box_id)
        .bind(&event.runtime)
        .bind(event.data.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(|error| failed("an event would not go down", error))?;

        Ok(seq as u64)
    }

    async fn events_after(
        &self,
        after: u64,
        until_ms: u64,
        limit: usize,
    ) -> Result<Vec<crate::EventRecord>> {
        let rows = sqlx::query(self.q(
            "SELECT seq, at_ms, kind, owner, box_id, runtime, data FROM events
             WHERE seq > ? AND at_ms <= ? ORDER BY seq LIMIT ?",
        ))
        .bind(after as i64)
        .bind(until_ms as i64)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| failed("the events would not be read", error))?;

        rows.iter()
            .map(|row| {
                let read = |error| failed("an event would not be read", error);
                let seq: i64 = row.try_get(0).map_err(read)?;
                let at_ms: i64 = row.try_get(1).map_err(read)?;
                let data: String = row.try_get(6).map_err(read)?;

                Ok(crate::EventRecord {
                    seq: seq as u64,
                    at_ms: at_ms as u64,
                    kind: row.try_get(2).map_err(read)?,
                    owner: row.try_get(3).map_err(read)?,
                    box_id: row.try_get(4).map_err(read)?,
                    runtime: row.try_get(5).map_err(read)?,
                    data: serde_json::from_str(&data).unwrap_or_default(),
                })
            })
            .collect()
    }

    async fn prune_events(&self, before_ms: u64) -> Result<u64> {
        let done = sqlx::query(self.q("DELETE FROM events WHERE at_ms < ?"))
            .bind(before_ms as i64)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("old events would not go away", error))?;

        Ok(done.rows_affected())
    }

    async fn put_note(
        &self,
        kind: &str,
        key: &str,
        value: &str,
        until_ms: Option<u64>,
    ) -> Result<()> {
        sqlx::query(self.q("INSERT INTO notes (kind, key, value, until_ms)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (kind, key) DO UPDATE SET
                 value = excluded.value,
                 until_ms = excluded.until_ms"))
        .bind(kind)
        .bind(key)
        .bind(value)
        .bind(until_ms.map(|at| at as i64))
        .execute(&self.pool)
        .await
        .map_err(|error| failed("a note would not go down", error))?;

        Ok(())
    }

    async fn get_note(&self, kind: &str, key: &str) -> Result<Option<String>> {
        sqlx::query_scalar(self.q("SELECT value FROM notes
             WHERE kind = ? AND key = ? AND (until_ms IS NULL OR until_ms > ?)"))
        .bind(kind)
        .bind(key)
        .bind(now_ms() as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| failed("a note would not be read", error))
    }

    async fn list_notes(&self, kind: &str, prefix: &str) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query(self.q("SELECT key, value FROM notes
             WHERE kind = ? AND substr(key, 1, ?) = ? AND (until_ms IS NULL OR until_ms > ?)
             ORDER BY key"))
        .bind(kind)
        .bind(i32::try_from(prefix.chars().count()).unwrap_or(i32::MAX))
        .bind(prefix)
        .bind(now_ms() as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| failed("the notes would not be read", error))?;

        rows.iter()
            .map(|row| {
                let key: String = row
                    .try_get(0)
                    .map_err(|error| failed("a note would not be read", error))?;
                let value: String = row
                    .try_get(1)
                    .map_err(|error| failed("a note would not be read", error))?;
                Ok((key, value))
            })
            .collect()
    }

    async fn forget_note(&self, kind: &str, key: &str) -> Result<()> {
        sqlx::query(self.q("DELETE FROM notes WHERE kind = ? AND key = ?"))
            .bind(kind)
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("a note would not go away", error))?;

        Ok(())
    }

    async fn forget_notes(&self, kind: &str, prefix: &str) -> Result<()> {
        sqlx::query(self.q("DELETE FROM notes WHERE kind = ? AND substr(key, 1, ?) = ?"))
            .bind(kind)
            .bind(i32::try_from(prefix.chars().count()).unwrap_or(i32::MAX))
            .bind(prefix)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("the notes would not go away", error))?;

        Ok(())
    }

    async fn prune_notes(&self, now_ms: u64) -> Result<u64> {
        let done = sqlx::query(self.q("DELETE FROM notes WHERE until_ms <= ?"))
            .bind(now_ms as i64)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("old notes would not go away", error))?;

        Ok(done.rows_affected())
    }

    async fn append(
        &self,
        id: &str,
        actor: Actor,
        event: TraceEvent,
        frame: Option<String>,
    ) -> Result<u64> {
        let kind = kind_of(&event);

        for _ in 0..ATTEMPTS {
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|error| failed("no transaction to append in", error))?;

            let held: Option<i64> =
                sqlx::query_scalar(self.q("SELECT next FROM sequences WHERE box_id = ?"))
                    .bind(id)
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(|error| failed("the watermark would not come back", error))?;

            let seq = match held {
                Some(next) => next,
                None => {
                    let last: Option<i64> =
                        sqlx::query_scalar(self.q("SELECT MAX(seq) FROM entries WHERE box_id = ?"))
                            .bind(id)
                            .fetch_one(&mut *transaction)
                            .await
                            .map_err(|error| {
                                failed("the last sequence would not come back", error)
                            })?;

                    last.map_or(0, |last| last + 1)
                }
            };
            let entry = TraceEntry {
                seq: seq as u64,
                at_ms: now_ms(),
                actor,
                event: event.clone(),
                frame: frame.clone(),
            };

            let body = serde_json::to_string(&entry).map_err(|error| {
                Error::Internal(format!("an entry would not serialise: {error}"))
            })?;

            let put =
                sqlx::query(self.q(
                    "INSERT INTO entries (box_id, seq, at_ms, kind, entry) VALUES (?, ?, ?, ?, ?)",
                ))
                .bind(id)
                .bind(seq)
                .bind(entry.at_ms as i64)
                .bind(&kind)
                .bind(body)
                .execute(&mut *transaction)
                .await;

            match put {
                Ok(_) => {
                    sqlx::query(self.q("INSERT INTO sequences (box_id, next) VALUES (?, ?)
                         ON CONFLICT (box_id) DO UPDATE SET next = excluded.next"))
                    .bind(id)
                    .bind(seq + 1)
                    .execute(&mut *transaction)
                    .await
                    .map_err(|error| failed("the watermark would not move", error))?;

                    transaction
                        .commit()
                        .await
                        .map_err(|error| failed("an entry would not commit", error))?;

                    return Ok(entry.seq);
                }
                Err(error) if taken_already(&error) => continue,
                Err(error) => return Err(failed("an entry would not go down", error)),
            }
        }

        Err(Error::Unavailable(format!(
            "gave up assigning a sequence for {id} after {ATTEMPTS} tries"
        )))
    }

    async fn entries(&self, id: &str, after: Option<u64>, limit: usize) -> Result<Vec<TraceEntry>> {
        let above = after.map_or(-1, |seq| seq as i64);

        let rows: Vec<String> = sqlx::query_scalar(
            self.q("SELECT entry FROM entries WHERE box_id = ? AND seq > ? ORDER BY seq LIMIT ?"),
        )
        .bind(id)
        .bind(above)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| failed("a trace would not come back", error))?;

        rows.iter()
            .map(|body| {
                serde_json::from_str(body)
                    .map_err(|error| Error::Corrupt(format!("an entry does not parse: {error}")))
            })
            .collect()
    }
    async fn frames_before(&self, id: &str, before_ms: u64) -> Result<Vec<String>> {
        let rows: Vec<String> =
            sqlx::query_scalar(self.q("SELECT entry FROM entries WHERE box_id = ? AND at_ms < ?"))
                .bind(id)
                .bind(before_ms as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|error| failed("the old entries would not come back", error))?;

        let mut named = Vec::new();
        for body in &rows {
            let entry: TraceEntry = serde_json::from_str(body)
                .map_err(|error| Error::Corrupt(format!("an entry does not parse: {error}")))?;

            if let Some(hash) = entry.frame {
                named.push(hash);
            }
        }

        Ok(named)
    }

    async fn prune_entries(&self, id: &str, before_ms: u64) -> Result<u64> {
        let gone = sqlx::query(self.q("DELETE FROM entries WHERE box_id = ? AND at_ms < ?"))
            .bind(id)
            .bind(before_ms as i64)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("the old entries would not go", error))?;

        Ok(gone.rows_affected())
    }
}

#[async_trait]
impl Frames for Sql {
    async fn put(&self, id: &str, hash: &str, png: &[u8]) -> Result<()> {
        sqlx::query(
            self.q("INSERT INTO frames (box_id, hash, png) VALUES (?, ?, ?)
             ON CONFLICT (box_id, hash) DO NOTHING"),
        )
        .bind(id)
        .bind(hash)
        .bind(png.to_vec())
        .execute(&self.pool)
        .await
        .map_err(|error| failed("a frame would not go down", error))?;

        Ok(())
    }

    async fn get(&self, id: &str, hash: &str) -> Result<Option<Arc<Vec<u8>>>> {
        let row = sqlx::query(self.q("SELECT png FROM frames WHERE box_id = ? AND hash = ?"))
            .bind(id)
            .bind(hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| failed("a frame would not come back", error))?;

        row.map(|row| {
            row.try_get::<Vec<u8>, _>(0)
                .map(Arc::new)
                .map_err(|error| Error::Corrupt(format!("a frame does not read back: {error}")))
        })
        .transpose()
    }

    async fn drop_frames(&self, id: &str, hashes: &[String]) -> Result<()> {
        for hash in hashes {
            sqlx::query(self.q("DELETE FROM frames WHERE box_id = ? AND hash = ?"))
                .bind(id)
                .bind(hash)
                .execute(&self.pool)
                .await
                .map_err(|error| failed("a frame would not go", error))?;
        }

        Ok(())
    }

    async fn forget(&self, id: &str) -> Result<()> {
        sqlx::query(self.q("DELETE FROM frames WHERE box_id = ?"))
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|error| failed("frames would not be forgotten", error))?;

        Ok(())
    }
}

fn kind_of(event: &TraceEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| value.get("kind")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn taken_already(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(inner) if DatabaseError::is_unique_violation(&**inner))
}

fn failed(what: &str, error: sqlx::Error) -> Error {
    Error::Unavailable(format!("{what}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);

            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default();
            let mine = NEXT.fetch_add(1, Ordering::Relaxed);

            Self(std::env::temp_dir().join(format!("holm-storage-{nanos}-{mine}.db")))
        }

        fn url(&self) -> String {
            format!("sqlite:{}?mode=rwc", self.0.display())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).ok();
        }
    }

    fn postgres() -> Option<String> {
        std::env::var("HOLM_STORAGE_POSTGRES_URL")
            .ok()
            .filter(|url| !url.is_empty())
    }

    #[test]
    fn test_a_url_is_shown_without_its_password() {
        assert_eq!(
            shown("postgresql://holm.user:p@ss:w0rd@db.example.com:5432/postgres?options=x"),
            "postgresql://holm.user:***@db.example.com:5432/postgres?options=x"
        );
        assert_eq!(
            shown("postgres://holm@db.example.com/db"),
            "postgres://holm@db.example.com/db"
        );
        assert_eq!(shown("sqlite:/tmp/holm.db"), "sqlite:/tmp/holm.db");
    }

    #[tokio::test]
    async fn test_a_store_that_will_not_open_does_not_say_its_password() {
        let Err(why) = Sql::open("mysql://holm:secret-word@db.example.com/db").await else {
            panic!("this store speaks no MySQL");
        };

        assert!(!why.to_string().contains("secret-word"), "{why}");
    }

    #[tokio::test]
    async fn test_sqlite_locks_one_name_for_one_holder() {
        let scratch = Scratch::new();
        let store = Sql::open(&scratch.url()).await.expect("opened");
        conformance::locks(Arc::new(store)).await;
    }

    #[tokio::test]
    async fn test_a_lease_holds_a_name_across_two_processes_on_one_database() {
        let scratch = Scratch::new();
        let one = Sql::open(&scratch.url()).await.expect("opened").leasing();
        let two = Sql::open(&scratch.url()).await.expect("opened").leasing();

        let held = one.lock("box_1/take").await.expect("locked");
        assert!(
            !two.claim("box_1/take", "another").await.expect("asked"),
            "another process does not get a name that is held"
        );
        assert!(
            two.claim("box_2/take", "another").await.expect("asked"),
            "but it gets another name"
        );

        drop(held);
        let waited =
            tokio::time::timeout(std::time::Duration::from_secs(5), two.lock("box_1/take"))
                .await
                .expect("the name is free once the holder lets go");
        assert!(waited.is_ok());
    }

    #[tokio::test]
    async fn test_sqlite_behaves_like_a_store() {
        let scratch = Scratch::new();
        let store = Sql::open(&scratch.url()).await.expect("opened");

        conformance::store(&store).await;
        conformance::runtimes(&store).await;
        conformance::images(&store).await;
        conformance::notes(&store).await;
        conformance::jobs(&store).await;
        conformance::events(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_behaves_like_a_frame_store() {
        let scratch = Scratch::new();
        let store = Sql::open(&scratch.url()).await.expect("opened");

        conformance::frames(&store).await;
    }

    #[tokio::test]
    async fn test_sqlite_prunes() {
        let scratch = Scratch::new();
        let store = Sql::open(&scratch.url()).await.expect("opened");

        conformance::pruning(&store).await;
        conformance::dropping(&store).await;
    }

    #[tokio::test]
    async fn test_opening_twice_leaves_what_was_there() {
        let scratch = Scratch::new();

        {
            let store = Sql::open(&scratch.url()).await.expect("opened");
            store
                .append("box_1", Actor::Agent, TraceEvent::BoxDeleted, None)
                .await
                .expect("appended");
        }

        let reopened = Sql::open(&scratch.url()).await.expect("opened again");
        let seq = reopened
            .append("box_1", Actor::Agent, TraceEvent::BoxDeleted, None)
            .await
            .expect("appended");

        assert_eq!(
            seq, 1,
            "the sequence comes from the table, so a second process carries on \
             rather than writing over the first"
        );
        assert_eq!(
            reopened
                .entries("box_1", None, 10)
                .await
                .expect("read back")
                .len(),
            2,
            "and the schema went on again without losing what it held"
        );
    }

    #[tokio::test]
    async fn test_what_happened_is_its_own_column() {
        let scratch = Scratch::new();
        let store = Sql::open(&scratch.url()).await.expect("opened");

        store
            .append("box_1", Actor::Person, TraceEvent::BoxDeleted, None)
            .await
            .expect("appended");

        let kinds: Vec<String> =
            sqlx::query_scalar(AssertSqlSafe("SELECT kind FROM entries".to_string()))
                .fetch_all(&store.pool)
                .await
                .expect("asked");

        assert_eq!(
            kinds,
            vec!["box_deleted".to_string()],
            "an operator asking for takeovers should not have to parse every row"
        );
    }

    #[test]
    fn test_postgres_counts_its_placeholders() {
        let written = "INSERT INTO boxes (id, record) VALUES (?, ?) WHERE id = ?";

        assert_eq!(
            numbered(Dialect::Sqlite, written),
            written,
            "SQLite is written as it runs"
        );
        assert_eq!(
            numbered(Dialect::Postgres, written),
            "INSERT INTO boxes (id, record) VALUES ($1, $2) WHERE id = $3"
        );
    }

    #[tokio::test]
    async fn test_a_url_naming_no_dialect_is_refused_before_it_connects() {
        assert!(Sql::open("mysql://nowhere/db").await.is_err());
    }

    #[tokio::test]
    async fn test_postgres_behaves_like_a_store() {
        let Some(url) = postgres() else {
            return;
        };

        let store = Sql::open(&url).await.expect("opened");
        store.forget_box("box_1").await.expect("a clean start");
        store.forget_box("box_2").await.expect("a clean start");
        store
            .forget_box("never_stored")
            .await
            .expect("a clean start");
        Frames::forget(&store, "box_1")
            .await
            .expect("a clean start");
        Frames::forget(&store, "box_2")
            .await
            .expect("a clean start");

        conformance::store(&store).await;
        conformance::runtimes(&store).await;
        conformance::images(&store).await;
        conformance::notes(&store).await;
        conformance::jobs(&store).await;
        conformance::events(&store).await;
        conformance::frames(&store).await;
    }
}
