//! The engine spine, as the desktop app sees it after the M1a daemon extraction (Task 6).
//!
//! The encrypted `EventLog`, the embedder, the reasoner, and the evolve scheduler NO LONGER live in
//! this crate — they moved into the `bossclawd` daemon (Task 4), which is now the single owner of
//! the DEK + SQLCipher store. What remains app-side is:
//! - the [`Engine`] facade: the same 29-method surface the commands call, delegating each call to an
//!   [`client::EngineClient`] over a [`transport::Transport`] to the daemon;
//! - the client's RETURN TYPES ([`EngineError`], [`EngineOpError`], [`EngineState`],
//!   [`EngineStatus`], [`EvolveTelemetry`], [`ProposalSummary`], [`MandateSummary`],
//!   [`MandateWriteSummary`], [`PreviewData`], [`ApplyResult`], [`HitWithText`]) — unchanged, so the
//!   commands + their DTOs are untouched;
//! - the app-side Ollama status probe ([`ollama_probe`], called directly by `engine_ollama_status` —
//!   a localhost status check, no engine involved);
//! - the reasoner CONFIG/DATA types the commands + client returns still name
//!   ([`reason::ReasonerConfig`], [`reason::ReasonerMode`], [`reason::CloudProvider`],
//!   [`reason::provider_str`], [`reason::REASONER_MODEL_ID`]).
//!
//! The reasoner PROVIDER machinery (build/consent/readiness) moved to the daemon with the engine.
//!
//! History: this module was the in-process `EngineHandle`; see
//! docs/superpowers/specs/2026-06-22-desktop-engine-spine-design.md for that era and
//! docs/superpowers/specs/2026-07-02-air-agent-memory-hub-m1a-daemon.md for the daemon split.

pub mod client;
pub mod daemon;
pub mod language_pack;
pub mod ollama_probe;
pub mod reason;
pub mod transport;

use serde::Serialize;
use std::fmt;
use std::sync::Arc;

use client::EngineClient;
use transport::Transport;

/// Errors from opening / accessing the engine. Mapped to `EngineState` for the UI. The daemon's
/// copied engine produces these; the client rebuilds the exact variant off the wire so the app-side
/// `Display` renders byte-for-byte what the in-process engine did (the Task 5 string-parity contract).
#[derive(Debug)]
pub enum EngineError {
    /// No identity yet — the brain is not created before onboarding.
    NotOnboarded,
    /// Exactly one of (brain key, DEK) is present — never re-mint (would orphan the DB).
    KeystoreInconsistent,
    /// The DB could not be opened with the stored DEK (wrong key or unopenable).
    KeystoreDbMismatch(String),
    /// A keychain or other I/O error.
    Vault(String),
    /// A background task failed to join.
    Join(String),
    /// The daemon (bossclawd) is unreachable — the transport failed to connect/handshake, or a
    /// request errored and the reconnect+retry also failed. Mirrors [`EngineOpError::Unavailable`]
    /// for teardown's distinct error type: "the brain is DOWN" has no in-process analogue, so it is
    /// kept SEPARATE from `Vault` (a real keystore/keychain fault) — `reset_identity` treats a
    /// down-daemon teardown as non-fatal (identity is already cleared) while STILL surfacing `Vault`.
    /// Constructed only by `client.rs`'s `teardown` from a transport failure.
    Unavailable(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NotOnboarded => write!(f, "not onboarded"),
            EngineError::KeystoreInconsistent => write!(f, "engine keystore inconsistent"),
            EngineError::KeystoreDbMismatch(e) => write!(f, "engine keystore/DB mismatch: {e}"),
            EngineError::Vault(e) => write!(f, "engine keychain error: {e}"),
            EngineError::Join(e) => write!(f, "engine task error: {e}"),
            EngineError::Unavailable(e) => write!(f, "memory service unavailable: {e}"),
        }
    }
}

