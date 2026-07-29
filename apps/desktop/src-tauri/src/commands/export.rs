//! Rung-5 SP-V1 export: the owner-facing "turn these memories into a sealed file" command.
//!
//! Three steps, in this order:
//! 1. **Mint the identity binding** (the ID card) if this brain has none. Only the APP holds the
//!    AIR identity key (`air/did_wba.rs`), so only the app can sign the card — the daemon validates
//!    and stores it, and never mints. This is the ONE mutation an export performs (S1); everything
//!    after it is a pure read.
//! 2. **Ask the daemon to build + seal** the `.airmem` for the owner's selection.
//! 3. **Save it** where the owner chooses, all-or-nothing.
//!
//! # Nothing is published
//! Exporting writes one local file. It sends nothing anywhere; the owner decides who receives it.
//!
//! # "Nothing happened" is not quite true of a FAILED first export
//! Step 1 runs before step 2, so an export that the daemon then refuses (or that the owner cancels
//! at the save dialog) can still leave the minted binding stored. That is harmless and deliberate —
//! the card is the brain's ID, not a record of any export, and it is idempotent and first-write-wins
//! — but it is the one durable trace a failed export leaves, so it is written down rather than
//! implied away.
//!
//! # This module never READS a `.airmem`
//! It only writes one, so it has no parse boundary and deliberately does NOT carry `air-verify`'s
//! `MAX_INPUT_BYTES` ceiling. If a future app-side path ever PARSES a bundle (an in-app verifier),
//! that ceiling must be hoisted into `bossclaw-bundle` and shared — never re-typed here.

use crate::air::did_wba::AgentKeypair;
use crate::air::identity::IdentityStore;
use crate::commands::identity::AppState;
use crate::engine::{Engine, EngineOpError};
use bossclaw_bundle::{Binding, BindingPayload, BINDING_PURPOSE};
use bossclawd_proto::ExportSelectionWire;
use std::path::Path;
use tauri::State;

/// The only epoch SP-V1 mints. `epoch` is reserved for future key-rotation semantics (spec A8/C9);
/// until rotation ships there is exactly one card per brain, so every later mint attempt is the
/// no-op [`ensure_binding`] swallows.
const BINDING_EPOCH: u64 = 1;

/// The daemon's first-write-wins refusal ("binding epoch N already stored"). Matched on this
/// SUBSTRING because that is what makes the mint idempotent: there is no "is a binding stored?"
/// wire op, so every export re-offers the card and this exact refusal means "yours is already the
/// stored one". Coupled to `bossclawd`'s `set_binding` message by construction — a reworded daemon
/// refusal would make the second export fail loudly (never silently mis-seal), which is the safe
/// direction for the coupling to break in.
const EPOCH_ALREADY_STORED: &str = "already stored";

/// Shown when the app has no AIR identity to sign the card with. Export needs the identity key, so
/// this is a genuine precondition, not a fault.
const MISSING_IDENTITY: &str =
    "no AIR identity yet — finish onboarding before exporting signed memories";

/// The `.airmem` file extension, for the save dialog's filter and default name.
const AIRMEM_EXTENSION: &str = "airmem";

/// Export the selected memories as a sealed `.airmem`. Returns the saved path, or `None` if the
/// owner cancelled the save dialog.
///
/// Every refusal from core arrives verbatim (empty selection, a note that is no longer current, a
/// non-external "note", an over-size bundle, too many items) — the owner is told WHICH refusal
/// happened, never a friendlier text that hides it.
#[tauri::command]
pub async fn export_bundle(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    note_event_ids: Vec<String>,
    session_event_ids: Vec<String>,
    ingest_event_ids: Vec<String>,
    description: String,
) -> Result<Option<String>, String> {
    let onboarded = state.identity_store.is_onboarded();
    let text = sealed_bundle(
        state.engine.as_ref(),
        &state.identity_store,
        onboarded,
        ExportSelectionWire { note_event_ids, session_event_ids, ingest_event_ids, description },
    )
    .await?;
    save_airmem(&app, &text).await
}

