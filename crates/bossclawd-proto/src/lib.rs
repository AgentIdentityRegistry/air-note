//! # bossclawd-proto
//!
//! Wire protocol for `bossclawd`, the local Unix-socket daemon that owns the
//! desktop app's memory engine. This crate is deliberately tiny: it is the
//! length-prefixed frame codec and nothing else. Request/Response types and the
//! handshake are layered on top of these frames elsewhere (M1a Task 2).
//!
//! ## Frame format
//!
//! Each frame is a big-endian [`u32`] length prefix followed by exactly that
//! many payload bytes. Big-endian is the conventional network byte order and
//! keeps the on-wire form language-agnostic. A zero-length payload is a valid
//! frame (the prefix is `0` and no body follows).
//!
//! ## Size guard
//!
//! [`MAX_FRAME`] (32 MiB) caps the payload of a single frame. The cap is
//! enforced on both ends: [`write_frame`] refuses to send an oversize payload,
//! and [`read_frame`] rejects an oversize *declared* length **before** it
//! allocates or reads the body. This makes the reader safe against a hostile or
//! buggy peer that announces a huge length to force a large allocation.

#![forbid(unsafe_code)]

pub mod types;

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::types::{
    ApplyResultWire, ConflictProposalWire, EngineStatusWire, EvolveReportMirror, EvolveStatusMirror,
    EvolveTelemetryWire, FileRecordMirror, GrantMirror, HitMirror, IngestReportMirror,
    MandateSummaryWire, MandateWriteSummaryWire, ModelStatusWire, NoteWire, PreviewDataWire,
    ProposalSummaryWire, ReasonerConfigWire, RecallStatsWire, ResolveActionWire, SessionDetailWire,
    SessionSummaryWire,
};

/// The wire-protocol version. Bumped on any breaking change to [`Request`],
/// [`Response`], or the frame semantics so the [`Hello`]/[`HelloOk`] handshake can
/// reject a peer built against an incompatible protocol before any op is dispatched.
pub const PROTO_VERSION: u32 = 1;

/// The privilege a connection requests at the [`Hello`] handshake. The daemon enforces a per-role
/// op-allowlist in `dispatch` (fail-closed: an op not explicitly allowed for the role is refused).
///
/// Defaults to [`Role::App`] (full access) so a peer that omits the field — including any build
/// predating this field — connects exactly as before: the desktop app is unchanged. Only a client
/// that OPTS INTO [`Role::MemoryClient`] is scoped down (the `air-memory-mcp` adapter). This is the
/// "Simple" bar (least-privilege-by-default); cryptographic capability tokens ("Strict") are a
/// deferred future hardening — a same-uid process can already forge `App`, so this does not defend
/// against a malicious peer, only scopes a cooperative one.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Role {
    /// Full access to every wire op (the default; the desktop app).
    #[default]
    App,
    /// A scoped coding-agent client: may invoke ONLY `Recall` + `Remember`, the SP3 session-capture
    /// pokes (`CaptureNotify` + `Snapshot`), and the Rung-3 Phase-3 resolution ops (`ListConflicts` +
    /// `ResolveConflict`, the I8 relaxation) — never the destructive/listing/telemetry ops.
    MemoryClient,
}

impl Role {
    /// Fail-closed per-op allowlist. `App` may invoke every op. `MemoryClient` may invoke ONLY
    /// [`Request::Recall`], [`Request::Remember`], [`Request::CaptureNotify`], [`Request::Snapshot`]
    /// (the read+write loop plus the SP3 session-capture pokes), and the Rung-3 Phase-3 resolution ops
    /// [`Request::ListConflicts`] + [`Request::ResolveConflict`] (the I8 relaxation); **every other
    /// variant — present or future — is refused by default** (the `matches!` denies anything not
    /// explicitly listed). A new `Request` variant is therefore refused for `MemoryClient` until
    /// someone deliberately adds it here.
    pub fn allows(&self, req: &Request) -> bool {
        match self {
            Role::App => true,
            Role::MemoryClient => matches!(
                req,
                Request::Recall { .. }
                    | Request::Remember { .. }
                    | Request::CaptureNotify { .. }
                    | Request::Snapshot { .. }
                    // I8 RELAXATION (owner decision 2026-07-17, design §0/§4): for a locally-installed
                    // AIR Agent, local Claude Code IS the owner. The resolve ops are guest-reachable;
                    // safety rests on reversible retire + the visibility digest + the signed log, NOT a
                    // rate cap (the per-connection limiter is a no-op vs a reconnecting MCP client).
                    | Request::ListConflicts { .. }
                    | Request::ResolveConflict { .. }
            ),
        }
    }
}

/// The first frame the client sends after connecting: its protocol version.
///
/// Its own serde type (NOT a [`Request`] variant): the handshake runs once per
/// connection, before any request, in each direction. The daemon answers with
/// [`HelloOk`] iff `proto_version` matches [`PROTO_VERSION`].
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Hello {
    /// The protocol version the client was built against.
    pub proto_version: u32,
    /// The privilege the connection requests. `#[serde(default)]` so a peer that omits it (any
    /// build predating this field) is treated as [`Role::App`] — the app is unchanged (I3).
    #[serde(default)]
    pub role: Role,
}

/// The daemon's reply to a [`Hello`] with a matching version: its PID (so the
/// client can log/verify the single owner) and the version it speaks back.
///
/// Its own serde type (NOT a [`Response`] variant), sent as the first frame from
/// the daemon in reply to [`Hello`].
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct HelloOk {
    /// The daemon process id (single-owner arbitration / diagnostics).
    pub pid: u32,
    /// The protocol version the daemon speaks (equals [`PROTO_VERSION`]).
    pub proto_version: u32,
}

