//! The daemon's connection dispatch: handshake → `Request` → `EngineHandle` → `Response`.
//!
//! One `EngineHandle` is shared (behind an `Arc`) across every connection; the accept loop
//! (in `main.rs`) spawns one task per connection, each holding a clone of that `Arc`. A panic
//! in one connection task cannot take down the daemon (task isolation) — and this module never
//! `.unwrap()`s wire input, so malformed frames/JSON close the connection rather than panic.
//!
//! # Handshake
//! The FIRST frame a client sends MUST deserialize as a [`Hello`]. On a version match the daemon
//! replies [`HelloOk`]; on a mismatch (or a non-`Hello` first frame) it closes without dispatching
//! any op — so a version-skewed client surfaces "engine unavailable" rather than mis-deserializing.
//!
//! # Error mapping (pre-resolved by the proto design)
//! - `EngineError::NotOnboarded` (surfaced as `EngineOpError::Open(NotOnboarded)`) → [`Response::NotOnboarded`].
//! - `EngineOpError::Busy(op)` → [`Response::Busy`] carrying the op name.
//! - everything else → [`Response::Err`] carrying the TYPED kind + the variant's INNER message —
//!   NEVER `e.to_string()`. Display prefixes ("engine error: ", "memory model unavailable: ", …)
//!   are applied exactly once, app-side, after the client rebuilds the typed variant; shipping a
//!   pre-rendered string would double-prefix every error (the Task 5 string-parity regression).
//!   The inner messages are already user-facing-safe (no DEK/key material, no other-user paths —
//!   they embed only the op name, an engine message, or a path the caller itself supplied).

use std::sync::Arc;

