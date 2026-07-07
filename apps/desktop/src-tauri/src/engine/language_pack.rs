//! The multilingual language-pack downloader (rung 2, U3). Preflight disk → fetch the 3 files from
//! the pinned GitHub Release into a namespaced temp dir → per-file sha256 verify (fail-closed, rm on
//! mismatch) → atomic temp→rename into `<data_dir>/models/<id>/` → write the `air-model.json`
//! id-binding from the VERIFIED safetensors sha (invariant I4 "verify then name"). No `bossclaw-core`
//! dependency: this only prepares files on disk; the daemon enables + migrates via `SetActiveModel`.
//!
//! Verification split (keeps ALL three files fail-closed while honoring the unit-test contract): the
//! tiny, human-auditable, trust-on-first-use tokenizer/config pins are verified in
//! `download_and_install` right after each fetch (the `scripts/fetch-model.sh` pattern); the
//! security-critical weights are verified in `install_verified` against the exact sha that names the
//! id-binding, so I4 holds — nothing is renamed into place until the binding sha matches the bytes.

// Staging allowance: the `download_and_install` entrypoint (and the pipeline it drives) is wired to a
// Tauri command by Task B2; until then these fns are reached only from the unit tests below, so the
// non-test bin build sees them as unused. Mirrors the engine/client.rs Task-5-era staging pattern —
// remove when B2 lands.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The enabled multilingual model id (the folder name under `<data_dir>/models/`).
pub const MULTILINGUAL_MODEL_ID: &str = "minishlab/potion-multilingual-128M";
/// The local id-binding filename (read by the daemon's resolver).
const ID_BINDING_FILE: &str = "air-model.json";
/// The weights file — the real attack surface, verified against the sha that names the id-binding (I4).
const WEIGHTS_FILE: &str = "model.safetensors";
/// GitHub Release asset base (Ops task O1 uploaded these three assets under this tag).
const RELEASE_BASE: &str =
    "https://github.com/AgentIdentityRegistry/air-note/releases/download/models-multilingual-128M-v1";
/// Headroom for the ~506 MB download plus a transient copy during the atomic rename (~1.5 GB).
const REQUIRED_FREE_BYTES: u64 = 1_500_000_000;

/// One pinned pack file: its release asset name, expected sha256 (hex), and exact byte length. The
/// safetensors sha is cross-verified against the model2vec source; tokenizer/config are trust-on-
/// first-use pins (tiny + human-auditable), the same policy as `scripts/fetch-model.sh`.
struct PackFile {
    name: &'static str,
    sha256: &'static str,
    bytes: u64,
}

/// The three files + their pinned sha256 and sizes (GitHub Release `models-multilingual-128M-v1`).
const PACK_FILES: &[PackFile] = &[
    PackFile {
        name: WEIGHTS_FILE,
        sha256: "14b5eb39cb4ce5666da8ad1f3dc6be4346e9b2d601c073302fa0a31bf7943397",
        bytes: 512_361_560,
    },
    PackFile {
        name: "tokenizer.json",
        sha256: "19f1909063da3cfe3bd83a782381f040dccea475f4816de11116444a73e1b6a1",
        bytes: 18_616_131,
    },
    PackFile {
        name: "config.json",
        sha256: "595e4cab2093732efd5dbe084fd5c1826b5eea693b73b4c1fd971672867d2e54",
        bytes: 271,
    },
];

/// Progress callback: `(bytes_done, bytes_total)` over the whole pack download.
pub type ProgressFn<'a> = dyn FnMut(u64, u64) + Send + 'a;

/// Refuse early if the destination volume lacks `required` free bytes (fail before any network I/O).
/// Uses `fs2::available_space` on the nearest existing ancestor of `dest_root`.
pub fn preflight_disk(dest_root: &Path, required: u64) -> Result<(), String> {
    let probe = existing_ancestor(dest_root);
    let free = fs2::available_space(&probe).map_err(|e| format!("could not read free disk space: {e}"))?;
    if free < required {
        return Err(format!(
            "not enough disk space (need ~{:.1} GB free, have {:.1} GB)",
            required as f64 / 1e9,
            free as f64 / 1e9
        ));
    }
    Ok(())
}

