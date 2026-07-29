//! Rung-5 SP-V1 — the daemon side of the export ops, driven over a REAL `bossclawd` accept loop
//! (`server::spawn_for_test`) with a hermetic engine (in-memory vault + mock providers).
//!
//! `authz.rs` proves these three ops are App-only; this file proves what the App itself gets:
//!
//! - **`SetBinding` validation (spec §2.3 C-NEW-4).** Core stores what it is given, so the daemon is
//!   the only gate. Each of the four refusals is exercised — purpose tag, identity signature,
//!   brain-key match, and first-write-wins epoch idempotency — because a card that slips past any of
//!   them would make every FUTURE export fail verification, long after the write that caused it.
//! - **`ExportBundle` refusal ordering + the happy path.** The bundle the wire returns is parsed and
//!   VERIFIED with the real `bossclaw_bundle::verify`, so the whole wire path (daemon-stamped
//!   `created_at` → core export → seal → frame) is proven end-to-end, not just type-checked.
//!
//! Unix-only (the daemon + socket are Unix-only).
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use bossclaw_canon::sign::{sign_bytes, SigningKey};
use bossclaw_bundle::{
    binding_signing_bytes, Binding, BindingPayload, BINDING_PURPOSE,
};
use bossclawd::capture::render::Rendered;
use bossclawd::capture::store::{store_capture, CaptureIdentity, CAPTURE_TOOL};
use bossclawd::engine::EngineHandle;
use bossclawd::server;
use bossclawd_proto::{
    read_frame, write_frame, BundleLimitWire, ExportSelectionWire, Hello, HelloOk, OpErrorKindWire,
    Request, Response, Role, PROTO_VERSION,
};
use tokio::net::UnixStream;

/// The did the fixture brain is onboarded with; the binding claims the same one (verify requires
/// `manifest.did == binding.payload.did`, and core copies the did straight off the stored card).
const FIXTURE_DID: &str = "did:wba:example.com:tester";

/// A connected `App` test client. Mirrors `authz.rs`'s `RoleClient` (each integration test binary in
/// this crate owns its own client + spawn helper — they cannot share `#[cfg(test)]` code).
struct AppClient {
    stream: UnixStream,
}

impl AppClient {
    async fn connect(sock_path: &std::path::Path) -> Self {
        let mut stream = UnixStream::connect(sock_path).await.expect("connect to daemon socket");
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };
        write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap()).await.expect("send Hello");
        let reply = read_frame(&mut stream).await.expect("read HelloOk");
        let hello_ok: HelloOk = serde_json::from_slice(&reply).expect("parse HelloOk");
        assert_eq!(hello_ok.proto_version, PROTO_VERSION, "daemon speaks our version");
        Self { stream }
    }

    async fn call(&mut self, req: Request) -> Response {
        write_frame(&mut self.stream, &serde_json::to_vec(&req).unwrap()).await.expect("send req");
        let frame = read_frame(&mut self.stream).await.expect("read resp");
        serde_json::from_slice(&frame).expect("parse resp")
    }

    async fn brain_key(&mut self) -> String {
        match self.call(Request::BrainVerifyingKey { onboarded: true }).await {
            Response::BrainVerifyingKey(key) => key,
            other => panic!("BrainVerifyingKey should succeed, got {other:?}"),
        }
    }
}

/// Spawn a daemon on a fresh temp socket + temp home (onboarded via a valid `identity.json`) and
/// RETURN the shared engine handle. The session-export test needs it for setup no wire op can do:
/// storing a capture (`store_capture`) and reading the signed fold back. Mirrors
/// `server::spawn_for_test`, but hands back the `Arc<EngineHandle>` it serves on — the same shape
/// `memory_client_loop.rs` uses for the SP3 capture tests.
async fn spawn_daemon_with_engine() -> (tempfile::TempDir, PathBuf, Arc<EngineHandle>) {
    use std::os::unix::fs::PermissionsExt;

    use tokio::net::UnixListener;

    bossclawd::vault::seed_secret_cache_for_test(Default::default());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    std::fs::write(
        home.join("identity.json"),
        serde_json::json!({
            "did": FIXTURE_DID,
            "name": "Tester",
            "created_at": "2026-07-09T00:00:00+00:00"
        })
        .to_string(),
    )
    .unwrap();
    let engine = Arc::new(server::test_engine(home.clone()));
    let sock_path = home.join("bossclawd.sock");
    // Bind BEFORE returning so a client connect cannot race the bind, and pin 0600 exactly as
    // production does.
    let listener = UnixListener::bind(&sock_path).expect("bind test socket");
    std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    tokio::spawn(server::run_accept_loop(engine.clone(), listener));
    (dir, sock_path, engine)
}

