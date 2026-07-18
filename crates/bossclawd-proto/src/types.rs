//! Wire mirror types for the `bossclawd` protocol.
//!
//! `bossclaw-core`'s boundary types are deliberately NOT `Serialize` (Task 0
//! inventory), so this module defines a serde-able **mirror** for each one plus
//! lossless `From`/`Into` conversions in both directions. The daemon converts
//! core → mirror before sending; the client converts mirror → core after
//! receiving (and vice-versa for request payloads). JSON (serde_json) is the
//! payload encoding; the frame codec in [`crate::write_frame`] carries the bytes.
//!
//! Two families live here, handled differently (matching the plan):
//!
//! 1. **`bossclaw-core` types** — full mirror + `From`/`Into` BOTH ways +
//!    round-trip test. Grouped by their source core module.
//! 2. **Desktop-crate summary types** — the app's `Engine` surface returns these
//!    today; their producers move daemon-side in M1a Tasks 4-6. Proto defines its
//!    own wire struct with fields matching the current desktop struct + its DTO.
//!    NO `From`/`Into` here — the desktop crate isn't touched until Tasks 5/6, so
//!    the conversions will live there. Serde round-trip tests only.
//!
//! NOTE for future readers: several Family-1 core mirrors (`MandateMirror`,
//! `PendingProposalMirror`, `MandateWriteRecordMirror`, `WriteOpMirror`) are NOT
//! referenced by the [`crate::Response`] enum — the wire carries the Family-2
//! `*Wire` summary projections instead, exactly as today's Tauri DTOs do. Those
//! mirrors are consumed daemon-internally starting Task 4 (the daemon holds the
//! core types and converts at its own seams). Do not "clean them up" as dead code.
//!
//! Integer widths: `usize`/`u128` ride the wire as-is because the daemon and the
//! app are ALWAYS the same host, arch, and build (a same-host Unix-socket
//! protocol — the app installs/spawns its own daemon). A hypothetical width
//! mismatch would degrade to a serde parse error on the oversized value, never
//! silent corruption.

use serde::{Deserialize, Serialize};

// ============================================================================
// Family 1 — `bossclaw-core` mirrors (From/Into both ways)
// ============================================================================

// ---- graph.rs ----
// Grant       ⇐⇒ bossclaw_core::Grant        (DTO source of truth: `GrantDto`, commands/engine.rs:7)
// Mandate     ⇐⇒ bossclaw_core::Mandate      (DTO source of truth: `MandateDto`, commands/engine.rs:388)
// FileRecord  ⇐⇒ bossclaw_core::graph::FileRecord (DTO source of truth: `FileRecordDto`, commands/engine.rs:19)

/// Wire mirror of [`bossclaw_core::Grant`] — a folded folder read-grant.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct GrantMirror {
    pub canonical_root: String,
    pub granted_at: String,
    pub revoked: bool,
}

impl From<bossclaw_core::Grant> for GrantMirror {
    fn from(g: bossclaw_core::Grant) -> Self {
        Self { canonical_root: g.canonical_root, granted_at: g.granted_at, revoked: g.revoked }
    }
}

impl From<GrantMirror> for bossclaw_core::Grant {
    fn from(m: GrantMirror) -> Self {
        Self { canonical_root: m.canonical_root, granted_at: m.granted_at, revoked: m.revoked }
    }
}

/// Wire mirror of [`bossclaw_core::Mandate`] — a signed standing derivation goal.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct MandateMirror {
    pub mandate_grant_id: String,
    pub target: String,
    pub source_scope: String,
    pub recipe: String,
    pub granted_at: String,
    pub revoked: bool,
}

impl From<bossclaw_core::Mandate> for MandateMirror {
    fn from(m: bossclaw_core::Mandate) -> Self {
        Self {
            mandate_grant_id: m.mandate_grant_id,
            target: m.target,
            source_scope: m.source_scope,
            recipe: m.recipe,
            granted_at: m.granted_at,
            revoked: m.revoked,
        }
    }
}

impl From<MandateMirror> for bossclaw_core::Mandate {
    fn from(m: MandateMirror) -> Self {
        Self {
            mandate_grant_id: m.mandate_grant_id,
            target: m.target,
            source_scope: m.source_scope,
            recipe: m.recipe,
            granted_at: m.granted_at,
            revoked: m.revoked,
        }
    }
}

/// Wire mirror of [`bossclaw_core::graph::FileRecord`] — a folded ingested-file
/// record. Note: `FileRecordDto` adds a derived `writable` flag; that is a
/// desktop-side projection, NOT part of the core record, so it is absent here.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct FileRecordMirror {
    pub canonical_path: String,
    pub file_event_id: String,
    pub content_hash: String,
    pub grant_root: String,
}

impl From<bossclaw_core::graph::FileRecord> for FileRecordMirror {
    fn from(f: bossclaw_core::graph::FileRecord) -> Self {
        Self {
            canonical_path: f.canonical_path,
            file_event_id: f.file_event_id,
            content_hash: f.content_hash,
            grant_root: f.grant_root,
        }
    }
}

impl From<FileRecordMirror> for bossclaw_core::graph::FileRecord {
    fn from(m: FileRecordMirror) -> Self {
        Self {
            canonical_path: m.canonical_path,
            file_event_id: m.file_event_id,
            content_hash: m.content_hash,
            grant_root: m.grant_root,
        }
    }
}

// ---- ingest.rs ----
// IngestReport ⇐⇒ bossclaw_core::IngestReport (DTO source: `IngestReportDto`+`SkipDto`, commands/engine.rs:49/43)
// `IngestReport` has NO `Clone` (Task 0), so conversions consume by value and the
// round-trip test constructs two identical values to compare.

/// Wire mirror of one `(path, reason)` entry in an [`IngestReportMirror`]'s
/// `skipped`/`failed` lists. Core carries these as `(PathBuf, String)` tuples;
/// the mirror uses a named struct (matching `SkipDto`) so the JSON is readable.
/// `path` is a lossy-UTF8 rendering of the core `PathBuf`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct SkipMirror {
    pub path: String,
    pub reason: String,
}

