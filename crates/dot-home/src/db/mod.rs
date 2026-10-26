//! SQLite, one connection, every query on the blocking pool.
//!
//! A home server sees a handful of requests per second, so one serialized connection in
//! WAL mode is simpler and more predictable than a pool, and it makes multi-statement
//! changes trivially atomic.
use rusqlite::{Connection, OptionalExtension};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

const MIGRATIONS: &[&str] = &[
    include_str!("schema.sql"),
    include_str!("migrate2.sql"),
    include_str!("migrate3.sql"),
    include_str!("migrate4.sql"),
    include_str!("migrate5.sql"),
    include_str!("migrate6.sql"),
    include_str!("migrate7.sql"),
    include_str!("migrate8.sql"),
    include_str!("migrate9.sql"),
];

/// Shared handle to the database.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

/// A database failure. Details go to the log, never to clients.
#[derive(Debug, thiserror::Error)]
#[error("database: {0}")]
pub struct DbError(#[from] pub rusqlite::Error);

impl Db {
    /// Open (creating if needed) and migrate.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// In-memory database for tests.
    pub fn memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> anyhow::Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let current = i64::try_from(MIGRATIONS.len())?;
        anyhow::ensure!(
            version <= current,
            "the database is from a newer Familiar (schema {version}); upgrade the server"
        );
        for (index, sql) in MIGRATIONS
            .iter()
            .enumerate()
            .skip(usize::try_from(version)?)
        {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", index + 1)?;
            tx.commit()?;
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run `f` with the connection on the blocking pool.
    pub async fn call<T, F>(&self, f: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            // A panic inside a query poisons the lock; the connection itself is still fine.
            let mut guard = conn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            f(&mut guard)
        })
        .await
        .map_err(|_| DbError(rusqlite::Error::InvalidQuery))?
        .map_err(DbError)
    }

    /// Read a setting.
    pub async fn setting(&self, key: &'static str) -> Result<Option<String>, DbError> {
        self.call(move |c| {
            c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
        })
        .await
    }

    /// Write a setting.
    pub async fn set_setting(&self, key: &'static str, value: String) -> Result<(), DbError> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
            .map(|_| ())
        })
        .await
    }

    /// Consistent online copy (`VACUUM INTO`).
    pub async fn backup(&self, to: std::path::PathBuf) -> Result<(), DbError> {
        self.call(move |c| {
            c.execute("VACUUM INTO ?1", [to.to_string_lossy().as_ref()])
                .map(|_| ())
        })
        .await
    }
}

/// Current unix time in seconds.
pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// Unix seconds to RFC 3339 (UTC).
pub fn rfc3339(seconds: i64) -> String {
    jiff::Timestamp::from_second(seconds)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// Optional unix seconds to RFC 3339.
pub fn rfc3339_opt(seconds: Option<i64>) -> Option<String> {
    seconds.map(rfc3339)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrates_once_and_settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = Db::open(&path).unwrap();
        db.set_setting("require_totp", "true".into()).await.unwrap();
        db.set_setting("require_totp", "false".into())
            .await
            .unwrap();
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.setting("require_totp").await.unwrap().as_deref(),
            Some("false")
        );
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    }
}
