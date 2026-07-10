//! Whole-DB encrypted SQLite store (SQLCipher via rusqlite `bundled-sqlcipher`).
//! The DEK is supplied by the caller (desktop fetches it from the OS keychain);
//! the crate never reads the keychain itself.

use std::path::Path;

use rusqlite::Connection;
use zeroize::Zeroizing;

use crate::error::BossclawError;

/// An open, encrypted SQLite connection. Single-threaded by construction;
/// `EventLog` owns the serialization (see `log.rs`).
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) an encrypted DB at `path`, keyed by `dek`.
    /// Fails if the file exists and the key is wrong.
    pub fn open(path: &Path, dek: &[u8; 32]) -> Result<Self, BossclawError> {
        let conn = Connection::open(path)?;
        let key_hex = Zeroizing::new(hex::encode(dek));
        let pragma = Zeroizing::new(format!("PRAGMA key = \"x'{}'\"", *key_hex));
        conn.execute_batch(&pragma)?;
        // Force a read so a wrong key errors here (SQLCipher is lazy otherwise).
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .map_err(|_| BossclawError::Store("wrong key or corrupt db".into()))?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        Ok(Self { conn })
    }

    /// Execute a statement with no parameters (DDL / simple writes).
    pub fn exec(&self, sql: &str) -> Result<(), BossclawError> {
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    /// Query a single `String` column from the first row.
    pub fn query_one(&self, sql: &str) -> Result<String, BossclawError> {
        let v = self.conn.query_row(sql, [], |r| r.get::<_, String>(0))?;
        Ok(v)
    }

    /// Borrow the underlying connection (used by `EventLog`).
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }
}