/// Errors from the operational commands (grant/ingest/list/evolve/…). The client rebuilds these off
/// the wire; the `Display` strings are the SAME the in-process engine produced (string parity), so
/// the command layer's `.map_err(|e| e.to_string())` surfaces identical messages to the webview.
#[derive(Debug)]
pub enum EngineOpError {
    Open(EngineError),
    Core(String),
    Embedder(String),
    /// Reasoner BUILD failure — part of the daemon's `ReasonerProvider` seam error surface, carried
    /// across the wire as its own typed kind. Nothing app-side constructs it directly (the client
    /// rebuilds it from `OpErrorKindWire::Reasoner`), so it is only ever produced daemon-side.
    Reasoner(String),
    /// A serialized op is already in flight; the `&'static str` names it ("ingest" | "evolve").
    Busy(&'static str),
    /// The on-disk file changed since the proposal was drafted; the re-gate at confirm
    /// fails closed. Carries the reason. Nothing is written.
    Stale(String),
    /// The folder's write-grant was revoked between propose and apply; re-gate fails closed.
    Revoked(String),
    /// The FRESH re-gate verdict is loud (secret-/value-shaped or Delete) but the caller did not
    /// pass `acknowledged_loud == true`. The op refuses to write — the UI must show the
    /// "I've reviewed this" confirm and retry with the ack. Carries the reason.
    NeedsLoudConfirm(String),
    /// A mandate grant was refused by an engine grant-time guard (recipe too long, source scope not resolvable,
    /// target not write-granted, or target under a read-grant root). Carries the reason so the
    /// New-mandate form can show *why*. Distinct from `Core` so the UI can style it as a validation
    /// error, not an engine fault.
    Rejected(String),
    /// The daemon (bossclawd) is unreachable — the persistent socket failed to connect / handshake,
    /// or a request errored and the single reconnect+retry also failed (M1a Task 5, `SocketTransport`).
    /// Distinct from `Core` (an engine-side fault): this means the daemon is DOWN / not answering, so
    /// the UI can show "memory service unavailable" rather than an engine error. Carries a short reason.
    /// Constructed by the daemon transport (`engine/transport.rs`) + mapped by `engine/client.rs`.
    Unavailable(String),
    Join(String),
}

impl std::fmt::Display for EngineOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineOpError::Open(e) => write!(f, "{e}"),
            EngineOpError::Core(m) => write!(f, "engine error: {m}"),
            EngineOpError::Embedder(m) => write!(f, "memory model unavailable: {m}"),
            EngineOpError::Reasoner(m) => write!(f, "reasoner unavailable: {m}"),
            EngineOpError::Busy(op) => write!(f, "an {op} is already running"),
            EngineOpError::Stale(m) => write!(f, "the file changed since this was suggested: {m}"),
            EngineOpError::Revoked(m) => write!(f, "edits aren't allowed in this folder anymore: {m}"),
            EngineOpError::NeedsLoudConfirm(m) => write!(f, "this change needs an explicit review confirmation: {m}"),
            EngineOpError::Rejected(m) => write!(f, "{m}"),
            EngineOpError::Unavailable(m) => write!(f, "memory service unavailable: {m}"),
            EngineOpError::Join(m) => write!(f, "engine task error: {m}"),
        }
    }
}

/// A row in the Review queue, projected from one open `PendingProposal`. The
/// `requires_loud_modal` is lifted out of the propose-time `verdict_summary` for the badge/card.
/// The client fills it field-by-field from `ProposalSummaryWire`.
#[derive(Debug, Clone)]
pub struct ProposalSummary {
    pub id: String,
    pub target: String,
    pub op: String,
    pub new_content_hash: String,
    pub rationale: String,
    pub requires_loud_modal: bool,
    /// The proposer's producer stamp (`"m6b-reconciler"` / `"m6c-mandate-proposer"`), surfaced so
    /// the Review UI can label a mandate-driven rewrite "from mandate".
    pub producer: String,
}

/// A mandate row for the desktop Mandates list. The client fills it from `MandateSummaryWire`.
#[derive(Debug, Clone)]
pub struct MandateSummary {
    pub mandate_grant_id: String,
    pub target: String,
    pub source_scope: String,
    pub recipe: String,
    pub granted_at: String,
    pub revoked: bool,
}