use bossclawd_proto::types::{
    ApplyResultWire, CloudProviderWire, EngineStateWire, EngineStatusWire, EvolveStatusMirror,
    EvolveTelemetryWire, MandateSummaryWire, MandateWriteSummaryWire, ModelStateWire,
    ModelStatusWire, PreviewDataWire, ProposalSummaryWire, ReasonerConfigWire, ReasonerModeWire,
    ReindexProgressWire,
};
use bossclawd_proto::{
    read_frame, write_frame, Hello, HelloOk, HitWire, OpErrorKindWire, Request, Response,
    PROTO_VERSION,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::engine::reason::{CloudProvider, ReasonerConfig, ReasonerMode};
use crate::engine::{
    ApplyResult, EngineError, EngineHandle, EngineOpError, EngineStatus, EvolveTelemetry, HitWithText,
    MandateSummary, MandateWriteSummary, PreviewData, ProposalSummary,
};

/// Run one connection to completion over any framed byte stream (a real `UnixStream` in
/// production, an in-process duplex in tests). Owns the read/write halves; the shared engine
/// is borrowed. Returns when the peer disconnects, sends a bad handshake, or an I/O error ends
/// the stream — never propagates a per-op engine error to the caller (those become `Response`s).
///
/// `read`/`write` are separate generics so callers can pass the two halves of a split stream.
pub async fn serve_connection<R, W>(
    engine: Arc<EngineHandle>,
    mut read: R,
    mut write: W,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // ── Handshake: the first frame MUST be a Hello with a matching version. ──
    // A read error / EOF here is a client that vanished before handshaking — just return.
    let first = read_frame(&mut read).await?;
    let Ok(hello) = serde_json::from_slice::<Hello>(&first) else {
        // Not a Hello frame (garbage or a Request sent first) → refuse without dispatching.
        // Best-effort error frame so the client sees a reason, then close. Protocol-level faults
        // have no typed engine variant → the generic `Core` kind.
        let _ = write_frame(
            &mut write,
            &encode(&protocol_err("expected Hello handshake".into())),
        )
        .await;
        return Ok(());
    };
    if hello.proto_version != PROTO_VERSION {
        // Version skew: close after an error frame. The client maps a closed/failed handshake to
        // "engine unavailable" rather than mis-deserializing a later frame against the wrong schema.
        let _ = write_frame(
            &mut write,
            &encode(&protocol_err(format!(
                "protocol version mismatch: daemon {PROTO_VERSION}, client {}",
                hello.proto_version
            ))),
        )
        .await;
        return Ok(());
    }
    let hello_ok = HelloOk { pid: std::process::id(), proto_version: PROTO_VERSION };
    let Ok(hello_ok_bytes) = serde_json::to_vec(&hello_ok) else {
        // Unreachable in practice (HelloOk is plain owned data), but on the impossible
        // serialization failure close cleanly rather than send an empty frame the client would
        // mis-parse — symmetry with `encode`'s never-panic fallback.
        return Ok(());
    };
    write_frame(&mut write, &hello_ok_bytes).await?;

    // ── Dispatch loop: one Request → one Response per frame, until the peer closes. ──
    loop {
        let frame = match read_frame(&mut read).await {
            Ok(f) => f,
            // Clean EOF or any read error → the connection is done. Not a fault.
            Err(_) => return Ok(()),
        };
        // A malformed Request frame must NOT panic or kill the daemon: reply Err and keep serving.
        let response = match serde_json::from_slice::<Request>(&frame) {
            Ok(req) => dispatch(&engine, req).await,
            Err(e) => protocol_err(format!("malformed request frame: {e}")),
        };
        // A write error means the peer went away mid-reply → end the connection.
        if write_frame(&mut write, &encode(&response)).await.is_err() {
            return Ok(());
        }
    }
}

/// A daemon protocol-level fault (bad handshake, malformed frame, encode failure) as a `Response`.
/// These have no typed engine variant, so they ride the generic `Core` kind — the client renders
/// them under Core's "engine error: " prefix, which is apt for a daemon-side fault.
fn protocol_err(message: String) -> Response {
    Response::Err { kind: OpErrorKindWire::Core, message }
}

/// Serialize a `Response` to frame bytes. `Response` is a plain serde enum of owned data, so
/// `to_vec` cannot realistically fail; on the impossible error we fall back to an `Err` frame
/// rather than `unwrap` (never panic on the serve path).
fn encode(resp: &Response) -> Vec<u8> {
    serde_json::to_vec(resp).unwrap_or_else(|_| {
        serde_json::to_vec(&protocol_err("response serialization failed".into()))
            .unwrap_or_default()
    })
}

/// Map one `Request` to its engine call and convert the result to a `Response`. This is the 1:1
/// dispatch table over the Task 0 inventory (29 wire ops). Each arm calls the matching
/// `EngineHandle` method with the `onboarded` flag the client supplied and folds the result
/// through [`op_result`] / [`unit_result`] / a dedicated converter.
async fn dispatch(engine: &Arc<EngineHandle>, req: Request) -> Response {
    match req {
        // ── Status (never-erroring on the engine side; always a Status response). ──
        Request::Status { onboarded } => Response::Status(status_wire(engine.status(onboarded).await)),

        // ── Grant / write-grant mutations (unit-returning). ──
        Request::AddGrant { onboarded, path } => unit_result(engine.add_grant(onboarded, path).await),
        Request::RevokeGrant { onboarded, path } => {
            unit_result(engine.revoke_grant(onboarded, path).await)
        }
        Request::SetFolderWritable { onboarded, path, on } => {
            unit_result(engine.set_folder_writable(onboarded, path, on).await)
        }

        // ── Read ops. ──
        Request::ListWritable { onboarded } => {
            op_result(engine.list_writable(onboarded).await, Response::ListWritable)
        }
        Request::ListGrants { onboarded } => op_result(engine.list_grants(onboarded).await, |grants| {
            Response::ListGrants(grants.into_iter().map(Into::into).collect())
        }),
        Request::ListFiles { onboarded } => op_result(engine.list_files(onboarded).await, |files| {
            Response::ListFiles(files.into_iter().map(Into::into).collect())
        }),

        // ── Ingest / recall. ──
        Request::RunIngest { onboarded } => {
            op_result(engine.run_ingest(onboarded).await, |r| Response::RunIngest(r.into()))
        }
        Request::Recall { onboarded, query, k } => {
            op_result(engine.recall(onboarded, query, k).await, |hits| {
                Response::Recall(hits.into_iter().map(hit_wire).collect())
            })
        }

        // ── Evolve. ──
        Request::EvolveOnce { onboarded } => {
            op_result(engine.evolve_once(onboarded).await, |r| Response::EvolveOnce(r.into()))
        }
        Request::EvolveStatus { onboarded } => {
            op_result(engine.evolve_status(onboarded).await, |(status, tel)| {
                Response::EvolveStatus {
                    status: EvolveStatusMirror::from(status),
                    telemetry: telemetry_wire(tel),
                }
            })
        }
        Request::SetEvolveEnabled { onboarded, enabled } => {
            unit_result(engine.set_evolve_enabled(onboarded, enabled).await)
        }
        Request::SetProposalsEnabled { onboarded, enabled } => {
            unit_result(engine.set_proposals_enabled(onboarded, enabled).await)
        }
        Request::SetMandatesEnabled { onboarded, enabled } => {
            unit_result(engine.set_mandates_enabled(onboarded, enabled).await)
        }
        Request::MandatesEnabled { onboarded } => {
            op_result(engine.mandates_enabled(onboarded).await, Response::MandatesEnabled)
        }

        // ── Mandate CRUD + activity. ──
        Request::AddMandate { onboarded, target, source_scope, recipe } => op_result(
            engine.add_mandate(onboarded, target, source_scope, recipe).await,
            |m| Response::AddMandate(mandate_summary_wire(m)),
        ),
        Request::RevokeMandate { onboarded, mandate_grant_id } => {
            unit_result(engine.revoke_mandate(onboarded, mandate_grant_id).await)
        }
        Request::ListMandates { onboarded } => {
            op_result(engine.list_mandates(onboarded).await, |ms| {
                Response::ListMandates(ms.into_iter().map(mandate_summary_wire).collect())
            })
        }
        Request::MandateWrites { onboarded } => {
            op_result(engine.mandate_writes(onboarded).await, |ws| {
                Response::MandateWrites(ws.into_iter().map(mandate_write_summary_wire).collect())
            })
        }

        // ── Review queue: proposals, preview, apply, undo, decline. ──
        Request::ListProposals { onboarded } => {
            op_result(engine.list_proposals(onboarded).await, |ps| {
                Response::ListProposals(ps.into_iter().map(proposal_summary_wire).collect())
            })
        }
        Request::ProposalPreview { onboarded, id } => op_result(
            engine.proposal_preview(onboarded, id).await,
            |p| Response::ProposalPreview(preview_data_wire(p)),
        ),
        Request::ApplyProposal { onboarded, id, acknowledged_loud } => op_result(
            engine.apply_proposal(onboarded, id, acknowledged_loud).await,
            |r| Response::ApplyProposal(apply_result_wire(r)),
        ),
        Request::UndoApply { onboarded, file_written_id } => {
            unit_result(engine.undo_apply(onboarded, file_written_id).await)
        }
        Request::DeclineProposal { onboarded, id, reason } => {
            unit_result(engine.decline_proposal(onboarded, id, reason).await)
        }

        // ── Teardown (identity reset). Returns `EngineError`, not `EngineOpError`. ──
        Request::Teardown => match engine.teardown().await {
            Ok(()) => Response::Ok,
            // Teardown never returns NotOnboarded (it deletes unconditionally); errors map through
            // the shared `EngineError` chokepoint below.
            Err(e) => engine_error_response(e),
        },

        // ── Reasoner config (Milestone D). ──
        Request::GetReasonerConfig { onboarded } => {
            // `reasoner_config_or_default` is infallible (fail-safe Local default on any error).
            Response::ReasonerConfig(reasoner_config_wire(
                engine.reasoner_config_or_default(onboarded).await,
            ))
        }
        Request::GetReasonerReady { onboarded } => {
            // `reasoner_ready_or_false` is infallible (fail-closed false on any error).
            Response::ReasonerReady(engine.reasoner_ready_or_false(onboarded).await)
        }
        Request::SetReasonerConfig { onboarded, config } => {
            unit_result(engine.set_reasoner_config(onboarded, config).await)
        }
        Request::EnableCloudReasoner { onboarded, config } => {
            unit_result(engine.enable_cloud_reasoner(onboarded, config).await)
        }

        // ── Language pack (rung 2). ──
        Request::SetActiveModel { onboarded, model_id, safetensors_sha } => {
            unit_result(engine.set_active_model(onboarded, model_id, safetensors_sha).await)
        }
        Request::ModelStatus { onboarded } => {
            Response::ModelStatus(model_status_wire(engine.model_status(onboarded).await))
        }
    }
}

// ── Result folding ─────────────────────────────────────────────────────────

/// Fold an `EngineOpError`-returning op that yields a value: apply `ok` to the value on success,
/// else map the error to the right signal (`NotOnboarded` / `Busy` / `Err`).
fn op_result<T>(result: Result<T, EngineOpError>, ok: impl FnOnce(T) -> Response) -> Response {
    match result {
        Ok(v) => ok(v),
        Err(e) => op_error_response(e),
    }
}

/// Fold a unit-returning `EngineOpError` op: `Ok(())` → `Response::Ok`, else the mapped signal.
fn unit_result(result: Result<(), EngineOpError>) -> Response {
    match result {
        Ok(()) => Response::Ok,
        Err(e) => op_error_response(e),
    }
}

/// The single error-mapping chokepoint (pre-resolved by the proto design):
/// - `Open(NotOnboarded)` → `Response::NotOnboarded` (a signal, not a fault — the UI shows onboarding);
/// - `Busy(op)` → `Response::Busy(op)` (a serialized ingest/evolve is already running);
/// - `Open(other)` → the shared [`engine_error_response`] `EngineError` mapping;
/// - every other variant → its own [`OpErrorKindWire`] carrying the INNER message — never
///   `e.to_string()`, so the client rebuilds the exact typed variant and Display prefixes are
///   applied exactly once, app-side (the string-parity contract).
///
/// Exhaustive on purpose (no `_` wildcard): a new `EngineOpError` variant must force a deliberate
/// kind mapping here at compile time rather than silently collapsing into a generic string.
///
/// `pub` so the desktop client's string-parity regression test can drive the REAL daemon-side
/// mapping (typed error → wire encode/decode → client mapping → Display) without a socket.
pub fn op_error_response(e: EngineOpError) -> Response {
    let (kind, message) = match e {
        EngineOpError::Open(open) => return engine_error_response(open),
        EngineOpError::Busy(op) => return Response::Busy(op.to_string()),
        EngineOpError::Core(m) => (OpErrorKindWire::Core, m),
        EngineOpError::Embedder(m) => (OpErrorKindWire::Embedder, m),
        EngineOpError::Reasoner(m) => (OpErrorKindWire::Reasoner, m),
        EngineOpError::Stale(m) => (OpErrorKindWire::Stale, m),
        EngineOpError::Revoked(m) => (OpErrorKindWire::Revoked, m),
        EngineOpError::NeedsLoudConfirm(m) => (OpErrorKindWire::NeedsLoudConfirm, m),
        EngineOpError::Rejected(m) => (OpErrorKindWire::Rejected, m),
        EngineOpError::Join(m) => (OpErrorKindWire::Join, m),
    };
    Response::Err { kind, message }
}

/// The `EngineError` mapping — shared by the op path (via `EngineOpError::Open`) and teardown's
/// `Result<(), EngineError>`. Same typed-kind contract as [`op_error_response`]; exhaustive for the
/// same reason. `pub` for the same parity-test reason.
pub fn engine_error_response(e: EngineError) -> Response {
    let (kind, message) = match e {
        // A signal, not a fault (teardown itself never produces it, but the op-path `Open` gate does).
        EngineError::NotOnboarded => return Response::NotOnboarded,
        // Unit-shaped: no inner message to carry.
        EngineError::KeystoreInconsistent => (OpErrorKindWire::KeystoreInconsistent, String::new()),
        EngineError::KeystoreDbMismatch(m) => (OpErrorKindWire::KeystoreDbMismatch, m),
        EngineError::Vault(m) => (OpErrorKindWire::Vault, m),
        // `EngineError::Join` and `EngineOpError::Join` render identically, so the kind is shared.
        EngineError::Join(m) => (OpErrorKindWire::Join, m),
    };
    Response::Err { kind, message }
}

// ── Family-1 mirrors have `From`; the conversions above use `.into()`. ──
// ── Family-2 wire structs have NO `From` in proto (the desktop crate isn't a proto dep), so the
//    daemon maps them field-by-field here (the fields match 1:1 with the copied engine types). ──

fn status_wire(s: EngineStatus) -> EngineStatusWire {
    EngineStatusWire {
        state: match s.state {
            crate::engine::EngineState::NotOnboarded => EngineStateWire::NotOnboarded,
            crate::engine::EngineState::Ready => EngineStateWire::Ready,
            crate::engine::EngineState::KeystoreInconsistent => EngineStateWire::KeystoreInconsistent,
            crate::engine::EngineState::KeystoreDbMismatch => EngineStateWire::KeystoreDbMismatch,
            crate::engine::EngineState::ChainFailed => EngineStateWire::ChainFailed,
        },
        event_count: s.event_count,
        chain_ok: s.chain_ok,
    }
}

/// Map the daemon's `(ModelState, progress)` (rung 2) to the wire `ModelStatusWire` the Settings card
/// polls. Exhaustive on `ModelState` on purpose: a new state variant must force a wire mapping here.
fn model_status_wire(
    (state, reindex, active_model_id): (
        crate::engine::embed::ModelState,
        Option<(u64, u64)>,
        String,
    ),
) -> ModelStatusWire {
    let state = match state {
        crate::engine::embed::ModelState::Ok => ModelStateWire::Ok,
        crate::engine::embed::ModelState::Missing { expected } => {
            ModelStateWire::Missing { expected }
        }
        crate::engine::embed::ModelState::Mismatch { expected, loaded } => {
            ModelStateWire::Mismatch { expected, loaded }
        }
        crate::engine::embed::ModelState::Failed { reason } => ModelStateWire::Failed { reason },
    };
    ModelStatusWire {
        state,
        reindex: reindex.map(|(done, total)| ReindexProgressWire { done, total }),
        active_model_id: Some(active_model_id),
    }
}

/// A recall hit + its hydrated snippet text. The core `Hit` → `HitMirror` conversion is proto's
/// `From`; the snippet pairs alongside it exactly as the desktop's `HitWithText` does.
fn hit_wire(h: HitWithText) -> HitWire {
    HitWire { hit: h.hit.into(), text: h.text }
}

fn telemetry_wire(t: EvolveTelemetry) -> EvolveTelemetryWire {
    EvolveTelemetryWire {
        last_tick_ms: t.last_tick_ms,
        error_count: t.error_count,
        last_error: t.last_error,
        last_tainted_snippets: t.last_tainted_snippets,
    }
}

fn proposal_summary_wire(p: ProposalSummary) -> ProposalSummaryWire {
    ProposalSummaryWire {
        id: p.id,
        target: p.target,
        op: p.op,
        new_content_hash: p.new_content_hash,
        rationale: p.rationale,
        requires_loud_modal: p.requires_loud_modal,
        producer: p.producer,
    }
}

fn mandate_summary_wire(m: MandateSummary) -> MandateSummaryWire {
    MandateSummaryWire {
        mandate_grant_id: m.mandate_grant_id,
        target: m.target,
        source_scope: m.source_scope,
        recipe: m.recipe,
        granted_at: m.granted_at,
        revoked: m.revoked,
    }
}

fn mandate_write_summary_wire(w: MandateWriteSummary) -> MandateWriteSummaryWire {
    MandateWriteSummaryWire {
        file_written_id: w.file_written_id,
        target: w.target,
        written_at: w.written_at,
        undone: w.undone,
    }
}

fn preview_data_wire(p: PreviewData) -> PreviewDataWire {
    PreviewDataWire {
        path: p.path,
        folder: p.folder,
        rationale: p.rationale,
        op: p.op,
        old_text: p.old_text,
        new_text: p.new_text,
        requires_loud_modal: p.requires_loud_modal,
        taint: p.taint,
    }
}

fn apply_result_wire(r: ApplyResult) -> ApplyResultWire {
    ApplyResultWire { file_written_id: r.file_written_id }
}

fn reasoner_config_wire(c: ReasonerConfig) -> ReasonerConfigWire {
    ReasonerConfigWire {
        mode: match c.mode {
            ReasonerMode::Local => ReasonerModeWire::Local,
            ReasonerMode::Cloud => ReasonerModeWire::Cloud,
        },
        provider: match c.provider {
            CloudProvider::Anthropic => CloudProviderWire::Anthropic,
            CloudProvider::OpenAiCompat => CloudProviderWire::OpenAiCompat,
            CloudProvider::Gemini => CloudProviderWire::Gemini,
        },
        model: c.model,
        base_url: c.base_url,
    }
}

// ── Accept loop (shared by main.rs and spawn_for_test) ──────────────────────

/// Accept connections forever on `listener`, serving each on its own task with a clone of the
/// shared engine. Used by BOTH the production `main.rs` and [`spawn_for_test`], so the roundtrip
/// tests exercise the exact production accept path — including the peer-uid check below.
///
/// # Same-uid trust boundary (egress-security review M-1)
/// The socket is already `0600`, which excludes other uids at the FS layer; as defense-in-depth
/// the loop ALSO reads the connecting peer's credentials (`SO_PEERCRED` on Linux /
/// `LOCAL_PEERCRED` on macOS, via tokio's safe `peer_cred()` binding — no `unsafe`, no platform
/// `cfg`) and rejects, with a stderr log, any peer whose uid differs from the daemon's effective
/// uid. Unreadable credentials are rejected fail-closed. This makes the trust boundary explicit
/// and pre-pays M1b, when non-app clients (Claude Code) first connect. Within the boundary any
/// same-uid process can invoke every wire op — per-op authorization is deferred to M1b (spec,
/// Safety section).
///
/// Never returns under normal operation: accept errors are transient (e.g. fd exhaustion) and
/// are logged + survived; a panic in one connection task cannot take the loop down (task
/// isolation, and `serve_connection` never unwraps wire input).
pub async fn run_accept_loop(engine: Arc<EngineHandle>, listener: tokio::net::UnixListener) {
    let our_uid = nix::unistd::geteuid().as_raw();
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                // Peer-uid check BEFORE any frame is read: same effective uid only.
                match stream.peer_cred() {
                    Ok(cred) if cred.uid() == our_uid => {}
                    Ok(cred) => {
                        eprintln!(
                            "bossclawd: rejected connection from uid {} (daemon uid {our_uid})",
                            cred.uid()
                        );
                        continue; // stream drops here → connection closed, nothing served
                    }
                    Err(e) => {
                        eprintln!(
                            "bossclawd: rejected connection with unreadable peer credentials: {e}"
                        );
                        continue;
                    }
                }
                let engine = engine.clone();
                tokio::spawn(async move {
                    let (read, write) = stream.into_split();
                    // Per-connection errors are logged, never fatal to the daemon.
                    if let Err(e) = serve_connection(engine, read, write).await {
                        eprintln!("bossclawd: connection ended with I/O error: {e}");
                    }
                });
            }
            Err(e) => {
                // An accept error is transient; log and keep serving.
                eprintln!("bossclawd: accept error: {e}");
            }
        }
    }
}