/// [`spawn_daemon_with_engine`] for the tests that only drive the socket.
async fn spawn_onboarded_daemon() -> (tempfile::TempDir, PathBuf) {
    let (dir, sock_path, _engine) = spawn_daemon_with_engine().await;
    (dir, sock_path)
}

/// Mint an identity binding exactly as the app will (spec §2.3): sign `jcs(payload)` with the
/// identity key and wrap the signature as multibase.
fn mint_binding(identity_key: &SigningKey, brain_verifying_key: &str, epoch: u64) -> Binding {
    let payload = BindingPayload {
        brain_verifying_key: brain_verifying_key.to_string(),
        identity_verifying_key: multibase::encode(
            multibase::Base::Base58Btc,
            identity_key.verifying_key().to_bytes(),
        ),
        did: FIXTURE_DID.to_string(),
        purpose: BINDING_PURPOSE.to_string(),
        epoch,
        created_at: "2026-07-21T00:00:00Z".to_string(),
    };
    let identity_signature = sign_bytes(&binding_signing_bytes(&payload), identity_key);
    Binding { payload, identity_signature }
}

fn attestation(binding: &Binding) -> serde_json::Value {
    serde_json::to_value(binding).expect("a binding always serializes")
}

/// Assert a response is the typed `Rejected` and that its message names the check that fired.
fn assert_rejected(resp: &Response, needle: &str) {
    match resp {
        Response::Err { kind: OpErrorKindWire::Rejected, message } => assert!(
            message.contains(needle),
            "expected a {needle:?} rejection, got {message:?}"
        ),
        other => panic!("expected a typed Rejected mentioning {needle:?}, got {other:?}"),
    }
}

/// C-NEW-4: the daemon VALIDATES an app-minted binding before storing it. A correctly minted card is
/// accepted; each of the four refusals fires on its own; and a repeated epoch is REJECTED rather than
/// silently overwriting the stored card (first-write-wins, A8/C9).
#[tokio::test]
async fn set_binding_validates_before_storing() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut app = AppClient::connect(&sock).await;
    let brain_key = app.brain_key().await;
    let identity_key = SigningKey::from_bytes(&[7u8; 32]);

    // A correctly minted card is stored.
    let good = mint_binding(&identity_key, &brain_key, 1);
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&good) })
        .await;
    assert!(matches!(resp, Response::Ok), "a valid binding is stored, got {resp:?}");

    // FIRST-WRITE-WINS: a DIFFERENT card re-using epoch 1 is refused, never overwritten.
    let mut replay = mint_binding(&SigningKey::from_bytes(&[9u8; 32]), &brain_key, 1);
    replay.payload.created_at = "2026-07-22T00:00:00Z".to_string();
    replay.identity_signature =
        sign_bytes(&binding_signing_bytes(&replay.payload), &SigningKey::from_bytes(&[9u8; 32]));
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&replay) })
        .await;
    assert_rejected(&resp, "already stored");

    // Wrong PURPOSE: a card minted for another identity-signed protocol can never be stored, so it
    // can never be lifted into a bundle (the verifier enforces the same tag on the read side).
    let mut wrong_purpose = mint_binding(&identity_key, &brain_key, 2);
    wrong_purpose.payload.purpose = "did-control-challenge".to_string();
    wrong_purpose.identity_signature =
        sign_bytes(&binding_signing_bytes(&wrong_purpose.payload), &identity_key);
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&wrong_purpose) })
        .await;
    assert_rejected(&resp, "purpose");

    // Wrong BRAIN KEY: storing this would make every future export fail verification.
    let other_brain = multibase::encode(
        multibase::Base::Base58Btc,
        SigningKey::from_bytes(&[11u8; 32]).verifying_key().to_bytes(),
    );
    let wrong_brain = mint_binding(&identity_key, &other_brain, 3);
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&wrong_brain) })
        .await;
    assert_rejected(&resp, "brain key");

    // Tampered PAYLOAD (the signature no longer covers it) → the L1 internal check fires.
    let mut tampered = mint_binding(&identity_key, &brain_key, 4);
    tampered.payload.did = "did:wba:evil.com:attacker".to_string();
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&tampered) })
        .await;
    assert_rejected(&resp, "signature");

    // Not a binding at all → refused at the parse boundary, never appended.
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: serde_json::Value::Null })
        .await;
    assert_rejected(&resp, "malformed binding");

    // The epoch-1 card that was accepted FIRST is still the stored one — every refusal above left
    // the log alone, so the export still seals under the original card's did.
    let selection = ExportSelectionWire {
        note_event_ids: vec![],
        session_event_ids: vec![],
        ingest_event_ids: vec!["01J-NOT-A-REAL-INGEST".into()],
        description: "probe".into(),
    };
    let resp = app.call(Request::ExportBundle { onboarded: true, selection }).await;
    assert_rejected(&resp, "is not a current file");
}