/// One Mandate-activity row. The client fills it from `MandateWriteSummaryWire`.
#[derive(Debug, Clone)]
pub struct MandateWriteSummary {
    pub file_written_id: String,
    pub target: String,
    pub written_at: String,
    pub undone: bool,
}

/// Everything the Review card renders for one proposal: paths, the "Why", op, both text halves,
/// and the propose-time loud-modal/taint flags. `old_text`/`new_text` are lossy-UTF8 (the engine
/// only proposes against UTF-8 targets; non-UTF8 is rejected at synthesis).
#[derive(Debug, Clone)]
pub struct PreviewData {
    pub path: String,
    pub folder: String,
    pub rationale: String,
    pub op: String,
    pub old_text: String,
    pub new_text: String,
    pub requires_loud_modal: bool,
    pub taint: String,
}

/// One Memory-browser Library row (SP3 C1): the desktop mirror of `SessionSummaryWire`. Like the
/// other Family-2 summary structs, the wire type has no core equivalent and no proto `From` (proto
/// isn't a desktop dep), so the client fills this field-by-field. `started_at`/`ended_at` are
/// unix-epoch seconds; `approx_bytes` is the rendered `.md` size (a cheap "how big" hint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub project: String,
    pub tool: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub approx_bytes: u64,
}

/// One captured session's detail (SP3 C1): its [`SessionSummary`] plus the daemon-rendered Markdown
/// body. The desktop mirror of `SessionDetailWire`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDetail {
    pub summary: SessionSummary,
    pub markdown: String,
}

/// One `remember` note projected for the Memory-browser notes list (SP3 C1). The desktop mirror of
/// `NoteWire`. `superseded_by` is `Some(new_event_id)` once the note has been edited, else `None`;
/// `created_at` is unix-epoch seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub event_id: String,
    pub text: String,
    pub created_at: i64,
    pub superseded_by: Option<String>,
}

/// One recorded recall miss — a recall that found nothing — for the retrieval-floor tuning UI
/// (SP3 C1). The desktop mirror of `RecallMissWire`. `at` is unix-epoch seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallMiss {
    pub query: String,
    pub at: i64,
}

/// Recall hit/miss telemetry (SP3 C1): the desktop mirror of `RecallStatsWire` — lifetime `total`
/// recalls, how many were `misses`, and the most `recent_misses` (bounded daemon-side).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecallStats {
    pub total: u64,
    pub misses: u64,
    pub recent_misses: Vec<RecallMiss>,
}

/// The coarse engine state surfaced to the UI (distinguishes setup states from faults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    NotOnboarded,
    Ready,
    KeystoreInconsistent,
    KeystoreDbMismatch,
    ChainFailed,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub state: EngineState,
    pub event_count: i64,
    pub chain_ok: bool,
}

/// Desktop mirror of `bossclawd_proto::types::ModelStateWire` (rung 2): the loaded-vs-intended
/// embedder state the language-pack Settings card renders. The client rebuilds this off the wire;
/// the daemon's own `embed::ModelState` never crosses the process boundary (the two meet only over
/// `ModelStatusWire`). `Ok` = the intended model serves; `Missing`/`Mismatch` are the fail-loud
/// re-download states; `Failed` = a background migration errored while the old model keeps serving.
/// The language-pack command layer (`engine_model_status`, Task B2) maps this to `ModelStatusDto`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    /// The daemon serves the intended model (or the bundled English default).
    Ok,
    /// The signed intent names a model whose folder is absent (prompt a re-download).
    Missing {
        /// The model id the signed record expected to find on disk.
        expected: String,
    },
    /// The intended model's weights failed their sha256 integrity check (prompt a re-download).
    Mismatch {
        /// The model id the signed record expected.
        expected: String,
        /// The sha256 actually found on disk.
        loaded: String,
    },
    /// A background re-embed migration failed; the OLD model still serves (retryable).
    Failed {
        /// A short, user-safe reason for the Settings card.
        reason: String,
    },
}