// ── Test-spawn helper ────────────────────────────────────────────────────────
// Lives in the LIB (not the bin) behind `#[doc(hidden)] pub` because integration tests
// (`tests/roundtrip.rs`) cannot see `#[cfg(test)]` items of the bin — they link the lib.

/// Spin a real `bossclawd` accept loop on `sock_path` backed by a temp engine home at `home`,
/// wired with an in-memory test vault + mock embedder/reasoner (NEVER the OS keychain — a
/// keychain-ACL prompt hangs CI forever). Returns immediately; the loop runs on the test's
/// tokio runtime until the process ends (the temp dir cleanup drops the socket).
///
/// The `onboarded` gate is per-request (the client sends it), so this helper does not create an
/// identity file; a test drives onboarded/not-onboarded purely through the `Request` flag.
#[doc(hidden)]
#[cfg(any(test, feature = "test-helpers"))]
pub async fn spawn_for_test(sock_path: std::path::PathBuf, home: std::path::PathBuf) {
    use tokio::net::UnixListener;

    use std::os::unix::fs::PermissionsExt;

    let engine = Arc::new(test_engine(home));
    // Bind the listener BEFORE returning so the client can connect without a race.
    let listener = UnixListener::bind(&sock_path).expect("bind test socket");
    // Pin the socket to 0600 (owner-only), matching the production `bind_socket_0600` so tests can
    // assert the confidentiality mode over a real socket file.
    std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))
        .expect("chmod test socket 0600");
    // The PRODUCTION accept loop (shared with main.rs), so the roundtrip tests exercise the real
    // accept path — including the same-uid peer-credential check (the test client connects from
    // this same process, so it passes the check).
    tokio::spawn(run_accept_loop(engine, listener));
}