/// A binding naming a FOREIGN did is refused. `export_bundle` copies `payload.did` straight into
/// `manifest.did`, so storing one would silently re-attribute every future bundle — and nothing
/// downstream catches it in SP-V1 (`binding.did == manifest.did` holds by construction, and only the
/// inert `OfflineResolver` ships). A card naming the OWNER's did, identical in every other respect,
/// is accepted — so the refusal is provably the did check and not some other gate.
#[tokio::test]
async fn set_binding_refuses_a_foreign_did() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut app = AppClient::connect(&sock).await;
    let brain_key = app.brain_key().await;
    let identity_key = SigningKey::from_bytes(&[7u8; 32]);

    let mut foreign = mint_binding(&identity_key, &brain_key, 1);
    foreign.payload.did = "did:wba:evil.com:attacker".to_string();
    // Re-sign, so the identity signature is VALID over the foreign did — the card is internally
    // consistent and only the owner-identity check can reject it.
    foreign.identity_signature =
        sign_bytes(&binding_signing_bytes(&foreign.payload), &identity_key);
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&foreign) })
        .await;
    assert_rejected(&resp, "did does not match");

    // Nothing was stored: the export still reports "no identity binding stored".
    let selection = ExportSelectionWire {
        note_event_ids: vec!["01J-DOES-NOT-EXIST".into()],
        session_event_ids: vec![],
        ingest_event_ids: vec![],
        description: "probe".into(),
    };
    let resp = app.call(Request::ExportBundle { onboarded: true, selection }).await;
    assert_rejected(&resp, "no identity binding stored");

    // The SAME epoch, same keys, same everything — but the owner's did — is accepted.
    let owned = mint_binding(&identity_key, &brain_key, 1);
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&owned) })
        .await;
    assert!(matches!(resp, Response::Ok), "the owner's own did is accepted, got {resp:?}");
}

/// The typed `ExportBundle` refusals reach the wire in core's order: an empty selection is refused
/// before the binding is even looked up, and a non-empty selection on a brain with no stored card is
/// refused with the binding signal (so the app can tell "select something" from "mint first").
#[tokio::test]
async fn export_bundle_refusals_are_typed_and_ordered() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut app = AppClient::connect(&sock).await;

    let empty = ExportSelectionWire {
        note_event_ids: vec![],
        session_event_ids: vec![],
        ingest_event_ids: vec![],
        description: "nothing selected".into(),
    };
    let resp = app.call(Request::ExportBundle { onboarded: true, selection: empty }).await;
    assert_rejected(&resp, "empty selection");

    // Non-empty, but no binding has been minted yet → BindingUnavailable, NOT a per-item complaint:
    // the binding lookup precedes the per-class gather.
    let unbound = ExportSelectionWire {
        note_event_ids: vec!["01J-DOES-NOT-EXIST".into()],
        session_event_ids: vec![],
        ingest_event_ids: vec![],
        description: "no card yet".into(),
    };
    let resp = app.call(Request::ExportBundle { onboarded: true, selection: unbound }).await;
    assert_rejected(&resp, "no identity binding stored");
}

