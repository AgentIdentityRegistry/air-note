use super::types::Did;
use crate::secrets::SecretsVault;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityMetadata {
    pub did: Did,
    pub name: String,
    /// The published unique `@handle` (lowercased, charset-validated), once claimed.
    /// `#[serde(default)]` so identity.json written before Milestone G still loads.
    #[serde(default)]
    pub username: Option<String>,
    pub created_at: String,
}

/// Storage layout:
///   - Private key bytes → SecretsVault under key "air-agent.agent.signing_key"
///   - Agent secret (returned by AIR) → SecretsVault under "air-agent.agent.air_secret"
///   - Public metadata → JSON at <app_data_dir>/identity.json
///
/// `Clone` is cheap (both fields are `Arc`/`PathBuf`); the SP3 evolve scheduler holds a
/// clone for its onboarding read across the spawn boundary.
#[derive(Clone)]
pub struct IdentityStore {
    vault: Arc<dyn SecretsVault>,
    data_dir: PathBuf,
}

impl IdentityStore {
    pub fn new(vault: Arc<dyn SecretsVault>, data_dir: PathBuf) -> Self {
        Self { vault, data_dir }
    }

    const SIGNING_KEY: &'static str = "air-agent.agent.signing_key";
    const AIR_SECRET: &'static str = "air-agent.agent.air_secret";
    const METADATA_FILE: &'static str = "identity.json";

    pub fn save_signing_key(&self, bytes: &[u8; 32]) -> Result<(), String> {
        self.vault.set(Self::SIGNING_KEY, &hex::encode(bytes))
    }

    /// Used in Phase 3 (attestation signing): loads the private key from SecretsVault
    /// so AgentKeypair::from_secret_bytes can re-hydrate it on app start.
    pub fn load_signing_key(&self) -> Result<Option<[u8; 32]>, String> {
        match self.vault.get(Self::SIGNING_KEY)? {
            Some(hex_str) => {
                let bytes = hex::decode(&hex_str).map_err(|e| e.to_string())?;
                if bytes.len() != 32 {
                    return Err(format!("expected 32 bytes, got {}", bytes.len()));
                }
                let mut out = [0u8; 32];
                out.copy_from_slice(&bytes);
                Ok(Some(out))
            }
            None => Ok(None),
        }
    }

    pub fn save_air_secret(&self, secret: &str) -> Result<(), String> {
        self.vault.set(Self::AIR_SECRET, secret)
    }

    /// Used in Phase 3: loads the AIR per-agent secret so update() calls can authenticate.
    pub fn load_air_secret(&self) -> Result<Option<String>, String> {
        self.vault.get(Self::AIR_SECRET)
    }