/// Build a hermetic `EngineHandle` for tests: in-memory vault + mock embedder + mock reasoner.
/// Uses `bossclaw_core::MockEmbedder`/`ScriptedReasoner` (public, non-cfg-gated in core) so the
/// integration test — which can't reach the lib's `#[cfg(test)]` mock providers — still gets a
/// keychain-free engine.
#[cfg(any(test, feature = "test-helpers"))]
pub fn test_engine(home: std::path::PathBuf) -> EngineHandle {
    test_engine_with_embedder(home, Arc::new(TestEmbedderProvider))
}

/// Like [`test_engine`] but with a CALLER-SUPPLIED embedder provider. Added for `memharness`
/// (memory-strategy Phase 0): the harness must measure the REAL production embedder
/// (`engine::embed::ResourceModel2Vec`), not the dim-8 mock — while keeping the in-memory
/// vault (keychain-free) and the scripted reasoner (no evolve, no reasoner egress). Behind
/// `test-helpers` like its sibling; never reaches production builds.
#[cfg(any(test, feature = "test-helpers"))]
pub fn test_engine_with_embedder(
    home: std::path::PathBuf,
    embedder: Arc<dyn crate::engine::embed::EmbedderProvider>,
) -> EngineHandle {
    EngineHandle::new(Arc::new(TestVault::default()), home, embedder, Arc::new(TestReasonerProvider))
}