/// Wire mirror of [`bossclaw_core::IngestReport`] — one ingest run's accounting.
///
/// The `PathBuf`s in core's `skipped`/`failed` are rendered lossily to `String`
/// on the way out. On the way back they become `PathBuf`s again via
/// `PathBuf::from` — this is lossless for the valid-UTF8 paths the desktop
/// produces, matching how `IngestReportDto` already flattens them for the webview.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct IngestReportMirror {
    pub ingested: usize,
    pub superseded: usize,
    pub deduped: usize,
    pub skipped: Vec<SkipMirror>,
    pub failed: Vec<SkipMirror>,
}

impl From<bossclaw_core::IngestReport> for IngestReportMirror {
    fn from(r: bossclaw_core::IngestReport) -> Self {
        let map = |v: Vec<(std::path::PathBuf, String)>| -> Vec<SkipMirror> {
            v.into_iter()
                .map(|(p, reason)| SkipMirror { path: p.to_string_lossy().into_owned(), reason })
                .collect()
        };
        Self {
            ingested: r.ingested,
            superseded: r.superseded,
            deduped: r.deduped,
            skipped: map(r.skipped),
            failed: map(r.failed),
        }
    }
}

impl From<IngestReportMirror> for bossclaw_core::IngestReport {
    fn from(m: IngestReportMirror) -> Self {
        let map = |v: Vec<SkipMirror>| -> Vec<(std::path::PathBuf, String)> {
            v.into_iter().map(|s| (std::path::PathBuf::from(s.path), s.reason)).collect()
        };
        Self {
            ingested: m.ingested,
            superseded: m.superseded,
            deduped: m.deduped,
            skipped: map(m.skipped),
            failed: map(m.failed),
        }
    }
}

// ---- evolve.rs ----
// EvolveReport ⇐⇒ bossclaw_core::EvolveReport (DTO source: `EvolveReportDto`, commands/engine.rs:234 — DTO is a SUBSET; mirror carries ALL core fields)
// EvolveStatus ⇐⇒ bossclaw_core::EvolveStatus (DTO source: `EvolveStatusDto`, commands/engine.rs:206 — DTO MERGES telemetry; mirror carries the core fields only)

/// Wire mirror of [`bossclaw_core::EvolveReport`] — one evolve tick's counts.
///
/// `EvolveReportDto` surfaces only a subset to the webview; the wire mirror
/// carries ALL core fields (the daemon must ferry the full report; the desktop
/// projects it to the DTO subset in Task 5/6).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct EvolveReportMirror {
    pub entities_minted: usize,
    pub links_emitted: usize,
    pub invalidates_emitted: usize,
    pub pages_emitted: usize,
    pub pages_superseded: usize,
    pub memories_processed: usize,
    pub proposals_emitted: usize,
    pub proposals_rejected: usize,
    pub proposals_elided_cap: usize,
    pub skipped_disabled: bool,
    pub tainted_recall_snippets: usize,
}

impl From<bossclaw_core::EvolveReport> for EvolveReportMirror {
    fn from(r: bossclaw_core::EvolveReport) -> Self {
        Self {
            entities_minted: r.entities_minted,
            links_emitted: r.links_emitted,
            invalidates_emitted: r.invalidates_emitted,
            pages_emitted: r.pages_emitted,
            pages_superseded: r.pages_superseded,
            memories_processed: r.memories_processed,
            proposals_emitted: r.proposals_emitted,
            proposals_rejected: r.proposals_rejected,
            proposals_elided_cap: r.proposals_elided_cap,
            skipped_disabled: r.skipped_disabled,
            tainted_recall_snippets: r.tainted_recall_snippets,
        }
    }
}

impl From<EvolveReportMirror> for bossclaw_core::EvolveReport {
    fn from(m: EvolveReportMirror) -> Self {
        Self {
            entities_minted: m.entities_minted,
            links_emitted: m.links_emitted,
            invalidates_emitted: m.invalidates_emitted,
            pages_emitted: m.pages_emitted,
            pages_superseded: m.pages_superseded,
            memories_processed: m.memories_processed,
            proposals_emitted: m.proposals_emitted,
            proposals_rejected: m.proposals_rejected,
            proposals_elided_cap: m.proposals_elided_cap,
            skipped_disabled: m.skipped_disabled,
            tainted_recall_snippets: m.tainted_recall_snippets,
        }
    }
}

/// Wire mirror of [`bossclaw_core::EvolveStatus`] — a snapshot of loop health.
///
/// This is the CORE status only. The desktop `EvolveStatusDto` additionally
/// merges session telemetry (`last_tainted_snippets` and the real values for the
/// three stub fields); that merge stays desktop-side. The daemon ships this core
/// status plus the telemetry (see [`EvolveTelemetryWire`]) separately.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct EvolveStatusMirror {
    pub queue_depth: usize,
    pub last_tick_ms: Option<u128>,
    pub error_count: usize,
    pub last_error: Option<String>,
    pub enabled: bool,
}

impl From<bossclaw_core::EvolveStatus> for EvolveStatusMirror {
    fn from(s: bossclaw_core::EvolveStatus) -> Self {
        Self {
            queue_depth: s.queue_depth,
            last_tick_ms: s.last_tick_ms,
            error_count: s.error_count,
            last_error: s.last_error,
            enabled: s.enabled,
        }
    }
}

impl From<EvolveStatusMirror> for bossclaw_core::EvolveStatus {
    fn from(m: EvolveStatusMirror) -> Self {
        Self {
            queue_depth: m.queue_depth,
            last_tick_ms: m.last_tick_ms,
            error_count: m.error_count,
            last_error: m.last_error,
            enabled: m.enabled,
        }
    }
}

// ---- recall.rs ----
// Hit          ⇐⇒ bossclaw_core::Hit          (DTO source: `HitDto`, commands/engine.rs:76 — DTO adds hydrated `text`; mirror carries the core `Hit` only)
// RecallSource ⇐⇒ bossclaw_core::RecallSource (enum; `HitDto` renders it as snake_case strings)
// `Hit` has NO `PartialEq` (Task 0) → its round-trip test asserts field-by-field.

/// Wire mirror of [`bossclaw_core::RecallSource`] — which retrieval arm surfaced
/// a hit. Externally tagged (serde default); the desktop DTO renders these as the
/// snake_case strings `"vector"`/`"keyword"` for the webview, but on the daemon
/// wire the enum travels structurally (converted to core `RecallSource` first).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum RecallSourceMirror {
    Vector,
    Keyword,
}

