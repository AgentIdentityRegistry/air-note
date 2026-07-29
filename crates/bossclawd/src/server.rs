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

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bossclawd_proto::types::{
    ApplyResultWire, CloudProviderWire, ConflictProposalWire, EngineStateWire, EngineStatusWire,
    EvolveStatusMirror, EvolveTelemetryWire, MandateSummaryWire, MandateWriteSummaryWire,
    ModelStateWire, ModelStatusWire, NoteWire, PreviewDataWire, ProposalSummaryWire,
    ReasonerConfigWire, ReasonerModeWire, RecallStatsWire, ReindexProgressWire, SessionDetailWire,
    SessionSummaryWire,
};
use bossclawd_proto::{
    read_frame, write_frame, BundleLimitWire, ExportSelectionWire, Hello, HelloOk, HitWire,
    OpErrorKindWire, Request, Response, RetireTarget, Role, MAX_FRAME, PROTO_VERSION,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::capture::paths::{claude_projects_root, open_transcript_confined, valid_session_id};
use crate::capture::render::{render_bounds, render_transcript};
use crate::capture::store::{
    capture_body, delete_capture, read_capture_markdown, store_capture, CaptureIdentity,
    CaptureStoreError, CAPTURE_TOOL,
};
use crate::engine::reason::{CloudProvider, ReasonerConfig, ReasonerMode};
use crate::engine::{
    ApplyResult, EngineError, EngineHandle, EngineOpError, EngineStatus, EvolveTelemetry, HitWithText,
    MandateSummary, MandateWriteSummary, PreviewData, ProposalSummary,
};

/// Per-connection budget for the two expensive capture pokes (`CaptureNotify` + `Snapshot`): at
/// most [`CAPTURE_POKE_RATE`] of them per [`CAPTURE_POKE_WINDOW`]. The cheap, frequently-expected
/// `Recall`/`Remember` are NOT counted. See [`CaptureRateLimiter`].
const CAPTURE_POKE_RATE: usize = 10;
/// The fixed window over which [`CAPTURE_POKE_RATE`] is measured.
const CAPTURE_POKE_WINDOW: Duration = Duration::from_secs(60);

/// A per-connection fixed-window token bucket for the capture pokes. Each [`serve_connection`] owns
/// one, so the budget bounds a SINGLE misbehaving connection — of ANY role. The limiter runs BEFORE
/// the role gate, so it also caps an `App` connection (the `GUEST_`-scoped framing would overstate
/// it); in practice only the `MemoryClient` guest fires these ops in a loop.
///
/// LIMITATION (documented on purpose): production MCP clients (the `air-memory-mcp` adapter)
/// reconnect per call, so a fresh connection gets a fresh bucket — this does NOT bound
/// reconnect-spam, which is scoped by the same-uid trust boundary + the socket's `0600` mode, not by
/// this limiter. It is a cheap guard against one chatty/looping connection, checked at the daemon
/// boundary (a clock read here is fine — the dispatch core stays clock-free).
struct CaptureRateLimiter {
    window_start: Instant,
    count: usize,
}

impl CaptureRateLimiter {
    fn new(now: Instant) -> Self {
        Self { window_start: now, count: 0 }
    }

    /// Record one capture poke at `now`. Returns `true` if it is within the per-window cap (so the
    /// caller dispatches it), `false` if it exceeds the cap (so the caller refuses it). The window
    /// is a simple fixed window: once [`CAPTURE_POKE_WINDOW`] has elapsed the count resets.
    fn check(&mut self, now: Instant) -> bool {
        if now.duration_since(self.window_start) >= CAPTURE_POKE_WINDOW {
            self.window_start = now;
            self.count = 0;
        }
        self.count += 1;
        self.count <= CAPTURE_POKE_RATE
    }
}

/// The two capture pokes the rate limiter counts (the expensive/side-effect-ish guest ops). Kept as
/// a tiny predicate so the loop reads cleanly and A11's `Snapshot` handler shares the exact set.
fn is_rate_limited_op(req: &Request) -> bool {
    matches!(req, Request::CaptureNotify { .. } | Request::Snapshot { .. })
}

/// The clean per-connection rate-limit refusal. Rides the [`OpErrorKindWire::Rejected`] kind (a
/// policy refusal, like the path/id rejects) with a message that names the rate limit; it echoes no
/// request content.
fn rate_limited_response() -> Response {
    Response::Err {
        kind: OpErrorKindWire::Rejected,
        message: "rate limit exceeded for capture pokes on this connection".to_string(),
    }
}

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
    // The role the connection requested, captured once BEFORE the loop so no borrow of `hello`
    // spans a `.await`. Every op on this connection is authorized against it (U2).
    let role = hello.role;
    // Per-connection capture-poke budget (reset by construction on every new connection).
    let mut rate_limiter = CaptureRateLimiter::new(Instant::now());

    // ── Dispatch loop: one Request → one Response per frame, until the peer closes. ──
    loop {
        let frame = match read_frame(&mut read).await {
            Ok(f) => f,
            // Clean EOF or any read error → the connection is done. Not a fault.
            Err(_) => return Ok(()),
        };
        // A malformed Request frame must NOT panic or kill the daemon: reply Err and keep serving.
        let response = match serde_json::from_slice::<Request>(&frame) {
            // Rate-limit the two expensive capture pokes BEFORE dispatch (a clock read here is fine —
            // the daemon boundary). Non-capture ops never touch the bucket (short-circuit).
            Ok(req) if is_rate_limited_op(&req) && !rate_limiter.check(Instant::now()) => {
                rate_limited_response()
            }
            Ok(req) => dispatch(&engine, role, req).await,
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

/// A per-op authorization refusal (U2). Rides the [`OpErrorKindWire::NotPermitted`] kind; the
/// message is generic (it does not echo the op) so nothing about the refused request leaks.
fn not_permitted_response() -> Response {
    Response::Err {
        kind: OpErrorKindWire::NotPermitted,
        message: "operation not permitted for this connection's role".to_string(),
    }
}

/// Rewrite a guest (`MemoryClient`) request's `onboarded` flag to the daemon's own onboarding
/// truth so a guest can never assert onboarding to force a keystore mint. Returns `None` for any
/// request that is NOT an explicitly-handled guest op — the caller then refuses it fail-closed.
/// This deliberately does NOT pass unknown variants through: if a future task widens
/// `Role::allows` to admit a new guest op without adding it here, that op is REFUSED (safe)
/// rather than silently trusting the client's self-asserted `onboarded` flag. `Role::allows` and
/// this function must stay in sync; the fail-closed `None` makes any drift safe.
fn override_onboarding_for_guest(req: Request, onboarded: bool) -> Option<Request> {
    match req {
        Request::Recall { query, k, .. } => Some(Request::Recall { onboarded, query, k }),
        Request::Remember { text, .. } => Some(Request::Remember { onboarded, text }),
        // SP3 session-capture pokes — the guest surface widens by EXACTLY these two (`Role::allows`
        // already admits them). Rewrite `onboarded` to the daemon's OWN truth so a guest can never
        // assert onboarding to force a keystore mint — identical treatment to Recall/Remember. A10
        // lands the `CaptureNotify` dispatch arm; A11 lands `Snapshot`'s (the guest plumbing is done
        // here for both so A11 reuses it).
        Request::CaptureNotify { session_id, transcript_path, .. } => {
            Some(Request::CaptureNotify { onboarded, session_id, transcript_path })
        }
        Request::Snapshot { project, source, session_id, transcript_path, .. } => {
            Some(Request::Snapshot { onboarded, project, source, session_id, transcript_path })
        }
        // Rung-3 Phase-3 (I8 relaxation): the two resolve ops are guest ops (`Role::allows` admits them), so
        // they need explicit passthrough arms — a missing arm → None → refused. Rewrite `onboarded` to the
        // daemon's OWN truth like every other guest op.
        Request::ListConflicts { .. } => Some(Request::ListConflicts { onboarded }),
        Request::ResolveConflict { proposal_id, action, .. } => {
            Some(Request::ResolveConflict { onboarded, proposal_id, action })
        }
        _ => None,
    }
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
async fn dispatch(engine: &Arc<EngineHandle>, role: Role, req: Request) -> Response {
    // ── Per-op authorization (U2 / I1), fail-closed: a role may invoke only its allow-listed ops. ──
    if !role.allows(&req) {
        return not_permitted_response();
    }
    // A guest-pass (`MemoryClient`) must not be able to ASSERT onboarding — that would let it force
    // a keystore mint / brain creation. The daemon computes onboarding itself for that role; `App`
    // keeps its self-asserted flag (I3 — the app is unchanged).
    let req = match role {
        Role::App => req,
        Role::MemoryClient => match override_onboarding_for_guest(req, engine.is_onboarded_local()) {
            Some(req) => req,
            None => return not_permitted_response(),
        },
    };
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
            // Keep the query for best-effort telemetry (the engine call consumes it). A short-string
            // clone is negligible next to the embed + ANN + SQL the recall itself does.
            let telemetry_query = query.clone();
            match engine.recall(onboarded, query, k).await {
                Ok(hits) => {
                    record_recall_telemetry(engine, &telemetry_query, &hits);
                    Response::Recall(hits.into_iter().map(hit_wire).collect())
                }
                Err(e) => op_error_response(e),
            }
        }
        Request::Remember { onboarded, text } => {
            op_result(engine.remember(onboarded, text).await, Response::Remember)
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

        // ── SP3 §4c: IMMEDIATE capture (A10). Guest-reachable, capture-gated, confined render. ──
        Request::CaptureNotify { session_id, transcript_path, .. } => {
            capture_notify(engine, session_id, transcript_path).await
        }

        // ── SP3 §5: orientation SNAPSHOT (A11). Guest-reachable, NOT capture-gated (a read over
        // already-captured memory + a live transcript is useful even when ongoing capture is off).
        // The build is infallible: it always returns a valid, fenced, sanitized, ≤4 KB snapshot —
        // minimal (just the recall affordance) at worst — so the SessionStart hook can NEVER break
        // (I1). Every memory-derived field is neutralized + fenced inside the builder. ──
        Request::Snapshot { project, source, session_id, transcript_path, .. } => {
            Response::Snapshot(
                crate::capture::snapshot::build(engine, &project, &source, session_id, transcript_path)
                    .await,
            )
        }

        // ── SP3 §8: recall-miss telemetry read (A12). App-only — `Role::allows` denies it for a
        // guest (`MemoryClient`), so no guest onboarding plumbing is needed here. ──
        Request::RecallStats { .. } => Response::RecallStats(recall_stats(engine)),

        // ── SP3 §7/§9: forget + listing (A13). ALL App-only — `Role::allows` denies each of
        // these for a `MemoryClient` BEFORE dispatch (the guest never reaches here), so no guest
        // onboarding plumbing is needed. The SP3 read/mutate family resolves onboarding from the
        // daemon's OWN identity (never a client flag); the two capture-flag ops keep the App's
        // self-asserted flag (I3 — the app is trusted/unchanged). ──
        Request::ListSessions { .. } => op_result(engine.current_sessions().await, |sessions| {
            Response::ListSessions(sessions.into_iter().map(session_summary_wire).collect())
        }),
        Request::GetSession { session_id, .. } => get_session(engine, session_id).await,
        Request::DeleteSession { session_id, .. } => delete_session(engine, session_id).await,
        Request::ListNotes { .. } => op_result(engine.current_notes().await, |notes| {
            Response::ListNotes(notes.into_iter().map(note_wire).collect())
        }),
        Request::SupersedeNote { event_id, text, .. } => {
            // DELIBERATE asymmetry vs GetSession/DeleteSession (which use id-free reject messages):
            // SupersedeNote lets the core primitive's typed `Rejected` message flow through WITH the
            // App-supplied `event_id` (no-such-event / not-a-note / already-superseded). This is safe
            // AND diagnostically useful — NOT the I4 hostile-capture surface: this is an App-only op
            // (guest-denied at the role gate), and the `event_id` is a daemon-minted ULID the App
            // itself obtained via `ListNotes` and echoes back over the same trusted App socket (never
            // an attacker-influenced capture path). So the id is worth surfacing to disambiguate the
            // reject, whereas GetSession/DeleteSession keep static messages for symmetry with the I4
            // capture-path rejects that DO carry hostile bytes.
            op_result(engine.supersede_note(event_id, text).await, Response::Superseded)
        }
        // ── Rung 3 §7.3: retire / unretire (App-only — `Role::allows` denies both for a
        // `MemoryClient`, so the guest never reaches here; onboarding is the daemon's OWN verdict,
        // resolved inside the engine wrapper). ──
        Request::RetireMemory { target, .. } => match target {
            RetireTarget::Note { event_id } => {
                op_result(engine.retire_memory(event_id).await, Response::Retired)
            }
            // Rung 3 §7.2 passage-granularity retire: append a `passage_retired` marker for the
            // (session_id, passage_id). The engine wrapper resolves onboarding and folds an unknown
            // session / out-of-range / already-retired passage to a typed `Rejected`. (Passage UNretire
            // is core-only in Phase 1 — the wire `Unretire` op is note-only — so there is no arm for it.)
            RetireTarget::Passage { session_id, passage_id } => {
                op_result(engine.retire_passage(session_id, passage_id).await, Response::Retired)
            }
        },
        Request::Unretire { retired_event_id, .. } => {
            op_result(engine.unretire(retired_event_id).await, Response::Retired)
        }
        // ── Rung 3 §Phase-3: conflict resolution (guest-reachable — the I8 relaxation). ──
        Request::ListConflicts { onboarded } => {
            op_result(engine.list_conflicts(onboarded).await, |rows| {
                // Sanitize DAEMON-SIDE (MINOR-1: air-memory-mcp has bossclawd only as a dev-dep and cannot
                // call `sanitize_injected` in prod). Belt-and-suspenders even though the fields are ids +
                // the content-free `templated_why` today: a future change that puts model text in `why`
                // can never regress into an unfenced injection.
                Response::ListConflicts(rows.into_iter().map(sanitize_conflict_row).collect())
            })
        }
        Request::ResolveConflict { onboarded, proposal_id, action } => {
            op_result(engine.resolve_conflict(onboarded, proposal_id, action.into()).await, |outcome| {
                let (applied, marker_event_id) = match outcome {
                    bossclaw_core::ResolveOutcome::Applied(id) => (true, Some(id)),
                    bossclaw_core::ResolveOutcome::NoOp => (false, None),
                };
                Response::ResolveConflict { applied, marker_event_id }
            })
        }
        Request::SetCaptureEnabled { onboarded, enabled, backfill } => {
            // `at` is the ONLY clock read on the dispatch core — the daemon boundary supplies it so
            // core stays clock-free (the Integrations toggle path carries `backfill` from the App).
            unit_result(engine.set_capture_enabled(onboarded, enabled, backfill, now_unix_secs()).await)
        }
        Request::CaptureEnabled { onboarded } => {
            op_result(engine.capture_enabled(onboarded).await, Response::CaptureEnabled)
        }
        Request::SetReflectEnabled { onboarded, enabled } => {
            unit_result(engine.set_reflect_enabled(onboarded, enabled).await)
        }
        Request::ReflectEnabled { onboarded } => {
            op_result(engine.reflect_enabled(onboarded).await, Response::ReflectEnabled)
        }
        // ── Rung-5 SP-V1 §2.3/§2.4: brain-key read, binding write, sealed export. ALL App-only —
        // `Role::allows` denies each for a `MemoryClient` BEFORE dispatch (a guest may read
        // memories, but never read the brain key, write the identity binding, or seal an export),
        // so no guest onboarding plumbing is needed here. The App keeps its self-asserted
        // `onboarded` flag (I3), exactly like the reflect/capture toggles above. ──
        Request::BrainVerifyingKey { onboarded } => {
            op_result(engine.brain_verifying_key(onboarded).await, Response::BrainVerifyingKey)
        }
        Request::SetBinding { onboarded, attestation } => {
            unit_result(engine.set_binding(onboarded, attestation).await)
        }
        Request::ExportBundle { onboarded, selection } => {
            export_dispatch(engine, onboarded, selection).await
        }
    }
}

/// Rung-5 §2.4 — the `ExportBundle` seam: coarse size guard → bounded session-body reads → build +
/// seal → the authoritative pre-frame size guard.
///
/// The estimate runs FIRST so an over-cap selection costs no body read and no seal. The core export
/// then runs its own ordering (`verify_chain` → empty → binding → per-class guards → build), so a
/// log that does not verify can never be sealed from.
async fn export_dispatch(
    engine: &EngineHandle,
    onboarded: bool,
    selection: ExportSelectionWire,
) -> Response {
    use bossclaw_core::log::{ExportError, ExportSelection, MAX_EXPORT_BYTES};

    let selection = ExportSelection {
        note_event_ids: selection.note_event_ids,
        session_event_ids: selection.session_event_ids,
        ingest_event_ids: selection.ingest_event_ids,
        description: selection.description,
        // The daemon boundary stamps the export time so core stays clock-free and a client can
        // never backdate a seal.
        created_at: crate::engine::now_rfc3339(),
    };

    // 1. Coarse, pre-body, pre-seal guard.
    match engine.estimate_export_bytes(onboarded, selection.clone()).await {
        Ok(bytes) if bytes > MAX_EXPORT_BYTES => {
            return Response::BundleTooLarge {
                bytes,
                max: MAX_EXPORT_BYTES,
                limit: BundleLimitWire::ExportBytes,
            }
        }
        Ok(_) => {}
        Err(e) => return op_error_response(e),
    }

    // 2. Read the selected session bodies (bounded by `read_capture_markdown`, and confined to the
    // daemon-authored `sessions/` dir — the SAME defense-in-depth check `get_session` applies), then
    // STRIP THE FRONT MATTER.
    //
    // A-N1, and the reason `capture_body` is called here: `read_capture_markdown` returns the WHOLE
    // document, and `compose_document`'s front-matter block carries the project PATH, the
    // `session_id`, and the body `sha256`. A session is disclosed as a SEAL_VOUCHED item — content
    // ONLY — and core already reduces the `display` project to its folder name (owner ruling
    // `d0c5b14`); handing over the unstripped document would smuggle the very path that ruling
    // removed straight back in through `content`.
    //
    // A body we cannot read (unconfined path, out-of-band deletion, unreadable file) is simply LEFT
    // ABSENT from the map: core already substitutes its placeholder for a missing id, so the export
    // still completes and the placeholder string stays single-sourced in core.
    let mut session_bodies = std::collections::HashMap::new();
    if !selection.session_event_ids.is_empty() {
        let sessions = match engine.current_sessions().await {
            Ok(s) => s,
            Err(e) => return op_error_response(e),
        };
        let sessions_dir = engine.data_dir().map(|d| d.join("sessions"));
        for id in &selection.session_event_ids {
            let Some(captured) = sessions.iter().find(|c| &c.event_id == id) else { continue };
            let confined = sessions_dir
                .as_ref()
                .is_some_and(|dir| Path::new(&captured.path).starts_with(dir));
            if !confined {
                continue;
            }
            if let Ok(document) = read_capture_markdown(Path::new(&captured.path)) {
                session_bodies.insert(id.clone(), capture_body(&document).to_string());
            }
        }
    }

    // 3. Build + seal, then map each typed refusal to its own wire signal.
    match engine.export_bundle(onboarded, selection, session_bodies).await {
        // An open/join failure keeps flowing through the shared chokepoint, so a not-onboarded
        // brain still answers with the onboarding SIGNAL rather than a fault.
        Err(e) => op_error_response(e),
        Ok(Ok(text)) => {
            let response = Response::Bundle(text);
            frame_guard(&response).unwrap_or(response)
        }
        Ok(Err(ExportError::BundleTooLarge { bytes, max })) => {
            // Core measured the `.airmem` TEXT against its own byte belt, so this refusal carries
            // TEXT semantics — NOT the escaped-frame semantics `frame_guard` reports.
            Response::BundleTooLarge { bytes, max, limit: BundleLimitWire::ExportBytes }
        }
        Ok(Err(ExportError::ChainInvalid)) => {
            export_rejected("cannot export: memory chain does not verify")
        }
        Ok(Err(ExportError::EmptySelection)) => export_rejected("empty selection"),
        Ok(Err(ExportError::BindingUnavailable)) => export_rejected("no identity binding stored"),
        Ok(Err(ExportError::NotExportable(reason))) => export_rejected(&reason),
        Ok(Err(ExportError::Core(message))) => {
            Response::Err { kind: OpErrorKindWire::Core, message }
        }
    }
}

/// The AUTHORITATIVE pre-frame size bound. `None` = the response fits and may be sent as-is;
/// `Some(refusal)` = send this typed [`Response::BundleTooLarge`] instead.
///
/// Why it exists at all: the frame written on the wire is `serde_json::to_vec(&response)`, which
/// JSON-ESCAPES the already-JSON `.airmem`. That overhead (≈2× for escape-heavy text, ≈4× for
/// stamped items, whose `event_bytes` are escaped JSON being escaped again) dwarfs the 2 MiB gap
/// between core's `MAX_EXPORT_BYTES` (30 MiB) and [`MAX_FRAME`] (32 MiB) — so a bundle that passed
/// BOTH the pre-build estimate AND core's own byte belt can still overflow the frame. Without this,
/// `write_frame` errors and the connection closes with NO response, and the app shows "engine
/// unavailable" instead of "too big, select fewer memories" (spec §7: a generic frame error must
/// never replace the typed refusal).
///
/// Extracted as a pure function so the bound is unit-testable on BOTH sides of the boundary — as a
/// `match` arm inside `export_dispatch` it needed a real ~30 MiB export to reach, and review proved
/// that deleting it there was invisible to the whole suite.
///
/// A serialization failure returns `None` (the response is sent, and the send path's own
/// `encode` fallback handles it) — this function's job is the SIZE verdict, not error reporting.
fn frame_guard(response: &Response) -> Option<Response> {
    let wire = serde_json::to_vec(response).ok()?;
    (wire.len() > MAX_FRAME).then_some(Response::BundleTooLarge {
        bytes: wire.len() as u64,
        max: MAX_FRAME as u64,
        limit: BundleLimitWire::WireFrame,
    })
}

/// A typed `Rejected` refusal for the Rung-5 export ops. DELIBERATELY distinct from
/// [`capture_rejected`], whose contract is a STATIC check name that never echoes attacker-influenced
/// capture bytes (I4): an export refusal carries core's typed reason, which may embed an App-supplied
/// event id ("note {id} not found"). That is the same asymmetry [`Request::SupersedeNote`] already
/// documents — `ExportBundle` is App-only (guest-denied at the role gate) and the ids are
/// daemon-minted ULIDs the App itself obtained from a listing op and echoed back over the trusted App
/// socket, so surfacing them disambiguates the reject and is never a hostile-capture surface.
fn export_rejected(message: &str) -> Response {
    Response::Err { kind: OpErrorKindWire::Rejected, message: message.to_string() }
}

/// SP3 §9 (A13) — the App-only `GetSession` detail read. Validates the App-supplied `session_id`
/// (defense in depth, A5), folds the CURRENT sessions to find it, then reads its daemon-authored
/// `.md` (bounded) to assemble the [`SessionDetailWire`]. A `session_id` that is not a CURRENT
/// session (never captured, superseded away, or owner-DELETED) is a clean `Rejected` — the UI
/// needs to tell "already deleted" from a real fault (spec §3). Never echoes the session id.
async fn get_session(engine: &EngineHandle, session_id: String) -> Response {
    // Validate BEFORE building any path (A5 D1), even though App-supplied. A hostile id can never
    // match a folded (validated) capture anyway, but rejecting here keeps the id out of any path.
    if !valid_session_id(&session_id) {
        return capture_rejected("session not found or deleted");
    }
    let sessions = match engine.current_sessions().await {
        Ok(s) => s,
        Err(e) => return op_error_response(e),
    };
    let Some(cs) = sessions.into_iter().find(|c| c.session_id == session_id) else {
        // Not current (deleted / superseded / never captured) → the specced clean Rejected.
        return capture_rejected("session not found or deleted");
    };
    // Defense in depth: the folded path is always daemon-constructed as `<data_dir>/sessions/<sid>.md`
    // (store_capture + heal both build it that way), so enforce that confinement invariant explicitly
    // rather than trust it implicitly before reading the body off disk.
    if let Some(data_dir) = engine.data_dir() {
        if !Path::new(&cs.path).starts_with(data_dir.join("sessions")) {
            return capture_rejected("session not found or deleted");
        }
    }
    // Read the daemon-authored body (bounded). A read failure is our OWN i/o (e.g. an out-of-band
    // deletion racing the fold) → a clean, path-free Core fault (never the OS path).
    let markdown = match read_capture_markdown(Path::new(&cs.path)) {
        Ok(m) => m,
        Err(_) => {
            return Response::Err {
                kind: OpErrorKindWire::Core,
                message: "capture body could not be read".to_string(),
            }
        }
    };
    Response::Session(SessionDetailWire { summary: session_summary_wire(cs), markdown })
}

/// SP3 §7 (A13) — the App-only `DeleteSession` (honest, durable forget, I7). Validates the id then
/// delegates to [`delete_capture`], which removes the `.md` AND appends the `session_deleted`
/// tombstone (both recall arms then exclude it; it survives a daemon restart). Idempotent by
/// construction (A7): deleting a missing/unknown session removes nothing and tombstones nothing
/// current → still `Ok`. Never echoes the session id.
async fn delete_session(engine: &EngineHandle, session_id: String) -> Response {
    if !valid_session_id(&session_id) {
        return capture_rejected("invalid session id");
    }
    let Some(data_dir) = engine.data_dir() else {
        return Response::Err {
            kind: OpErrorKindWire::Core,
            message: "capture store data dir unresolvable".to_string(),
        };
    };
    match delete_capture(engine, data_dir, &session_id).await {
        Ok(()) => Response::Ok,
        Err(e) => capture_store_error_response(e),
    }
}

/// The daemon-boundary wall-clock read (Unix seconds) for `SetCaptureEnabled`'s `at`. This is the
/// ONLY clock read on the dispatch core; core stays clock-free (it takes the timestamp as an arg).
/// A pre-epoch clock (impossible in practice) folds to 0 rather than panicking.
fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// SP3 §4c — the IMMEDIATE capture path (the `CaptureNotify` poke, guest-reachable). An explicit
/// SessionEnd hook fires this: render the just-ended transcript to a signed capture NOW, bypassing
/// the sweeper's quiet-mtime floor (the explicit poke IS the quiet signal). Confinement, the D2
/// size cap, the identity, and the signed `session_captured` event all reuse the SAME primitives the
/// sweeper (A9) uses, so notify + sweep produce byte-identical captures for one file.
///
/// I10: gated on `capture_enabled`. A poke on a brain that has NOT consented to capture is a silent
/// no-op success — the hook fired, but nothing is written (no `sessions/` dir, no event). Onboarding
/// is the daemon's OWN verdict (never a client flag), exactly like [`EngineHandle::capture_session`].
///
/// Every rejection is a CLEAN typed error that NEVER echoes the attacker-influenced session id /
/// transcript path (spec I4).
async fn capture_notify(engine: &EngineHandle, session_id: String, transcript_path: String) -> Response {
    // I10 gate FIRST: capture disabled (or not onboarded → capture_enabled is false) → no side
    // effects, clean Ok. The daemon computes onboarding itself; a guest can never assert it.
    let onboarded = engine.is_onboarded_local();
    if !engine.capture_enabled(onboarded).await.unwrap_or(false) {
        return Response::Ok;
    }
    // A5 D1: validate the session id BEFORE any path is built from it.
    if !valid_session_id(&session_id) {
        return capture_rejected("invalid session id");
    }
    // A5 D3: confine + open the transcript under the Claude projects root (careful-open: O_NOFOLLOW
    // per component, no canonicalize). A rejection maps to a clean error — never the hostile bytes.
    let root = claude_projects_root();
    let file = match open_transcript_confined(&root, Path::new(&transcript_path)) {
        Ok(f) => f,
        Err(_) => return capture_rejected("transcript path rejected by capture-path discipline"),
    };
    // Enforce the notify-vs-sweep-agree invariant: the sweeper keys a capture by the transcript
    // filename STEM (and derives `project` from the path too), so a request whose `session_id`
    // disagrees with the stem would store a phantom keyed by the request id that the stem-keyed
    // sweeper never supersedes. Reject the mismatched pair (a buggy hook) cleanly — this is what
    // makes "notify + sweep produce byte-identical captures for one file" an ENFORCED invariant,
    // not just a doc claim. `session_id` is already ASCII-validated, so the `to_str` compare is exact.
    if Path::new(&transcript_path).file_stem().and_then(|s| s.to_str()) != Some(session_id.as_str()) {
        return capture_rejected("session id does not match the transcript filename");
    }
    // A6: render (the D2 size cap lives here). Any render failure → a clean rejection.
    let rendered = match render_transcript(file, &render_bounds()) {
        Ok(r) => r,
        Err(_) => return capture_rejected("transcript could not be rendered"),
    };
    // Identity mirrors the sweeper: session_id from the (validated) request, project = the parent
    // slug dir of the confined path, tool = claude-code — so notify + sweep agree for one file.
    let id = CaptureIdentity {
        session_id,
        project: transcript_project_slug(&transcript_path),
        tool: CAPTURE_TOOL.to_string(),
    };
    // The data dir is db_path's parent (single-sourced with is_onboarded_local); unresolvable → a
    // clean Core fault rather than an unwrap.
    let Some(data_dir) = engine.data_dir() else {
        return Response::Err {
            kind: OpErrorKindWire::Core,
            message: "capture store data dir unresolvable".to_string(),
        };
    };
    // A7: atomic 0600 write under the 0700 sessions dir THEN the signed event (file-then-event).
    match store_capture(engine, data_dir, &id, &rendered).await {
        Ok(()) => Response::Ok,
        Err(e) => capture_store_error_response(e),
    }
}

/// A clean capture-path rejection (`Rejected` kind) whose message is a STATIC check name — it never
/// echoes the attacker-influenced session id / transcript path (spec I4).
fn capture_rejected(message: &str) -> Response {
    Response::Err { kind: OpErrorKindWire::Rejected, message: message.to_string() }
}

/// The project slug = the transcript's parent DIRECTORY name (Claude Code's cwd-encoding), used
/// verbatim as a stable per-repo key — exactly as the sweeper derives it from its scan (so notify +
/// sweep tag one file identically). Empty if the path has no parent dir (degenerate; the store still
/// writes, keyed by session_id).
fn transcript_project_slug(transcript_path: &str) -> String {
    Path::new(transcript_path)
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Map a [`CaptureStoreError`] to a clean wire error. The store collapses its underlying
/// `EngineOpError` into a String (so the typed engine kind is already gone by here); a store fault
/// therefore rides the generic `Core` kind, and no attacker-influenced path is ever echoed.
fn capture_store_error_response(e: CaptureStoreError) -> Response {
    match e {
        // The id was already A5-validated in the handler; a reject here is still clean (no bytes).
        CaptureStoreError::InvalidSessionId => capture_rejected("invalid session id"),
        // Our OWN sessions-dir I/O — surface a clean, path-free Core fault (never the OS path).
        CaptureStoreError::Io(_) => {
            Response::Err { kind: OpErrorKindWire::Core, message: "capture store i/o error".to_string() }
        }
        // The engine refused/failed to record the signed event; its message is engine-internal
        // (op name / engine message — no attacker path). Ride the generic Core kind.
        CaptureStoreError::Engine(m) => Response::Err { kind: OpErrorKindWire::Core, message: m },
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

/// Record one recall's outcome to the best-effort telemetry store (spec §8 tuning signal), AFTER
/// the recall itself has completed. Built per-call from the daemon's data dir (`Telemetry` just
/// holds a path); ANY failure is swallowed inside [`crate::telemetry::Telemetry::record`] — plus an
/// unresolvable data dir or an un-openable store here is skipped — so telemetry can NEVER fail a
/// recall (critic m2). Records the QUERY + hit count + top score only; never any result text.
fn record_recall_telemetry(engine: &EngineHandle, query: &str, hits: &[HitWithText]) {
    let Some(data_dir) = engine.data_dir() else { return };
    let Ok(telemetry) = crate::telemetry::Telemetry::open(data_dir) else { return };
    telemetry.record(query, hits.len(), hits.first().map(|h| h.hit.score));
}

/// Read the recall telemetry back for the App-only `RecallStats` op — the symmetric READ to
/// [`record_recall_telemetry`]'s write. Best-effort: an unresolvable data dir, an un-openable store,
/// or a stats-read error all fold to the EMPTY default (`0/0/[]`), so the read-only tuning panel
/// degrades to zeros rather than a failed op.
fn recall_stats(engine: &EngineHandle) -> RecallStatsWire {
    engine
        .data_dir()
        .and_then(|d| crate::telemetry::Telemetry::open(d).ok())
        .and_then(|t| t.stats().ok())
        .unwrap_or_default()
}

/// Map one folded `CurrentSession` to its wire summary (the fields match 1:1; the on-disk `path`
/// + `sha256` + `event_id` stay daemon-internal and are deliberately NOT surfaced).
fn session_summary_wire(cs: bossclaw_core::log::CurrentSession) -> SessionSummaryWire {
    SessionSummaryWire {
        // The capture EVENT id — the address `ExportSelection::session_event_ids` takes (Rung-5
        // §2.4). It was previously dropped here, which is what left the Library able to list
        // sessions but unable to name one for export. A daemon-minted ULID, so nothing to sanitize.
        event_id: cs.event_id,
        session_id: cs.session_id,
        title: cs.title,
        project: cs.project,
        tool: cs.tool,
        started_at: cs.started_at,
        ended_at: cs.ended_at,
        approx_bytes: cs.approx_bytes,
    }
}

/// Map one folded `CurrentNote` to its wire form. `superseded_by` is always `None` in the
/// current-only fold (a superseded note is excluded); the field mirrors the wire shape for a
/// possible future edit-history view.
fn note_wire(n: bossclaw_core::log::CurrentNote) -> NoteWire {
    NoteWire {
        event_id: n.event_id,
        text: n.text,
        created_at: n.created_at,
        superseded_by: n.superseded_by,
    }
}

/// Build a sanitized [`ConflictProposalWire`] from a core row (MINOR-1). The refs are ids
/// (structurally safe → the plain `From<ConflictRef>` conversion); the free-ish string fields are
/// passed through `sanitize_injected` (single-line, ≤ `SNAPSHOT_FIELD_MAX`) so no memory-derived
/// text can ever cross the wire un-neutralized. This is the ONLY builder of `ConflictProposalWire`:
/// proto deliberately exposes NO whole-row `From<ConflictProposalRow>` (which would pass the strings
/// through verbatim), because the daemon owns the neutralization — the MCP adapter cannot call
/// `sanitize_injected` in prod.
fn sanitize_conflict_row(row: bossclaw_core::ConflictProposalRow) -> ConflictProposalWire {
    use crate::capture::snapshot::sanitize_injected;
    ConflictProposalWire {
        id: row.id,
        a_ref: row.a_ref.into(),
        b_ref: row.b_ref.into(),
        winner_hint: sanitize_injected(&row.winner_hint),
        confidence_band: sanitize_injected(&row.confidence_band),
        why: sanitize_injected(&row.why),
        detected_at: row.detected_at,
    }
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

/// A fresh, shareable in-memory vault handle for tests that must reopen an engine on the SAME
/// `home`/data_dir across a simulated daemon RESTART. The keystore persists the brain signing key
/// and the DEK in this vault, so a genuine reopen MUST reuse the same vault instance — a fresh
/// [`test_engine`] mints a fresh empty vault and would re-mint keys that can't decrypt the existing
/// DB (`KeystoreDbMismatch`). Returned as an opaque `Arc<dyn SecretsVault>` the caller clones into
/// two [`test_engine_with_vault`] handles. `TestVault` itself stays crate-private; this is the
/// integration-test seam the forget-suite daemon-restart test needs (`memory_client_loop` and
/// `language_pack` document the same crate-private limitation).
#[cfg(any(test, feature = "test-helpers"))]
pub fn shared_test_vault() -> Arc<dyn crate::secrets::SecretsVault> {
    Arc::new(TestVault::default())
}

/// Like [`test_engine`] but with a CALLER-SUPPLIED vault (see [`shared_test_vault`]) so two engines
/// can share ONE keystore across a reopen — the daemon-level companion to core's engine-level
/// reopen tests. Mock embedder + scripted reasoner, exactly like [`test_engine`].
#[cfg(any(test, feature = "test-helpers"))]
pub fn test_engine_with_vault(
    home: std::path::PathBuf,
    vault: Arc<dyn crate::secrets::SecretsVault>,
) -> EngineHandle {
    EngineHandle::new(vault, home, Arc::new(TestEmbedderProvider), Arc::new(TestReasonerProvider))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The guest onboarding-override is fail-closed by construction: it rewrites the `onboarded`
    /// flag for the two handled guest ops and returns `None` for anything else, so drift between
    /// `Role::allows` and this function refuses a new op rather than trusting the client's flag.
    #[test]
    fn guest_onboarding_override_is_fail_closed_for_unhandled_ops() {
        // A guest op with a client-asserted `onboarded: false` is rewritten to the daemon's truth.
        match override_onboarding_for_guest(
            Request::Recall { onboarded: false, query: "q".into(), k: 1 },
            true,
        ) {
            Some(Request::Recall { onboarded, query, k }) => {
                assert!(onboarded, "the daemon's onboarding truth overwrites the client flag");
                assert_eq!(query, "q");
                assert_eq!(k, 1);
            }
            other => panic!("expected Some(Recall), got {other:?}"),
        }
        match override_onboarding_for_guest(
            Request::Remember { onboarded: false, text: "t".into() },
            true,
        ) {
            Some(Request::Remember { onboarded, text }) => {
                assert!(onboarded, "the daemon's onboarding truth overwrites the client flag");
                assert_eq!(text, "t");
            }
            other => panic!("expected Some(Remember), got {other:?}"),
        }
        // The SP3 capture pokes (A10) are admitted for the guest with the daemon's onboarding truth.
        match override_onboarding_for_guest(
            Request::CaptureNotify {
                onboarded: false,
                session_id: "s".into(),
                transcript_path: "/p.jsonl".into(),
            },
            true,
        ) {
            Some(Request::CaptureNotify { onboarded, session_id, transcript_path }) => {
                assert!(onboarded, "the daemon's onboarding truth overwrites the client flag");
                assert_eq!(session_id, "s");
                assert_eq!(transcript_path, "/p.jsonl");
            }
            other => panic!("expected Some(CaptureNotify), got {other:?}"),
        }
        match override_onboarding_for_guest(
            Request::Snapshot {
                onboarded: false,
                project: "/r".into(),
                source: "startup".into(),
                session_id: None,
                transcript_path: None,
            },
            true,
        ) {
            Some(Request::Snapshot { onboarded, .. }) => {
                assert!(onboarded, "the daemon's onboarding truth overwrites the client flag");
            }
            other => panic!("expected Some(Snapshot), got {other:?}"),
        }
        // A NON-guest op (not on the allowlist) returns None → the caller refuses it fail-closed,
        // so a future allowlist widening that forgets this function can never leak a mint-forge.
        assert!(
            override_onboarding_for_guest(Request::Status { onboarded: false }, true).is_none(),
            "an unhandled op must fail closed (None), not pass through with the client's flag"
        );
        // Rung 3 retire ops are App-only (guest-refused): they are NOT handled here, so they fall
        // through to None just like any other non-guest op — the role gate then refuses them.
        assert!(
            override_onboarding_for_guest(
                Request::RetireMemory { onboarded: false, target: RetireTarget::Note { event_id: "e".into() } },
                true,
            )
            .is_none(),
            "RetireMemory is App-only and must fail closed (None) for a guest"
        );
        assert!(
            override_onboarding_for_guest(
                Request::Unretire { onboarded: false, retired_event_id: "r".into() },
                true,
            )
            .is_none(),
            "Unretire is App-only and must fail closed (None) for a guest"
        );
    }

    #[test]
    fn resolution_ops_are_not_rate_limited() {
        // Per §0 the per-connection limiter cannot bound a reconnecting MCP client, so the resolve ops are
        // deliberately NOT rate-limited — reversibility + the visibility digest are the controls.
        assert!(!is_rate_limited_op(&Request::ListConflicts { onboarded: true }));
        assert!(!is_rate_limited_op(&Request::ResolveConflict {
            onboarded: true, proposal_id: "P".into(), action: bossclawd_proto::types::ResolveActionWire::KeepBoth,
        }));
    }

    #[test]
    fn guest_onboarding_override_passes_through_the_two_resolution_ops() {
        // I8 relaxation: the two resolve ops are guest ops, so they MUST get an explicit passthrough arm
        // (a missing arm → None → refused). The client's `onboarded` is overwritten with the daemon's truth.
        match override_onboarding_for_guest(Request::ListConflicts { onboarded: false }, true) {
            Some(Request::ListConflicts { onboarded }) => assert!(onboarded),
            other => panic!("expected Some(ListConflicts), got {other:?}"),
        }
        match override_onboarding_for_guest(
            Request::ResolveConflict { onboarded: false, proposal_id: "P".into(), action: bossclawd_proto::types::ResolveActionWire::Dismiss },
            true,
        ) {
            Some(Request::ResolveConflict { onboarded, proposal_id, .. }) => {
                assert!(onboarded, "daemon truth overwrites the client flag");
                assert_eq!(proposal_id, "P");
            }
            other => panic!("expected Some(ResolveConflict), got {other:?}"),
        }
    }

    /// `sanitize_conflict_row` routes each free-ish string field (`winner_hint` / `confidence_band` /
    /// `why`) through `sanitize_injected` — so no newline/CR/tab survives, the `<<<`/`>>>` fence markers
    /// can never be reproduced, and every field is capped to `SNAPSHOT_FIELD_MAX` — while passing `id`,
    /// `detected_at`, and both refs through UNCHANGED (T13 review: the daemon is the ONLY sanitizing
    /// builder, so this per-field routing is what stops memory-derived text crossing the wire un-fenced).
    #[test]
    fn sanitize_conflict_row_neutralizes_strings_and_passes_ids_through() {
        use bossclaw_core::index::ConflictRef;
        use crate::capture::snapshot::SNAPSHOT_FIELD_MAX;
        let a = ConflictRef::Note { event_id: "a-id".into() };
        let b = ConflictRef::Passage { session_id: "s-id".into(), passage_id: 3 };
        let row = bossclaw_core::ConflictProposalRow {
            id: "P-exact".into(),
            a_ref: a.clone(),
            b_ref: b.clone(),
            // Hostile: newlines, a NUL control char, `<<<`/`>>>` fence text, and (in `why`) enough length
            // that the visible-char cap must bite.
            winner_hint: "ne\nwer".into(),
            confidence_band: "hi\u{0000}gh <<<FENCE>>>".into(),
            why: format!("line1\nline2 <<<SYSTEM>>> {}", "x".repeat(SNAPSHOT_FIELD_MAX + 50)),
            detected_at: 42,
        };
        let wire = sanitize_conflict_row(row);

        // id / detected_at / both refs pass through EXACTLY (structurally safe — never sanitized).
        assert_eq!(wire.id, "P-exact");
        assert_eq!(wire.detected_at, 42);
        assert_eq!(wire.a_ref, a.into());
        assert_eq!(wire.b_ref, b.into());

        // Every string field carries `sanitize_injected`'s guarantees: single-line, fence-free, bounded.
        for field in [&wire.winner_hint, &wire.confidence_band, &wire.why] {
            assert!(
                !field.contains('\n') && !field.contains('\r') && !field.contains('\t'),
                "single-line: {field:?}"
            );
            assert!(!field.contains("<<") && !field.contains(">>"), "fence-free: {field:?}");
            assert!(field.chars().count() <= SNAPSHOT_FIELD_MAX, "bounded: {field:?}");
        }
    }

    /// The authoritative pre-frame bound fires on an over-frame bundle and stays out of the way
    /// otherwise. BOTH sides are asserted: a guard that always fires would break every export, and a
    /// guard that never fires (the regression this exists to catch — review deleted it and the whole
    /// suite still passed) drops the typed refusal for a closed connection.
    ///
    /// Escape-heavy text, so the over-frame case is reached without materializing a real 30 MiB
    /// bundle: each `\"` pair doubles under JSON escaping, so ~half of MAX_FRAME of it lands just
    /// over the bound.
    #[test]
    fn frame_guard_refuses_only_an_over_frame_response() {
        // Each `\"` pair is 2 raw bytes that escape to 4. At MAX_FRAME/3 pairs the raw text is
        // 2/3·MAX_FRAME (comfortably UNDER the frame) while the escaped wire is 4/3·MAX_FRAME
        // (over it) — exactly the escaping-overhead case this guard exists for.
        let over = Response::Bundle("\\\"".repeat(MAX_FRAME / 3));
        match frame_guard(&over) {
            Some(Response::BundleTooLarge { bytes, max, limit }) => {
                assert!(bytes > MAX_FRAME as u64, "reports the ESCAPED length: {bytes}");
                assert_eq!(max, MAX_FRAME as u64, "reports the frame ceiling");
                assert_eq!(limit, BundleLimitWire::WireFrame, "tagged as frame semantics");
            }
            other => panic!("an over-frame bundle must be refused, got {other:?}"),
        }

        // The raw text alone is UNDER the frame — proving the guard measures the escaped wire, not
        // the string it was handed (a guard reading `text.len()` would pass this one through).
        let Response::Bundle(raw) = &over else { unreachable!() };
        assert!(raw.len() < MAX_FRAME, "raw text is under the frame; only escaping pushes it over");

        // Just UNDER: a normal bundle is returned untouched.
        assert!(
            frame_guard(&Response::Bundle("{\"manifest\":{}}".to_string())).is_none(),
            "a bundle that fits must pass through so the export actually reaches the app"
        );
        // A non-Bundle response is never refused on size (the guard is response-shape agnostic).
        assert!(frame_guard(&Response::Ok).is_none());
    }
}
