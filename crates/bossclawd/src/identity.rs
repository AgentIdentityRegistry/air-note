//! Daemon-local onboarding check.
//!
//! The app's authority for onboarding is `IdentityStore::is_onboarded()` =
//! "`<data_dir>/identity.json` exists AND parses" (see
//! `apps/desktop/src-tauri/src/air/identity.rs:77-98`, `METADATA_FILE = "identity.json"`).
//! The daemon's scheduler + boot reseed compute onboarded THE SAME WAY over the SAME
//! data_dir. Per-request ops do NOT use this — they trust the `onboarded: bool` each
//! `Request` carries (the app stays the authority for its own UI flow, exactly like the
//! `EngineHandle` methods that take `onboarded` as a parameter today).

use serde::Deserialize;
use std::path::Path;

/// The file the app writes its public identity metadata to (mirrors
/// `IdentityStore::METADATA_FILE`). Kept in sync with the app by convention (same value).
const METADATA_FILE: &str = "identity.json";

/// A minimal mirror of the app's `IdentityMetadata` — only the fields that MUST be present
/// for `load_metadata` to succeed. `serde` rejects the file if `did`/`name`/`created_at`
/// are missing, matching the app's parse: a partial/garbage file is "not onboarded".
/// (`username` is `#[serde(default)]` in the app, so it is not required here.)
#[derive(Deserialize)]
struct IdentityMetadataProbe {
    /// Kept `serde_json::Value` (NOT `String`) deliberately: [`is_onboarded`]'s verdict must stay
    /// byte-for-byte the app's — "the file parses" — and narrowing this to `String` would newly
    /// report a non-string `did` as NOT onboarded. [`owner_did`] does the string narrowing instead,
    /// where a `None` is harmless.
    did: serde_json::Value,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    created_at: String,
}

/// `true` iff `<data_dir>/identity.json` exists and parses as identity metadata — the
/// SAME test the app's `IdentityStore::is_onboarded()` applies. Any read/parse failure
/// (absent, unreadable, malformed) is "not onboarded" (fail-safe: a daemon that can't
/// confirm onboarding never advances the scheduler).
pub fn is_onboarded(data_dir: &Path) -> bool {
    let path = data_dir.join(METADATA_FILE);
    let Ok(json) = std::fs::read_to_string(&path) else {
        return false;
    };
    serde_json::from_str::<IdentityMetadataProbe>(&json).is_ok()
}

/// The owner's AIR did from `<data_dir>/identity.json`, or `None` if it cannot be read as a string
/// (absent, unreadable, malformed, or a non-string `did`). The app's `Did` is a newtype over `String`
/// (`air/types.rs:4`), so it always serializes as a plain JSON string.
///
/// Added for the Rung-5 `SetBinding` did check: an identity binding naming a FOREIGN did is copied
/// straight into `manifest.did` by the export, so every bundle would be attributed to that did.
/// `None` is deliberately NOT a refusal at the call site — see the note there.
pub fn owner_did(data_dir: &Path) -> Option<String> {
    let json = std::fs::read_to_string(data_dir.join(METADATA_FILE)).ok()?;
    let probe: IdentityMetadataProbe = serde_json::from_str(&json).ok()?;
    probe.did.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_file_is_not_onboarded() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_onboarded(dir.path()));
    }

    #[test]
    fn valid_metadata_is_onboarded() {
        let dir = tempfile::tempdir().unwrap();
        // Mirror what the app writes (did/name/created_at present; username omitted is fine).
        let meta = serde_json::json!({
            "did": "did:wba:example.com:alice",
            "name": "Alice",
            "created_at": "2026-07-02T00:00:00+00:00"
        });
        std::fs::write(dir.path().join(METADATA_FILE), meta.to_string()).unwrap();
        assert!(is_onboarded(dir.path()));
    }

    #[test]
    fn malformed_metadata_is_not_onboarded() {
        let dir = tempfile::tempdir().unwrap();
        // Missing required fields (`name`/`created_at`) → parse fails → not onboarded.
        std::fs::write(dir.path().join(METADATA_FILE), r#"{"did":"x"}"#).unwrap();
        assert!(!is_onboarded(dir.path()));
        // Non-JSON garbage → not onboarded.
        std::fs::write(dir.path().join(METADATA_FILE), "not json").unwrap();
        assert!(!is_onboarded(dir.path()));
    }
}