impl From<bossclaw_core::RecallSource> for RecallSourceMirror {
    fn from(s: bossclaw_core::RecallSource) -> Self {
        match s {
            bossclaw_core::RecallSource::Vector => RecallSourceMirror::Vector,
            bossclaw_core::RecallSource::Keyword => RecallSourceMirror::Keyword,
        }
    }
}

impl From<RecallSourceMirror> for bossclaw_core::RecallSource {
    fn from(m: RecallSourceMirror) -> Self {
        match m {
            RecallSourceMirror::Vector => bossclaw_core::RecallSource::Vector,
            RecallSourceMirror::Keyword => bossclaw_core::RecallSource::Keyword,
        }
    }
}

/// Wire mirror of [`bossclaw_core::Hit`] — one recall result with fused score and
/// provenance. `HitDto` additionally carries a hydrated `text` snippet; that
/// hydration is a desktop/recall concern, so the mirror carries the core `Hit`
/// fields only (the snippet ferries separately, as it does today via `HitWithText`).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct HitMirror {
    pub event_id: String,
    pub score: f32,
    pub sources: Vec<RecallSourceMirror>,
    pub kind: String,
}

impl From<bossclaw_core::Hit> for HitMirror {
    fn from(h: bossclaw_core::Hit) -> Self {
        Self {
            event_id: h.event_id,
            score: h.score,
            sources: h.sources.into_iter().map(RecallSourceMirror::from).collect(),
            kind: h.kind,
        }
    }
}

impl From<HitMirror> for bossclaw_core::Hit {
    fn from(m: HitMirror) -> Self {
        Self {
            event_id: m.event_id,
            score: m.score,
            sources: m.sources.into_iter().map(bossclaw_core::RecallSource::from).collect(),
            kind: m.kind,
        }
    }
}

// ---- log.rs ----
// PendingProposal    ⇐⇒ bossclaw_core::PendingProposal    (DTO source: `ProposalDto`, commands/engine.rs:254)
// MandateWriteRecord ⇐⇒ bossclaw_core::MandateWriteRecord (DTO source: `MandateWriteDto`, commands/engine.rs:427)
// Both core types are `#[cfg(unix)]`; the mirror is unconditional (a same-host
// Unix-socket protocol is unix-only in practice, but the mirror + serde tests
// build on any target — the conversions are gated to unix where the core type exists).

/// Wire mirror of [`bossclaw_core::PendingProposal`] — one open write-proposal
/// projected for the Review queue. `inducing_key` and `verdict_summary` are
/// free-form JSON in core (`serde_json::Value`), carried through verbatim.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct PendingProposalMirror {
    pub id: String,
    pub target: String,
    pub op: String,
    pub new_content_hash: String,
    pub rationale: String,
    pub inducing_key: serde_json::Value,
    pub source_event_ids: Vec<String>,
    pub producer: String,
    pub verdict_summary: serde_json::Value,
    pub base_content_hash: Option<String>,
}

#[cfg(unix)]
impl From<bossclaw_core::PendingProposal> for PendingProposalMirror {
    fn from(p: bossclaw_core::PendingProposal) -> Self {
        Self {
            id: p.id,
            target: p.target,
            op: p.op,
            new_content_hash: p.new_content_hash,
            rationale: p.rationale,
            inducing_key: p.inducing_key,
            source_event_ids: p.source_event_ids,
            producer: p.producer,
            verdict_summary: p.verdict_summary,
            base_content_hash: p.base_content_hash,
        }
    }
}

#[cfg(unix)]
impl From<PendingProposalMirror> for bossclaw_core::PendingProposal {
    fn from(m: PendingProposalMirror) -> Self {
        Self {
            id: m.id,
            target: m.target,
            op: m.op,
            new_content_hash: m.new_content_hash,
            rationale: m.rationale,
            inducing_key: m.inducing_key,
            source_event_ids: m.source_event_ids,
            producer: m.producer,
            verdict_summary: m.verdict_summary,
            base_content_hash: m.base_content_hash,
        }
    }
}

/// Wire mirror of [`bossclaw_core::MandateWriteRecord`] — one applied write
/// attributed to a mandate, projected for the Mandate-activity list.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct MandateWriteRecordMirror {
    pub file_written_id: String,
    pub target: String,
    pub written_at: String,
    pub undone: bool,
}

#[cfg(unix)]
impl From<bossclaw_core::MandateWriteRecord> for MandateWriteRecordMirror {
    fn from(r: bossclaw_core::MandateWriteRecord) -> Self {
        Self {
            file_written_id: r.file_written_id,
            target: r.target,
            written_at: r.written_at,
            undone: r.undone,
        }
    }
}

#[cfg(unix)]
impl From<MandateWriteRecordMirror> for bossclaw_core::MandateWriteRecord {
    fn from(m: MandateWriteRecordMirror) -> Self {
        Self {
            file_written_id: m.file_written_id,
            target: m.target,
            written_at: m.written_at,
            undone: m.undone,
        }
    }
}

// ---- actuator.rs ----
// WriteOp ⇐⇒ bossclaw_core::WriteOp (enum; variant names Create/Edit/Delete)

/// Wire mirror of [`bossclaw_core::WriteOp`] — the kind of write a proposal
/// requests. A closed set: create, overwrite (edit), or hard-delete.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteOpMirror {
    Create,
    Edit,
    Delete,
}

impl From<bossclaw_core::WriteOp> for WriteOpMirror {
    fn from(op: bossclaw_core::WriteOp) -> Self {
        match op {
            bossclaw_core::WriteOp::Create => WriteOpMirror::Create,
            bossclaw_core::WriteOp::Edit => WriteOpMirror::Edit,
            bossclaw_core::WriteOp::Delete => WriteOpMirror::Delete,
        }
    }
}

impl From<WriteOpMirror> for bossclaw_core::WriteOp {
    fn from(m: WriteOpMirror) -> Self {
        match m {
            WriteOpMirror::Create => bossclaw_core::WriteOp::Create,
            WriteOpMirror::Edit => bossclaw_core::WriteOp::Edit,
            WriteOpMirror::Delete => bossclaw_core::WriteOp::Delete,
        }
    }
}