/// sha256 a file and compare (hex) to `expected`. `Err` (fail-closed) on any mismatch or read error.
pub fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let got = hex::encode(Sha256::digest(&bytes));
    if got != expected {
        return Err(format!("file check failed for {} (got {got}, want {expected})", path.display()));
    }
    Ok(())
}

/// I4 "verify then name": verify the staged weights against `safetensors_sha` — the sha that will NAME
/// this model in the id-binding — then ATOMICALLY rename the staging dir into `dest_dir` and write
/// `air-model.json` from that verified sha. Fail-closed: on a weights mismatch the staging dir is
/// removed and nothing is installed. (The tokenizer/config TOFU pins were verified at download.)
pub fn install_verified(staging: &Path, dest_dir: &Path, model_id: &str, safetensors_sha: &str) -> Result<(), String> {
    if let Err(e) = verify_file(&staging.join(WEIGHTS_FILE), safetensors_sha) {
        let _ = std::fs::remove_dir_all(staging);
        return Err(e);
    }
    if let Some(parent) = dest_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    // Replace any prior partial install so the atomic rename lands cleanly.
    if dest_dir.exists() {
        std::fs::remove_dir_all(dest_dir).map_err(|e| format!("clear stale {}: {e}", dest_dir.display()))?;
    }
    std::fs::rename(staging, dest_dir).map_err(|e| format!("atomic install rename failed: {e}"))?;
    let binding = serde_json::json!({ "model_id": model_id, "safetensors_sha": safetensors_sha });
    std::fs::write(
        dest_dir.join(ID_BINDING_FILE),
        serde_json::to_vec(&binding).map_err(|e| format!("encode id-binding: {e}"))?,
    )
    .map_err(|e| format!("write id-binding: {e}"))?;
    Ok(())
}

/// The full flow: preflight → stream each file into a namespaced temp dir (with progress) → verify →
/// atomic install → id-binding. Returns `(model_id, safetensors_sha)` for the caller to pass to
/// `SetActiveModel`. Fail-closed: any error removes the staging dir and never touches the model dir.
pub async fn download_and_install(
    models_root: &Path,
    on_progress: &mut ProgressFn<'_>,
) -> Result<(String, String), String> {
    preflight_disk(models_root, REQUIRED_FREE_BYTES)?;
    let staging = models_root.join(format!(".tmp-{}", unique_suffix()));
    std::fs::create_dir_all(&staging).map_err(|e| format!("mkdir staging: {e}"))?;
    let outcome = fetch_verify_install(models_root, &staging, on_progress).await;
    if outcome.is_err() {
        // Never leave a partial download behind; the model dir itself is only ever created by
        // `install_verified`'s atomic rename of a fully-verified staging dir.
        let _ = std::fs::remove_dir_all(&staging);
    }
    outcome
}

