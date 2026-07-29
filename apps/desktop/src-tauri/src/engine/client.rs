//! The daemon-backed engine client (M1a Task 5).
//!
//! [`EngineClient`] is a drop-in for today's in-process `EngineHandle`: every public method mirrors
//! the `EngineHandle` signature 1:1, but instead of touching an `EventLog` it builds a [`Request`],
//! sends it over a [`Transport`] to `bossclawd`, and folds the [`Response`] back into the same
//! desktop return type the in-process engine produced. Task 6 (a separate change) swaps the app's
//! `AppState` from `EngineHandle` to `EngineClient<SocketTransport>`; this task adds ONLY the client
//! + transport, leaving the in-process engine untouched.
//!
//! # Error mapping (the single chokepoint is [`op_error_from_response`])
//! - [`Response::Ok`] → `Ok(())` for unit ops (or the payload variant for value ops);
//! - [`Response::NotOnboarded`] → [`EngineOpError::Open`]`(`[`EngineError::NotOnboarded`]`)` — the
//!   same shape the in-process `get_or_open` gate produces, so callers/tests are unchanged;
//! - [`Response::Busy`]`(s)` → [`EngineOpError::Busy`]`(&'static str)` via a match on the daemon's
//!   op literal (`"ingest"`/`"evolve"`), defaulting UNKNOWN strings to `"ingest"` (documented below);
//! - [`Response::Err`]` { kind, message }` → the EXACT typed variant the daemon's engine produced
//!   (`Stale`/`Revoked`/`NeedsLoudConfirm`/`Rejected`/… — see [`op_error_from_wire`]). The wire
//!   carries the variant's INNER message, so the app-side Display renders byte-for-byte what the
//!   in-process engine rendered — no double prefix, no typed → `Core` collapse. Guarded by the
//!   string-parity tests below (the Task 6 regression bar);
//! - a transport failure ([`Transport::request`] returned `Err`) → that [`EngineOpError::Unavailable`]
//!   verbatim (the daemon is down).
//!
//! # Wire → desktop conversions
//! Family-1 mirrors (`GrantMirror`, `IngestReportMirror`, `HitMirror`, …) already have
//! `Into<core-type>` in proto, so those use `.into()`. Family-2 summary structs
//! (`ProposalSummaryWire`, `MandateSummaryWire`, `PreviewDataWire`, `EngineStatusWire`,
//! `ReasonerConfigWire`, …) have NO `From` in proto by design (the desktop crate isn't a proto dep),
//! so the field-by-field conversions live HERE, next to their only consumer.
//!
//! # Swallow-behavior parity
//! `status`, `reasoner_config_or_default`, and `reasoner_ready_or_false` are INFALLIBLE on the
//! in-process engine (they swallow every error into a default). The client preserves that EXACTLY:
//! a daemon-unavailable `reasoner_ready_or_false` returns `false`, `reasoner_config_or_default`
//! returns the Local default, and `status` returns a `KeystoreDbMismatch`/`NotOnboarded` status —
//! never an error, never a panic.
//!
//! # Wired in (M1a Task 6)
//! The running app now consumes this: `AppState.engine` is the [`super::Engine`] facade, which
//! delegates every method to an `EngineClient`. The Task-5-era `#![allow(dead_code)]` staging
//! allowance is therefore gone — the whole surface is live in the bin build.

use std::path::PathBuf;
use std::sync::Arc;

use bossclawd_proto::types::{
    EngineStateWire, EngineStatusWire, MandateSummaryWire, MandateWriteSummaryWire, ModelStateWire,
    ModelStatusWire, NoteWire, PreviewDataWire, ProposalSummaryWire, ReasonerConfigWire,
    ReasonerModeWire, RecallMissWire, RecallStatsWire, SessionDetailWire, SessionSummaryWire,
};
use bossclawd_proto::{
    BundleLimitWire, ExportSelectionWire, HitWire, OpErrorKindWire, Request, Response,
};

use super::reason::{CloudProvider, ReasonerConfig, ReasonerMode};
use super::transport::Transport;
use super::{
    ApplyResult, EngineError, EngineOpError, EngineState, EngineStatus, EvolveTelemetry, HitWithText,
    MandateSummary, MandateWriteSummary, ModelState, Note, PreviewData, ProposalSummary, RecallMiss,
    RecallStats, SessionDetail, SessionSummary,
};

/// A daemon-backed drop-in for today's in-process `EngineHandle`. Generic over the [`Transport`] so
/// the ops can be unit-tested with a fake transport (and driven live with [`super::transport::SocketTransport`]).
/// Stateless beyond the transport — all engine state lives in `bossclawd`.
///
/// `T: ?Sized` so `EngineClient<dyn Transport>` is a valid, SINGLE concrete type: the [`super::Engine`]
/// facade holds exactly that, letting the (non-generic) `AppState` pick its transport at construction
/// with one `Arc<dyn Transport>` — no double-`Arc`, no blanket impl.
pub struct EngineClient<T: Transport + ?Sized> {
    transport: Arc<T>,
}

impl<T: Transport + ?Sized> EngineClient<T> {
    /// Wrap a transport.
    pub fn new(transport: Arc<T>) -> Self {
        Self { transport }
    }

    // ── Status (infallible — swallows every error into a status, like `EngineHandle::status`). ──

    /// Mirrors `EngineHandle::status`: never errors. A transport failure becomes a
    /// `KeystoreDbMismatch` status (the engine's own "can't open" state), so the UI shows a fault
    /// state rather than crashing — the closest in-process analogue to "daemon down".
    ///
    /// `onboarded == false` short-circuits to [`not_onboarded_status`] WITHOUT a transport call,
    /// matching the in-process `get_or_open` gate (which returns `NotOnboarded` before opening the
    /// engine). This makes a not-onboarded user with a DOWN daemon see the clean `NotOnboarded`
    /// state — not the `KeystoreDbMismatch` fault a transport failure would otherwise yield.
    pub async fn status(&self, onboarded: bool) -> EngineStatus {
        if !onboarded {
            return not_onboarded_status();
        }
        match self.transport.request(Request::Status { onboarded }).await {
            Ok(Response::Status(w)) => status_from_wire(w),
            // NotOnboarded can also arrive as a status (state=NotOnboarded) — but if the daemon ever
            // sent the bare signal, honour it as the NotOnboarded status.
            Ok(Response::NotOnboarded) => not_onboarded_status(),
            // Any other response (Err/Busy/unexpected variant) or a transport failure → the engine's
            // "can't open" fault state, matching `status`'s never-error contract.
            _ => keystore_mismatch_status(),
        }
    }

    // ── Grant / write-grant mutations (unit-returning). ──

    /// Mirrors `EngineHandle::add_grant`.
    pub async fn add_grant(&self, onboarded: bool, path: PathBuf) -> Result<(), EngineOpError> {
        self.unit(Request::AddGrant { onboarded, path }).await
    }

    /// Mirrors `EngineHandle::revoke_grant`.
    pub async fn revoke_grant(&self, onboarded: bool, path: PathBuf) -> Result<(), EngineOpError> {
        self.unit(Request::RevokeGrant { onboarded, path }).await
    }