// ---- index.rs + log.rs (Rung-3 Phase-3 conflict resolution) ----
// ConflictRef         ⇐⇒ bossclaw_core::index::ConflictRef   (both ways: daemon builds from core, client reads back)
// ConflictProposalRow  ⇒ ConflictProposalWire                (one way: the daemon BUILDS the sanitized read row)
// ResolveAction       ⇐⇒ bossclaw_core::ResolveAction        (both ways)
// All three core types are UNGATED (portable data types — log.rs/index.rs), unlike the `#[cfg(unix)]`
// `PendingProposal`/`MandateWriteRecord` above, so these mirrors + conversions are unconditional.

/// Wire mirror of [`bossclaw_core::index::ConflictRef`]. Externally tagged so both variants are
/// self-describing on the wire; converts BOTH ways (the daemon builds it from core, the client reads it).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum ConflictRefWire {
    /// A memory note by event id.
    Note { event_id: String },
    /// A session passage by session id + ordinal.
    Passage { session_id: String, passage_id: usize },
}

impl From<bossclaw_core::index::ConflictRef> for ConflictRefWire {
    fn from(r: bossclaw_core::index::ConflictRef) -> Self {
        match r {
            bossclaw_core::index::ConflictRef::Note { event_id } => ConflictRefWire::Note { event_id },
            bossclaw_core::index::ConflictRef::Passage { session_id, passage_id } => {
                ConflictRefWire::Passage { session_id, passage_id }
            }
        }
    }
}

impl From<ConflictRefWire> for bossclaw_core::index::ConflictRef {
    fn from(r: ConflictRefWire) -> Self {
        match r {
            ConflictRefWire::Note { event_id } => bossclaw_core::index::ConflictRef::Note { event_id },
            ConflictRefWire::Passage { session_id, passage_id } => {
                bossclaw_core::index::ConflictRef::Passage { session_id, passage_id }
            }
        }
    }
}

/// Wire mirror of [`bossclaw_core::ConflictProposalRow`] — one pending conflict for the read surface. The
/// string fields (`winner_hint`/`confidence_band`/`why`) are daemon-sanitized when the `ListConflicts`
/// response is BUILT (server-side), so the client always receives single-line, bounded text (MINOR-1).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ConflictProposalWire {
    pub id: String,
    pub a_ref: ConflictRefWire,
    pub b_ref: ConflictRefWire,
    pub winner_hint: String,
    pub confidence_band: String,
    pub why: String,
    pub detected_at: i64,
}

impl From<bossclaw_core::ConflictProposalRow> for ConflictProposalWire {
    fn from(p: bossclaw_core::ConflictProposalRow) -> Self {
        ConflictProposalWire {
            id: p.id,
            a_ref: p.a_ref.into(),
            b_ref: p.b_ref.into(),
            winner_hint: p.winner_hint,
            confidence_band: p.confidence_band,
            why: p.why,
            detected_at: p.detected_at,
        }
    }
}

/// Wire mirror of [`bossclaw_core::ResolveAction`] (the four deterministic actions). Converts both ways.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResolveActionWire {
    RetireOlder,
    RetireNewer,
    KeepBoth,
    Dismiss,
}

impl From<ResolveActionWire> for bossclaw_core::ResolveAction {
    fn from(a: ResolveActionWire) -> Self {
        match a {
            ResolveActionWire::RetireOlder => bossclaw_core::ResolveAction::RetireOlder,
            ResolveActionWire::RetireNewer => bossclaw_core::ResolveAction::RetireNewer,
            ResolveActionWire::KeepBoth => bossclaw_core::ResolveAction::KeepBoth,
            ResolveActionWire::Dismiss => bossclaw_core::ResolveAction::Dismiss,
        }
    }
}

impl From<bossclaw_core::ResolveAction> for ResolveActionWire {
    fn from(a: bossclaw_core::ResolveAction) -> Self {
        match a {
            bossclaw_core::ResolveAction::RetireOlder => ResolveActionWire::RetireOlder,
            bossclaw_core::ResolveAction::RetireNewer => ResolveActionWire::RetireNewer,
            bossclaw_core::ResolveAction::KeepBoth => ResolveActionWire::KeepBoth,
            bossclaw_core::ResolveAction::Dismiss => ResolveActionWire::Dismiss,
        }
    }
}

// ============================================================================
// Family 2 — Desktop-crate summary wire structs (serde only; NO From/Into here)
//
// The app's `Engine` surface returns these today; their producers move daemon-side
// in Tasks 4-6, so they must travel the wire. Fields match the current desktop
// structs in `apps/desktop/src-tauri/src/engine/mod.rs` (+ `engine/reason.rs` for
// the reasoner config) and their DTOs. The desktop crate is NOT a dependency of
// proto (would be circular), so the `From`/`Into` conversions live in the desktop
// crate when Tasks 5/6 rewire it. Round-trip is proven by serde tests only.
// ============================================================================

/// Wire form of the desktop `ProposalSummary` (`engine/mod.rs:121`; DTO
/// `ProposalDto`, commands/engine.rs:254). A Review-queue row.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ProposalSummaryWire {
    pub id: String,
    pub target: String,
    pub op: String,
    pub new_content_hash: String,
    pub rationale: String,
    pub requires_loud_modal: bool,
    pub producer: String,
}

/// Wire form of the desktop `MandateSummary` (`engine/mod.rs:154`; DTO
/// `MandateDto`, commands/engine.rs:388). A Mandates-list row.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct MandateSummaryWire {
    pub mandate_grant_id: String,
    pub target: String,
    pub source_scope: String,
    pub recipe: String,
    pub granted_at: String,
    pub revoked: bool,
}

/// Wire form of the desktop `MandateWriteSummary` (`engine/mod.rs:165`; DTO
/// `MandateWriteDto`, commands/engine.rs:427). A Mandate-activity row.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct MandateWriteSummaryWire {
    pub file_written_id: String,
    pub target: String,
    pub written_at: String,
    pub undone: bool,
}

/// Wire form of the desktop `PreviewData` (`engine/mod.rs:195`; DTO `PreviewDto`,
/// commands/engine.rs:281). Everything the Review card renders for one proposal.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct PreviewDataWire {
    pub path: String,
    pub folder: String,
    pub rationale: String,
    pub op: String,
    pub old_text: String,
    pub new_text: String,
    pub requires_loud_modal: bool,
    pub taint: String,
}