/// The coarse pre-build guard, over the real wire: an over-cap selection comes back as the TYPED
/// `BundleTooLarge` with export-text semantics — before a single session body is read.
///
/// `approx_bytes` is caller-supplied fold METADATA, so a capture can claim a body far larger than
/// anything on disk; that is what lets this reach a 30 MiB bound with a tiny file, and it is also
/// the exact shape the estimate exists to catch (it cannot open the body to check).
///
/// This is also the live half of the Task-11 triage: the refusal path now probes the chain first
/// (`server::oversize_verdict`), so a healthy brain must still get the SIZE answer and not a chain
/// complaint. The broken-chain half is unit-tested on the pure function — reaching it here would
/// mean corrupting an encrypted store from a test.
#[tokio::test]
async fn an_over_cap_selection_is_refused_with_the_typed_size_signal() {
    let (dir, sock, engine) = spawn_daemon_with_engine().await;
    let mut app = AppClient::connect(&sock).await;
    let brain_key = app.brain_key().await;

    let identity = CaptureIdentity {
        session_id: "oversize-session".into(),
        project: "/Users/someone/air-note".into(),
        tool: CAPTURE_TOOL.into(),
    };
    let rendered = Rendered {
        title: "a session that claims to be enormous".into(),
        markdown: "IGNORED BY THE STORE".into(),
        body: "## You\nhi\n".into(),
        sha256: "ee".repeat(32),
        started_at: 1_783_764_000,
        ended_at: 1_783_764_009,
        // The claim the estimate has to take at face value.
        approx_bytes: bossclaw_core::log::MAX_EXPORT_BYTES,
        dropped_torn_tail: false,
        oversized_lines: 0,
        skipped_unknown: 0,
    };
    store_capture(&engine, dir.path(), &identity, &rendered).await.expect("capture stored");

    let binding = mint_binding(&SigningKey::from_bytes(&[7u8; 32]), &brain_key, 1);
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&binding) })
        .await;
    assert!(matches!(resp, Response::Ok), "binding stored, got {resp:?}");

    let event_id = engine
        .current_sessions()
        .await
        .expect("session fold")
        .into_iter()
        .find(|s| s.session_id == "oversize-session")
        .expect("the capture is a CURRENT session")
        .event_id;

    let selection = ExportSelectionWire {
        note_event_ids: vec![],
        session_event_ids: vec![event_id],
        ingest_event_ids: vec![],
        description: "too much".into(),
    };
    match app.call(Request::ExportBundle { onboarded: true, selection }).await {
        Response::BundleTooLarge { bytes, max, limit } => {
            assert!(bytes > max, "the estimate ({bytes}) crossed the ceiling ({max})");
            assert_eq!(max, bossclaw_core::log::MAX_EXPORT_BYTES, "the export-bytes ceiling");
            assert_eq!(limit, BundleLimitWire::ExportBytes, "text semantics, not frame semantics");
        }
        other => panic!("an over-cap selection must be refused with the typed signal, got {other:?}"),
    }
}

/// End-to-end over the socket: remember a note, mint + store the binding, export it — and VERIFY the
/// returned `.airmem` with the real verifier. Proves the whole daemon seam (daemon-stamped
/// `created_at`, core export, master seal, wire frame) produces a bundle a foreign verifier accepts,
/// and that the seal is over THIS brain's key.
#[tokio::test]
async fn export_bundle_round_trip_verifies() {
    let (_dir, sock) = spawn_onboarded_daemon().await;
    let mut app = AppClient::connect(&sock).await;
    let brain_key = app.brain_key().await;

    let note_id = match app
        .call(Request::Remember { onboarded: true, text: "Aria Novak signed the lease".into() })
        .await
    {
        Response::Remember(id) => id,
        other => panic!("Remember should succeed, got {other:?}"),
    };

    let binding = mint_binding(&SigningKey::from_bytes(&[7u8; 32]), &brain_key, 1);
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&binding) })
        .await;
    assert!(matches!(resp, Response::Ok), "binding stored, got {resp:?}");

    let selection = ExportSelectionWire {
        note_event_ids: vec![note_id],
        session_event_ids: vec![],
        ingest_event_ids: vec![],
        description: "one note for my lawyer".into(),
    };
    let text = match app.call(Request::ExportBundle { onboarded: true, selection }).await {
        Response::Bundle(text) => text,
        other => panic!("ExportBundle should return a sealed bundle, got {other:?}"),
    };

    let airmem: bossclaw_bundle::Airmem =
        serde_json::from_str(&text).expect("the wire text is a parseable .airmem");
    assert_eq!(airmem.manifest.brain_verifying_key, brain_key, "sealed by THIS brain");
    assert_eq!(airmem.manifest.did, FIXTURE_DID, "the sealed did is the binding's did");
    assert_eq!(airmem.manifest.item_count, 1, "exactly the selected note");
    bossclaw_bundle::verify(&airmem, &bossclaw_bundle::OfflineResolver)
        .expect("the exported bundle verifies");
}