    /// Mirrors `EngineHandle::set_folder_writable`.
    pub async fn set_folder_writable(
        &self,
        onboarded: bool,
        path: PathBuf,
        on: bool,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::SetFolderWritable { onboarded, path, on }).await
    }

    // ── Read ops. ──

    /// Mirrors `EngineHandle::list_writable`.
    pub async fn list_writable(&self, onboarded: bool) -> Result<Vec<String>, EngineOpError> {
        match self.request(Request::ListWritable { onboarded }).await? {
            Response::ListWritable(roots) => Ok(roots),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::list_grants` (returns core `Grant`s via the proto `GrantMirror` Into).
    pub async fn list_grants(
        &self,
        onboarded: bool,
    ) -> Result<Vec<bossclaw_core::Grant>, EngineOpError> {
        match self.request(Request::ListGrants { onboarded }).await? {
            Response::ListGrants(grants) => Ok(grants.into_iter().map(Into::into).collect()),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::list_files` (core `FileRecord`s via the `FileRecordMirror` Into).
    pub async fn list_files(
        &self,
        onboarded: bool,
    ) -> Result<Vec<bossclaw_core::graph::FileRecord>, EngineOpError> {
        match self.request(Request::ListFiles { onboarded }).await? {
            Response::ListFiles(files) => Ok(files.into_iter().map(Into::into).collect()),
            other => Err(unexpected(other)),
        }
    }

    // ── Ingest / recall. ──

    /// Mirrors `EngineHandle::run_ingest` (core `IngestReport` via the `IngestReportMirror` Into).
    pub async fn run_ingest(
        &self,
        onboarded: bool,
    ) -> Result<bossclaw_core::IngestReport, EngineOpError> {
        match self.request(Request::RunIngest { onboarded }).await? {
            Response::RunIngest(r) => Ok(r.into()),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::recall` — each hit paired with its hydrated snippet text.
    pub async fn recall(
        &self,
        onboarded: bool,
        query: String,
        k: usize,
    ) -> Result<Vec<HitWithText>, EngineOpError> {
        match self.request(Request::Recall { onboarded, query, k }).await? {
            Response::Recall(hits) => Ok(hits.into_iter().map(hit_with_text_from_wire).collect()),
            other => Err(unexpected(other)),
        }
    }

    // ── Evolve. ──

    /// Mirrors `EngineHandle::evolve_once` (core `EvolveReport` via the `EvolveReportMirror` Into).
    pub async fn evolve_once(
        &self,
        onboarded: bool,
    ) -> Result<bossclaw_core::EvolveReport, EngineOpError> {
        match self.request(Request::EvolveOnce { onboarded }).await? {
            Response::EvolveOnce(r) => Ok(r.into()),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::evolve_status` — the core status + the daemon's session telemetry.
    pub async fn evolve_status(
        &self,
        onboarded: bool,
    ) -> Result<(bossclaw_core::EvolveStatus, EvolveTelemetry), EngineOpError> {
        match self.request(Request::EvolveStatus { onboarded }).await? {
            Response::EvolveStatus { status, telemetry } => {
                Ok((status.into(), telemetry_from_wire(telemetry)))
            }
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::set_evolve_enabled`.
    pub async fn set_evolve_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        self.unit(Request::SetEvolveEnabled { onboarded, enabled }).await
    }

    /// Mirrors `EngineHandle::set_proposals_enabled`.
    pub async fn set_proposals_enabled(
        &self,
        onboarded: bool,
        enabled: bool,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::SetProposalsEnabled { onboarded, enabled }).await
    }

    /// Mirrors `EngineHandle::set_mandates_enabled`.
    pub async fn set_mandates_enabled(
        &self,
        onboarded: bool,
        enabled: bool,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::SetMandatesEnabled { onboarded, enabled }).await
    }

    /// Mirrors `EngineHandle::mandates_enabled`.
    pub async fn mandates_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        match self.request(Request::MandatesEnabled { onboarded }).await? {
            Response::MandatesEnabled(b) => Ok(b),
            other => Err(unexpected(other)),
        }
    }

    // ── Mandate CRUD + activity. ──

    /// Mirrors `EngineHandle::add_mandate` (Family-2 `MandateSummaryWire` → `MandateSummary`).
    pub async fn add_mandate(
        &self,
        onboarded: bool,
        target: PathBuf,
        source_scope: PathBuf,
        recipe: String,
    ) -> Result<MandateSummary, EngineOpError> {
        match self.request(Request::AddMandate { onboarded, target, source_scope, recipe }).await? {
            Response::AddMandate(m) => Ok(mandate_summary_from_wire(m)),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::revoke_mandate`.
    pub async fn revoke_mandate(
        &self,
        onboarded: bool,
        mandate_grant_id: String,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::RevokeMandate { onboarded, mandate_grant_id }).await
    }

    /// Mirrors `EngineHandle::list_mandates`.
    pub async fn list_mandates(
        &self,
        onboarded: bool,
    ) -> Result<Vec<MandateSummary>, EngineOpError> {
        match self.request(Request::ListMandates { onboarded }).await? {
            Response::ListMandates(ms) => Ok(ms.into_iter().map(mandate_summary_from_wire).collect()),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::mandate_writes`.
    pub async fn mandate_writes(
        &self,
        onboarded: bool,
    ) -> Result<Vec<MandateWriteSummary>, EngineOpError> {
        match self.request(Request::MandateWrites { onboarded }).await? {
            Response::MandateWrites(ws) => {
                Ok(ws.into_iter().map(mandate_write_summary_from_wire).collect())
            }
            other => Err(unexpected(other)),
        }
    }

    // ── Review queue: proposals, preview, apply, undo, decline. ──

    /// Mirrors `EngineHandle::list_proposals`.
    pub async fn list_proposals(
        &self,
        onboarded: bool,
    ) -> Result<Vec<ProposalSummary>, EngineOpError> {
        match self.request(Request::ListProposals { onboarded }).await? {
            Response::ListProposals(ps) => {
                Ok(ps.into_iter().map(proposal_summary_from_wire).collect())
            }
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::proposal_preview`.
    pub async fn proposal_preview(
        &self,
        onboarded: bool,
        id: String,
    ) -> Result<PreviewData, EngineOpError> {
        match self.request(Request::ProposalPreview { onboarded, id }).await? {
            Response::ProposalPreview(p) => Ok(preview_data_from_wire(p)),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::apply_proposal` (Family-2 `ApplyResultWire` → `ApplyResult`).
    pub async fn apply_proposal(
        &self,
        onboarded: bool,
        id: String,
        acknowledged_loud: bool,
    ) -> Result<ApplyResult, EngineOpError> {
        match self.request(Request::ApplyProposal { onboarded, id, acknowledged_loud }).await? {
            Response::ApplyProposal(r) => Ok(ApplyResult { file_written_id: r.file_written_id }),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `EngineHandle::undo_apply`.
    pub async fn undo_apply(
        &self,
        onboarded: bool,
        file_written_id: String,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::UndoApply { onboarded, file_written_id }).await
    }

    /// Mirrors `EngineHandle::decline_proposal`.
    pub async fn decline_proposal(
        &self,
        onboarded: bool,
        id: String,
        reason: String,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::DeclineProposal { onboarded, id, reason }).await
    }

    // ── Teardown (identity reset). Returns `EngineError`, not `EngineOpError`. ──

    /// Mirrors `EngineHandle::teardown`. Its return is `Result<(), EngineError>`; a daemon error is
    /// rebuilt into its exact typed `EngineError` variant (see [`teardown_error_from_response`]); a
    /// transport failure maps to [`EngineError::Unavailable`] (NOT `Vault`) so `reset_identity` can
    /// tell "the brain is DOWN" (non-fatal once the identity is cleared) apart from a real keystore
    /// fault (which must still surface). The transport's own reason is preserved.
    pub async fn teardown(&self) -> Result<(), EngineError> {
        match self.transport.request(Request::Teardown).await {
            Ok(Response::Ok) => Ok(()),
            Ok(other) => Err(teardown_error_from_response(other)),
            Err(e) => Err(EngineError::Unavailable(e.to_string())),
        }
    }

    // ── Reasoner config (Milestone D). ──

    /// Mirrors `EngineHandle::reasoner_config_or_default`: INFALLIBLE — the fail-SAFE Local default
    /// on ANY error (transport down, `Err`/`Busy`/unexpected response), never surfaces an error.
    pub async fn reasoner_config_or_default(&self, onboarded: bool) -> ReasonerConfig {
        match self.transport.request(Request::GetReasonerConfig { onboarded }).await {
            Ok(Response::ReasonerConfig(w)) => reasoner_config_from_wire(w),
            _ => ReasonerConfig::default(),
        }
    }

    /// Mirrors `EngineHandle::reasoner_ready_or_false`: INFALLIBLE — fail-CLOSED to `false` on ANY
    /// error (transport down, `Err`/`Busy`/unexpected response), never surfaces an error.
    pub async fn reasoner_ready_or_false(&self, onboarded: bool) -> bool {
        matches!(
            self.transport.request(Request::GetReasonerReady { onboarded }).await,
            Ok(Response::ReasonerReady(true))
        )
    }

    /// Mirrors `EngineHandle::set_reasoner_config` (free-form JSON config, as the engine takes today).
    pub async fn set_reasoner_config(
        &self,
        onboarded: bool,
        config: serde_json::Value,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::SetReasonerConfig { onboarded, config }).await
    }

    /// Mirrors `EngineHandle::enable_cloud_reasoner`.
    pub async fn enable_cloud_reasoner(
        &self,
        onboarded: bool,
        config: serde_json::Value,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::EnableCloudReasoner { onboarded, config }).await
    }

    // ── Language pack (rung 2). Consumed by the command layer (B2): `engine_set_active_model` /
    //    `engine_model_status`. ──

    /// Mirrors `EngineHandle::set_active_model`: enable the multilingual language pack. The daemon
    /// validates the folder+sha, writes the signed consent, and runs the re-embed migration in the
    /// background; this call returns once that synchronous prologue completes.
    pub async fn set_active_model(
        &self,
        onboarded: bool,
        model_id: String,
        safetensors_sha: String,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::SetActiveModel { onboarded, model_id, safetensors_sha }).await
    }

    /// Mirrors `EngineHandle::model_status`: the loaded-vs-intended model state + live re-index
    /// progress (rung 2), rebuilt from the wire into the desktop [`ModelStatus`].
    pub async fn model_status(&self, onboarded: bool) -> Result<ModelStatus, EngineOpError> {
        match self.request(Request::ModelStatus { onboarded }).await? {
            Response::ModelStatus(w) => Ok(model_status_from_wire(w)),
            other => Err(unexpected(other)),
        }
    }

    // ── Session capture (SP3 B5). App-only ops. Like every sibling, `onboarded` is THREADED from the
    //    caller (the command layer's real `is_onboarded()`), NOT hardcoded true: the daemon's App role
    //    trusts the wire flag, so a hardcoded true would `get_or_open(true)` and mint a brain.db even
    //    pre-onboarding. Threading the real flag makes a pre-onboarding call cleanly `NotOnboarded`
    //    with zero side effects. ──

    /// Mirrors `EngineHandle::set_capture_enabled`: enable/disable ongoing session capture, with
    /// `backfill` additionally consenting (or not) to the one-time import of pre-existing sessions.
    /// The CALLER owns the M4 guarantee by choosing `backfill`: Connect passes `backfill == consent`
    /// (both flags in one call); the Integrations toggle + Disconnect pass `backfill: false`
    /// (forward-only). The daemon supplies the `at` timestamp, so the wire carries only the flags.
    pub async fn set_capture_enabled(
        &self,
        onboarded: bool,
        enabled: bool,
        backfill: bool,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::SetCaptureEnabled { onboarded, enabled, backfill }).await
    }

    /// Mirrors `EngineHandle::capture_enabled`: read the sticky ongoing-capture flag (default CLOSED).
    /// Mirrors the `mandates_enabled` read shape.
    pub async fn capture_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        match self.request(Request::CaptureEnabled { onboarded }).await? {
            Response::CaptureEnabled(b) => Ok(b),
            other => Err(unexpected(other)),
        }
    }

    // ── Reflection (Rung-4 R4-A). App-only ops, like the capture pair: `onboarded` is THREADED from
    //    the caller, never hardcoded true, so a pre-onboarding call cleanly `NotOnboarded`s with no
    //    brain minted. The `SetCaptureEnabled` wiring, minus `backfill` — reflection is a plain bool
    //    switch (no history import), and the daemon's reflect sweeper self-gates on the flag alone. ──

    /// Enable/disable reflection. App-only; the daemon supplies nothing extra (a plain bool switch).
    pub async fn set_reflect_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        self.unit(Request::SetReflectEnabled { onboarded, enabled }).await
    }

    /// Read the sticky reflect-enabled flag (default CLOSED). Mirrors `capture_enabled`.
    pub async fn reflect_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        match self.request(Request::ReflectEnabled { onboarded }).await? {
            Response::ReflectEnabled(b) => Ok(b),
            other => Err(unexpected(other)),
        }
    }

    // ── Memory-browser Library (SP3 C1). App-only ops: the daemon's role gate denies `MemoryClient`
    //    for each, and it resolves onboarding from its OWN identity — but the wire still carries an
    //    `onboarded` flag, so like every sibling (B5) it is THREADED from the caller's real
    //    `is_onboarded()`, never hardcoded true. The wire summary/detail/note/stats structs have no
    //    core equivalent and no proto `From` (Family-2; see module docs), so the field-by-field
    //    conversions live HERE next to their only consumer. ──

    /// Mirrors the daemon's `ListSessions`: the captured-session summaries for the Library
    /// (`SessionSummaryWire` → desktop [`SessionSummary`]).
    pub async fn list_sessions(&self, onboarded: bool) -> Result<Vec<SessionSummary>, EngineOpError> {
        match self.request(Request::ListSessions { onboarded }).await? {
            Response::ListSessions(ss) => Ok(ss.into_iter().map(session_summary_from_wire).collect()),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `GetSession`: one captured session's summary + rendered Markdown. A `session_id` that
    /// is not a CURRENT session (deleted / superseded / never captured) maps to the typed
    /// [`EngineOpError::Rejected`] ("session not found or deleted"), so the UI can tell "already
    /// deleted" from a real fault (spec §3) — the daemon returns that clean reject for the case.
    pub async fn get_session(
        &self,
        onboarded: bool,
        session_id: String,
    ) -> Result<SessionDetail, EngineOpError> {
        match self.request(Request::GetSession { onboarded, session_id }).await? {
            Response::Session(d) => Ok(session_detail_from_wire(d)),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `DeleteSession`: tombstone a captured session (honest, durable forget). Idempotent by
    /// construction daemon-side — deleting an unknown-but-valid id is a clean `Ok`.
    pub async fn delete_session(&self, onboarded: bool, session_id: String) -> Result<(), EngineOpError> {
        self.unit(Request::DeleteSession { onboarded, session_id }).await
    }

    /// Mirrors `ListNotes`: the current (non-superseded) `remember` notes (`NoteWire` → [`Note`]).
    pub async fn list_notes(&self, onboarded: bool) -> Result<Vec<Note>, EngineOpError> {
        match self.request(Request::ListNotes { onboarded }).await? {
            Response::ListNotes(ns) => Ok(ns.into_iter().map(note_from_wire).collect()),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `SupersedeNote`: replace a note's text, yielding the new (superseding) event id. An
    /// unknown / non-note / already-superseded `event_id` maps to the typed [`EngineOpError::Rejected`]
    /// (the daemon lets the core primitive's message flow through with the App-supplied id).
    pub async fn supersede_note(
        &self,
        onboarded: bool,
        event_id: String,
        text: String,
    ) -> Result<String, EngineOpError> {
        match self.request(Request::SupersedeNote { onboarded, event_id, text }).await? {
            Response::Superseded(id) => Ok(id),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `RecallStats`: recall hit/miss telemetry for the retrieval-floor tuning UI
    /// (`RecallStatsWire` → [`RecallStats`]).
    pub async fn recall_stats(&self, onboarded: bool) -> Result<RecallStats, EngineOpError> {
        match self.request(Request::RecallStats { onboarded }).await? {
            Response::RecallStats(s) => Ok(recall_stats_from_wire(s)),
            other => Err(unexpected(other)),
        }
    }

    // ── Rung-5 SP-V1 export (§2.3/§2.4). App-only ops (the daemon's role gate denies `MemoryClient`
    //    for all three: a guest may read memories, but never read the brain key, write the identity
    //    binding, or seal an export). `onboarded` is THREADED from the caller like every sibling. ──

    /// Mirrors the daemon's `BrainVerifyingKey` (§2.3): this brain's ACTUAL verifying key, multibase
    /// base58btc. The app must mint the identity binding over the value it reads HERE — never over a
    /// remembered or app-derived key, or the daemon's brain-key check refuses the card.
    pub async fn brain_verifying_key(&self, onboarded: bool) -> Result<String, EngineOpError> {
        match self.request(Request::BrainVerifyingKey { onboarded }).await? {
            Response::BrainVerifyingKey(k) => Ok(k),
            other => Err(unexpected(other)),
        }
    }

    /// Mirrors `SetBinding` (§2.3): store the app-minted identity binding (the ID card). The daemon
    /// VALIDATES before storing (purpose tag, identity signature over `jcs(payload)`, brain-key
    /// match, owner did, first-write-wins epoch) and returns a typed [`EngineOpError::Rejected`] for
    /// each refusal — including "binding epoch N already stored", which the caller treats as the
    /// idempotent success it is.
    pub async fn set_binding(
        &self,
        onboarded: bool,
        attestation: serde_json::Value,
    ) -> Result<(), EngineOpError> {
        self.unit(Request::SetBinding { onboarded, attestation }).await
    }

    /// Mirrors `ExportBundle` (§2.4): build a sealed `.airmem` for `selection`, returned as its
    /// canonical JSON text. Pure-read (the export runs `verify_chain` first and never seals from a
    /// log that does not verify).
    ///
    /// [`Response::BundleTooLarge`] is a typed SIZE REFUSAL, not a fault, so it folds into
    /// [`EngineOpError::Rejected`] carrying [`bundle_too_large_message`]'s limit-aware text — the app
    /// shows "select fewer memories", never "engine unavailable".
    pub async fn export_bundle(
        &self,
        onboarded: bool,
        selection: ExportSelectionWire,
    ) -> Result<String, EngineOpError> {
        match self.request(Request::ExportBundle { onboarded, selection }).await? {
            Response::Bundle(text) => Ok(text),
            Response::BundleTooLarge { bytes, max, limit } => {
                Err(EngineOpError::Rejected(bundle_too_large_message(bytes, max, limit)))
            }
            other => Err(unexpected(other)),
        }
    }

    // ── Internal request helpers ─────────────────────────────────────────────

    /// Send `req` and unwrap the daemon's non-success signals to a typed error. Success and
    /// value-carrying responses pass through for the caller to destructure; the three error signals
    /// (`NotOnboarded`/`Busy`/`Err`) become the mapped [`EngineOpError`]. A transport failure passes
    /// the `Unavailable` through verbatim.
    async fn request(&self, req: Request) -> Result<Response, EngineOpError> {
        match self.transport.request(req).await? {
            resp @ (Response::NotOnboarded | Response::Busy(_) | Response::Err { .. }) => {
                Err(op_error_from_response(resp))
            }
            ok => Ok(ok),
        }
    }

    /// A unit-returning op: `Response::Ok` → `Ok(())`, the error signals → the mapped error, any
    /// unexpected value-carrying response → `Core` (protocol drift — should be unreachable).
    async fn unit(&self, req: Request) -> Result<(), EngineOpError> {
        match self.request(req).await? {
            Response::Ok => Ok(()),
            other => Err(unexpected(other)),
        }
    }
}

// ── Error mapping ────────────────────────────────────────────────────────────

/// The single response→error chokepoint (see the module docs). ONLY the three error signals are
/// valid inputs; a value/`Ok` response is a caller bug (it should have been destructured), so it
/// maps to a `Core` protocol-drift error rather than silently succeeding.
fn op_error_from_response(resp: Response) -> EngineOpError {
    match resp {
        // The daemon's NotOnboarded mirrors the in-process gate's `Open(NotOnboarded)`, so callers
        // and their tests see the identical typed error they do against the in-process engine.
        Response::NotOnboarded => EngineOpError::Open(EngineError::NotOnboarded),
        Response::Busy(s) => EngineOpError::Busy(busy_op_from_str(&s)),
        Response::Err { kind, message } => op_error_from_wire(kind, message),
        other => EngineOpError::Core(format!("unexpected daemon response: {other:?}")),
    }
}

/// Rebuild the EXACT typed error from the wire kind + inner message — the inverse of the daemon's
/// `op_error_response`/`engine_error_response`. This is what keeps app-side Display strings
/// byte-identical to the in-process engine's (the string-parity contract): the wire never carries a
/// pre-rendered Display string, so each prefix is applied exactly once, here, by the rebuilt
/// variant's own `Display`.
///
/// The three `EngineError` kinds wrap into `Open(..)` — on the op path that is exactly where they
/// came from (`EngineOpError::Open` open-failures), and `Open`'s Display is a passthrough, so
/// parity holds. `Join` maps to `EngineOpError::Join`: the op-path producer may have been either
/// `EngineOpError::Join` or `Open(EngineError::Join)`, but both render "engine task error: …", so
/// one arm serves both losslessly (string-wise).
fn op_error_from_wire(kind: OpErrorKindWire, message: String) -> EngineOpError {
    match kind {
        OpErrorKindWire::Core => EngineOpError::Core(message),
        OpErrorKindWire::Embedder => EngineOpError::Embedder(message),
        OpErrorKindWire::Reasoner => EngineOpError::Reasoner(message),
        OpErrorKindWire::Stale => EngineOpError::Stale(message),
        OpErrorKindWire::Revoked => EngineOpError::Revoked(message),
        OpErrorKindWire::NeedsLoudConfirm => EngineOpError::NeedsLoudConfirm(message),
        OpErrorKindWire::Rejected => EngineOpError::Rejected(message),
        OpErrorKindWire::Join => EngineOpError::Join(message),
        OpErrorKindWire::Vault => EngineOpError::Open(EngineError::Vault(message)),
        OpErrorKindWire::KeystoreInconsistent => {
            EngineOpError::Open(EngineError::KeystoreInconsistent)
        }
        OpErrorKindWire::KeystoreDbMismatch => {
            EngineOpError::Open(EngineError::KeystoreDbMismatch(message))
        }
        // The app is always `App`, so it never receives this; the arm keeps the match exhaustive.
        OpErrorKindWire::NotPermitted => EngineOpError::Core(message),
    }
}

/// Render a [`Response::BundleTooLarge`] for the owner, LIMIT-AWARE (proto's own requirement): the
/// two refusal paths measure genuinely different quantities, so `bytes`/`max` are meaningless
/// without the tag. [`BundleLimitWire::ExportBytes`] counts the `.airmem` text itself;
/// [`BundleLimitWire::WireFrame`] counts that text after JSON-escaping into the daemon's reply
/// frame (≈2–4× larger). Conflating them would tell the owner a size and a ceiling that do not
/// describe the same thing, so each arm names WHAT was measured. Both are fixed the same way, and
/// the fix is the last sentence of both messages.
fn bundle_too_large_message(bytes: u64, max: u64, limit: BundleLimitWire) -> String {
    match limit {
        BundleLimitWire::ExportBytes => format!(
            "too big to export: the signed file would be {bytes} bytes, over the {max}-byte limit. \
             Select fewer memories and try again."
        ),
        BundleLimitWire::WireFrame => format!(
            "too big to export: the reply carrying the signed file would be {bytes} bytes, over the \
             {max}-byte limit (packing it for delivery makes it larger than the file itself). \
             Select fewer memories and try again."
        ),
    }
}

/// A value/`Ok` response arriving where an op expected its OWN variant is protocol drift (a client
/// vs daemon dispatch-table mismatch). Surface it as `Core` rather than panicking.
fn unexpected(resp: Response) -> EngineOpError {
    EngineOpError::Core(format!("unexpected daemon response: {resp:?}"))
}

/// Teardown's response→error mapping (`EngineError`, not `EngineOpError`) — the inverse of the
/// daemon's `engine_error_response`. Factored out of `teardown` so the string-parity test can drive
/// it directly. Teardown's engine surface only produces the four `EngineError` kinds (+ the
/// NotOnboarded signal, unreachable from teardown itself); an op-shaped kind arriving here is
/// protocol drift and folds into `Vault` (the engine's generic I/O variant) carrying the message.
fn teardown_error_from_response(resp: Response) -> EngineError {
    match resp {
        Response::NotOnboarded => EngineError::NotOnboarded,
        Response::Err { kind, message } => match kind {
            OpErrorKindWire::Vault => EngineError::Vault(message),
            OpErrorKindWire::KeystoreInconsistent => EngineError::KeystoreInconsistent,
            OpErrorKindWire::KeystoreDbMismatch => EngineError::KeystoreDbMismatch(message),
            OpErrorKindWire::Join => EngineError::Join(message),
            _ => EngineError::Vault(message),
        },
        other => EngineError::Vault(format!("unexpected response to teardown: {other:?}")),
    }
}

/// Map the daemon's `Busy` op literal back to the engine's `&'static str`. The engine only ever
/// names `"ingest"` / `"evolve"` (the two serialized locks). An UNKNOWN string defaults to
/// `"ingest"` — a documented, sensible fallback: it keeps the error a benign "already running"
/// (never a hard fault) even if a future op family introduces a new busy name before this match is
/// updated. The `&'static str` requirement forbids echoing the wire string verbatim.
fn busy_op_from_str(s: &str) -> &'static str {
    match s {
        "evolve" => "evolve",
        "ingest" => "ingest",
        _ => "ingest",
    }
}

// ── Family-2 wire → desktop-type conversions (no `From` in proto by design; see module docs) ──

fn status_from_wire(w: EngineStatusWire) -> EngineStatus {
    EngineStatus { state: engine_state_from_wire(w.state), event_count: w.event_count, chain_ok: w.chain_ok }
}

fn engine_state_from_wire(w: EngineStateWire) -> EngineState {
    match w {
        EngineStateWire::NotOnboarded => EngineState::NotOnboarded,
        EngineStateWire::Ready => EngineState::Ready,
        EngineStateWire::KeystoreInconsistent => EngineState::KeystoreInconsistent,
        EngineStateWire::KeystoreDbMismatch => EngineState::KeystoreDbMismatch,
        EngineStateWire::ChainFailed => EngineState::ChainFailed,
    }
}

/// Desktop-side language-pack status (rung 2): the client mirror of `ModelStatusWire`. The facade
/// (`super::Engine::model_status`) returns this to the language-pack command layer (B2), which is its
/// only consumer.
pub struct ModelStatus {
    /// The loaded-vs-intended model state.
    pub state: ModelState,
    /// Live re-index progress (`(done, total)`) while a migration runs; `None` when idle.
    pub reindex: Option<(u64, u64)>,
    /// The model id currently SERVED (a `Complete` record's id, else the bundled English base id), so
    /// the card can show "Multilingual active". `None` only from a daemon predating the wire field.
    pub active_model_id: Option<String>,
}

/// `ModelStatusWire` → desktop [`ModelStatus`] (rung 2). Exhaustive on `ModelStateWire` on purpose:
/// a new wire state must force a desktop mapping here.
fn model_status_from_wire(w: ModelStatusWire) -> ModelStatus {
    let state = match w.state {
        ModelStateWire::Ok => ModelState::Ok,
        ModelStateWire::Missing { expected } => ModelState::Missing { expected },
        ModelStateWire::Mismatch { expected, loaded } => ModelState::Mismatch { expected, loaded },
        ModelStateWire::Failed { reason } => ModelState::Failed { reason },
    };
    ModelStatus {
        state,
        reindex: w.reindex.map(|p| (p.done, p.total)),
        active_model_id: w.active_model_id,
    }
}

/// The `status` swallow-default when the daemon is down: the engine's "can't open" fault state.
fn keystore_mismatch_status() -> EngineStatus {
    EngineStatus { state: EngineState::KeystoreDbMismatch, event_count: 0, chain_ok: false }
}

/// The `status` value for a bare `NotOnboarded` signal (never-erroring, matches the in-process path).
fn not_onboarded_status() -> EngineStatus {
    EngineStatus { state: EngineState::NotOnboarded, event_count: 0, chain_ok: false }
}

/// A `HitWire` (Family-1 `HitMirror` Into for the hit + the paired snippet) → desktop `HitWithText`.
fn hit_with_text_from_wire(w: HitWire) -> HitWithText {
    HitWithText { hit: w.hit.into(), text: w.text }
}

fn telemetry_from_wire(w: bossclawd_proto::types::EvolveTelemetryWire) -> EvolveTelemetry {
    EvolveTelemetry {
        last_tick_ms: w.last_tick_ms,
        error_count: w.error_count,
        last_error: w.last_error,
        last_tainted_snippets: w.last_tainted_snippets,
    }
}

fn proposal_summary_from_wire(w: ProposalSummaryWire) -> ProposalSummary {
    ProposalSummary {
        id: w.id,
        target: w.target,
        op: w.op,
        new_content_hash: w.new_content_hash,
        rationale: w.rationale,
        requires_loud_modal: w.requires_loud_modal,
        producer: w.producer,
    }
}

fn mandate_summary_from_wire(w: MandateSummaryWire) -> MandateSummary {
    MandateSummary {
        mandate_grant_id: w.mandate_grant_id,
        target: w.target,
        source_scope: w.source_scope,
        recipe: w.recipe,
        granted_at: w.granted_at,
        revoked: w.revoked,
    }
}

fn mandate_write_summary_from_wire(w: MandateWriteSummaryWire) -> MandateWriteSummary {
    MandateWriteSummary {
        file_written_id: w.file_written_id,
        target: w.target,
        written_at: w.written_at,
        undone: w.undone,
    }
}

// ── SP3 C1 Library wire → desktop conversions (Family-2; no proto `From`). ──

fn session_summary_from_wire(w: SessionSummaryWire) -> SessionSummary {
    SessionSummary {
        session_id: w.session_id,
        title: w.title,
        project: w.project,
        tool: w.tool,
        started_at: w.started_at,
        ended_at: w.ended_at,
        approx_bytes: w.approx_bytes,
    }
}

fn session_detail_from_wire(w: SessionDetailWire) -> SessionDetail {
    SessionDetail { summary: session_summary_from_wire(w.summary), markdown: w.markdown }
}

fn note_from_wire(w: NoteWire) -> Note {
    Note {
        event_id: w.event_id,
        text: w.text,
        created_at: w.created_at,
        superseded_by: w.superseded_by,
    }
}

fn recall_miss_from_wire(w: RecallMissWire) -> RecallMiss {
    RecallMiss { query: w.query, at: w.at }
}

fn recall_stats_from_wire(w: RecallStatsWire) -> RecallStats {
    RecallStats {
        total: w.total,
        misses: w.misses,
        recent_misses: w.recent_misses.into_iter().map(recall_miss_from_wire).collect(),
    }
}

fn preview_data_from_wire(w: PreviewDataWire) -> PreviewData {
    PreviewData {
        path: w.path,
        folder: w.folder,
        rationale: w.rationale,
        op: w.op,
        old_text: w.old_text,
        new_text: w.new_text,
        requires_loud_modal: w.requires_loud_modal,
        taint: w.taint,
    }
}

fn reasoner_config_from_wire(w: ReasonerConfigWire) -> ReasonerConfig {
    ReasonerConfig {
        mode: match w.mode {
            ReasonerModeWire::Local => ReasonerMode::Local,
            ReasonerModeWire::Cloud => ReasonerMode::Cloud,
        },
        provider: match w.provider {
            bossclawd_proto::types::CloudProviderWire::Anthropic => CloudProvider::Anthropic,
            bossclawd_proto::types::CloudProviderWire::OpenAiCompat => CloudProvider::OpenAiCompat,
            bossclawd_proto::types::CloudProviderWire::Gemini => CloudProvider::Gemini,
        },
        model: w.model,
        base_url: w.base_url,
    }
}

// ── Tests: a real `bossclawd` accept loop over a temp Unix socket, driven through the client. ──
// Hermetic (in-memory vault + mock providers via `bossclawd`'s `test-helpers`), NEVER the OS
// keychain. Every await is bounded by a TEST-side timeout so a hang fails the test rather than
// wedging CI.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::transport::{SocketTransport, Transport};
    use bossclawd_proto::{Request, Response};
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Generous per-call test-side bound: an op should answer near-instantly against the mock
    /// providers, so any await that takes this long is a hang (a test failure), not slowness.
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// Await `fut`, failing the test (rather than hanging CI) if it exceeds [`TEST_TIMEOUT`].
    async fn bounded<F: std::future::Future>(fut: F) -> F::Output {
        tokio::time::timeout(TEST_TIMEOUT, fut).await.expect("call must not hang")
    }

    /// A hand-driven daemon we can fully KILL and RESTART on the same socket path — for the reconnect
    /// and daemon-gone tests. Uses the SAME production accept loop (`run_accept_loop`) and hermetic
    /// engine (`test_engine`) the daemon's own roundtrip tests use.
    ///
    /// # Why a dedicated runtime on its own thread (not `tokio::spawn` on the test runtime)
    /// `run_accept_loop` `tokio::spawn`s a DETACHED task per accepted connection. Aborting just the
    /// accept-loop handle would stop NEW connections but leave every already-accepted connection task
    /// alive — so a client's persistent stream would keep talking to a live handler, and "daemon
    /// gone" would be a lie. Instead each daemon owns a `current_thread` runtime on its own OS thread;
    /// shutting that runtime down (`shutdown_timeout`) tears down the accept loop AND all its
    /// connection tasks together — a true, complete daemon death. `restart` spins a fresh one.
    struct TestDaemon {
        dir: TempDir,
        sock: std::path::PathBuf,
        rt: Option<DaemonRuntime>,
    }

    /// A running daemon runtime: the tokio runtime lives on its own thread; dropping/joining the
    /// thread (after `shutdown_timeout`) guarantees every daemon task is gone.
    struct DaemonRuntime {
        shutdown: Arc<tokio::sync::Notify>,
        thread: std::thread::JoinHandle<()>,
    }

    impl TestDaemon {
        /// Bind a fresh daemon on a temp socket under a temp home. Returns once the listener is bound
        /// (so a client connect can't race the bind).
        async fn spawn() -> Self {
            // HERMETIC: pre-seed the daemon's process-global provider-key cache to EMPTY so any op
            // that reads a provider-key fingerprint (`GetReasonerReady` → `current_key_fingerprint`)
            // short-circuits and NEVER hits the OS keychain (a keychain-ACL prompt hangs CI forever).
            // The cache is a static shared by every daemon instance, so one empty seed covers all.
            bossclawd::vault::seed_secret_cache_for_test(std::collections::HashMap::new());
            let dir = tempfile::tempdir().unwrap();
            let sock = dir.path().join("bossclawd.sock");
            let rt = Self::start_runtime(&sock, dir.path().to_path_buf());
            Self { dir, sock, rt: Some(rt) }
        }

        /// Start a daemon on its own current-thread runtime + OS thread. Binds the listener on that
        /// thread and blocks until it is bound (a `SyncSender` handshake) before returning, so the
        /// caller can connect immediately without a bind race.
        fn start_runtime(sock: &std::path::Path, home: std::path::PathBuf) -> DaemonRuntime {
            // Stage an onboarded-identity marker (`identity.json`, the daemon's `METADATA_FILE`) in the
            // daemon home so the SP3 C1 App-only ops (ListSessions / GetSession / DeleteSession /
            // ListNotes / SupersedeNote) see an onboarded brain: unlike the pre-SP3 ops (which trust
            // the client-passed `onboarded` flag), those resolve onboarding from the daemon's OWN
            // `is_onboarded_local()` verdict (A13), which reads this file. Mirrors the daemon's own
            // `tests/forget.rs` fixture; harmless to every pre-SP3 op (they never read it).
            std::fs::create_dir_all(&home).expect("daemon home dir");
            std::fs::write(
                home.join("identity.json"),
                serde_json::json!({
                    "did": "did:wba:example.com:tester",
                    "name": "Tester",
                    "created_at": "2026-07-11T00:00:00+00:00"
                })
                .to_string(),
            )
            .expect("stage onboarded identity marker");
            let shutdown = Arc::new(tokio::sync::Notify::new());
            let shutdown_for_thread = shutdown.clone();
            let sock = sock.to_path_buf();
            let (bound_tx, bound_rx) = std::sync::mpsc::sync_channel::<()>(0);
            let thread = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build daemon runtime");
                rt.block_on(async move {
                    let listener =
                        tokio::net::UnixListener::bind(&sock).expect("bind test socket");
                    let engine = Arc::new(bossclawd::server::test_engine(home));
                    // Signal "bound" AFTER the listener exists so the client can't race the bind.
                    bound_tx.send(()).expect("signal bound");
                    // Serve until this daemon is told to shut down. `select!` cancels the accept loop
                    // on notify; dropping `rt` right after tears down every spawned connection task.
                    tokio::select! {
                        _ = bossclawd::server::run_accept_loop(engine, listener) => {}
                        _ = shutdown_for_thread.notified() => {}
                    }
                });
                // Runtime dropped here (end of scope) → all daemon tasks are gone.
            });
            bound_rx.recv().expect("daemon bound");
            DaemonRuntime { shutdown, thread }
        }

        /// Fully kill the daemon: notify the accept loop to stop, join the runtime thread (which drops
        /// the runtime and every connection task with it), and remove the socket file so a fresh
        /// connect fails (ENOENT) rather than attaching to a dead listener.
        fn kill(&mut self) {
            if let Some(rt) = self.rt.take() {
                // notify_one, NOT notify_waiters: notify_one banks a permit when no waiter is
                // registered yet, so a kill racing the daemon thread's first `notified()` poll
                // (`start_runtime` returns at the bind handshake, before the thread reaches
                // `select!`) can never be lost (notify_waiters stores no permit and would wedge
                // join()). Same fix as memharness's TestDaemon (crates/memharness/src/daemon.rs).
                rt.shutdown.notify_one();
                rt.thread.join().expect("daemon thread joins");
            }
            let _ = std::fs::remove_file(&self.sock);
        }

        /// Restart a killed daemon on the SAME socket path. Uses a FRESH temp home: the hermetic
        /// `test_engine` keeps its DEK in an in-memory `TestVault` that dies with the killed runtime,
        /// so reopening the OLD brain.db under a new (fresh-DEK) vault would fail to decrypt. The
        /// reconnect test only needs to prove the TRANSPORT reconnects to a LIVE daemon, so a fresh
        /// Ready brain is the right fixture; cross-restart persistence is the daemon's own concern
        /// (it shares the OS keychain in production), out of scope for this transport test.
        fn restart(&mut self) {
            assert!(self.rt.is_none(), "restart called on a live daemon");
            let fresh_home = self.dir.path().join("restart-home");
            std::fs::create_dir_all(&fresh_home).expect("fresh restart home");
            self.rt = Some(Self::start_runtime(&self.sock, fresh_home));
        }

        /// A client over a real `SocketTransport` pointed at this daemon's socket.
        fn client(&self) -> EngineClient<SocketTransport> {
            EngineClient::new(Arc::new(SocketTransport::new(self.sock.clone())))
        }
    }

    impl Drop for TestDaemon {
        /// Clean up the daemon thread if a test didn't `kill()` it explicitly (so no thread leaks
        /// across tests).
        fn drop(&mut self) {
            if let Some(rt) = self.rt.take() {
                // notify_one for the same lost-wakeup reason as `kill()` above.
                rt.shutdown.notify_one();
                let _ = rt.thread.join();
            }
        }
    }

    // ── The specced RED (Step 1): status + recall return the SAME shapes the in-process Engine did. ──

    #[tokio::test]
    async fn status_shape_parity_over_socket() {
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        // Onboarded → fresh brain opens Ready with exactly the 6 primed config events + intact chain,
        // IDENTICAL to `EngineHandle::status` (see `mod.rs::onboarded_opens_fresh_brain_and_memoizes`).
        // (SP3 A8 → 4; Rung-3 Phase-2 conflict-detect force-off → 5; Rung-4 R4-A reflect force-off → 6.)
        let st = bounded(client.status(true)).await;
        assert!(matches!(st.state, EngineState::Ready), "state was {:?}", st.state);
        assert_eq!(st.event_count, 6, "prime_switches wrote the 6 sticky config events");
        assert!(st.chain_ok, "fresh brain chain verifies");
        // Not-onboarded → the NotOnboarded state (never-erroring), same as the in-process Engine.
        let st = bounded(client.status(false)).await;
        assert!(matches!(st.state, EngineState::NotOnboarded));
    }

    #[tokio::test]
    async fn recall_shape_parity_over_socket() {
        let daemon = TestDaemon::spawn().await;
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.txt"), "ferris the crab loves rust").unwrap();
        let client = daemon.client();

        bounded(client.add_grant(true, src.path().to_path_buf())).await.expect("add_grant");
        bounded(client.run_ingest(true)).await.expect("run_ingest");
        let hits = bounded(client.recall(true, "ferris crab".to_string(), 5)).await.expect("recall");
        // Same shape as `EngineHandle::recall`: `Vec<HitWithText>` with the hydrated snippet text.
        assert!(hits.iter().any(|h| h.text.contains("ferris")), "recall hydrates the ingested snippet");
    }

    // ── Daemon-drop → reconnect: after the daemon restarts, the NEXT call succeeds (never hangs). ──

    #[tokio::test]
    async fn reconnects_after_daemon_drop() {
        let mut daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        // Prime a live connection.
        assert!(matches!(bounded(client.status(true)).await.state, EngineState::Ready));
        // Daemon vanishes, then comes back on the same socket.
        daemon.kill();
        daemon.restart();
        // The next call must reconnect transparently and succeed — NOT hang, NOT error out.
        let st = bounded(client.status(true)).await;
        assert!(matches!(st.state, EngineState::Ready), "reconnect restored the engine");
    }

    // ── Daemon gone (no restart) → a fallible op returns Unavailable, and NEVER hangs. ──

    #[tokio::test]
    async fn unavailable_when_daemon_gone() {
        let mut daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        assert!(matches!(bounded(client.status(true)).await.state, EngineState::Ready));
        daemon.kill(); // gone for good — no restart.
        // A fallible op maps the transport failure to Unavailable (bounded — must not hang).
        let err = bounded(client.add_grant(true, std::path::PathBuf::from("/tmp/x")))
            .await
            .expect_err("daemon gone → Err");
        assert!(matches!(err, EngineOpError::Unavailable(_)), "got {err:?}");
    }

    // ── Clean-shutdown contract for BOTH notify sites, at the tightest spawn→shutdown timing (no
    // client op in between, so shutdown lands right after the bind handshake). This is the window
    // the `notify_one` fix protects: `notify_one` banks a permit if the daemon thread hasn't yet
    // registered its `notified()` waiter, so neither `kill()` (explicit) nor `Drop` (implicit) can
    // lose the signal and wedge `join()`. `bounded` turns any regression into a test failure within
    // TEST_TIMEOUT rather than a hung CI job. NOTE: the race is scheduler-timed and not
    // deterministically reproducible here — the standing guard is the `notify_one` fix + its
    // comment; this test proves the shutdown path stays clean and non-hanging. ──

    #[tokio::test]
    async fn shutdown_after_spawn_completes_cleanly() {
        // Explicit kill(): notify + join must complete, and the socket is removed.
        let mut killed = TestDaemon::spawn().await;
        let sock = killed.sock.clone();
        bounded(tokio::task::spawn_blocking(move || killed.kill()))
            .await
            .expect("kill+join completes");
        assert!(!sock.exists(), "kill() removes the socket");

        // Implicit Drop (no kill()): dropping must also notify + join without wedging.
        let dropped = TestDaemon::spawn().await;
        bounded(tokio::task::spawn_blocking(move || drop(dropped)))
            .await
            .expect("Drop's notify + join completes");
    }

    // ── Fix 2 (Med2): `status(false)` short-circuits to NotOnboarded WITHOUT a transport call. ──
    // The in-process `get_or_open` gate returns NotOnboarded before opening anything when
    // onboarded=false; the client must match that EXACTLY — even with NO daemon behind the socket it
    // returns the clean `NotOnboarded`, never the `KeystoreDbMismatch` fault it uses for a real
    // transport failure. Driven against a NEVER-bound socket: were the short-circuit removed, the
    // request would fail transport-side and this would (correctly) fail with `KeystoreDbMismatch`.

    #[tokio::test]
    async fn status_not_onboarded_short_circuits_without_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("bossclawd.sock"); // never bound — nobody is listening.
        let client: EngineClient<SocketTransport> =
            EngineClient::new(Arc::new(SocketTransport::new(sock)));
        // No daemon at all, yet onboarded=false returns the clean NotOnboarded (no transport call).
        let st = bounded(client.status(false)).await;
        assert!(
            matches!(st.state, EngineState::NotOnboarded),
            "status(false) must short-circuit to NotOnboarded even with no daemon, got {:?}",
            st.state
        );
    }

    // ── Swallow-behavior parity under an unavailable daemon: `_or_false`/`_or_default` don't error. ──

    #[tokio::test]
    async fn or_default_and_or_false_swallow_when_unavailable() {
        let mut daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        assert!(matches!(bounded(client.status(true)).await.state, EngineState::Ready));
        daemon.kill();
        // `reasoner_ready_or_false` must return false (fail-closed), NOT surface an error.
        assert!(!bounded(client.reasoner_ready_or_false(true)).await, "ready swallows to false");
        // `reasoner_config_or_default` must return the Local default, NOT surface an error.
        let cfg = bounded(client.reasoner_config_or_default(true)).await;
        assert!(matches!(cfg.mode, ReasonerMode::Local), "config swallows to Local");
        // `status` also never errors — it degrades to the "can't open" fault state.
        let st = bounded(client.status(true)).await;
        assert!(matches!(st.state, EngineState::KeystoreDbMismatch), "status swallows to fault state");
    }

    // ── NotOnboarded mapping: a gated op with onboarded=false → Open(NotOnboarded). ──

    #[tokio::test]
    async fn not_onboarded_maps_to_open_not_onboarded() {
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        let err = bounded(client.list_grants(false)).await.expect_err("gated op not-onboarded errors");
        assert!(matches!(err, EngineOpError::Open(EngineError::NotOnboarded)), "got {err:?}");
    }

    // ── Busy mapping: `Response::Busy(op)` → `EngineOpError::Busy(&'static str)`. ──
    // Driven with a fake transport so the mapping is deterministic (racing two real ingests is flaky).

    struct BusyTransport(&'static str);
    #[async_trait::async_trait]
    impl Transport for BusyTransport {
        async fn request(&self, _req: Request) -> Result<Response, EngineOpError> {
            Ok(Response::Busy(self.0.to_string()))
        }
    }

    #[tokio::test]
    async fn busy_ingest_maps_to_busy_ingest() {
        let client = EngineClient::new(Arc::new(BusyTransport("ingest")));
        let err = bounded(client.run_ingest(true)).await.expect_err("Busy → Err");
        assert!(matches!(err, EngineOpError::Busy("ingest")), "got {err:?}");
    }

    #[tokio::test]
    async fn busy_evolve_maps_to_busy_evolve() {
        let client = EngineClient::new(Arc::new(BusyTransport("evolve")));
        let err = bounded(client.evolve_once(true)).await.expect_err("Busy → Err");
        assert!(matches!(err, EngineOpError::Busy("evolve")), "got {err:?}");
    }

    #[tokio::test]
    async fn busy_unknown_op_falls_back_to_ingest() {
        // An unknown Busy string defaults to the "ingest" fallback (documented in `busy_op_from_str`).
        let client = EngineClient::new(Arc::new(BusyTransport("something-new")));
        let err = bounded(client.run_ingest(true)).await.expect_err("Busy → Err");
        assert!(matches!(err, EngineOpError::Busy("ingest")), "got {err:?}");
    }

    // ── Err mapping: a Core-kind `Response::Err` maps to `EngineOpError::Core` with the INNER
    //    message (the full per-kind matrix is covered by the string-parity tests above). ──

    struct ErrTransport;
    #[async_trait::async_trait]
    impl Transport for ErrTransport {
        async fn request(&self, _req: Request) -> Result<Response, EngineOpError> {
            Ok(Response::Err { kind: OpErrorKindWire::Core, message: "engine boom".to_string() })
        }
    }

    #[tokio::test]
    async fn err_response_maps_to_core() {
        let client = EngineClient::new(Arc::new(ErrTransport));
        let err = bounded(client.add_grant(true, std::path::PathBuf::from("/x"))).await.expect_err("Err → Err");
        match err {
            EngineOpError::Core(m) => assert_eq!(m, "engine boom"),
            other => panic!("expected Core, got {other:?}"),
        }
    }

    // ── String-parity regression guard (the Task 6 acceptance bar): every typed error the daemon
    //    can produce must cross the wire and Display EXACTLY as the in-process engine displayed it.
    //    Drives the REAL daemon-side mapping (`op_error_response`/`engine_error_response`), the real
    //    wire encode/decode, and the real client mapping — no socket needed. A double prefix (e.g.
    //    "engine error: engine error: …") or a lost typed variant fails here.

    #[test]
    fn typed_op_errors_survive_the_wire_string_parity() {
        use bossclawd::engine::{EngineError as DaemonEngineError, EngineOpError as DaemonOpError};
        let cases: Vec<DaemonOpError> = vec![
            DaemonOpError::Core("boom".into()),
            DaemonOpError::Embedder("model missing".into()),
            DaemonOpError::Reasoner("provider build failed".into()),
            DaemonOpError::Stale("file changed since propose".into()),
            DaemonOpError::Revoked("write grant revoked".into()),
            DaemonOpError::NeedsLoudConfirm("secret-shaped change".into()),
            DaemonOpError::Rejected("recipe too long".into()),
            DaemonOpError::Join("task panicked".into()),
            DaemonOpError::Busy("ingest"),
            DaemonOpError::Busy("evolve"),
            DaemonOpError::Open(DaemonEngineError::NotOnboarded),
            DaemonOpError::Open(DaemonEngineError::KeystoreInconsistent),
            DaemonOpError::Open(DaemonEngineError::KeystoreDbMismatch("wrong dek".into())),
            DaemonOpError::Open(DaemonEngineError::Vault("keychain io".into())),
            DaemonOpError::Open(DaemonEngineError::Join("open join failed".into())),
        ];
        for e in cases {
            // The in-process string TODAY (the daemon engine is a verbatim copy of the desktop's,
            // so this is exactly what the UI showed before the daemon existed).
            let expected = e.to_string();
            // Daemon-side mapping → wire bytes → client-side Response → typed EngineOpError.
            let resp = bossclawd::server::op_error_response(e);
            let bytes = serde_json::to_vec(&resp).expect("encode response");
            let back: Response = serde_json::from_slice(&bytes).expect("decode response");
            let mapped = op_error_from_response(back);
            assert_eq!(
                mapped.to_string(),
                expected,
                "typed error must survive the wire byte-for-byte"
            );
        }
    }

    #[test]
    fn teardown_engine_errors_survive_the_wire_string_parity() {
        use bossclawd::engine::EngineError as DaemonEngineError;
        let cases: Vec<DaemonEngineError> = vec![
            DaemonEngineError::Vault("keychain io".into()),
            DaemonEngineError::KeystoreInconsistent,
            DaemonEngineError::KeystoreDbMismatch("wrong dek".into()),
            DaemonEngineError::Join("teardown join failed".into()),
            DaemonEngineError::NotOnboarded,
        ];
        for e in cases {
            let expected = e.to_string();
            let resp = bossclawd::server::engine_error_response(e);
            let bytes = serde_json::to_vec(&resp).expect("encode response");
            let back: Response = serde_json::from_slice(&bytes).expect("decode response");
            let mapped = teardown_error_from_response(back);
            assert_eq!(
                mapped.to_string(),
                expected,
                "teardown error must survive the wire byte-for-byte"
            );
        }
    }

    // ── Teardown maps a daemon Err to EngineError::Vault (its distinct error type). ──

    #[tokio::test]
    async fn teardown_ok_roundtrips() {
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        // A fresh brain teardown succeeds (deletes unconditionally) — Ok, distinct EngineError type.
        bounded(client.teardown()).await.expect("teardown Ok");
    }

    // ── Per-family roundtrips through the client. ──

    #[tokio::test]
    async fn grants_family_roundtrip() {
        let daemon = TestDaemon::spawn().await;
        let src = tempfile::tempdir().unwrap();
        let client = daemon.client();
        assert!(bounded(client.list_grants(true)).await.expect("list").is_empty());
        bounded(client.add_grant(true, src.path().to_path_buf())).await.expect("add_grant");
        let grants = bounded(client.list_grants(true)).await.expect("list_grants");
        assert_eq!(grants.len(), 1);
        assert!(!grants[0].revoked, "new grant is active");
        // writable + files families round-trip too (empty on a grant-only brain).
        assert!(bounded(client.list_writable(true)).await.expect("list_writable").is_empty());
        assert!(bounded(client.list_files(true)).await.expect("list_files").is_empty());
    }

    #[tokio::test]
    async fn ingest_and_evolve_family_roundtrip() {
        let daemon = TestDaemon::spawn().await;
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.txt"), "hello world").unwrap();
        let client = daemon.client();
        bounded(client.add_grant(true, src.path().to_path_buf())).await.expect("add_grant");
        let report = bounded(client.run_ingest(true)).await.expect("run_ingest");
        assert_eq!(report.ingested, 1, "one file ingested");
        assert_eq!(bounded(client.list_files(true)).await.expect("list_files").len(), 1);
        // Evolve status shape: core status + telemetry, matching `EngineHandle::evolve_status`.
        let (status, telemetry) = bounded(client.evolve_status(true)).await.expect("evolve_status");
        assert!(!status.enabled, "evolve primed off");
        assert_eq!(telemetry.error_count, 0);
        // Toggle + read-back the mandates flag (unit op + bool op round-trip).
        bounded(client.set_mandates_enabled(true, true)).await.expect("set_mandates_enabled");
        assert!(bounded(client.mandates_enabled(true)).await.expect("mandates_enabled"));
        // A manual evolve tick over an empty queue is a valid no-op report.
        let tick = bounded(client.evolve_once(true)).await.expect("evolve_once");
        assert_eq!(tick.memories_processed, 0, "empty-queue tick processes nothing");
    }

    #[tokio::test]
    async fn set_and_get_capture_enabled_over_the_socket() {
        // SP3 B5: the capture flag round-trips through the real hermetic daemon. A fresh brain has
        // capture CLOSED (A8 default-off + boot force-off); enabling it (the Connect call shape,
        // backfill=true) flips the read to true.
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        assert!(!bounded(client.capture_enabled(true)).await.expect("capture_enabled"), "default closed");
        bounded(client.set_capture_enabled(true, true, true)).await.expect("set_capture_enabled");
        assert!(bounded(client.capture_enabled(true)).await.expect("capture_enabled"), "enabled reads back");
    }

    #[tokio::test]
    async fn set_and_get_reflect_enabled_over_the_socket() {
        // Rung-4 R4-A: the reflect flag round-trips through the real hermetic daemon. A fresh brain has
        // reflection CLOSED (default-off + boot force-off, mirroring capture); enabling it (a plain bool
        // switch — no backfill) flips the read to true.
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        assert!(!bounded(client.reflect_enabled(true)).await.unwrap(), "default CLOSED");
        bounded(client.set_reflect_enabled(true, true)).await.unwrap();
        assert!(bounded(client.reflect_enabled(true)).await.unwrap(), "reads back enabled");
    }

    #[tokio::test]
    async fn proposals_family_roundtrip_empty() {
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        // No proposals on a fresh brain — the list op round-trips to an empty Vec (shape parity).
        assert!(bounded(client.list_proposals(true)).await.expect("list_proposals").is_empty());
        // mandate_writes + list_mandates also empty.
        assert!(bounded(client.mandate_writes(true)).await.expect("mandate_writes").is_empty());
        assert!(bounded(client.list_mandates(true)).await.expect("list_mandates").is_empty());
    }

    #[tokio::test]
    async fn mandate_family_rejected_maps_typed() {
        // A mandate whose target is NOT write-granted is refused by the engine's grant-time guard as
        // the TYPED `Rejected` — and the typed wire (`OpErrorKindWire::Rejected`) carries it across
        // intact, so the New-mandate form can still style it as a validation error, exactly as with
        // the in-process engine. (The full `AddMandate → MandateSummary` happy path is covered by
        // the daemon's own roundtrip suite.)
        let daemon = TestDaemon::spawn().await;
        let scope = tempfile::tempdir().unwrap();
        let client = daemon.client();
        bounded(client.add_grant(true, scope.path().to_path_buf())).await.expect("read-grant scope");
        let err = bounded(client.add_mandate(
            true,
            std::path::PathBuf::from("/definitely/not/granted.md"),
            scope.path().to_path_buf(),
            "recipe".to_string(),
        ))
        .await
        .expect_err("ungranted target → error");
        assert!(matches!(err, EngineOpError::Rejected(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn reasoner_config_family_roundtrip() {
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        // Default is Local, fail-safe (shape parity with `reasoner_config_or_default`).
        let cfg = bounded(client.reasoner_config_or_default(true)).await;
        assert!(matches!(cfg.mode, ReasonerMode::Local));
        // A Local config is not cloud-ready (fail-closed).
        assert!(!bounded(client.reasoner_ready_or_false(true)).await);
        // Persist a Cloud config (config only — no consent) and read it back through the client.
        bounded(client.set_reasoner_config(
            true,
            serde_json::json!({"mode":"cloud","provider":"anthropic","model":"claude-sonnet-4-6","base_url":null}),
        ))
        .await
        .expect("set_reasoner_config");
        let cfg = bounded(client.reasoner_config_or_default(true)).await;
        assert!(matches!(cfg.mode, ReasonerMode::Cloud), "cloud config persisted");
        assert_eq!(cfg.model, "claude-sonnet-4-6");
        // Still not ready (fail-closed R1 — no signed consent).
        assert!(!bounded(client.reasoner_ready_or_false(true)).await);
    }

    // ── M1a Task 7: fail-closed MID-request. The existing `reconnects_after_daemon_drop` and
    //    `unavailable_when_daemon_gone` kill the daemon BETWEEN calls; this one kills it while a
    //    request is ON THE WIRE (the daemon received the request frame but dies before replying), so
    //    the client's in-flight `read_frame` sees EOF. It must surface `Unavailable` within the bound
    //    — never hang, never panic. Driven with a purpose-built fake daemon (not the mock-provider
    //    `TestDaemon`, whose ops answer too fast to reliably race a kill against).

    /// A fake daemon that completes the `Hello`/`HelloOk` handshake, READS the first request frame,
    /// then DROPS the connection WITHOUT replying — the on-wire equivalent of the daemon being killed
    /// exactly mid-request. It serves ONE connection then stops listening, so the transport's
    /// reconnect-once also fails (daemon truly gone) → the client maps it to `Unavailable`.
    async fn spawn_dies_mid_request_daemon() -> (TempDir, std::path::PathBuf) {
        use bossclawd_proto::{read_frame, write_frame, Hello, HelloOk, PROTO_VERSION};
        use tokio::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("bossclawd.sock");
        let listener = UnixListener::bind(&sock).expect("bind fake daemon");
        tokio::spawn(async move {
            // Serve exactly one connection, then the listener drops (gone for good).
            let (mut stream, _addr) = listener.accept().await.expect("accept");
            // Real handshake so the client gets past connect and issues its request.
            let _hello: Hello =
                serde_json::from_slice(&read_frame(&mut stream).await.expect("read Hello"))
                    .expect("parse Hello");
            let hello_ok = HelloOk { pid: std::process::id(), proto_version: PROTO_VERSION };
            write_frame(&mut stream, &serde_json::to_vec(&hello_ok).unwrap())
                .await
                .expect("send HelloOk");
            // Receive the request frame (it's now "on the wire", the daemon has it) ...
            let _req = read_frame(&mut stream).await.expect("read request frame");
            // ... then die mid-request: drop the connection WITHOUT replying. The client's in-flight
            // `read_frame` sees EOF; the listener drops at end of task, so the reconnect fails too.
            drop(stream);
        });
        (dir, sock)
    }

    #[tokio::test]
    async fn unavailable_when_daemon_dies_mid_request() {
        let (_dir, sock) = spawn_dies_mid_request_daemon().await;
        let client: EngineClient<SocketTransport> =
            EngineClient::new(Arc::new(SocketTransport::new(sock)));
        // A fallible op whose request the daemon receives then dies before answering. The transport
        // reconnects once (fails — daemon gone) and maps to Unavailable. Bounded: MUST NOT hang.
        let err = bounded(client.add_grant(true, std::path::PathBuf::from("/tmp/x")))
            .await
            .expect_err("daemon dies mid-request → Err");
        assert!(matches!(err, EngineOpError::Unavailable(_)), "got {err:?}");
    }

    // ── SP3 C1: the Memory-browser Library ops round-trip through the real hermetic daemon. These
    //    assert fresh-brain SHAPE PARITY (empty lists / zeroed stats) + the typed rejects the UI
    //    distinguishes; the populated happy-path FIELD MAPPING is asserted by the `CannedTransport`
    //    tests below (and the daemon's own roundtrip suite). Mirrors `proposals_family_roundtrip_empty`
    //    + `mandate_family_rejected_maps_typed`. Every await is `bounded` so a hang fails, never wedges. ──

    #[tokio::test]
    async fn library_read_ops_roundtrip_empty() {
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        // Fresh brain: no captured sessions, no notes, zeroed recall stats (shape parity).
        assert!(bounded(client.list_sessions(true)).await.expect("list_sessions").is_empty());
        assert!(bounded(client.list_notes(true)).await.expect("list_notes").is_empty());
        let stats = bounded(client.recall_stats(true)).await.expect("recall_stats");
        assert_eq!(stats.total, 0);
        assert_eq!(stats.misses, 0);
        assert!(stats.recent_misses.is_empty());
    }

    #[tokio::test]
    async fn get_session_unknown_maps_rejected() {
        // A session id that is not a CURRENT capture (deleted / superseded / never captured) → the
        // typed `Rejected` with the specced "session not found or deleted" message, so the UI can tell
        // "already deleted" from a real fault (spec §3). Proves the op reaches the engine over the
        // socket and the daemon's clean reject maps to `EngineOpError::Rejected` intact.
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        let err = bounded(client.get_session(true, "deadbeef-1111-2222-3333-444455556666".to_string()))
            .await
            .expect_err("unknown session → Rejected");
        match err {
            EngineOpError::Rejected(m) => assert_eq!(m, "session not found or deleted"),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_session_unknown_is_idempotent_ok() {
        // Deleting an unknown-but-valid-format id removes nothing and tombstones nothing current →
        // still a clean `Ok` (idempotent by construction, A7). Proves the unit op round-trips.
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        bounded(client.delete_session(true, "deadbeef-1111-2222-3333-444455556666".to_string()))
            .await
            .expect("delete_session on an unknown id is a clean Ok");
    }

    #[tokio::test]
    async fn supersede_note_unknown_event_maps_rejected() {
        // Superseding an unknown / non-note / already-superseded event id is the core primitive's
        // typed `Rejected` (InvalidInput → Rejected in the daemon engine), carried across the wire
        // intact so the notes UI can style it as a validation error, not an engine fault.
        let daemon = TestDaemon::spawn().await;
        let client = daemon.client();
        let err = bounded(client.supersede_note(true, "no-such-event".to_string(), "new text".to_string()))
            .await
            .expect_err("unknown note event → Rejected");
        assert!(matches!(err, EngineOpError::Rejected(_)), "got {err:?}");
    }

    // ── Populated wire → desktop FIELD MAPPING, driven by a canned transport so the mapping is exact
    //    and deterministic (the hermetic daemon tests above prove reachability on empty/reject paths).
    //    This is the equivalent of the `BusyTransport`/`ErrTransport` fakes already in this module. ──

    /// A transport that returns one canned response regardless of the request — for asserting the
    /// client's `*_from_wire` field mapping precisely.
    struct CannedTransport(Response);
    #[async_trait::async_trait]
    impl Transport for CannedTransport {
        async fn request(&self, _req: Request) -> Result<Response, EngineOpError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn list_sessions_maps_wire_fields() {
        use bossclawd_proto::types::SessionSummaryWire;
        let wire = SessionSummaryWire {
            event_id: "e1".into(),
            session_id: "s1".into(),
            title: "Refactor".into(),
            project: "air-note".into(),
            tool: "claude-code".into(),
            started_at: 100,
            ended_at: 200,
            approx_bytes: 4096,
        };
        let client = EngineClient::new(Arc::new(CannedTransport(Response::ListSessions(vec![wire]))));
        let got = bounded(client.list_sessions(true)).await.expect("list_sessions");
        assert_eq!(got.len(), 1);
        let s = &got[0];
        assert_eq!(s.session_id, "s1");
        assert_eq!(s.title, "Refactor");
        assert_eq!(s.project, "air-note");
        assert_eq!(s.tool, "claude-code");
        assert_eq!(s.started_at, 100);
        assert_eq!(s.ended_at, 200);
        assert_eq!(s.approx_bytes, 4096);
    }

    #[tokio::test]
    async fn get_session_maps_detail_and_summary_fields() {
        use bossclawd_proto::types::{SessionDetailWire, SessionSummaryWire};
        let wire = SessionDetailWire {
            summary: SessionSummaryWire {
                event_id: "e2".into(),
                session_id: "s2".into(),
                title: "T".into(),
                project: "p".into(),
                tool: "claude-code".into(),
                started_at: 1,
                ended_at: 2,
                approx_bytes: 8,
            },
            markdown: "# hello\n".into(),
        };
        let client = EngineClient::new(Arc::new(CannedTransport(Response::Session(wire))));
        let d = bounded(client.get_session(true, "s2".into())).await.expect("get_session");
        assert_eq!(d.summary.session_id, "s2");
        assert_eq!(d.summary.approx_bytes, 8);
        assert_eq!(d.markdown, "# hello\n");
    }

    #[tokio::test]
    async fn list_notes_maps_wire_fields() {
        use bossclawd_proto::types::NoteWire;
        let client = EngineClient::new(Arc::new(CannedTransport(Response::ListNotes(vec![
            NoteWire { event_id: "n1".into(), text: "remember me".into(), created_at: 42, superseded_by: None },
            NoteWire {
                event_id: "n2".into(),
                text: "edited".into(),
                created_at: 43,
                superseded_by: Some("n3".into()),
            },
        ]))));
        let got = bounded(client.list_notes(true)).await.expect("list_notes");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].event_id, "n1");
        assert_eq!(got[0].text, "remember me");
        assert_eq!(got[0].created_at, 42);
        assert_eq!(got[0].superseded_by, None);
        assert_eq!(got[1].superseded_by.as_deref(), Some("n3"));
    }

    #[tokio::test]
    async fn recall_stats_maps_wire_fields_including_recent_misses() {
        use bossclawd_proto::types::{RecallMissWire, RecallStatsWire};
        let client = EngineClient::new(Arc::new(CannedTransport(Response::RecallStats(RecallStatsWire {
            total: 10,
            misses: 3,
            recent_misses: vec![
                RecallMissWire { query: "who is Aria?".into(), at: 1000 },
                RecallMissWire { query: "the port".into(), at: 2000 },
            ],
        }))));
        let s = bounded(client.recall_stats(true)).await.expect("recall_stats");
        assert_eq!(s.total, 10);
        assert_eq!(s.misses, 3);
        assert_eq!(s.recent_misses.len(), 2);
        assert_eq!(s.recent_misses[0].query, "who is Aria?");
        assert_eq!(s.recent_misses[0].at, 1000);
        assert_eq!(s.recent_misses[1].query, "the port");
    }

    #[tokio::test]
    async fn supersede_note_returns_new_event_id() {
        let client = EngineClient::new(Arc::new(CannedTransport(Response::Superseded("new-id".into()))));
        let id = bounded(client.supersede_note(true, "old-id".into(), "text".into()))
            .await
            .expect("supersede_note");
        assert_eq!(id, "new-id");
    }

    // ── Rung-5 SP-V1 export ops ──

    fn empty_selection() -> ExportSelectionWire {
        ExportSelectionWire {
            note_event_ids: vec![],
            session_event_ids: vec![],
            ingest_event_ids: vec![],
            description: String::new(),
        }
    }

    #[tokio::test]
    async fn brain_verifying_key_returns_the_daemons_key() {
        let client =
            EngineClient::new(Arc::new(CannedTransport(Response::BrainVerifyingKey("zBrain".into()))));
        let k = bounded(client.brain_verifying_key(true)).await.expect("brain_verifying_key");
        assert_eq!(k, "zBrain", "the app mints over the daemon's ACTUAL key, verbatim");
    }

    #[tokio::test]
    async fn export_bundle_returns_the_airmem_text() {
        let client = EngineClient::new(Arc::new(CannedTransport(Response::Bundle("{\"manifest\":{}}".into()))));
        let text = bounded(client.export_bundle(true, empty_selection())).await.expect("export_bundle");
        assert_eq!(text, "{\"manifest\":{}}");
    }

    /// `BundleTooLarge` is a typed SIZE REFUSAL: it must reach the UI as `Rejected` ("select fewer
    /// memories"), never as a fault — and the message must name WHICH quantity was measured, because
    /// the two limits count different things (proto's `BundleLimitWire` contract).
    #[tokio::test]
    async fn bundle_too_large_is_a_limit_aware_rejection_not_a_fault() {
        let client = EngineClient::new(Arc::new(CannedTransport(Response::BundleTooLarge {
            bytes: 31_457_281,
            max: 31_457_280,
            limit: BundleLimitWire::ExportBytes,
        })));
        let err = bounded(client.export_bundle(true, empty_selection())).await.expect_err("refused");
        let text = match &err {
            EngineOpError::Rejected(m) => m.clone(),
            other => panic!("expected Rejected, got {other:?}"),
        };
        assert!(text.contains("31457281") && text.contains("31457280"), "both numbers shown: {text}");
        assert!(text.contains("signed file"), "names the FILE quantity: {text}");
        assert!(!text.contains("delivery"), "must not borrow the frame arm's wording: {text}");
        assert!(text.contains("Select fewer memories"), "tells the owner the fix: {text}");

        // The frame arm measures the ESCAPED reply, so it says so — a renderer that ignored `limit`
        // would show these bytes against the file wording and mislead the owner.
        let client = EngineClient::new(Arc::new(CannedTransport(Response::BundleTooLarge {
            bytes: 40_000_000,
            max: 33_554_432,
            limit: BundleLimitWire::WireFrame,
        })));
        let err = bounded(client.export_bundle(true, empty_selection())).await.expect_err("refused");
        let text = match &err {
            EngineOpError::Rejected(m) => m.clone(),
            other => panic!("expected Rejected, got {other:?}"),
        };
        assert!(text.contains("delivery"), "names the FRAME quantity: {text}");
        assert!(text.contains("Select fewer memories"), "tells the owner the fix: {text}");
    }
}