/// Wire form of the desktop `EngineState` (`engine/mod.rs:209`). The coarse engine
/// state the status surfaces. Snake_case to match the desktop enum's serde repr.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum EngineStateWire {
    NotOnboarded,
    Ready,
    KeystoreInconsistent,
    KeystoreDbMismatch,
    ChainFailed,
}

/// Wire form of the desktop `EngineStatus` (`engine/mod.rs:218`, already
/// `Serialize`). The status the UI polls.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct EngineStatusWire {
    pub state: EngineStateWire,
    pub event_count: i64,
    pub chain_ok: bool,
}

/// The loaded-vs-intended embedder state for the language-pack UI (invariants I3/U5/U6). `Ok` when
/// the daemon serves the intended model; `Missing`/`Mismatch` are the fail-loud states (recall
/// refuses rather than silently serving English) the Settings card surfaces as re-download; `Failed`
/// is a background migration that errored while the OLD model keeps serving (the signed record stays
/// `InProgress`, so it is retryable) — distinct from a still-running migration so the UI shows
/// "migration failed — retry" rather than a spinner.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum ModelStateWire {
    /// The daemon serves the intended model (or the bundled English default).
    Ok,
    /// The signed intent names a model whose folder is absent (e.g. profile copied to a new machine).
    Missing {
        /// The model id the signed record expected to find on disk.
        expected: String,
    },
    /// The intended model's folder exists but its safetensors sha does not match the signed sha.
    Mismatch {
        /// The model id the signed record expected.
        expected: String,
        /// The sha256 actually found on disk.
        loaded: String,
    },
    /// A background re-embed migration failed; the OLD model still serves and the record stays
    /// `InProgress` (retryable). Carries a short, user-safe reason for the Settings card.
    Failed {
        /// Why the migration failed (a rendered engine error message; carries no secret material).
        reason: String,
    },
}

/// Re-index progress during a background language-pack migration (U6). `done`/`total` count
/// embeddable events; `done == total` at completion, just before the state returns to `Ok`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReindexProgressWire {
    /// Events re-embedded under the new model so far.
    pub done: u64,
    /// Total embeddable events the migration must re-embed.
    pub total: u64,
}

/// The language-pack status the Settings card polls (U6): the model state + optional live re-index
/// progress. Kept DISTINCT from [`EngineStatusWire`] so every existing `Status` consumer is untouched
/// (the invariance guard) and the card can render migration progress without widening the base status.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ModelStatusWire {
    /// The loaded-vs-intended model state.
    pub state: ModelStateWire,
    /// Live re-index progress while a migration runs; `None` when idle.
    pub reindex: Option<ReindexProgressWire>,
    /// The model id currently SERVED — the `Complete` signed record's model id once the migration has
    /// flipped, else the bundled English base id (an absent/`InProgress` record keeps English serving
    /// until the flip). Lets the Settings card show "Multilingual active" and stop offering a
    /// redundant re-download. `None` only from a daemon that predates this field.
    pub active_model_id: Option<String>,
}

/// Wire form of the desktop `EvolveTelemetry` (`engine/mod.rs:236`). Session-scoped
/// evolve telemetry the engine's core `EvolveStatus` stubs; produced daemon-side by
/// `record_tick` and merged into `EvolveStatusDto` desktop-side (Tasks 5/6).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct EvolveTelemetryWire {
    pub last_tick_ms: Option<u128>,
    pub error_count: usize,
    pub last_error: Option<String>,
    pub last_tainted_snippets: Option<usize>,
}

/// Wire form of the desktop `ApplyResult` (`engine/mod.rs:791`; DTO
/// `ApplyResultDto`, commands/engine.rs:309). The audit id of a successful apply.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ApplyResultWire {
    pub file_written_id: String,
}

/// Wire form of the desktop `ReasonerMode` (`engine/reason.rs:102`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ReasonerModeWire {
    Local,
    Cloud,
}

/// Wire form of the desktop `CloudProvider` (`engine/cloud_reasoner.rs:355`).
/// Snake-case/kebab strings match `provider_str` (`engine/reason.rs:127`):
/// `"anthropic"` / `"openai-compat"` / `"gemini"`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum CloudProviderWire {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai-compat")]
    OpenAiCompat,
    #[serde(rename = "gemini")]
    Gemini,
}

/// Wire form of the desktop `ReasonerConfig` (`engine/reason.rs:109`; DTO
/// `ReasonerConfigDto`, commands/engine.rs:532). The reasoner config the scheduler
/// reads (mode + provider + model + optional base_url). The `ready` flag on the DTO
/// is a live-computed readiness projection, NOT part of the config struct, so it is
/// absent here — the daemon computes readiness separately (the `GetReasonerReady` op).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ReasonerConfigWire {
    pub mode: ReasonerModeWire,
    pub provider: CloudProviderWire,
    pub model: String,
    pub base_url: Option<String>,
}

// ---- SP3 (Memory Hub "never forgets"): session archive + notes + recall telemetry ----
// Serde-only wire structs (Family 2): the daemon produces them from its capture store / event log
// (SP3 daemon tasks); the desktop Library renders them. NO From/Into here — no core mirror exists,
// they are produced daemon-side directly. Round-trip proven by serde tests only.

/// Wire summary of one captured Claude Code session — a Memory-browser Library row. `started_at`/
/// `ended_at` are unix-epoch seconds; `approx_bytes` is the rendered `.md`'s size (a cheap Library
/// "how big" hint, not a security boundary).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct SessionSummaryWire {
    pub session_id: String,
    pub title: String,
    pub project: String,
    pub tool: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub approx_bytes: u64,
}

/// Wire detail of one captured session: its [`SessionSummaryWire`] plus the deterministically
/// rendered Markdown body (the shoebox artifact). The `GetSession` op's payload.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct SessionDetailWire {
    pub summary: SessionSummaryWire,
    pub markdown: String,
}

/// Wire form of one `remember` note projected for the Memory-browser notes list. `superseded_by` is
/// `Some(new_event_id)` once the note has been edited (superseded), else `None`. `created_at` is
/// unix-epoch seconds.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct NoteWire {
    pub event_id: String,
    pub text: String,
    pub created_at: i64,
    pub superseded_by: Option<String>,
}