/// A recall `Hit` paired with its hydrated snippet text (`recall.rs::Hit` carries no text).
/// The command layer maps this to `HitDto`; the snippet is best-effort (missing → empty).
#[derive(Debug)]
pub struct HitWithText {
    pub hit: bossclaw_core::Hit,
    pub text: String,
}

/// Session-scoped evolve telemetry, carried across the wire in `EvolveStatusWire`'s telemetry half.
/// (The daemon owns the live values now; the app just renders them.)
#[derive(Default, Clone)]
pub struct EvolveTelemetry {
    pub last_tick_ms: Option<u128>,
    pub error_count: usize,
    pub last_error: Option<String>,
    /// File-derived (`is_external`) snippet count sent by the most recent CLOUD tick
    /// (spec R4 egress transparency). `None` until a cloud tick runs this session.
    pub last_tainted_snippets: Option<usize>,
}

/// The outcome of a successful apply: the audit `file_written` id (also the handle for Undo).
#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub file_written_id: String,
}

/// The engine facade the commands call. It is a thin wrapper over an [`EngineClient`] whose
/// transport is chosen at construction: [`transport::SocketTransport`] in production (talking to the
/// running `bossclawd`), and the in-process `DuplexTransport` in the command tests. Every public
/// method mirrors the old `EngineHandle` signature 1:1 and simply forwards to the client, so the
/// command bodies + their tests are unchanged (only how `AppState.engine` is BUILT changed).
///
/// # Why a trait-object transport (documented choice)
/// `EngineClient<T: Transport + ?Sized>` is generic, but `AppState` is a concrete (non-generic)
/// struct that both `main.rs` (Socket) and the command tests (Duplex) construct. Rather than make
/// `AppState` generic (which would ripple through every command signature and Tauri's
/// `State<AppState>`), the facade holds `EngineClient<dyn Transport>` — a single concrete type over
/// the client's own `Arc<dyn Transport>` (one `Arc`, no re-wrapping). The dynamic dispatch is
/// negligible next to a socket round trip.
pub struct Engine {
    client: EngineClient<dyn Transport>,
}

