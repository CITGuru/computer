//! Rows in a database, in whichever of two dialects the URL names.
//!
//! One implementation rather than two. What differs between SQLite and
//! Postgres is small and named here: the placeholder, and the type a blob goes
//! in. Everything else — the upsert, the window a read takes, the primary key
//! that makes a sequence unique — both have spoken since 2016.
//!
//! # Why a database at all
//!
//! [`crate::files`] already keeps a record across a restart, and does it
//! without a driver or a migration. What it cannot do is answer a question
//! nobody wrote a key for. So the columns here are the ones an operator starts
//! from — which box, when, what kind of thing happened — and the row itself is
//! held as JSON beside them, which is what lets a field be added later with no
//! migration behind it.
//!
//! # More than one writer
//!
//! This is the backend that allows it. A file store reads its next sequence
//! from the keys and holds it, which is sound only while one process writes a
//! box. Here the sequence is taken inside a transaction and the primary key
//! refuses a repeat, so a second writer retries rather than overwrites.

use crate::{BoxRecord, Error, Frames, Result, Store, now_ms};
use async_trait::async_trait;
use computer_api::{Actor, TraceEntry, TraceEvent};
use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, AssertSqlSafe, Row, error::DatabaseError};
use std::sync::Arc;

/// How many times a racing writer re-reads the sequence before giving up.
///
/// Each loss costs one round trip, and losing five in a row means far more
/// writers on one box than a desktop has.
const ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    Sqlite,
    Postgres,
}

pub struct Sql {
    pool: AnyPool,
    dialect: Dialect,
}

impl Sql {
    /// Connects, and makes the tables if they are not there.
    ///
    /// `sqlite://…` or `postgres://…`. The scheme picks the dialect, because
    /// the driver does not report one back.
    pub async fn open(url: &str) -> Result<Self> {
        sqlx::any::install_default_drivers();

        let dialect = match url {
            _ if url.starts_with("sqlite:") => Dialect::Sqlite,
            _ if url.starts_with("postgres:") || url.starts_with("postgresql:") => {
                Dialect::Postgres
            }
            _ => {
                return Err(Error::Unavailable(format!(
                    "{url} names no dialect this store has: use sqlite:// or postgres://"
                )));
            }
        };

        let pool = AnyPoolOptions::new()
            .connect(url)
            .await
            .map_err(|error| Error::Unavailable(format!("{url}: {error}")))?;

        let store = Self { pool, dialect };
        store.migrate().await?;

        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        let blob = match self.dialect {
            Dialect::Sqlite => "BLOB",
            Dialect::Postgres => "BYTEA",
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
            // Every read of a trace is one box in sequence order, and every
            // sweep of frames is one box. Without these both are a scan.
            // The watermark, which outlives the rows it counted. Pruning a
            // trace empty must not hand a sequence out twice: a caller polling
            // from where it got to would read the repeat as loss.
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

    /// A statement in this dialect.
    ///
    /// Asserted safe because every one of them is a literal in this file with
    /// no caller's bytes in it: what varies is the placeholder, and values
    /// arrive bound.
    fn q(&self, written: &str) -> AssertSqlSafe<String> {
        AssertSqlSafe(numbered(self.dialect, written))
    }
}

/// These statements are written with `?`; Postgres counts instead.
///
/// Safe only because none of them holds a literal `?` inside a string.
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
            // Forgetting is not pruning: the box goes whole, so its watermark
            // has nothing left to protect.
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

            // From the watermark, falling back to the rows only for a database
            // written before there was one.
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
                // Somebody else took this sequence between the read and the
                // write. Read it again rather than overwrite theirs.
                Err(error) if taken_already(&error) => continue,
                Err(error) => return Err(failed("an entry would not go down", error)),
            }
        }

        Err(Error::Unavailable(format!(
            "gave up assigning a sequence for {id} after {ATTEMPTS} tries"
        )))
    }

    async fn entries(&self, id: &str, after: Option<u64>, limit: usize) -> Result<Vec<TraceEntry>> {
        // Sequences start at zero, so "everything" is everything above -1.
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
        // The rows rather than a column of their own: a frame hash is read on a
        // sweep and never on a request, so it does not earn one.
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
        // One statement each rather than an `IN` list, whose placeholder count
        // varies and would have to be built per call in two dialects.
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

/// The tag the event serialises under, lifted into its own column so a reader
/// can ask for takeovers without parsing every row.
fn kind_of(event: &TraceEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| value.get("kind")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn taken_already(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(inner) if DatabaseError::is_unique_violation(&**inner))
}

/// A database that will not answer is `Unavailable`: waiting is the only thing
/// a caller can do about it, and it is what a route turns into a 503.
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

    /// A database file of its own per test, taken away afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);

            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default();
            let mine = NEXT.fetch_add(1, Ordering::Relaxed);

            Self(std::env::temp_dir().join(format!("computer-storage-{nanos}-{mine}.db")))
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

    /// A Postgres to run against, where one is offered. Skipped rather than
    /// failed: a checkout with no database is the ordinary case.
    fn postgres() -> Option<String> {
        std::env::var("COMPUTER_STORAGE_POSTGRES_URL")
            .ok()
            .filter(|url| !url.is_empty())
    }

    #[tokio::test]
    async fn test_sqlite_behaves_like_a_store() {
        let scratch = Scratch::new();
        let store = Sql::open(&scratch.url()).await.expect("opened");

        conformance::store(&store).await;
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
        conformance::frames(&store).await;
    }
}