/// Wire form of one recorded recall miss — a recall that found nothing — for the retrieval-floor
/// tuning UI. `at` is unix-epoch seconds.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct RecallMissWire {
    pub query: String,
    pub at: i64,
}

/// Wire form of the recall-miss telemetry the `RecallStats` op returns: lifetime `total` recalls,
/// how many were `misses`, and the most `recent_misses` (bounded daemon-side) for the tuning UI.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, Debug)]
pub struct RecallStatsWire {
    pub total: u64,
    pub misses: u64,
    pub recent_misses: Vec<RecallMissWire>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Family 1: core-mirror round-trips (build core → mirror → serde → mirror → core → assert ==) ----

    #[test]
    fn grant_mirror_roundtrip() {
        let g = bossclaw_core::Grant {
            canonical_root: "/home/me/notes".to_string(),
            granted_at: "2026-07-02T00:00:00+00:00".to_string(),
            revoked: false,
        };
        let mirror: GrantMirror = g.clone().into();
        let bytes = serde_json::to_vec(&mirror).unwrap();
        let back: GrantMirror = serde_json::from_slice(&bytes).unwrap();
        let core_again: bossclaw_core::Grant = back.into();
        assert_eq!(g, core_again);
    }

    #[test]
    fn mandate_mirror_roundtrip() {
        let m = bossclaw_core::Mandate {
            mandate_grant_id: "01J-GRANT".to_string(),
            target: "/home/me/notes/summary.md".to_string(),
            source_scope: "/home/me/notes/sources".to_string(),
            recipe: "Keep summary.md a bullet list of the source titles.".to_string(),
            granted_at: "2026-07-02T01:02:03+00:00".to_string(),
            revoked: true,
        };
        let mirror: MandateMirror = m.clone().into();
        let bytes = serde_json::to_vec(&mirror).unwrap();
        let back: MandateMirror = serde_json::from_slice(&bytes).unwrap();
        let core_again: bossclaw_core::Mandate = back.into();
        assert_eq!(m, core_again);
    }

    #[test]
    fn file_record_mirror_roundtrip() {
        let f = bossclaw_core::graph::FileRecord {
            canonical_path: "/home/me/notes/a.md".to_string(),
            file_event_id: "01J-FILE".to_string(),
            content_hash: "deadbeef".to_string(),
            grant_root: "/home/me/notes".to_string(),
        };
        let mirror: FileRecordMirror = f.clone().into();
        let bytes = serde_json::to_vec(&mirror).unwrap();
        let back: FileRecordMirror = serde_json::from_slice(&bytes).unwrap();
        let core_again: bossclaw_core::graph::FileRecord = back.into();
        assert_eq!(f, core_again);
    }

    #[test]
    fn ingest_report_mirror_roundtrip() {
        // IngestReport has no Clone, so we build TWO identical values: one to
        // convert through the wire, one to compare the recovered value against.
        let make = || bossclaw_core::IngestReport {
            ingested: 3,
            superseded: 1,
            deduped: 2,
            skipped: vec![(std::path::PathBuf::from("/x/skip.bin"), "not valid UTF-8".to_string())],
            failed: vec![(std::path::PathBuf::from("/x/fail.txt"), "io error".to_string())],
        };
        let original = make();
        let mirror: IngestReportMirror = make().into();
        let bytes = serde_json::to_vec(&mirror).unwrap();
        let back: IngestReportMirror = serde_json::from_slice(&bytes).unwrap();
        let core_again: bossclaw_core::IngestReport = back.into();
        assert_eq!(original, core_again);
    }

    #[test]
    fn evolve_report_mirror_roundtrip() {
        let r = bossclaw_core::EvolveReport {
            entities_minted: 5,
            links_emitted: 4,
            invalidates_emitted: 1,
            pages_emitted: 2,
            pages_superseded: 1,
            memories_processed: 10,
            proposals_emitted: 3,
            proposals_rejected: 1,
            proposals_elided_cap: 2,
            skipped_disabled: false,
            tainted_recall_snippets: 7,
        };
        let mirror: EvolveReportMirror = r.clone().into();
        let bytes = serde_json::to_vec(&mirror).unwrap();
        let back: EvolveReportMirror = serde_json::from_slice(&bytes).unwrap();
        let core_again: bossclaw_core::EvolveReport = back.into();
        assert_eq!(r, core_again);
    }

    #[test]
    fn evolve_status_mirror_roundtrip() {
        let s = bossclaw_core::EvolveStatus {
            queue_depth: 12,
            last_tick_ms: Some(345),
            error_count: 2,
            last_error: Some("boom".to_string()),
            enabled: true,
        };
        let mirror: EvolveStatusMirror = s.clone().into();
        let bytes = serde_json::to_vec(&mirror).unwrap();
        let back: EvolveStatusMirror = serde_json::from_slice(&bytes).unwrap();
        let core_again: bossclaw_core::EvolveStatus = back.into();
        assert_eq!(s, core_again);
    }

    #[test]
    fn hit_mirror_roundtrip_fieldwise() {
        // `Hit` has no PartialEq (Task 0), so assert field-by-field.
        let h = bossclaw_core::Hit {
            event_id: "e1".to_string(),
            score: 0.75,
            sources: vec![bossclaw_core::RecallSource::Vector, bossclaw_core::RecallSource::Keyword],
            kind: "memory".to_string(),
        };
        let mirror: HitMirror = h.clone().into();
        let bytes = serde_json::to_vec(&mirror).unwrap();
        let back: HitMirror = serde_json::from_slice(&bytes).unwrap();
        let core_again: bossclaw_core::Hit = back.into();
        assert_eq!(h.event_id, core_again.event_id);
        assert_eq!(h.score, core_again.score);
        assert_eq!(h.kind, core_again.kind);
        assert_eq!(h.sources.len(), core_again.sources.len());
        for (a, b) in h.sources.iter().zip(core_again.sources.iter()) {
            assert_eq!(a, b, "each source arm round-trips");
        }
    }