/// Story C addressing: the `event_id` a `ListSessions` row carries IS the id `ExportBundle` accepts.
///
/// This closes the exact loop the gap lived in. `SessionSummaryWire` used to carry only
/// `session_id`, so the app could list sessions but had no way to name one in an `ExportSelection`
/// — the Library shipped `[]` and the honest copy "Sessions can't be exported yet", even though the
/// engine's seal-vouched session path already worked. Asserting the field merely EXISTS would not
/// have caught that; feeding the listed value straight into an export does, because the wrong id
/// (e.g. `session_id`) comes back `NotExportable`, which this test also pins.
#[tokio::test]
async fn a_listed_session_event_id_is_the_id_export_accepts() {
    let (dir, sock, engine) = spawn_daemon_with_engine().await;
    let mut app = AppClient::connect(&sock).await;
    let brain_key = app.brain_key().await;

    let identity = CaptureIdentity {
        session_id: "story-c-session".into(),
        project: "/Users/someone/air-note".into(),
        tool: CAPTURE_TOOL.into(),
    };
    let rendered = Rendered {
        title: "whole-brain backup".into(),
        markdown: "IGNORED BY THE STORE".into(),
        body: "## You\nBack up everything.\n\n## Assistant\nOn it.\n".into(),
        sha256: "cc".repeat(32),
        started_at: 1_783_764_000,
        ended_at: 1_783_764_009,
        approx_bytes: 64,
        dropped_torn_tail: false,
        oversized_lines: 0,
        skipped_unknown: 0,
    };
    store_capture(&engine, dir.path(), &identity, &rendered).await.expect("capture stored");

    // Take the id the APP would see — straight off the wire, never from the engine.
    let listed = match app.call(Request::ListSessions { onboarded: true }).await {
        Response::ListSessions(rows) => rows,
        other => panic!("ListSessions should succeed, got {other:?}"),
    };
    let row = listed.iter().find(|r| r.session_id == "story-c-session").expect("the capture is listed");
    assert!(!row.event_id.is_empty(), "the Library row carries an export address");
    assert_ne!(row.event_id, row.session_id, "the export address is the EVENT id, not the identity key");

    let binding = mint_binding(&SigningKey::from_bytes(&[7u8; 32]), &brain_key, 1);
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&binding) })
        .await;
    assert!(matches!(resp, Response::Ok), "binding stored, got {resp:?}");

    let select = |id: &str| ExportSelectionWire {
        note_event_ids: vec![],
        session_event_ids: vec![id.to_string()],
        ingest_event_ids: vec![],
        description: "story C".into(),
    };

    // The LISTED event id exports.
    let text = match app
        .call(Request::ExportBundle { onboarded: true, selection: select(&row.event_id) })
        .await
    {
        Response::Bundle(text) => text,
        other => panic!("the listed event_id must be exportable, got {other:?}"),
    };
    let airmem: bossclaw_bundle::Airmem = serde_json::from_str(&text).expect("parseable .airmem");
    assert_eq!(airmem.manifest.item_count, 1, "the session was exported");
    assert_eq!(airmem.items[0].kind, "session", "exported as a session item");
    assert!(text.contains("Back up everything."), "the transcript rode along");
    bossclaw_bundle::verify(&airmem, &bossclaw_bundle::OfflineResolver).expect("verifies");

    // The identity key is NOT an export address — pins that the row's two ids are not
    // interchangeable, so a future refactor cannot quietly swap them.
    let resp = app
        .call(Request::ExportBundle { onboarded: true, selection: select(&row.session_id) })
        .await;
    assert_rejected(&resp, "is not a current session");
}

// ── A-N1 sentinels for the session-export privacy test ────────────────────────────────────────
// Each is a string that MUST NOT survive into the sealed bundle. They are distinctive enough that
// a hit can only have come from the capture's front matter.

/// The owner's username, as it appears inside the stored project path.
const SENTINEL_USER: &str = "sentineluser";
/// The full project path the capture records — front matter writes it verbatim as `project:`.
const SENTINEL_PROJECT_PATH: &str = "/Users/sentineluser/wills-and-trusts";
/// The folder NAME the owner ruling (`d0c5b14`) DOES allow through, via `display.project`. The
/// positive control: its presence proves the test is not passing because the export shipped nothing.
const ALLOWED_PROJECT_NAME: &str = "wills-and-trusts";
/// The capture's session id — front matter writes it as `session_id:`.
const SENTINEL_SESSION_ID: &str = "sentinel-session-4f2a";
/// The body digest — front matter writes it as `sha256:`.
const SENTINEL_SHA: &str = "beefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafe";
/// The transcript text itself, which MUST survive (a session ships its full body).
const SENTINEL_BODY: &str = "## You\nHow do I revoke a trust?\n\n## Assistant\nFile a revocation.\n";