/// An in-memory `SecretsVault` for tests — NEVER touches the OS keychain.
#[cfg(any(test, feature = "test-helpers"))]
#[derive(Default)]
struct TestVault {
    store: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[cfg(any(test, feature = "test-helpers"))]
impl crate::secrets::SecretsVault for TestVault {
    fn set(&self, k: &str, v: &str) -> Result<(), String> {
        self.store.lock().unwrap().insert(k.into(), v.into());
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

/// A mock embedder provider yielding `bossclaw_core::MockEmbedder` (public in core).
#[cfg(any(test, feature = "test-helpers"))]
struct TestEmbedderProvider;

#[cfg(any(test, feature = "test-helpers"))]
impl crate::engine::embed::EmbedderProvider for TestEmbedderProvider {
    fn embedder(&self) -> Result<Arc<dyn bossclaw_core::Embedder>, EngineOpError> {
        Ok(Arc::new(bossclaw_core::MockEmbedder::new(8)))
    }
}

/// A mock reasoner provider yielding a `bossclaw_core::ScriptedReasoner` (public in core).
#[cfg(any(test, feature = "test-helpers"))]
struct TestReasonerProvider;

#[cfg(any(test, feature = "test-helpers"))]
impl crate::engine::reason::ReasonerProvider for TestReasonerProvider {
    fn reasoner(&self) -> Result<Arc<dyn bossclaw_core::Reasoner>, EngineOpError> {
        Ok(Arc::new(bossclaw_core::ScriptedReasoner::new("mock-reasoner")))
    }
}