/// One request from the client to the daemon — exactly one variant per wire op in
/// the Task 0 inventory (`docs/superpowers/plans/m1a-engine-surface.md`, the 29
/// rows of the "Called from Tauri commands" table). Scheduler-only and
/// engine-internal engine methods get NO variant (they run inside the daemon).
///
/// Every variant carries `onboarded: bool`, exactly as today's `EngineHandle`
/// methods do: the app computes it from its identity store and passes it through,
/// so the daemon's onboarding gate stays where it is (`get_or_open`). Path args are
/// [`PathBuf`] — fine on a same-host Unix-socket protocol.
///
/// Externally tagged (serde default): each variant serializes as
/// `{"VariantName": {..fields..}}`, which is self-describing and stable.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Request {
    /// `EngineHandle::status` → `engine_status`.
    Status { onboarded: bool },
    /// `EngineHandle::add_grant` → `engine_add_grant`.
    AddGrant { onboarded: bool, path: PathBuf },
    /// `EngineHandle::revoke_grant` → `engine_revoke_grant`.
    RevokeGrant { onboarded: bool, path: PathBuf },
    /// `EngineHandle::set_folder_writable` → `engine_set_folder_writable`.
    SetFolderWritable { onboarded: bool, path: PathBuf, on: bool },
    /// `EngineHandle::list_writable` → `engine_list_writable`.
    ListWritable { onboarded: bool },
    /// `EngineHandle::list_grants` → `engine_list_grants`.
    ListGrants { onboarded: bool },
    /// `EngineHandle::run_ingest` → `engine_run_ingest`.
    RunIngest { onboarded: bool },
    /// `EngineHandle::recall` → `engine_recall`.
    Recall { onboarded: bool, query: String, k: usize },
    /// `EngineHandle::evolve_once` → `engine_evolve_now` (also a scheduler tick).
    EvolveOnce { onboarded: bool },
    /// `EngineHandle::evolve_status` → `engine_evolve_status`.
    EvolveStatus { onboarded: bool },
    /// `EngineHandle::set_evolve_enabled` → `engine_set_evolve_enabled`.
    SetEvolveEnabled { onboarded: bool, enabled: bool },
    /// `EngineHandle::set_proposals_enabled` → `engine_set_proposals_enabled`.
    SetProposalsEnabled { onboarded: bool, enabled: bool },
    /// `EngineHandle::set_mandates_enabled` → `engine_set_mandates_enabled`.
    SetMandatesEnabled { onboarded: bool, enabled: bool },
    /// `EngineHandle::mandates_enabled` → `engine_mandates_enabled`.
    MandatesEnabled { onboarded: bool },
    /// `EngineHandle::add_mandate` → `engine_add_mandate`.
    AddMandate { onboarded: bool, target: PathBuf, source_scope: PathBuf, recipe: String },
    /// `EngineHandle::revoke_mandate` → `engine_revoke_mandate`.
    RevokeMandate { onboarded: bool, mandate_grant_id: String },
    /// `EngineHandle::list_mandates` → `engine_list_mandates`.
    ListMandates { onboarded: bool },
    /// `EngineHandle::mandate_writes` → `engine_mandate_writes`.
    MandateWrites { onboarded: bool },
    /// `EngineHandle::list_files` → `engine_list_files`.
    ListFiles { onboarded: bool },
    /// `EngineHandle::list_proposals` → `engine_list_proposals`.
    ListProposals { onboarded: bool },
    /// `EngineHandle::proposal_preview` → `engine_proposal_preview`.
    ProposalPreview { onboarded: bool, id: String },
    /// `EngineHandle::teardown` → identity-reset flow (`commands/identity.rs`).
    /// Teardown carries no `onboarded` today (it deletes the keystore + DB
    /// unconditionally); the wire op mirrors that exactly.
    Teardown,
    /// `EngineHandle::apply_proposal` → `engine_apply_proposal`.
    ApplyProposal { onboarded: bool, id: String, acknowledged_loud: bool },
    /// `EngineHandle::undo_apply` → `engine_undo_apply`.
    UndoApply { onboarded: bool, file_written_id: String },
    /// `EngineHandle::decline_proposal` → `engine_decline_proposal`.
    DeclineProposal { onboarded: bool, id: String, reason: String },
    /// `EngineHandle::reasoner_config_or_default` → half of `engine_get_reasoner_config`.
    GetReasonerConfig { onboarded: bool },
    /// `EngineHandle::reasoner_ready_or_false` → the other half of `engine_get_reasoner_config`.
    GetReasonerReady { onboarded: bool },
    /// `EngineHandle::set_reasoner_config` → `engine_set_reasoner_config`. The
    /// config is free-form JSON (as the engine method takes today).
    SetReasonerConfig { onboarded: bool, config: serde_json::Value },
    /// `EngineHandle::enable_cloud_reasoner` → `engine_enable_cloud_reasoner`.
    EnableCloudReasoner { onboarded: bool, config: serde_json::Value },
    /// `EngineHandle::set_active_model` → `engine_set_active_model` (rung 2). Enables the
    /// multilingual language pack: validate folder+sha, write the signed consent + in-progress
    /// marker, run the crash-safe re-embed migration in the background. Progress is polled via
    /// [`Request::ModelStatus`].
    SetActiveModel { onboarded: bool, model_id: String, safetensors_sha: String },
    /// `EngineHandle::model_status` → `engine_model_status` (rung 2). Loaded-vs-intended model state
    /// + live re-index progress. Polled by the Settings language-pack card.
    ModelStatus { onboarded: bool },
    /// `EngineHandle::remember` → the coding-agent write op (SP1 / M1b). Appends a signed
    /// `memory`-type event stamped `origin=external` (recallable, never auto-trusted). No
    /// Tauri command maps to it (it is reached only via the `air-memory-mcp` adapter as a
    /// `MemoryClient`); `onboarded` mirrors every other op.
    Remember { onboarded: bool, text: String },
    /// SP3 session-capture poke: render the just-ended Claude Code session to signed Markdown now.
    /// Reached by the app AND the `air-memory-mcp` guest (a `MemoryClient` op — the SessionEnd hook
    /// fires it); the daemon validates `session_id` and confines `transcript_path` before any read.
    CaptureNotify { onboarded: bool, session_id: String, transcript_path: String },
    /// SP3 orientation snapshot: return a lean, project-aware brief for a starting/compacted session.
    /// Reached by the app AND the `air-memory-mcp` guest (a `MemoryClient` op — the SessionStart hook
    /// fires it); `session_id`/`transcript_path` are present only for the `source=compact` flavor
    /// (absent for a fresh start, which needs no live-transcript digest).
    Snapshot {
        onboarded: bool,
        project: String,
        source: String,
        session_id: Option<String>,
        transcript_path: Option<String>,
    },
    /// SP3 Memory-browser listing: the captured-session summaries for the Library. App-only.
    ListSessions { onboarded: bool },
    /// SP3 Memory-browser read: one captured session's summary + rendered Markdown body. App-only.
    GetSession { onboarded: bool, session_id: String },
    /// SP3 Memory-browser delete: tombstone a captured session (honest forget). App-only (destructive).
    DeleteSession { onboarded: bool, session_id: String },
    /// SP3 Memory-browser listing: the current (non-superseded) `remember` notes. App-only.
    ListNotes { onboarded: bool },
    /// SP3 Memory-browser edit: supersede a note with new text, yielding the new event id. App-only.
    SupersedeNote { onboarded: bool, event_id: String, text: String },
    /// Rung 3 §7.3: retire a memory — a REVERSIBLE hide, distinct from `SupersedeNote`'s edit.
    /// `RetireTarget::Note` appends a distinct `note_retired` marker (recall/list drop it fold-time,
    /// no index rebuild); `RetireTarget::Passage` is the Task 7 passage-granularity variant (dispatched
    /// as a clean `Rejected` until then). App-only (guest-refused).
    RetireMemory { onboarded: bool, target: RetireTarget },
    /// Rung 3 §7.3: reverse a prior note retire, appending an `unretire` marker so the target is
    /// recalled/listed again. `Rejected` if the id is not CURRENTLY retired. App-only (guest-refused).
    Unretire { onboarded: bool, retired_event_id: String },
    /// SP3 recall-miss telemetry: the recall hit/miss counters + recent misses for the tuning UI.
    /// App-only.
    RecallStats { onboarded: bool },
    /// SP3 capture toggle: enable/disable ongoing session capture; `backfill` additionally consents to
    /// the one-time import of existing sessions (the Connect checkbox sets both flags in one call; the
    /// later Integrations toggle passes `backfill: false`). App-only. Follows the `SetMandatesEnabled`
    /// wire shape.
    SetCaptureEnabled { onboarded: bool, enabled: bool, backfill: bool },
    /// SP3 capture-toggle query: whether ongoing session capture is enabled. App-only. Mirrors
    /// `MandatesEnabled`.
    CaptureEnabled { onboarded: bool },
    /// Rung-4 R4-A: enable/disable the reflection loop. App-only (guest-refused). Follows the
    /// `SetCaptureEnabled` shape minus the capture-only `backfill` (reflection is a plain bool switch,
    /// like `SetMandatesEnabled`/conflict-detect). Acked with `Response::Ok`.
    SetReflectEnabled { onboarded: bool, enabled: bool },
    /// Rung-4 R4-A: read the sticky reflection flag (the toggle POSITION). App-only. Mirrors `CaptureEnabled`.
    ReflectEnabled { onboarded: bool },
    /// Rung-3 Phase-3: list the pending conflict proposals (already excludes coexist/dismissed). Reachable
    /// from `MemoryClient` (the I8 relaxation) so Claude Code can review conflicts. Sanitized daemon-side.
    ListConflicts { onboarded: bool },
    /// Rung-3 Phase-3: resolve one conflict proposal with a deterministic action. Reachable from
    /// `MemoryClient` (the I8 relaxation). No LLM, no egress; the retire actions are reversible.
    ResolveConflict { onboarded: bool, proposal_id: String, action: ResolveActionWire },
}