/// Mint-then-export, split from the command so the whole load-bearing sequence is unit-testable
/// without a Tauri `AppHandle` or a save dialog (the `integrations.rs` wiring-helper convention).
async fn sealed_bundle(
    engine: &Engine,
    identity: &IdentityStore,
    onboarded: bool,
    selection: ExportSelectionWire,
) -> Result<String, String> {
    ensure_binding(engine, identity, onboarded).await?;
    engine.export_bundle(onboarded, selection).await.map_err(|e| e.to_string())
}

/// Ensure this brain has an identity binding, minting one on the first export.
///
/// The brain key is READ BACK from the daemon every time (§2.3) rather than remembered: the card
/// commits to it, and the daemon refuses a card whose key does not match the brain it is stored in.
///
/// Idempotent by refusal: a repeat mint hits the daemon's first-write-wins epoch check and is
/// swallowed as success. Any OTHER rejection (wrong purpose, bad signature, wrong brain key, wrong
/// did) is surfaced — those mean the card is wrong, and a wrong card must never be papered over.
///
/// One consequence worth naming: because SP-V1 mints a single epoch, a brain that outlived a new
/// AIR identity would keep exporting under the STORED card. Rotation (a second epoch) is the A8/C9
/// follow-up that fixes it; today identity reset also resets the brain, so the pair cannot drift.
async fn ensure_binding(
    engine: &Engine,
    identity: &IdentityStore,
    onboarded: bool,
) -> Result<(), String> {
    let brain_verifying_key = engine.brain_verifying_key(onboarded).await.map_err(|e| e.to_string())?;
    let secret = identity.load_signing_key()?.ok_or_else(|| MISSING_IDENTITY.to_string())?;
    let did = identity.load_metadata()?.ok_or_else(|| MISSING_IDENTITY.to_string())?.did.0;
    let attestation = mint_binding(brain_verifying_key, &secret, did, now_rfc3339())?;
    match engine.set_binding(onboarded, attestation).await {
        Ok(()) => Ok(()),
        Err(EngineOpError::Rejected(message)) if message.contains(EPOCH_ALREADY_STORED) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Build + sign the ID card: the payload the format defines, signed by the AIR identity key over
/// `jcs(payload)` — the exact bytes the verifier re-checks.
///
/// Both multibase values are base58btc: the identity key as its BARE 32 bytes (the form
/// `bossclaw_bundle::decode_ed25519_key` compares by raw bytes), and the signature wrapped from the
/// RAW 64 bytes `AgentKeypair::sign` returns.
fn mint_binding(
    brain_verifying_key: String,
    identity_secret: &[u8; 32],
    did: String,
    created_at: String,
) -> Result<serde_json::Value, String> {
    let keypair = AgentKeypair::from_secret_bytes(identity_secret);
    let payload = BindingPayload {
        brain_verifying_key,
        identity_verifying_key: multibase::encode(
            multibase::Base::Base58Btc,
            keypair.public_key_bytes(),
        ),
        did,
        purpose: BINDING_PURPOSE.to_string(),
        epoch: BINDING_EPOCH,
        created_at,
    };
    let signature = keypair.sign(&bossclaw_bundle::binding_signing_bytes(&payload));
    let binding = Binding {
        payload,
        identity_signature: multibase::encode(multibase::Base::Base58Btc, signature),
    };
    serde_json::to_value(&binding).map_err(|e| format!("could not build the identity card: {e}"))
}

/// Mint time, RFC3339 (mirrors `commands/inbox.rs`).
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Ask the owner where to save, then write the bundle there. `Ok(None)` = the owner cancelled.
/// Mirrors `engine_pick_folder`'s dialog plumbing (`commands/engine.rs`), with `save_file`.
async fn save_airmem(app: &tauri::AppHandle, text: &str) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(default_file_name())
        .add_filter("Signed memory bundle", &[AIRMEM_EXTENSION])
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    // A cancelled dialog yields None; a dropped sender (window closed) also -> None.
    let Some(path) = rx.await.ok().flatten().and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    write_all_or_nothing(&path, text)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// The save dialog's suggested name — dated, so repeated exports don't silently overwrite.
fn default_file_name() -> String {
    format!("memories-{}.{AIRMEM_EXTENSION}", chrono::Utc::now().format("%Y-%m-%d"))
}

/// Write via a sibling temp file + rename (S1): a same-directory rename is atomic, so the chosen
/// path is either absent or a COMPLETE bundle — never a half-written file the owner might send.
///
/// The temp is a [`tempfile::NamedTempFile`], not a predictable `<target>.tmp`, and that choice
/// carries two guarantees a hand-rolled temp did not:
/// * **No fragment survives ANY failure.** It is deleted on drop, so a write that dies partway
///   (disk full, quota, I/O) leaves no truncated-but-readable copy of the owner's memories sitting
///   in what may well be a synced folder. Cleanup on the rename path alone was not enough.
/// * **No symlink redirect.** It is created `O_EXCL` under an unguessable name, so a pre-planted
///   `<target>.tmp` symlink can be neither guessed nor followed (`fs::write` follows symlinks;
///   `rename` does not, which is why only the temp needed fixing).
fn write_all_or_nothing(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    // The save dialog always yields an absolute path; a bare file name (tests, odd input) means
    // "the current directory" rather than an empty one `NamedTempFile` would refuse.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
        format!("could not create a temporary file in {}: {e}", parent.display())
    })?;
    tmp.write_all(text.as_bytes())
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    tmp.persist(path).map_err(|e| format!("could not save {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::air::types::Did;
    use crate::engine::transport::Transport;
    use crate::secrets::SecretsVault;
    use bossclawd_proto::{OpErrorKindWire, Request, Response};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// The brain key the fake daemon reports — the value a correct mint must commit to.
    const BRAIN_KEY: &str = "zBrainKeyFromTheDaemon";

    /// A fake transport that records EVERY request (the mint sequence spans three ops, so the
    /// `integrations.rs` last-request-only double isn't enough) and answers each with a canned
    /// response. `set_binding_error` lets a test make `SetBinding` refuse.
    struct RecordingTransport {
        seen: Mutex<Vec<Request>>,
        set_binding_error: Option<EngineOpError>,
    }
    impl RecordingTransport {
        fn new() -> Arc<Self> {
            Arc::new(Self { seen: Mutex::new(Vec::new()), set_binding_error: None })
        }
        fn refusing(error: EngineOpError) -> Arc<Self> {
            Arc::new(Self { seen: Mutex::new(Vec::new()), set_binding_error: Some(error) })
        }
        fn seen(&self) -> Vec<Request> {
            self.seen.lock().unwrap().clone()
        }
    }
    #[async_trait::async_trait]
    impl Transport for RecordingTransport {
        async fn request(&self, req: Request) -> Result<Response, EngineOpError> {
            let resp = match &req {
                Request::BrainVerifyingKey { .. } => Response::BrainVerifyingKey(BRAIN_KEY.into()),
                Request::SetBinding { .. } => match &self.set_binding_error {
                    // The daemon signals a refusal as a RESPONSE, not a transport failure — the
                    // client folds it into the typed error, exactly as production does.
                    Some(EngineOpError::Rejected(m)) => {
                        Response::Err { kind: OpErrorKindWire::Rejected, message: m.clone() }
                    }
                    Some(_) | None => Response::Ok,
                },
                Request::ExportBundle { .. } => Response::Bundle("{\"manifest\":{}}".into()),
                _ => Response::Ok,
            };
            self.seen.lock().unwrap().push(req);
            Ok(resp)
        }
    }

    /// In-memory secrets, so the identity store never touches the OS keychain in tests.
    struct MemoryVault(Mutex<HashMap<String, String>>);
    impl SecretsVault for MemoryVault {
        fn set(&self, key: &str, value: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn get(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    const TEST_DID: &str = "did:wba:example.com:me";

    /// An onboarded identity store (signing key + metadata) over a temp dir.
    fn onboarded_identity(dir: &Path) -> IdentityStore {
        let store = IdentityStore::new(
            Arc::new(MemoryVault(Mutex::new(HashMap::new()))),
            dir.to_path_buf(),
        );
        store.save_signing_key(&[7u8; 32]).expect("save key");
        store
            .save_metadata(&crate::air::identity::IdentityMetadata {
                did: Did(TEST_DID.into()),
                name: "Owner".into(),
                username: None,
                created_at: "2026-07-21T00:00:00Z".into(),
            })
            .expect("save metadata");
        store
    }

    fn engine_over(t: Arc<RecordingTransport>) -> Engine {
        Engine::new(t as Arc<dyn Transport>)
    }

    fn one_note() -> ExportSelectionWire {
        ExportSelectionWire {
            note_event_ids: vec!["n1".into()],
            session_event_ids: vec![],
            ingest_event_ids: vec![],
            description: "one fact".into(),
        }
    }

    /// The mint must produce a card the FORMAT's own checker accepts — not merely one that
    /// serializes. Uses `verify_binding_internal`, the same L1 check the daemon runs before storing.
    #[test]
    fn mint_produces_a_card_the_format_verifier_accepts() {
        let json = mint_binding(
            BRAIN_KEY.into(),
            &[7u8; 32],
            TEST_DID.into(),
            "2026-07-21T00:00:00Z".into(),
        )
        .expect("mint");
        let binding: Binding = serde_json::from_value(json).expect("card parses as a Binding");

        assert!(
            bossclaw_bundle::verify_binding_internal(&binding),
            "the identity signature must verify over jcs(payload)"
        );
        assert_eq!(binding.payload.purpose, BINDING_PURPOSE, "domain-separation tag");
        assert_eq!(binding.payload.brain_verifying_key, BRAIN_KEY, "commits to the daemon's key");
        assert_eq!(binding.payload.did, TEST_DID);
        assert_eq!(binding.payload.epoch, BINDING_EPOCH);
        assert!(
            bossclaw_bundle::decode_ed25519_key(&binding.payload.identity_verifying_key).is_some(),
            "the identity key decodes to raw Ed25519 bytes"
        );
        assert!(
            binding.identity_signature.starts_with('z'),
            "multibase base58btc, not the raw 64 bytes: {}",
            binding.identity_signature
        );
    }

    /// The first export reads the brain key, stores EXACTLY ONE card, then exports the selection.
    #[tokio::test]
    async fn first_export_mints_one_binding_then_exports_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let t = RecordingTransport::new();
        let engine = engine_over(t.clone());

        let text = sealed_bundle(&engine, &onboarded_identity(dir.path()), true, one_note())
            .await
            .expect("export");
        assert_eq!(text, "{\"manifest\":{}}");

        let seen = t.seen();
        assert_eq!(
            seen.iter().filter(|r| matches!(r, Request::SetBinding { .. })).count(),
            1,
            "exactly one card is stored, {seen:?}"
        );
        assert!(
            seen.iter().any(|r| matches!(r, Request::BrainVerifyingKey { .. })),
            "the brain key is read back, never assumed"
        );
        match seen.iter().find(|r| matches!(r, Request::ExportBundle { .. })).expect("exported") {
            Request::ExportBundle { selection, .. } => {
                assert_eq!(selection.note_event_ids, vec!["n1".to_string()]);
                assert_eq!(selection.description, "one fact");
            }
            other => panic!("expected ExportBundle, got {other:?}"),
        }
    }

    /// A second export re-offers the same epoch; the daemon's first-write-wins refusal is the
    /// idempotent success path, so the export still happens.
    #[tokio::test]
    async fn an_already_stored_epoch_is_success_and_the_export_proceeds() {
        let dir = tempfile::tempdir().unwrap();
        let t = RecordingTransport::refusing(EngineOpError::Rejected(
            "binding epoch 1 already stored".into(),
        ));
        let engine = engine_over(t.clone());

        let text = sealed_bundle(&engine, &onboarded_identity(dir.path()), true, one_note())
            .await
            .expect("a stored card is not an error");
        assert_eq!(text, "{\"manifest\":{}}");
        assert!(
            t.seen().iter().any(|r| matches!(r, Request::ExportBundle { .. })),
            "the export still runs after the no-op mint"
        );
    }

    /// Every OTHER binding refusal is a WRONG card and must stop the export — never be swallowed
    /// alongside the idempotent one.
    #[tokio::test]
    async fn a_real_binding_refusal_stops_the_export() {
        let dir = tempfile::tempdir().unwrap();
        let t = RecordingTransport::refusing(EngineOpError::Rejected(
            "binding brain key does not match this brain".into(),
        ));
        let engine = engine_over(t.clone());

        let err = sealed_bundle(&engine, &onboarded_identity(dir.path()), true, one_note())
            .await
            .expect_err("a wrong card must not seal anything");
        assert!(err.contains("does not match this brain"), "surfaced verbatim: {err}");
        assert!(
            !t.seen().iter().any(|r| matches!(r, Request::ExportBundle { .. })),
            "nothing is exported once the card is refused"
        );
    }

    /// Without an identity there is no key to sign the card with — a clear precondition message,
    /// and no wire write attempted.
    #[tokio::test]
    async fn export_without_an_identity_explains_itself() {
        let dir = tempfile::tempdir().unwrap();
        let t = RecordingTransport::new();
        let engine = engine_over(t.clone());
        let empty = IdentityStore::new(
            Arc::new(MemoryVault(Mutex::new(HashMap::new()))),
            dir.path().to_path_buf(),
        );

        let err = sealed_bundle(&engine, &empty, true, one_note()).await.expect_err("no identity");
        assert_eq!(err, MISSING_IDENTITY);
        assert!(!t.seen().iter().any(|r| matches!(r, Request::SetBinding { .. })));
    }

    /// Every name in `dir`, sorted — so a test can assert what a folder holds, not merely that the
    /// one file it expected is present.
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn saving_is_all_or_nothing_and_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("memories.airmem");
        write_all_or_nothing(&target, "{\"manifest\":{}}").expect("write");

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"manifest\":{}}");
        assert_eq!(entries(dir.path()), ["memories.airmem"], "the temp is renamed away, not left");

        // Overwriting an existing bundle replaces it whole.
        write_all_or_nothing(&target, "second").expect("overwrite");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
        assert_eq!(entries(dir.path()), ["memories.airmem"]);
    }

    /// The failure path the success test cannot reach: the bundle is written to the temp, and the
    /// SAVE then fails. Nothing readable may survive — a truncated `.airmem` fragment of the
    /// owner's memories left in a synced folder is exactly the leak this write path exists to
    /// prevent, and cleanup that lived only on the rename arm did not cover it.
    ///
    /// The failure is forced by aiming at a path that is already a DIRECTORY: the temp is created
    /// and written, then `persist` fails. Deterministic for every user, root included.
    #[test]
    fn a_failed_save_leaves_no_readable_fragment_behind() {
        let dir = tempfile::tempdir().unwrap();
        let occupied = dir.path().join("memories.airmem");
        std::fs::create_dir(&occupied).expect("something already sits at the chosen path");

        let err = write_all_or_nothing(&occupied, "{\"manifest\":{}}").expect_err("save fails");
        assert!(err.contains("could not save"), "names the failed step: {err}");
        assert_eq!(
            entries(dir.path()),
            ["memories.airmem"],
            "no temp fragment survives the failed save"
        );
        assert!(occupied.is_dir(), "and nothing clobbered what was already there");
    }

    /// A pre-planted symlink where a predictable temp name would land must not redirect the
    /// bundle's plaintext out of the folder the owner chose. The temp name is unguessable AND
    /// created `O_EXCL`, so the decoy is simply never touched.
    #[test]
    fn a_planted_temp_symlink_cannot_capture_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("attacker-readable");
        std::fs::write(&elsewhere, "empty").expect("decoy target");
        // The name the obvious implementation would have used.
        std::os::unix::fs::symlink(&elsewhere, dir.path().join("memories.airmem.tmp"))
            .expect("plant the decoy");

        let target = dir.path().join("memories.airmem");
        write_all_or_nothing(&target, "{\"manifest\":{}}").expect("write");

        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "empty", "the decoy never received it");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"manifest\":{}}");
    }
}