    #[test]
    fn recall_source_mirror_roundtrip() {
        for src in [bossclaw_core::RecallSource::Vector, bossclaw_core::RecallSource::Keyword] {
            let mirror: RecallSourceMirror = src.into();
            let bytes = serde_json::to_vec(&mirror).unwrap();
            let back: RecallSourceMirror = serde_json::from_slice(&bytes).unwrap();
            let core_again: bossclaw_core::RecallSource = back.into();
            assert_eq!(src, core_again);
        }
    }

    #[test]
    fn recall_source_mirror_wire_strings_are_pascal_case() {
        // INTENTIONAL: the daemon wire uses serde's default PascalCase variant
        // names; the snake_case `"vector"`/`"keyword"` strings are the DESKTOP
        // DTO's webview render (`HitDto`), applied after mirror → core conversion.
        // Pins the repr so an accidental future `rename_all` breaks loudly here.
        assert_eq!(serde_json::to_string(&RecallSourceMirror::Vector).unwrap(), "\"Vector\"");
        assert_eq!(serde_json::to_string(&RecallSourceMirror::Keyword).unwrap(), "\"Keyword\"");
    }

    #[test]
    fn write_op_mirror_roundtrip() {
        for op in [
            bossclaw_core::WriteOp::Create,
            bossclaw_core::WriteOp::Edit,
            bossclaw_core::WriteOp::Delete,
        ] {
            let mirror: WriteOpMirror = op.into();
            let bytes = serde_json::to_vec(&mirror).unwrap();
            let back: WriteOpMirror = serde_json::from_slice(&bytes).unwrap();
            let core_again: bossclaw_core::WriteOp = back.into();
            assert_eq!(op, core_again);
        }
    }

    #[cfg(unix)]
    #[test]
    fn pending_proposal_mirror_roundtrip() {
        let p = bossclaw_core::PendingProposal {
            id: "01J-PROP".to_string(),
            target: "/home/me/notes/summary.md".to_string(),
            op: "edit".to_string(),
            new_content_hash: "cafef00d".to_string(),
            rationale: "Reconcile the changed source.".to_string(),
            inducing_key: serde_json::json!({"src": "a", "relation": "supersedes", "dst": "b"}),
            source_event_ids: vec!["01J-SRC-1".to_string(), "01J-SRC-2".to_string()],
            producer: "m6b-reconciler".to_string(),
            verdict_summary: serde_json::json!({
                "requires_loud_modal": false,
                "taint": "clean",
                "allowed": true,
                "base_content_hash": "beef"
            }),
            base_content_hash: Some("beef".to_string()),
        };
        let mirror: PendingProposalMirror = p.clone().into();
        let bytes = serde_json::to_vec(&mirror).unwrap();
        let back: PendingProposalMirror = serde_json::from_slice(&bytes).unwrap();
        let core_again: bossclaw_core::PendingProposal = back.into();
        assert_eq!(p, core_again);
    }

    #[cfg(unix)]
    #[test]
    fn mandate_write_record_mirror_roundtrip() {
        let r = bossclaw_core::MandateWriteRecord {
            file_written_id: "01J-WRITTEN".to_string(),
            target: "/home/me/notes/summary.md".to_string(),
            written_at: "2026-07-02T02:03:04+00:00".to_string(),
            undone: false,
        };
        let mirror: MandateWriteRecordMirror = r.clone().into();
        let bytes = serde_json::to_vec(&mirror).unwrap();
        let back: MandateWriteRecordMirror = serde_json::from_slice(&bytes).unwrap();
        let core_again: bossclaw_core::MandateWriteRecord = back.into();
        assert_eq!(r, core_again);
    }