/// The subject of a [`Request::RetireMemory`]: either a whole `remember` note (by its event id) or a
/// single session passage (by `session_id` + passage index). Externally tagged like the request
/// enums; derives match the other payload types so it nests inside [`Request`]. The `Passage` arm is
/// carried on the wire now but its engine dispatch lands in Task 7 (§7.3) — until then the daemon
/// answers it with a clean `Rejected`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum RetireTarget {
    /// A whole `remember` note, addressed by its event id (obtained from `ListNotes`).
    Note { event_id: String },
    /// A single session passage, addressed by `session_id` + passage index (Task 7).
    Passage { session_id: String, passage_id: usize },
}

/// One response from the daemon to the client. Each success variant carries the
/// mirror/wire payload for its request; unit-returning ops share the [`Response::Ok`]
/// variant (they carry nothing — the op just needs to signal success).
///
/// Three non-success signals are kept DISTINCT rather than folded into a single
/// error string, because the desktop UI maps them to different behavior today:
///
/// - [`Response::NotOnboarded`] mirrors `EngineError::NotOnboarded` (the brain
///   isn't created yet) — the UI shows the onboarding path, not a fault.
/// - [`Response::Busy`] mirrors `EngineOpError::Busy` (a serialized ingest/evolve
///   is already in flight) — the UI shows "already running", not a fault. It carries
///   the op name (`"ingest"` / `"evolve"`) the core error names.
/// - [`Response::Err`] is every other error, as its TYPED kind + the INNER message
///   (never a pre-rendered Display string — the client reconstructs the exact typed
///   `EngineOpError`/`EngineError` variant so the app-side Display strings are
///   byte-identical to the in-process engine's; see [`OpErrorKindWire`]).
///
/// Externally tagged (serde default).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Response {
    /// Success for a unit-returning op (add/revoke grant, set-* toggles, teardown,
    /// undo, decline, set/enable reasoner config). Unambiguous because every
    /// value-returning op has its own dedicated variant below.
    Ok,
    /// `Status` result.
    Status(EngineStatusWire),
    /// `ListWritable` result — active writable roots (canonical paths).
    ListWritable(Vec<String>),
    /// `ListGrants` result.
    ListGrants(Vec<GrantMirror>),
    /// `RunIngest` result.
    RunIngest(IngestReportMirror),
    /// `Recall` result — each hit paired with its hydrated snippet text (mirrors
    /// today's `HitWithText`; `recall.rs::Hit` carries no text).
    Recall(Vec<HitWire>),
    /// `EvolveOnce` result.
    EvolveOnce(EvolveReportMirror),
    /// `EvolveStatus` result — the core status plus the session telemetry the
    /// daemon holds (the desktop merges them into `EvolveStatusDto` in Task 5/6).
    EvolveStatus { status: EvolveStatusMirror, telemetry: EvolveTelemetryWire },
    /// `MandatesEnabled` result — the sticky mandates flag.
    MandatesEnabled(bool),
    /// `AddMandate` result.
    AddMandate(MandateSummaryWire),
    /// `ListMandates` result.
    ListMandates(Vec<MandateSummaryWire>),
    /// `MandateWrites` result.
    MandateWrites(Vec<MandateWriteSummaryWire>),
    /// `ListFiles` result. Note: `FileRecordDto`'s derived `writable` flag is a
    /// desktop projection; the wire carries the core record + the writable roots
    /// come from `ListWritable`, so the desktop can recompute it (Task 5/6).
    ListFiles(Vec<FileRecordMirror>),
    /// `ListProposals` result.
    ListProposals(Vec<ProposalSummaryWire>),
    /// `ProposalPreview` result.
    ProposalPreview(PreviewDataWire),
    /// `ApplyProposal` result — the audit `file_written` id.
    ApplyProposal(ApplyResultWire),
    /// `GetReasonerConfig` result.
    ReasonerConfig(ReasonerConfigWire),
    /// `GetReasonerReady` result — the fail-closed cloud-readiness flag.
    ReasonerReady(bool),
    /// `ModelStatus` result (rung 2) — the loaded-vs-intended model state + live re-index progress.
    ModelStatus(ModelStatusWire),
    /// `Remember` result — the id of the newly appended `memory` event.
    Remember(String),
    /// `Snapshot` result — the fenced orientation text (empty when there is nothing to orient with).
    Snapshot(String),
    /// `ListSessions` result — the captured-session summaries for the Library.
    ListSessions(Vec<SessionSummaryWire>),
    /// `GetSession` result — one captured session's summary + rendered Markdown body.
    Session(SessionDetailWire),
    /// `ListNotes` result — the current (non-superseded) notes.
    ListNotes(Vec<NoteWire>),
    /// `SupersedeNote` result — the id of the new (superseding) note event.
    Superseded(String),
    /// `RetireMemory` / `Unretire` result — the id of the newly appended retire/unretire marker event.
    Retired(String),
    /// `ListConflicts` result — the pending (coexist/dismissed-filtered), daemon-sanitized proposals.
    ListConflicts(Vec<ConflictProposalWire>),
    /// `ResolveConflict` result — whether a terminal marker was newly applied, and its id when it was.
    /// `applied == false` is the idempotent no-op / roll-forward success (never an error to the agent).
    ResolveConflict { applied: bool, marker_event_id: Option<String> },
    /// `RecallStats` result — recall hit/miss telemetry for the tuning UI.
    RecallStats(RecallStatsWire),
    /// `CaptureEnabled` result — the sticky capture-enabled flag.
    CaptureEnabled(bool),
    /// `ReflectEnabled` result — the sticky reflect-enabled flag.
    ReflectEnabled(bool),
    /// The engine is not onboarded (mirrors `EngineError::NotOnboarded`). A signal,
    /// not a fault — the UI shows onboarding.
    NotOnboarded,
    /// A serialized op is already in flight (mirrors `EngineOpError::Busy`). Carries
    /// the op name (`"ingest"` / `"evolve"`).
    Busy(String),
    /// Any other error: the TYPED kind plus the variant's INNER message (never a
    /// pre-rendered Display string — Display prefixes are applied exactly once,
    /// app-side, after the client rebuilds the typed variant). `message` is empty
    /// for the unit-shaped `KeystoreInconsistent` kind.
    Err { kind: OpErrorKindWire, message: String },
}