/// Inner pipeline for `download_and_install` (split out so the caller can clean up `staging` on any error).
async fn fetch_verify_install(
    models_root: &Path,
    staging: &Path,
    on_progress: &mut ProgressFn<'_>,
) -> Result<(String, String), String> {
    let grand_total: u64 = PACK_FILES.iter().map(|f| f.bytes).sum();
    let client = reqwest::Client::new();
    let mut done: u64 = 0;
    for f in PACK_FILES {
        let url = format!("{RELEASE_BASE}/{}", f.name);
        let resp = client.get(&url).send().await.map_err(|e| format!("download {}: {e}", f.name))?;
        let mut resp = resp.error_for_status().map_err(|e| format!("download {}: {e}", f.name))?;
        let mut out = std::fs::File::create(staging.join(f.name)).map_err(|e| format!("create {}: {e}", f.name))?;
        // Stream chunks so progress + memory stay bounded on the ~488 MB safetensors.
        while let Some(chunk) = resp.chunk().await.map_err(|e| format!("read {}: {e}", f.name))? {
            use std::io::Write;
            out.write_all(&chunk).map_err(|e| format!("write {}: {e}", f.name))?;
            done += chunk.len() as u64;
            on_progress(done, grand_total);
        }
        // TOFU-pinned tokenizer/config are verified here, right after fetch (fetch-model.sh pattern);
        // the weights are verified in `install_verified` against the sha that names the binding (I4).
        if f.name != WEIGHTS_FILE {
            verify_file(&staging.join(f.name), f.sha256)?;
        }
    }
    let safetensors_sha = PACK_FILES
        .iter()
        .find(|f| f.name == WEIGHTS_FILE)
        .expect("WEIGHTS_FILE is one of PACK_FILES")
        .sha256
        .to_string();
    let dest = models_root.join(MULTILINGUAL_MODEL_ID);
    install_verified(staging, &dest, MULTILINGUAL_MODEL_ID, &safetensors_sha)?;
    Ok((MULTILINGUAL_MODEL_ID.to_string(), safetensors_sha))
}

/// The nearest existing ancestor of `p` (so preflight can stat a real dir even before `models/` exists).
fn existing_ancestor(p: &Path) -> PathBuf {
    let mut cur = p;
    loop {
        if cur.exists() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return PathBuf::from("/"),
        }
    }
}

/// A collision-resistant temp suffix (pid + time-nanos) without adding a uuid dependency.
fn unique_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{}", std::process::id(), nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_file_rejects_a_corrupt_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("model.safetensors");
        std::fs::write(&p, b"corrupt").unwrap();
        let err = verify_file(&p, "0000000000000000000000000000000000000000000000000000000000000000").unwrap_err();
        assert!(err.contains("check failed"), "{err}");
    }

    #[test]
    fn verify_file_accepts_a_matching_sha() {
        use sha2::{Digest, Sha256};
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("tokenizer.json");
        let bytes = b"{}";
        std::fs::write(&p, bytes).unwrap();
        let sha = hex::encode(Sha256::digest(bytes));
        assert!(verify_file(&p, &sha).is_ok());
    }

    #[test]
    fn install_verified_atomically_renames_and_writes_binding() {
        use sha2::{Digest, Sha256};
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".tmp-abc");
        std::fs::create_dir_all(&staging).unwrap();
        let weights = b"weights";
        std::fs::write(staging.join("model.safetensors"), weights).unwrap();
        std::fs::write(staging.join("tokenizer.json"), b"tok").unwrap();
        std::fs::write(staging.join("config.json"), b"cfg").unwrap();
        let sha = hex::encode(Sha256::digest(weights));
        let dest = tmp.path().join("minishlab/potion-multilingual-128M");

        install_verified(&staging, &dest, "minishlab/potion-multilingual-128M", &sha).unwrap();

        assert!(dest.join("model.safetensors").is_file(), "atomically renamed into place");
        assert!(!staging.exists(), "staging dir consumed by the rename");
        let binding: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dest.join("air-model.json")).unwrap()).unwrap();
        assert_eq!(binding["model_id"], "minishlab/potion-multilingual-128M");
        assert_eq!(binding["safetensors_sha"], sha, "id-binding written from the VERIFIED sha (I4)");
    }

    #[test]
    fn preflight_refuses_when_not_enough_free_space() {
        let tmp = tempfile::tempdir().unwrap();
        let err = preflight_disk(tmp.path(), u64::MAX).unwrap_err();
        assert!(err.contains("disk space"), "{err}");
        // A tiny requirement passes.
        assert!(preflight_disk(tmp.path(), 1).is_ok());
    }
}
