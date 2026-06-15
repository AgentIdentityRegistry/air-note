//! Signed high-water-mark: detects tail-truncation/rollback that a plain hash
//! chain cannot (a deleted tail still links cleanly). The desktop wires a
//! keychain-backed impl at M7; the crate ships a file-backed impl for tests and
//! headless use.
//!
//! Write discipline (spec §5.2): the event is appended FIRST, the watermark is
//! updated SECOND and debounced (callers decide cadence via `checkpoint`). On
//! open, `live_count < mark.count` is truncation; `live_count >= mark.count`
//! with a valid chain is benign catch-up.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::BossclawError;

/// The persisted mark.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mark {
    /// Event count at checkpoint time.
    pub count: i64,
    /// Tip event hash (hex) at checkpoint time.
    pub tip_hash: String,
}

/// A place to persist the signed high-water mark.
///
/// `Send + Sync` is required because `EventLog` (which holds a boxed
/// `HighWaterStore`) is shared as `Arc<EventLog>` across threads.
pub trait HighWaterStore: Send + Sync {
    /// Load the last mark, or `None` if never written.
    fn load(&self) -> Result<Option<Mark>, BossclawError>;
    /// Persist a new mark (overwrites).
    fn save(&self, mark: &Mark) -> Result<(), BossclawError>;
}

/// File-backed high-water store (JSON). For tests + headless use.
pub struct FileHighWater {
    path: PathBuf,
}

impl FileHighWater {
    /// Create a file-backed store at `path`.
    pub fn new(path: &Path) -> Self {
        Self { path: path.to_path_buf() }
    }
}

impl HighWaterStore for FileHighWater {
    fn load(&self) -> Result<Option<Mark>, BossclawError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, mark: &Mark) -> Result<(), BossclawError> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(mark)?)?;
        std::fs::rename(&tmp, &self.path)?; // atomic
        Ok(())
    }
}