/// The typed kind of a [`Response::Err`] payload — one arm per typed error variant that can cross
/// the wire, so the client can rebuild the EXACT `EngineOpError`/`EngineError` variant and app-side
/// Display strings stay byte-identical to the in-process engine's (no double-prefixing, no typed →
/// `Core` collapse). `NotOnboarded` and `Busy` are NOT here — they stay dedicated [`Response`]
/// signals, as today.
///
/// Kept in `lib.rs` (protocol-shaped, not a core-type mirror). The first eight arms mirror the
/// engine's `EngineOpError`; the last three mirror the `EngineError` surface that crosses the wire
/// (op-path open failures via `EngineOpError::Open`, and teardown's `Result<(), EngineError>`).
/// `Join` is shared: `EngineOpError::Join` and `EngineError::Join` render identically
/// ("engine task error: …"), so one kind serves both.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpErrorKindWire {
    /// `EngineOpError::Core` — also the daemon's protocol-level faults (malformed frame, handshake
    /// refusal), which have no in-process analogue and render fine under Core's generic prefix.
    Core,
    /// `EngineOpError::Embedder`.
    Embedder,
    /// `EngineOpError::Reasoner`.
    Reasoner,
    /// `EngineOpError::Stale`.
    Stale,
    /// `EngineOpError::Revoked`.
    Revoked,
    /// `EngineOpError::NeedsLoudConfirm`.
    NeedsLoudConfirm,
    /// `EngineOpError::Rejected`.
    Rejected,
    /// `EngineOpError::Join` / `EngineError::Join` (identical Display; see above).
    Join,
    /// `EngineError::Vault`.
    Vault,
    /// `EngineError::KeystoreInconsistent` (unit-shaped: `message` rides empty).
    KeystoreInconsistent,
    /// `EngineError::KeystoreDbMismatch`.
    KeystoreDbMismatch,
    /// A per-op authorization refusal (U2): the connection's [`Role`] does not allow the requested
    /// op. A daemon-protocol fault with no in-process engine analogue (like `Core`); the client
    /// renders it under the generic engine-error prefix. The desktop app never receives it (it is
    /// always `App`), but the arm keeps the wire enum exhaustive.
    NotPermitted,
}