    pub fn save_metadata(&self, meta: &IdentityMetadata) -> Result<(), String> {
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        let path = self.data_dir.join(Self::METADATA_FILE);
        let json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn load_metadata(&self) -> Result<Option<IdentityMetadata>, String> {
        let path = self.data_dir.join(Self::METADATA_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let mut meta: IdentityMetadata = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        // Tolerate-and-rewrite (see `repair_legacy_created_at`): repair in place so the bad
        // value stops propagating to every later reader. The rewrite is BEST-EFFORT — a
        // read-only data dir must not turn "load your identity" into a hard failure, and the
        // repaired value is returned either way.
        if let Some(repaired) = repair_legacy_created_at(&meta.created_at) {
            meta.created_at = repaired;
            let _ = self.save_metadata(&meta);
        }
        Ok(Some(meta))
    }

    pub fn clear(&self) -> Result<(), String> {
        self.vault.delete(Self::SIGNING_KEY)?;
        self.vault.delete(Self::AIR_SECRET)?;
        let path = self.data_dir.join(Self::METADATA_FILE);
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn is_onboarded(&self) -> bool {
        self.load_metadata().map(|m| m.is_some()).unwrap_or(false)
    }

    /// The app data dir this store persists identity under. Reused by the language-pack downloader
    /// (rung 2, B2) to resolve `<data_dir>/models/` — the daemon's default model-resolution root —
    /// so the app can install a downloaded pack where the daemon's own pull-resolution finds it.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

/// Repair the legacy `created_at` shape, or `None` if `raw` is not that shape.
///
/// An early placeholder in `MockAirClient::now_iso` formatted the Unix epoch into the
/// SECONDS field of a fixed 1970 date — `1970-01-01T00:00:1782190630Z` — producing a string
/// that parses as neither a timestamp nor an integer. Because the real epoch is embedded in
/// it, the true instant is RECOVERABLE, so repair is lossless and no DID is ever reissued.
///
/// The discriminator is exact: a genuine RFC3339 timestamp can never carry 60 or more in its
/// seconds field, so `1970-01-01T00:00:00Z` through `:59Z` are real instants and are left
/// alone. Anything not positively recognized returns `None` — a well-formed value is never
/// rewritten, and an unrecognized one is never silently replaced with "now".
fn repair_legacy_created_at(raw: &str) -> Option<String> {
    /// Genuine seconds fields are `00`..=`59`; at or above this, the field is the smuggled epoch.
    const MIN_SMUGGLED_EPOCH: i64 = 60;
    let seconds_field = raw.strip_prefix("1970-01-01T00:00:")?.strip_suffix('Z')?;
    let epoch: i64 = seconds_field.parse().ok()?;
    if epoch < MIN_SMUGGLED_EPOCH {
        return None;
    }
    chrono::DateTime::from_timestamp(epoch, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// One-time rename migration: copy the agent's identity secrets from the legacy
/// keychain service into the renamed one. Idempotent (new wins); best-effort delete of old.
pub fn migrate_identity_keys(old: &dyn SecretsVault, new: &dyn SecretsVault) -> Result<(), String> {
    const PAIRS: [(&str, &str); 2] = [
        ("bossclaw.agent.signing_key", IdentityStore::SIGNING_KEY),
        ("bossclaw.agent.air_secret", IdentityStore::AIR_SECRET),
    ];
    for (old_key, new_key) in PAIRS {
        if new.get(new_key)?.is_some() {
            continue;
        }
        if let Some(value) = old.get(old_key)? {
            new.set(new_key, &value)?;
            let _ = old.delete(old_key);
        }
    }
    Ok(())
}

/// One-time rename migration: copy `identity.json` from the legacy bundle-id data
/// dir to the renamed one (the dir name changes with the Tauri identifier). Never overwrites.
pub fn migrate_identity_metadata(new_data_dir: &Path, legacy_identifier: &str) -> Result<(), String> {
    let new_file = new_data_dir.join(IdentityStore::METADATA_FILE);
    if new_file.exists() {
        return Ok(());
    }
    let Some(base) = new_data_dir.parent() else {
        return Ok(());
    };
    let old_file = base.join(legacy_identifier).join(IdentityStore::METADATA_FILE);
    if !old_file.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(new_data_dir).map_err(|e| e.to_string())?;
    std::fs::copy(&old_file, &new_file).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod migrate_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MockVault {
        store: Mutex<HashMap<String, String>>,
    }
    impl MockVault {
        fn new() -> Self {
            Self { store: Mutex::new(HashMap::new()) }
        }
    }
    impl SecretsVault for MockVault {
        fn set(&self, k: &str, v: &str) -> Result<(), String> {
            self.store.lock().unwrap().insert(k.to_string(), v.to_string());
            Ok(())
        }
        fn get(&self, k: &str) -> Result<Option<String>, String> {
            Ok(self.store.lock().unwrap().get(k).cloned())
        }
        fn delete(&self, k: &str) -> Result<(), String> {
            self.store.lock().unwrap().remove(k);
            Ok(())
        }
    }

    #[test]
    fn migrates_then_idempotent() {
        let old = MockVault::new();
        let new = MockVault::new();
        old.set("bossclaw.agent.signing_key", "deadbeef").unwrap();
        old.set("bossclaw.agent.air_secret", "s3cr3t").unwrap();

        migrate_identity_keys(&old, &new).unwrap();
        assert_eq!(new.get(IdentityStore::SIGNING_KEY).unwrap(), Some("deadbeef".to_string()));
        assert_eq!(new.get(IdentityStore::AIR_SECRET).unwrap(), Some("s3cr3t".to_string()));
        assert_eq!(old.get("bossclaw.agent.signing_key").unwrap(), None);

        migrate_identity_keys(&old, &new).unwrap();
        assert_eq!(new.get(IdentityStore::SIGNING_KEY).unwrap(), Some("deadbeef".to_string()));
    }

    #[test]
    fn new_wins_over_old() {
        let old = MockVault::new();
        let new = MockVault::new();
        old.set("bossclaw.agent.signing_key", "OLD").unwrap();
        new.set(IdentityStore::SIGNING_KEY, "NEW").unwrap();
        migrate_identity_keys(&old, &new).unwrap();
        assert_eq!(new.get(IdentityStore::SIGNING_KEY).unwrap(), Some("NEW".to_string()));
    }

    #[test]
    fn copies_identity_json_to_renamed_dir() {
        let base = std::env::temp_dir().join("air_agent_meta_migration_test");
        let _ = std::fs::remove_dir_all(&base);
        let old_id = "ai.bossclaw.desktop";
        let old_dir = base.join(old_id);
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("identity.json"), b"{\"did\":\"x\"}").unwrap();
        let new_dir = base.join("ai.air-agent.desktop");

        migrate_identity_metadata(&new_dir, old_id).unwrap();
        assert!(new_dir.join("identity.json").exists());

        std::fs::write(new_dir.join("identity.json"), b"{\"did\":\"NEW\"}").unwrap();
        migrate_identity_metadata(&new_dir, old_id).unwrap();
        assert_eq!(std::fs::read(new_dir.join("identity.json")).unwrap(), b"{\"did\":\"NEW\"}".to_vec());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The legacy value is not merely malformed — the true instant is RECOVERABLE from it,
    /// because the placeholder embedded the real Unix epoch in the seconds field. Repair is
    /// therefore lossless, and no DID needs to be reissued.
    ///
    /// 1782190630 is the epoch from Peter's own identity.json; it is 2026-06-23T04:57:10Z,
    /// which matches that file's mtime exactly.
    #[test]
    fn legacy_created_at_is_repaired_losslessly() {
        assert_eq!(
            repair_legacy_created_at("1970-01-01T00:00:1782190630Z").as_deref(),
            Some("2026-06-23T04:57:10Z")
        );
    }

    /// Repair must be inert on anything it does not positively recognize, so a good value is
    /// never rewritten and an unknown shape is never silently replaced with "now".
    #[test]
    fn only_the_known_legacy_shape_is_repaired() {
        for untouched in [
            "2026-05-18T12:00:00Z",            // already correct
            "1970-01-01T00:00:00Z",            // a genuine epoch-zero instant, seconds in range
            "1970-01-01T00:00:notanumber Z",   // not the shape
            "",
        ] {
            assert_eq!(repair_legacy_created_at(untouched), None, "must not rewrite {untouched:?}");
        }
    }

    /// Tolerate-and-rewrite: a brain already carrying the legacy value is repaired on read AND
    /// persisted, so the bad string stops propagating — while did/name/username are preserved
    /// byte-for-byte. Reissuing the DID would be catastrophic and is never acceptable here.
    #[test]
    fn load_metadata_repairs_a_legacy_file_in_place_and_preserves_identity() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStore::new(std::sync::Arc::new(MockVault::new()), dir.path().to_path_buf());
        std::fs::write(
            dir.path().join("identity.json"),
            br#"{"did":"did:wba:bossclaw.ai:dc16ac8924c30ba0","name":"Peter's Forever Agent","created_at":"1970-01-01T00:00:1782190630Z"}"#,
        )
        .unwrap();

        let loaded = store.load_metadata().unwrap().expect("metadata loads");
        assert_eq!(loaded.created_at, "2026-06-23T04:57:10Z");
        assert_eq!(loaded.did.0, "did:wba:bossclaw.ai:dc16ac8924c30ba0", "the DID must never change");
        assert_eq!(loaded.name, "Peter's Forever Agent");

        let on_disk = std::fs::read_to_string(dir.path().join("identity.json")).unwrap();
        assert!(on_disk.contains("2026-06-23T04:57:10Z"), "repair must persist, got {on_disk}");
        assert!(!on_disk.contains("1970-01-01T00:00:1782190630Z"), "legacy value must be gone");
        assert!(on_disk.contains("dc16ac8924c30ba0"), "the DID must survive the rewrite");
    }
}