impl Engine {
    /// Build the facade over `transport`. Production passes an `Arc<SocketTransport>`; the command
    /// tests pass an `Arc<DuplexTransport>`. Both coerce to `Arc<dyn Transport>`, handed to the
    /// client as-is (single `Arc` — no double-wrap).
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { client: EngineClient::new(transport) }
    }

    // ── Status (infallible, mirrors the old `EngineHandle::status`). ──

    pub async fn status(&self, onboarded: bool) -> EngineStatus {
        self.client.status(onboarded).await
    }

    // ── Grant / write-grant mutations. ──

    pub async fn add_grant(&self, onboarded: bool, path: std::path::PathBuf) -> Result<(), EngineOpError> {
        self.client.add_grant(onboarded, path).await
    }

    pub async fn revoke_grant(&self, onboarded: bool, path: std::path::PathBuf) -> Result<(), EngineOpError> {
        self.client.revoke_grant(onboarded, path).await
    }

    pub async fn set_folder_writable(&self, onboarded: bool, path: std::path::PathBuf, on: bool) -> Result<(), EngineOpError> {
        self.client.set_folder_writable(onboarded, path, on).await
    }

    // ── Read ops. ──

    pub async fn list_writable(&self, onboarded: bool) -> Result<Vec<String>, EngineOpError> {
        self.client.list_writable(onboarded).await
    }

    pub async fn list_grants(&self, onboarded: bool) -> Result<Vec<bossclaw_core::Grant>, EngineOpError> {
        self.client.list_grants(onboarded).await
    }

    pub async fn list_files(&self, onboarded: bool) -> Result<Vec<bossclaw_core::graph::FileRecord>, EngineOpError> {
        self.client.list_files(onboarded).await
    }

    // ── Ingest / recall. ──

    pub async fn run_ingest(&self, onboarded: bool) -> Result<bossclaw_core::IngestReport, EngineOpError> {
        self.client.run_ingest(onboarded).await
    }

    pub async fn recall(&self, onboarded: bool, query: String, k: usize) -> Result<Vec<HitWithText>, EngineOpError> {
        self.client.recall(onboarded, query, k).await
    }

    // ── Evolve. ──

    pub async fn evolve_once(&self, onboarded: bool) -> Result<bossclaw_core::EvolveReport, EngineOpError> {
        self.client.evolve_once(onboarded).await
    }

    pub async fn evolve_status(
        &self,
        onboarded: bool,
    ) -> Result<(bossclaw_core::EvolveStatus, EvolveTelemetry), EngineOpError> {
        self.client.evolve_status(onboarded).await
    }

    pub async fn set_evolve_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        self.client.set_evolve_enabled(onboarded, enabled).await
    }

    pub async fn set_proposals_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        self.client.set_proposals_enabled(onboarded, enabled).await
    }

    pub async fn set_mandates_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        self.client.set_mandates_enabled(onboarded, enabled).await
    }

    pub async fn mandates_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        self.client.mandates_enabled(onboarded).await
    }

    // ── Mandate CRUD + activity. ──

    pub async fn add_mandate(
        &self,
        onboarded: bool,
        target: std::path::PathBuf,
        source_scope: std::path::PathBuf,
        recipe: String,
    ) -> Result<MandateSummary, EngineOpError> {
        self.client.add_mandate(onboarded, target, source_scope, recipe).await
    }

    pub async fn revoke_mandate(&self, onboarded: bool, mandate_grant_id: String) -> Result<(), EngineOpError> {
        self.client.revoke_mandate(onboarded, mandate_grant_id).await
    }

    pub async fn list_mandates(&self, onboarded: bool) -> Result<Vec<MandateSummary>, EngineOpError> {
        self.client.list_mandates(onboarded).await
    }

    pub async fn mandate_writes(&self, onboarded: bool) -> Result<Vec<MandateWriteSummary>, EngineOpError> {
        self.client.mandate_writes(onboarded).await
    }

    // ── Review queue: proposals, preview, apply, undo, decline. ──

    pub async fn list_proposals(&self, onboarded: bool) -> Result<Vec<ProposalSummary>, EngineOpError> {
        self.client.list_proposals(onboarded).await
    }

    pub async fn proposal_preview(&self, onboarded: bool, id: String) -> Result<PreviewData, EngineOpError> {
        self.client.proposal_preview(onboarded, id).await
    }

    pub async fn apply_proposal(
        &self,
        onboarded: bool,
        id: String,
        acknowledged_loud: bool,
    ) -> Result<ApplyResult, EngineOpError> {
        self.client.apply_proposal(onboarded, id, acknowledged_loud).await
    }

    pub async fn undo_apply(&self, onboarded: bool, file_written_id: String) -> Result<(), EngineOpError> {
        self.client.undo_apply(onboarded, file_written_id).await
    }

    pub async fn decline_proposal(&self, onboarded: bool, id: String, reason: String) -> Result<(), EngineOpError> {
        self.client.decline_proposal(onboarded, id, reason).await
    }

    // ── Teardown (identity reset). Returns `EngineError`, not `EngineOpError`. ──

    pub async fn teardown(&self) -> Result<(), EngineError> {
        self.client.teardown().await
    }

    // ── Reasoner config (Milestone D). ──

    pub async fn reasoner_config_or_default(&self, onboarded: bool) -> reason::ReasonerConfig {
        self.client.reasoner_config_or_default(onboarded).await
    }

    pub async fn reasoner_ready_or_false(&self, onboarded: bool) -> bool {
        self.client.reasoner_ready_or_false(onboarded).await
    }

    pub async fn set_reasoner_config(&self, onboarded: bool, config: serde_json::Value) -> Result<(), EngineOpError> {
        self.client.set_reasoner_config(onboarded, config).await
    }

    pub async fn enable_cloud_reasoner(&self, onboarded: bool, config: serde_json::Value) -> Result<(), EngineOpError> {
        self.client.enable_cloud_reasoner(onboarded, config).await
    }

    // ── Language pack (rung 2). Consumed by the command layer (B2): `engine_set_active_model` /
    //    `engine_model_status`. ──

    /// Mirrors `EngineClient::set_active_model`: enable the multilingual language pack (validate
    /// folder+sha, write signed consent, run the background re-embed migration). Returns once the
    /// synchronous validation + consent write complete; progress is polled via [`Self::model_status`].
    pub async fn set_active_model(
        &self,
        onboarded: bool,
        model_id: String,
        safetensors_sha: String,
    ) -> Result<(), EngineOpError> {
        self.client.set_active_model(onboarded, model_id, safetensors_sha).await
    }

    /// Mirrors `EngineClient::model_status`: the loaded-vs-intended model state + live re-index
    /// progress, for the Settings language-pack card poll.
    pub async fn model_status(&self, onboarded: bool) -> Result<client::ModelStatus, EngineOpError> {
        self.client.model_status(onboarded).await
    }

    /// Mirrors `EngineClient::set_capture_enabled` (SP3 B5): enable/disable ongoing capture; `backfill`
    /// consents (or not) to the one-time history import. The caller owns the M4 guarantee via `backfill`
    /// and threads the real `onboarded` flag (so a pre-onboarding call cleanly `NotOnboarded`s).
    pub async fn set_capture_enabled(
        &self,
        onboarded: bool,
        enabled: bool,
        backfill: bool,
    ) -> Result<(), EngineOpError> {
        self.client.set_capture_enabled(onboarded, enabled, backfill).await
    }

    /// Mirrors `EngineClient::capture_enabled` (SP3 B5): read the sticky ongoing-capture flag.
    pub async fn capture_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        self.client.capture_enabled(onboarded).await
    }

    /// Mirrors `EngineClient::set_reflect_enabled` (Rung-4 R4-A): enable/disable the reflection loop —
    /// a plain bool switch (no `backfill`; reflection imports no history). Threads the real `onboarded`.
    pub async fn set_reflect_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        self.client.set_reflect_enabled(onboarded, enabled).await
    }

    /// Mirrors `EngineClient::reflect_enabled` (Rung-4 R4-A): read the sticky reflect-enabled flag.
    pub async fn reflect_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        self.client.reflect_enabled(onboarded).await
    }

    // ── Memory-browser Library (SP3 C1). App-only ops (the daemon's role gate denies MemoryClient);
    //    `onboarded` is threaded from the caller like every sibling. ──

    /// Mirrors `EngineClient::list_sessions`: the captured-session summaries for the Library.
    pub async fn list_sessions(&self, onboarded: bool) -> Result<Vec<SessionSummary>, EngineOpError> {
        self.client.list_sessions(onboarded).await
    }

    /// Mirrors `EngineClient::get_session`: one captured session's summary + rendered Markdown. An
    /// unknown/deleted `session_id` surfaces as [`EngineOpError::Rejected`] ("session not found or
    /// deleted") so the UI can distinguish "already deleted" from a real fault.
    pub async fn get_session(&self, onboarded: bool, session_id: String) -> Result<SessionDetail, EngineOpError> {
        self.client.get_session(onboarded, session_id).await
    }

    /// Mirrors `EngineClient::delete_session`: tombstone a captured session (honest, durable forget).
    pub async fn delete_session(&self, onboarded: bool, session_id: String) -> Result<(), EngineOpError> {
        self.client.delete_session(onboarded, session_id).await
    }

    /// Mirrors `EngineClient::list_notes`: the current (non-superseded) `remember` notes.
    pub async fn list_notes(&self, onboarded: bool) -> Result<Vec<Note>, EngineOpError> {
        self.client.list_notes(onboarded).await
    }

    /// Mirrors `EngineClient::supersede_note`: replace a note's text, yielding the new event id.
    pub async fn supersede_note(&self, onboarded: bool, event_id: String, text: String) -> Result<String, EngineOpError> {
        self.client.supersede_note(onboarded, event_id, text).await
    }

    /// Mirrors `EngineClient::recall_stats`: recall hit/miss telemetry for the tuning UI.
    pub async fn recall_stats(&self, onboarded: bool) -> Result<RecallStats, EngineOpError> {
        self.client.recall_stats(onboarded).await
    }
}