/// A-N1 REGRESSION: an exported session discloses its transcript and NOTHING ELSE from the stored
/// `.md`. `read_capture_markdown` returns the WHOLE document, whose front matter carries the project
/// PATH, the `session_id`, and the body `sha256` — so wiring the raw reader into the export would
/// smuggle the owner's username and directory layout into every session bundle, undoing the owner
/// ruling (`d0c5b14`) that reduced `display.project` to a bare folder name.
///
/// Asserted as OUTCOMES on the serialized bundle: the transcript text and the allowed folder name are
/// present; the four sentinels are absent.
#[tokio::test]
async fn exported_session_discloses_the_body_but_no_front_matter_provenance() {
    let (dir, sock, engine) = spawn_daemon_with_engine().await;
    let mut app = AppClient::connect(&sock).await;
    let brain_key = app.brain_key().await;

    // Store a real capture whose front matter carries every sentinel.
    let rendered = Rendered {
        title: "revoking a trust".into(),
        markdown: "IGNORED BY THE STORE".into(),
        body: SENTINEL_BODY.into(),
        sha256: SENTINEL_SHA.into(),
        started_at: 1_783_764_000,
        ended_at: 1_783_764_009,
        approx_bytes: 42,
        dropped_torn_tail: false,
        oversized_lines: 0,
        skipped_unknown: 0,
    };
    let identity = CaptureIdentity {
        session_id: SENTINEL_SESSION_ID.into(),
        project: SENTINEL_PROJECT_PATH.into(),
        tool: CAPTURE_TOOL.into(),
    };
    store_capture(&engine, dir.path(), &identity, &rendered).await.expect("capture stored");

    // Sanity: the stored document really does contain what we are about to prove is stripped —
    // otherwise this test could pass against a capture format that never carried the provenance.
    let stored = std::fs::read_to_string(dir.path().join("sessions").join(format!("{SENTINEL_SESSION_ID}.md")))
        .expect("the capture .md is on disk");
    for sentinel in [SENTINEL_USER, SENTINEL_SESSION_ID, SENTINEL_SHA] {
        assert!(stored.contains(sentinel), "the stored .md front matter carries {sentinel:?}");
    }

    let event_id = engine
        .current_sessions()
        .await
        .expect("session fold")
        .into_iter()
        .find(|s| s.session_id == SENTINEL_SESSION_ID)
        .expect("the capture is a CURRENT session")
        .event_id;

    let binding = mint_binding(&SigningKey::from_bytes(&[7u8; 32]), &brain_key, 1);
    let resp = app
        .call(Request::SetBinding { onboarded: true, attestation: attestation(&binding) })
        .await;
    assert!(matches!(resp, Response::Ok), "binding stored, got {resp:?}");

    let selection = ExportSelectionWire {
        note_event_ids: vec![],
        session_event_ids: vec![event_id],
        ingest_event_ids: vec![],
        description: "one session".into(),
    };
    let text = match app.call(Request::ExportBundle { onboarded: true, selection }).await {
        Response::Bundle(text) => text,
        other => panic!("ExportBundle should return a sealed bundle, got {other:?}"),
    };

    // The bundle still discloses the transcript AND the allowed folder name (positive control).
    assert!(text.contains("How do I revoke a trust?"), "the transcript is disclosed");
    assert!(
        text.contains(ALLOWED_PROJECT_NAME),
        "the folder NAME still rides in display.project (owner ruling d0c5b14)"
    );
    // ...and none of the front-matter provenance.
    for sentinel in [SENTINEL_USER, "/Users/", SENTINEL_SESSION_ID, SENTINEL_SHA] {
        assert!(
            !text.contains(sentinel),
            "A-N1 LEAK: the sealed bundle carries {sentinel:?} — the capture front matter was not \
             stripped before disclosure"
        );
    }

    // The stripped bundle still verifies (stripping happens BEFORE the leaf hashes are taken).
    let airmem: bossclaw_bundle::Airmem = serde_json::from_str(&text).expect("parseable .airmem");
    bossclaw_bundle::verify(&airmem, &bossclaw_bundle::OfflineResolver)
        .expect("the exported session bundle verifies");
}