    #[test]
    fn conflict_ref_and_proposal_wire_roundtrip_both_ways() {
        use bossclaw_core::index::ConflictRef;
        for r in [
            ConflictRef::Note { event_id: "n1".into() },
            ConflictRef::Passage { session_id: "s1".into(), passage_id: 4 },
        ] {
            let wire: ConflictRefWire = r.clone().into();
            let bytes = serde_json::to_vec(&wire).unwrap();
            let back: ConflictRefWire = serde_json::from_slice(&bytes).unwrap();
            let core: ConflictRef = back.into();
            assert_eq!(core, r, "ConflictRef survives core → wire → serde → core");
        }
        let row = bossclaw_core::ConflictProposalRow {
            id: "P1".into(),
            a_ref: ConflictRef::Note { event_id: "a".into() },
            b_ref: ConflictRef::Passage { session_id: "s".into(), passage_id: 0 },
            winner_hint: "newer".into(),
            confidence_band: "high".into(),
            why: "templated".into(),
            detected_at: 42,
        };
        let wire: ConflictProposalWire = row.clone().into();
        let bytes = serde_json::to_vec(&wire).unwrap();
        let back: ConflictProposalWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.id, "P1");
        assert_eq!(back.winner_hint, "newer");
        assert_eq!(back.detected_at, 42);
    }

    #[test]
    fn resolve_action_wire_maps_to_core_both_ways() {
        use bossclaw_core::ResolveAction;
        for (w, c) in [
            (ResolveActionWire::RetireOlder, ResolveAction::RetireOlder),
            (ResolveActionWire::RetireNewer, ResolveAction::RetireNewer),
            (ResolveActionWire::KeepBoth, ResolveAction::KeepBoth),
            (ResolveActionWire::Dismiss, ResolveAction::Dismiss),
        ] {
            let core: ResolveAction = w.into();
            assert_eq!(core, c);
            let back: ResolveActionWire = c.into();
            assert_eq!(back, w);
        }
    }

    // ---- Family 2: desktop summary wire structs (serde-only round-trips) ----

    #[test]
    fn proposal_summary_wire_serde_roundtrip() {
        let s = ProposalSummaryWire {
            id: "01J-PROP".to_string(),
            target: "/x/a.md".to_string(),
            op: "edit".to_string(),
            new_content_hash: "hash".to_string(),
            rationale: "why".to_string(),
            requires_loud_modal: true,
            producer: "m6c-mandate-proposer".to_string(),
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: ProposalSummaryWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn mandate_summary_wire_serde_roundtrip() {
        let s = MandateSummaryWire {
            mandate_grant_id: "01J-G".to_string(),
            target: "/x/t.md".to_string(),
            source_scope: "/x/src".to_string(),
            recipe: "recipe".to_string(),
            granted_at: "2026-07-02T00:00:00+00:00".to_string(),
            revoked: false,
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: MandateSummaryWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn mandate_write_summary_wire_serde_roundtrip() {
        let s = MandateWriteSummaryWire {
            file_written_id: "01J-W".to_string(),
            target: "/x/t.md".to_string(),
            written_at: "2026-07-02T00:00:00+00:00".to_string(),
            undone: true,
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: MandateWriteSummaryWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn preview_data_wire_serde_roundtrip() {
        let s = PreviewDataWire {
            path: "/x/t.md".to_string(),
            folder: "/x".to_string(),
            rationale: "why".to_string(),
            op: "edit".to_string(),
            old_text: "old".to_string(),
            new_text: "new".to_string(),
            requires_loud_modal: false,
            taint: "clean".to_string(),
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: PreviewDataWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn engine_status_wire_serde_roundtrip() {
        for state in [
            EngineStateWire::NotOnboarded,
            EngineStateWire::Ready,
            EngineStateWire::KeystoreInconsistent,
            EngineStateWire::KeystoreDbMismatch,
            EngineStateWire::ChainFailed,
        ] {
            let s = EngineStatusWire { state, event_count: 42, chain_ok: true };
            let bytes = serde_json::to_vec(&s).unwrap();
            let back: EngineStatusWire = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn engine_state_wire_snake_case_matches_desktop() {
        // The desktop `EngineState` uses `#[serde(rename_all = "snake_case")]`; the
        // wire form MUST encode identically so the two are interchangeable JSON.
        assert_eq!(serde_json::to_string(&EngineStateWire::NotOnboarded).unwrap(), "\"not_onboarded\"");
        assert_eq!(
            serde_json::to_string(&EngineStateWire::KeystoreDbMismatch).unwrap(),
            "\"keystore_db_mismatch\""
        );
    }

    #[test]
    fn evolve_telemetry_wire_serde_roundtrip() {
        let s = EvolveTelemetryWire {
            last_tick_ms: Some(99),
            error_count: 1,
            last_error: Some("err".to_string()),
            last_tainted_snippets: Some(3),
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: EvolveTelemetryWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
        // Default is all-empty (matches the desktop `#[derive(Default)]`).
        assert_eq!(EvolveTelemetryWire::default(), EvolveTelemetryWire {
            last_tick_ms: None,
            error_count: 0,
            last_error: None,
            last_tainted_snippets: None,
        });
    }

    #[test]
    fn apply_result_wire_serde_roundtrip() {
        let s = ApplyResultWire { file_written_id: "01J-W".to_string() };
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: ApplyResultWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn reasoner_config_wire_serde_roundtrip() {
        let configs = [
            ReasonerConfigWire {
                mode: ReasonerModeWire::Local,
                provider: CloudProviderWire::Anthropic,
                model: String::new(),
                base_url: None,
            },
            ReasonerConfigWire {
                mode: ReasonerModeWire::Cloud,
                provider: CloudProviderWire::OpenAiCompat,
                model: "gpt-x".to_string(),
                base_url: Some("https://api.example.com".to_string()),
            },
            ReasonerConfigWire {
                mode: ReasonerModeWire::Cloud,
                provider: CloudProviderWire::Gemini,
                model: "gemini-x".to_string(),
                base_url: None,
            },
        ];
        for c in configs {
            let bytes = serde_json::to_vec(&c).unwrap();
            let back: ReasonerConfigWire = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(c, back);
        }
    }

    #[test]
    fn cloud_provider_wire_strings_match_provider_str() {
        // Must match `engine/reason.rs::provider_str` so a config serialized by the
        // daemon deserializes to the same provider the desktop expects.
        assert_eq!(serde_json::to_string(&CloudProviderWire::Anthropic).unwrap(), "\"anthropic\"");
        assert_eq!(serde_json::to_string(&CloudProviderWire::OpenAiCompat).unwrap(), "\"openai-compat\"");
        assert_eq!(serde_json::to_string(&CloudProviderWire::Gemini).unwrap(), "\"gemini\"");
    }

    #[test]
    fn reasoner_mode_wire_strings_are_snake_case() {
        assert_eq!(serde_json::to_string(&ReasonerModeWire::Local).unwrap(), "\"local\"");
        assert_eq!(serde_json::to_string(&ReasonerModeWire::Cloud).unwrap(), "\"cloud\"");
    }

    // ---- SP3 session archive + notes + recall telemetry (serde-only round-trips) ----

    #[test]
    fn session_summary_wire_serde_roundtrip() {
        let s = SessionSummaryWire {
            session_id: "01J-SESSION".to_string(),
            title: "Fix the login bug".to_string(),
            project: "/home/me/repo".to_string(),
            tool: "claude-code".to_string(),
            started_at: 1_752_000_000,
            ended_at: 1_752_003_600,
            approx_bytes: 4096,
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: SessionSummaryWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn session_detail_wire_serde_roundtrip() {
        let s = SessionDetailWire {
            summary: SessionSummaryWire {
                session_id: "01J-SESSION".to_string(),
                title: "Fix the login bug".to_string(),
                project: "/home/me/repo".to_string(),
                tool: "claude-code".to_string(),
                started_at: 1_752_000_000,
                ended_at: 1_752_003_600,
                approx_bytes: 4096,
            },
            markdown: "# Fix the login bug\n\n> user: it 500s\n".to_string(),
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: SessionDetailWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn note_wire_serde_roundtrip() {
        // Both `superseded_by` states (current note vs. edited/superseded note) round-trip.
        for superseded_by in [None, Some("01J-NEWER".to_string())] {
            let s = NoteWire {
                event_id: "01J-NOTE".to_string(),
                text: "prefer tabs over spaces".to_string(),
                created_at: 1_752_000_000,
                superseded_by,
            };
            let bytes = serde_json::to_vec(&s).unwrap();
            let back: NoteWire = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn recall_stats_wire_serde_roundtrip() {
        // Populated `recent_misses` also exercises the nested `RecallMissWire`.
        let s = RecallStatsWire {
            total: 128,
            misses: 12,
            recent_misses: vec![
                RecallMissWire { query: "who is Aria?".to_string(), at: 1_752_000_000 },
                RecallMissWire { query: "the port number".to_string(), at: 1_752_000_600 },
            ],
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: RecallStatsWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(s, back);
    }
}