/// A recall [`HitMirror`] paired with its hydrated snippet text, mirroring the
/// desktop's `HitWithText` (the core `Hit` carries no text; the daemon hydrates it
/// alongside the hit, exactly as the app does today). Kept in `lib.rs` (not
/// `types.rs`) because it is a protocol-shaped pairing, not a core-type mirror.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct HitWire {
    /// The recall hit (score + provenance + kind).
    pub hit: HitMirror,
    /// Best-effort snippet text (empty when the source event has none).
    pub text: String,
}

/// Maximum payload size, in bytes, of a single frame (32 MiB).
///
/// A frame whose payload exceeds this is rejected by both [`write_frame`] and
/// [`read_frame`]. The bound protects the reader from a peer that declares a
/// huge length to trigger an oversized allocation.
pub const MAX_FRAME: usize = 32 * 1024 * 1024;

/// Write one length-prefixed frame: a big-endian [`u32`] length followed by
/// `payload`.
///
/// # Cancellation safety
///
/// This function is **not** cancellation-safe: it makes multiple `write_all`
/// calls, so if the returned future is dropped mid-call (e.g. raced against a
/// timeout in a `tokio::select!`) the stream may be left at an arbitrary
/// mid-frame offset. Such a stream MUST NOT be reused for further framed I/O —
/// drop it and reconnect instead.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] if `payload` is longer than
/// [`MAX_FRAME`] (nothing is written in that case), or propagates any I/O error
/// from the underlying writer. The frame is flushed before returning so the
/// bytes are handed to the OS rather than sitting in a buffer.
pub async fn write_frame<W>(writer: &mut W, payload: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    if payload.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "frame payload of {} bytes exceeds MAX_FRAME ({MAX_FRAME} bytes)",
                payload.len()
            ),
        ));
    }
    // `payload.len() <= MAX_FRAME` (a `u32`-sized bound), so this never truncates.
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one length-prefixed frame written by [`write_frame`], returning its
/// payload bytes.
///
/// # Cancellation safety
///
/// This function is **not** cancellation-safe: it uses `read_exact`, so if the
/// returned future is dropped mid-call (e.g. raced against a timeout in a
/// `tokio::select!`) already-consumed prefix or body bytes are lost and the
/// stream is left at an arbitrary mid-frame offset. Such a stream MUST NOT be
/// reused for further framed I/O — drop it and reconnect instead.
///
/// # Errors
///
/// - [`io::ErrorKind::InvalidData`] if the declared length exceeds
///   [`MAX_FRAME`]. The oversize length is rejected **before** any body bytes
///   are allocated or read.
/// - [`io::ErrorKind::UnexpectedEof`] if the peer closes the connection before a
///   full frame has arrived — whether mid-prefix or mid-body. A clean EOF at a
///   frame boundary (before any prefix byte) surfaces the same way, so callers
///   treat `UnexpectedEof` as "the stream ended".
/// - any other I/O error from the underlying reader is propagated as-is.
pub async fn read_frame<R>(reader: &mut R) -> io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    // `read_exact` maps a peer close (0 bytes read while bytes are still needed)
    // to `UnexpectedEof`, so a mid-prefix or mid-body close errors rather than
    // hanging or panicking.
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("declared frame length {len} bytes exceeds MAX_FRAME ({MAX_FRAME} bytes)"),
        ));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn frame_roundtrip() {
        let (mut a, mut b) = duplex(1024);
        write_frame(&mut a, b"hello frame").await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"hello frame");
    }

    #[tokio::test]
    async fn empty_frame_roundtrip() {
        // A zero-length payload is a legitimate frame.
        let (mut a, mut b) = duplex(1024);
        write_frame(&mut a, b"").await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"");
    }

    #[tokio::test]
    async fn max_size_frame_roundtrip() {
        // A payload of exactly MAX_FRAME is allowed on both ends.
        let payload = vec![0x5au8; MAX_FRAME];
        // duplex buffer must hold prefix + body without a reader draining it.
        let (mut a, mut b) = duplex(MAX_FRAME + 4);
        write_frame(&mut a, &payload).await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), payload);
    }

    #[tokio::test]
    async fn write_rejects_oversize_payload() {
        // One byte past the cap must be refused, and nothing may be written.
        let payload = vec![0u8; MAX_FRAME + 1];
        let (mut a, mut b) = duplex(1024);
        let err = write_frame(&mut a, &payload).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        // Prove no partial frame leaked onto the wire: drop the writer so the
        // reader sees EOF immediately instead of blocking, then confirm the
        // reader gets a clean EOF (zero bytes buffered), not a stray prefix.
        drop(a);
        let read_err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(read_err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn read_rejects_oversize_declared_length_before_body() {
        // Hand-craft a frame header that declares MAX_FRAME + 1 bytes but send
        // NO body. If the reader tried to read the body it would block forever;
        // rejecting on the declared length alone means it returns promptly.
        let (mut a, mut b) = duplex(1024);
        let bogus_len = (MAX_FRAME + 1) as u32;
        a.write_all(&bogus_len.to_be_bytes()).await.unwrap();
        a.flush().await.unwrap();

        let err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn read_errors_on_eof_before_any_prefix() {
        // Peer closes at a frame boundary having sent nothing.
        let (a, mut b) = duplex(1024);
        drop(a);
        let err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn read_errors_on_peer_close_mid_frame() {
        // Peer sends a valid prefix promising 100 bytes, one body byte, then
        // closes. The reader must error (UnexpectedEof), not hang or panic.
        let (mut a, mut b) = duplex(1024);
        a.write_all(&100u32.to_be_bytes()).await.unwrap();
        a.write_all(b"x").await.unwrap();
        a.flush().await.unwrap();
        drop(a);

        let err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn two_frames_back_to_back() {
        // Framing must delimit correctly: two writes, two clean reads.
        let (mut a, mut b) = duplex(1024);
        write_frame(&mut a, b"first").await.unwrap();
        write_frame(&mut a, b"second").await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"first");
        assert_eq!(read_frame(&mut b).await.unwrap(), b"second");
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use crate::types::{
        EngineStateWire, IngestReportMirror, RecallSourceMirror, SkipMirror,
    };

    /// Round-trip a representative sample of Request variants (unit, path, scalar,
    /// mirror-payload, and free-form-JSON shapes) through JSON.
    #[test]
    fn request_response_serde_roundtrip() {
        // ---- Requests: one per shape family ----
        let requests = vec![
            Request::Status { onboarded: true },
            Request::AddGrant { onboarded: true, path: PathBuf::from("/home/me/notes") },
            Request::SetFolderWritable {
                onboarded: false,
                path: PathBuf::from("/home/me/notes"),
                on: true,
            },
            Request::Recall { onboarded: true, query: "who is Aria?".to_string(), k: 8 },
            Request::AddMandate {
                onboarded: true,
                target: PathBuf::from("/x/summary.md"),
                source_scope: PathBuf::from("/x/src"),
                recipe: "keep it a bullet list".to_string(),
            },
            Request::ApplyProposal {
                onboarded: true,
                id: "01J-PROP".to_string(),
                acknowledged_loud: true,
            },
            Request::Teardown,
            Request::Remember { onboarded: true, text: "remember me".to_string() },
            Request::GetReasonerConfig { onboarded: true },
            Request::GetReasonerReady { onboarded: true },
            Request::SetReasonerConfig {
                onboarded: true,
                config: serde_json::json!({"mode": "cloud", "provider": "anthropic"}),
            },
            Request::EnableCloudReasoner {
                onboarded: true,
                config: serde_json::json!({"mode": "cloud", "provider": "openai-compat", "base_url": "https://api.example.com"}),
            },
        ];
        for req in requests {
            let bytes = serde_json::to_vec(&req).unwrap();
            let back: Request = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(req, back, "request round-trips: {req:?}");
        }

        // ---- Responses: success, mirror-payload, and each signal ----
        let responses = vec![
            Response::Ok,
            Response::Status(EngineStatusWire {
                state: EngineStateWire::Ready,
                event_count: 3,
                chain_ok: true,
            }),
            Response::ListWritable(vec!["/home/me/notes".to_string()]),
            Response::RunIngest(IngestReportMirror {
                ingested: 2,
                superseded: 0,
                deduped: 1,
                skipped: vec![SkipMirror { path: "/x/a.bin".to_string(), reason: "not utf8".to_string() }],
                failed: vec![],
            }),
            Response::Recall(vec![HitWire {
                hit: HitMirror {
                    event_id: "e1".to_string(),
                    score: 0.5,
                    sources: vec![RecallSourceMirror::Vector],
                    kind: "memory".to_string(),
                },
                text: "hello".to_string(),
            }]),
            Response::MandatesEnabled(false),
            Response::ReasonerReady(true),
            Response::NotOnboarded,
            Response::Busy("ingest".to_string()),
            Response::Remember("01J-REMEMBERED".to_string()),
            Response::Err {
                kind: OpErrorKindWire::Core,
                message: "something went wrong".to_string(),
            },
        ];
        for resp in responses {
            let bytes = serde_json::to_vec(&resp).unwrap();
            let back: Response = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(resp, back, "response round-trips: {resp:?}");
        }
    }

    /// Every [`OpErrorKindWire`] arm round-trips inside a `Response::Err` payload — the typed-error
    /// contract the desktop's string-parity test builds on (a kind that didn't survive serde would
    /// silently collapse a typed error).
    #[test]
    fn op_error_kind_wire_roundtrips_every_arm() {
        let kinds = [
            OpErrorKindWire::Core,
            OpErrorKindWire::Embedder,
            OpErrorKindWire::Reasoner,
            OpErrorKindWire::Stale,
            OpErrorKindWire::Revoked,
            OpErrorKindWire::NeedsLoudConfirm,
            OpErrorKindWire::Rejected,
            OpErrorKindWire::Join,
            OpErrorKindWire::Vault,
            OpErrorKindWire::KeystoreInconsistent,
            OpErrorKindWire::KeystoreDbMismatch,
            OpErrorKindWire::NotPermitted,
        ];
        for kind in kinds {
            // KeystoreInconsistent is unit-shaped (empty message); every other kind carries one.
            let message = if kind == OpErrorKindWire::KeystoreInconsistent {
                String::new()
            } else {
                format!("inner message for {kind:?}")
            };
            let resp = Response::Err { kind, message };
            let bytes = serde_json::to_vec(&resp).unwrap();
            let back: Response = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(resp, back, "Err payload round-trips for {kind:?}");
        }
    }

    /// The rung-2 ops round-trip through JSON: the `SetActiveModel`/`ModelStatus` requests and every
    /// `ModelState` arm (including the `Failed` migration state) inside a `Response::ModelStatus`.
    #[test]
    fn rung2_ops_serde_roundtrip() {
        use crate::types::{ModelStateWire, ModelStatusWire, ReindexProgressWire};

        let req = Request::SetActiveModel {
            onboarded: true,
            model_id: "minishlab/potion-multilingual-128M".to_string(),
            safetensors_sha: "deadbeef".to_string(),
        };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(req, back);

        let status = Request::ModelStatus { onboarded: false };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&status).unwrap()).unwrap();
        assert_eq!(status, back);

        let responses = vec![
            Response::ModelStatus(ModelStatusWire {
                state: ModelStateWire::Ok,
                reindex: None,
                active_model_id: Some("minishlab/potion-multilingual-128M".to_string()),
            }),
            Response::ModelStatus(ModelStatusWire {
                state: ModelStateWire::Missing {
                    expected: "minishlab/potion-multilingual-128M".to_string(),
                },
                reindex: Some(ReindexProgressWire { done: 220, total: 1043 }),
                active_model_id: Some("minishlab/potion-base-8M".to_string()),
            }),
            Response::ModelStatus(ModelStatusWire {
                state: ModelStateWire::Mismatch {
                    expected: "minishlab/potion-multilingual-128M".to_string(),
                    loaded: "deadbeef".to_string(),
                },
                reindex: None,
                active_model_id: Some("minishlab/potion-base-8M".to_string()),
            }),
            Response::ModelStatus(ModelStatusWire {
                state: ModelStateWire::Failed { reason: "re-embed failed: model boom".to_string() },
                reindex: None,
                active_model_id: Some("minishlab/potion-base-8M".to_string()),
            }),
            // A daemon that predates the field emits `None`; the round-trip must preserve it.
            Response::ModelStatus(ModelStatusWire {
                state: ModelStateWire::Ok,
                reindex: None,
                active_model_id: None,
            }),
        ];
        for resp in responses {
            let back: Response = serde_json::from_slice(&serde_json::to_vec(&resp).unwrap()).unwrap();
            assert_eq!(resp, back, "ModelStatus round-trips: {resp:?}");
        }
    }

    /// The handshake frames round-trip and `PROTO_VERSION` is the pinned constant.
    #[test]
    fn handshake_serde_roundtrip() {
        let hello = Hello { proto_version: PROTO_VERSION, role: Role::App };
        let bytes = serde_json::to_vec(&hello).unwrap();
        let back: Hello = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(hello, back);

        let hello_ok = HelloOk { pid: 4321, proto_version: PROTO_VERSION };
        let bytes = serde_json::to_vec(&hello_ok).unwrap();
        let back: HelloOk = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(hello_ok, back);

        assert_eq!(PROTO_VERSION, 1, "M1a pins the wire protocol at v1");
    }

    /// The handshake types are DISTINCT from Request/Response: a `Hello` frame must
    /// not accidentally deserialize as a `Request` (they travel as separate frames).
    #[test]
    fn hello_is_not_a_request() {
        let hello_bytes = serde_json::to_vec(&Hello { proto_version: 1, role: Role::App }).unwrap();
        // `Hello` serializes as `{"proto_version":1}`, which is not any externally
        // tagged `Request` variant, so it must fail to parse as a `Request`.
        assert!(serde_json::from_slice::<Request>(&hello_bytes).is_err());
    }

    /// Spot-checks that a representative set of pre-SP3 destructive/egress/read ops stay refused for
    /// `MemoryClient` (`memory_client_allows_exactly_six_ops` covers the new SP3/Phase-3 ops). The true
    /// fail-closed guarantee is structural: `Role::allows`'s positive `matches!` allowlist denies every
    /// non-listed variant, present or future; this test guards against a regression that admits one of
    /// these specific ops.
    #[test]
    fn memory_client_refuses_pre_sp3_privileged_ops() {
        let allowed = [
            Request::Recall { onboarded: true, query: "q".into(), k: 1 },
            Request::Remember { onboarded: true, text: "t".into() },
        ];
        for req in &allowed {
            assert!(Role::MemoryClient.allows(req), "MemoryClient must allow {req:?}");
        }
        let refused = [
            Request::Teardown,
            Request::EnableCloudReasoner { onboarded: true, config: serde_json::Value::Null },
            Request::AddGrant { onboarded: true, path: std::path::PathBuf::from("/x") },
            Request::AddMandate {
                onboarded: true,
                target: std::path::PathBuf::from("/t"),
                source_scope: std::path::PathBuf::from("/s"),
                recipe: "r".into(),
            },
            Request::SetActiveModel { onboarded: true, model_id: "m".into(), safetensors_sha: "s".into() },
            Request::Status { onboarded: true },
            Request::RunIngest { onboarded: true },
        ];
        for req in &refused {
            assert!(!Role::MemoryClient.allows(req), "MemoryClient must REFUSE {req:?} (fail-closed)");
        }
        // App is allowed everything.
        for req in allowed.iter().chain(refused.iter()) {
            assert!(Role::App.allows(req), "App must allow {req:?}");
        }
    }

    /// `MemoryClient` allows EXACTLY the six guest ops — recall/remember, the SP3 capture and snapshot
    /// pokes, and the two Rung-3 Phase-3 resolution ops (`ListConflicts` + `ResolveConflict`, the I8
    /// relaxation) — and REFUSES every other new SP3 op (listing, get, delete, notes, supersede, stats,
    /// capture toggle/query, reflect toggle/query, retire/unretire). Enumerates all new variants so a future
    /// variant wrongly admitted to the guest role fails here (the fail-closed allowlist's positive guarantee, pinned per-op).
    #[test]
    fn memory_client_allows_exactly_six_ops() {
        use Request::*;
        let yes = [
            Recall { onboarded: true, query: "q".into(), k: 1 },
            Remember { onboarded: true, text: "t".into() },
            CaptureNotify { onboarded: true, session_id: "s".into(), transcript_path: "/p.jsonl".into() },
            Snapshot { onboarded: true, project: "/repo".into(), source: "startup".into(), session_id: None, transcript_path: None },
            ListConflicts { onboarded: true },
            ResolveConflict { onboarded: true, proposal_id: "P".into(), action: crate::types::ResolveActionWire::KeepBoth },
        ];
        for r in yes { assert!(Role::MemoryClient.allows(&r), "{r:?}"); }
        let no = [
            ListSessions { onboarded: true },
            GetSession { onboarded: true, session_id: "s".into() },
            DeleteSession { onboarded: true, session_id: "s".into() },
            ListNotes { onboarded: true },
            SupersedeNote { onboarded: true, event_id: "e".into(), text: "t".into() },
            RecallStats { onboarded: true },
            SetCaptureEnabled { onboarded: true, enabled: true, backfill: false },
            CaptureEnabled { onboarded: true },
            SetReflectEnabled { onboarded: true, enabled: true },
            ReflectEnabled { onboarded: true },
            RetireMemory { onboarded: true, target: RetireTarget::Note { event_id: "e".into() } },
            Unretire { onboarded: true, retired_event_id: "r".into() },
        ];
        for r in no { assert!(!Role::MemoryClient.allows(&r), "{r:?}"); }
    }

    /// Rung-4 R4-A: the two reflect-enable ops are App-only (guest-refused BY CONSTRUCTION — `Role::allows`
    /// is UNCHANGED, so they fall through the `MemoryClient` allowlist's `matches!` to `false`) and survive
    /// the externally-tagged JSON round-trip. Additive variants only → `PROTO_VERSION` stays 1.
    #[test]
    fn reflect_ops_are_app_only_and_round_trip() {
        use Request::*;
        // App allows both; MemoryClient (guest) refuses both — Role::allows is UNCHANGED.
        assert!(Role::App.allows(&SetReflectEnabled { onboarded: true, enabled: true }));
        assert!(Role::App.allows(&ReflectEnabled { onboarded: true }));
        assert!(!Role::MemoryClient.allows(&SetReflectEnabled { onboarded: true, enabled: true }),
            "reflect enable is App-only (guest-refused by construction)");
        assert!(!Role::MemoryClient.allows(&ReflectEnabled { onboarded: true }));
        // Additive → version stays 1.
        assert_eq!(PROTO_VERSION, 1);
        for req in [SetReflectEnabled { onboarded: true, enabled: false }, ReflectEnabled { onboarded: true }] {
            let back: Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
            assert_eq!(back, req);
        }
    }

    #[test]
    fn memory_client_allows_the_two_resolution_ops() {
        use Request::*;
        // I8 RELAXATION (owner decision 2026-07-17): the resolve ops are guest-reachable.
        assert!(Role::MemoryClient.allows(&ListConflicts { onboarded: true }));
        assert!(Role::MemoryClient.allows(&ResolveConflict {
            onboarded: true, proposal_id: "P".into(), action: crate::types::ResolveActionWire::KeepBoth,
        }));
    }

    #[test]
    fn resolution_ops_round_trip_serde() {
        let req = Request::ResolveConflict {
            onboarded: true, proposal_id: "P1".into(), action: crate::types::ResolveActionWire::RetireOlder,
        };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(back, req);
        let req = Request::ListConflicts { onboarded: true };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(back, req);
    }

    /// The SP3 `Snapshot` request round-trips through JSON with its `Option` fields populated — the
    /// externally-tagged variant survives serde unchanged (the backward-safe "add variants" contract).
    #[test]
    fn new_variants_round_trip_serde() {
        let req = Request::Snapshot { onboarded: true, project: "/r".into(), source: "compact".into(),
            session_id: Some("abc".into()), transcript_path: Some("/t.jsonl".into()) };
        let bytes = serde_json::to_vec(&req).unwrap();
        let back: Request = serde_json::from_slice(&bytes).unwrap();
        assert!(matches!(back, Request::Snapshot { .. }));

        // Rung 3 retire ops: both `RetireTarget` shapes and `Unretire` survive the externally-tagged
        // JSON round-trip unchanged (the same "add variants" back-compat contract).
        let note = Request::RetireMemory { onboarded: true, target: RetireTarget::Note { event_id: "e1".into() } };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&note).unwrap()).unwrap();
        assert_eq!(back, note);

        let passage = Request::RetireMemory {
            onboarded: true,
            target: RetireTarget::Passage { session_id: "s1".into(), passage_id: 3 },
        };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&passage).unwrap()).unwrap();
        assert_eq!(back, passage);

        let unretire = Request::Unretire { onboarded: true, retired_event_id: "r1".into() };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&unretire).unwrap()).unwrap();
        assert_eq!(back, unretire);
    }

    /// SP3 adds only new externally-tagged variants (backward-safe); the wire version must NOT bump — a
    /// bump would wedge already-connected users across an app upgrade (architect Critical).
    #[test]
    fn proto_version_still_one() { assert_eq!(PROTO_VERSION, 1); }

    /// A `Hello` frame that omits `role` (a peer predating the field) deserializes as `App` — the
    /// wire back-compat that keeps the app unchanged (I3).
    #[test]
    fn hello_role_defaults_to_app_on_missing_field() {
        let legacy = serde_json::json!({ "proto_version": PROTO_VERSION }).to_string();
        let hello: Hello = serde_json::from_str(&legacy).unwrap();
        assert_eq!(hello.role, Role::App);
    }
}
