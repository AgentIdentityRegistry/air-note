//! The append-only event log. The single source of truth.
//!
//! Appends are strictly serialized: one process-wide `Mutex` guards the
//! read-tip → hash → sign → insert critical section, so the hash chain can
//! never fork (spec §4 single-writer invariant). The evolve loop (M4) is NOT a
//! privileged writer — it calls `append` like everyone else.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use rusqlite::OptionalExtension;

use crate::embed::Embedder;
use crate::error::BossclawError;
use crate::event::{compute_hash, Event, ModelMeta};
use crate::evolve::{EvolveReport, EvolveStatus};
use crate::extract::{ResolveDecision, EVOLVE_BATCH, MAX_ENTITIES_PER_MEMORY, MAX_REFLECT};
use crate::graph::{
    entity_node_id, CONFIG_EVENT_TYPE, ENTITY_EVENT_TYPE, ENTITY_NODE_KIND, EXTERNAL_NODE_KIND,
    MANUAL_LINK_PRODUCER, MEMORY_EVENT_TYPE, MEMORY_NODE_KIND, UNRESOLVED_ENTITY_TYPE,
};
use crate::highwater::{HighWaterStore, Mark};
use crate::index::{HnswIndex, VectorIndex};
use crate::keyword;
use crate::recall::{
    fuse_scored_arms, Hit, NoopReranker, RecallOptions, RecallSource, Reranker, FUSION_FETCH,
    GRAPH_HOP_DECAY, GRAPH_MAX_HOPS, GRAPH_REINFORCE_TOPK, GRAPH_WEIGHT, HALF_LIFE_SECS,
    PIN_MULTIPLIER, RECENCY_WEIGHT,
};
use crate::sign::{sign_hash, verify_hash};
use crate::store::Store;

/// Reserved store-format version recorded in every `config` event.
///
/// Format-gating logic (refusing to open a store written by a future version)
/// is deferred to a later milestone. This constant is the single authoritative
/// source for what value gets written today.
pub const SCHEMA_VERSION: u32 = 1;

/// The distinctive phrase the engine's defense-in-depth loud-gate
/// ([`EventLog::execute_write_inner`]) embeds when it refuses a loud write made without
/// `acknowledged_loud`. Single-sourced so the refusal site here, the desktop classifier that maps
/// the refusal back to a "risky → leave queued" outcome (`bossclaw_core::LOUD_ACK_REQUIRED_MSG`),
/// and the loud-gate tests all agree on ONE string — no duplicated magic literal to drift.
pub const LOUD_ACK_REQUIRED_MSG: &str = "loud write requires acknowledged_loud";

/// Per-target depth of the N-deep recoverable-undo store (M6a, spec L3 §7.3).
/// After each write, older `undo_state` rows for the same `canonical_target` are
/// GC'd so at most this many remain (by `created_at`/rowid order). `16` is the
/// spec default; a single source so the capture, the GC, and the test agree.
const UNDO_DEPTH: usize = 16;

/// Statistics returned by [`EventLog::reembed_migration`].
///
/// Provides the §15 time-budget observability signal: callers (and handoff
/// records) use `elapsed_ms` to gauge migration cost before scheduling re-index
/// in production.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReembedStats {
    /// Number of events that had no vector for the new model and were
    /// successfully re-embedded during this migration.
    pub reembedded: usize,
    /// Number of stale `vectors` rows (under the old model) that were
    /// garbage-collected.
    pub gc_removed: usize,
    /// Wall-clock duration of the entire migration in milliseconds.
    pub elapsed_ms: u128,
}

/// The parsed content of the latest `config` event.
///
/// A `config` event uses `event_type = "config"` and carries a `content`
/// object with the following fields:
/// - `active_model_id`: identifier of the active embedding model.
/// - `dim`: vector dimensionality produced by that model.
/// - `schema_version`: reserved for format-gating in later milestones.
///
/// Only the LATEST config event is authoritative. Appending a new config event
/// is how the active model is rotated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveModel {
    /// Identifier of the active embedding model (e.g. `"mock-v1"`).
    pub active_model_id: String,
    /// Dimensionality of the vectors produced by the active model.
    /// Callers feeding this into an `Embedder` should convert with
    /// `usize::try_from(model.dim).expect("dim fits usize")`.
    pub dim: u32,
    /// Reserved: format-gating logic is deferred to a later milestone.
    pub schema_version: u32,
}

/// Whether an opt-in language-pack migration has finished. `InProgress` means consent was
/// recorded and re-embedding started but the atomic end-flip has NOT run (recall keeps serving
/// the OLD model); `Complete` means the multilingual model is live (recall serves it). The daemon
/// resumes an `InProgress` migration on boot (invariant I6).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MigrationState {
    /// Consent recorded and re-embedding started, but the atomic end-flip has NOT run — recall
    /// keeps serving the OLD model; the daemon resumes this on boot (invariant I6).
    InProgress,
    /// The re-embed finished and the multilingual model is live — recall serves it.
    Complete,
}

/// The signed opt-in language-pack record — the single source of truth (invariant I2) for which
/// embedding model is enabled, its verified safetensors sha (invariant I4), and the user's consent.
/// Stored under [`LANGUAGE_PACK_KEY`] in a signed, hash-chained `config` event. It carries NONE of
/// [`ActiveModel`]'s fields, so it never disturbs [`EventLog::active_model`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanguagePackRecord {
    /// The enabled model id (e.g. `"minishlab/potion-multilingual-128M"`).
    pub model_id: String,
    /// The sha256 of the model's `model.safetensors`, verified by the downloader before install
    /// and re-verified by the daemon at load (invariant I4 — the only guard, since both models are
    /// 256-dim and the dim probe cannot catch a mislabel).
    pub safetensors_sha: String,
    /// Whether the consent-gated re-embed has finished (see [`MigrationState`]).
    pub migration: MigrationState,
    /// RFC3339 timestamp the user consented, for audit surfacing.
    pub consented_at: String,
}

/// A hit from the M6c synthesis cache ([`EventLog::get_synthesis_cache`]).
///
/// Carries the synthesized expected file bytes TOGETHER with the synth-time lineage
/// that produced them (finding B, spec §5.2/§5.3): `source_event_ids_at_synth` is the
/// EXACT engine-gathered source ids read at synthesis time, so a later cache HIT can
/// union it with the then-current in-scope sources without ever silently dropping a
/// tainted source that left scope between synthesis and the hit. The cache is NEVER an
/// authorization source — the confirm path re-gates the bytes through the full M6a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthCacheRow {
    /// The synthesized expected whole-file bytes for this `(mandate, source-state)`.
    pub expected_bytes: Vec<u8>,
    /// The content hash recorded for `expected_bytes` at synthesis time.
    pub expected_hash: String,
    /// The exact engine-gathered source event ids read at synthesis time (finding B).
    /// Travels in the SAME row as the bytes so taint lineage can be unioned monotonically.
    pub source_event_ids_at_synth: Vec<String>,
}

/// The DECISION returned by the gather + cached-or-synth half of the mandate proposer
/// phase ([`EventLog::mandate_phase_for`], M6c §5.1/§5.2, Task 9a). It carries NO side
/// effect: producing it appends no event and runs no write gate. Task 9b turns a
/// [`MandateAction::Propose`] into a gated `write_proposal` and a [`MandateAction::Reject`]
/// into a recorded `write_rejected`; an [`MandateAction::Elide`] emits nothing and stays
/// retryable on the next tick.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MandateAction {
    /// The target is out of sync; synthesis produced new bytes to write. `op` is
    /// [`WriteOp::Create`] when the target file does not exist yet, else [`WriteOp::Edit`]
    /// — M6c never deletes. `expected` is the whole-file bytes; `lineage` is the
    /// engine-gathered, sorted+deduped union of the mandate id with the source event ids
    /// (NEVER any model-derived id — see the union site in `mandate_phase_for`).
    Propose {
        /// The synthesized whole-file bytes the proposal would write.
        expected: Vec<u8>,
        /// Engine-gathered lineage (mandate ∪ source event ids), sorted + deduped.
        lineage: Vec<String>,
        /// `Create` (target absent) or `Edit` (target present); never `Delete`.
        op: crate::actuator::WriteOp,
        /// The exact `sources_hash` `mandate_phase_for` computed over the SORTED
        /// `(canonical_path, content_hash)` pairs of the in-scope sources (Task 9a). Task
        /// 9b folds it into the suppression/recording `inducing_key`
        /// (`{mandate, target, sources_hash}`, §5.4) so the key matches
        /// [`EventLog::is_mandate_proposal_suppressed`]'s shape byte-for-byte — returned
        /// from the phase rather than recomputed so the two sites cannot drift.
        sources_hash: String,
    },
    /// Nothing to do — in sync, no in-scope sources, or over a directory-bomb cap. A
    /// RETRYABLE no-op: it appends NO event, so the same mandate is re-evaluated next tick.
    Elide,
    /// A genuine synthesis failure (e.g. the model returned empty content). Task 9b
    /// records a `write_rejected` for this. `reason` is the reject reason; `sources_hash`
    /// is the phase's computed source-state digest, carried so the recorded
    /// `write_rejected`'s `inducing_key` (`{mandate, target, sources_hash}`) is keyed on the
    /// SAME source-state a `Propose` would use — making the rejection TERMINAL for exactly
    /// that source-state (a later DIFFERENT source-state is a fresh, non-suppressed ask),
    /// mirroring M6b's same-key reject.
    Reject {
        /// The human-readable reject reason recorded on the `write_rejected`.
        reason: String,
        /// The source-state digest for the rejected attempt (keys the `inducing_key`).
        sources_hash: String,
    },
}

const GENESIS: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// DID stamped on engine-authored events (`link`/`invalidate`) in v1. Named so
/// the literal is single-sourced (like [`MANUAL_LINK_PRODUCER`]); M4/M7 will
/// replace this with the user's real DID threaded through [`EventLog::signer_did`].
const ENGINE_SIGNER_DID: &str = "did:wba:bossclaw-engine";

/// The `content` key carrying the evolve on/off switch in a control `config`
/// event (spec §8 / Rev 2 F2-sec). Single-sourced so the ONE writer
/// ([`EventLog::set_evolve_enabled`]) and the reader ([`EventLog::evolve_enabled`])
/// can never drift the key apart — a typo in one would silently disarm the
/// fail-closed off-switch.
const EVOLVE_ENABLED_KEY: &str = "evolve_enabled";

/// The `content` key carrying the M6b reconciliation-proposer on/off switch in a
/// control `config` event (M6b §5.3). Single-sourced so the ONE writer
/// ([`EventLog::set_proposals_enabled`]) and the reader
/// ([`EventLog::proposals_enabled`]) can never drift the key apart. INDEPENDENT of
/// [`EVOLVE_ENABLED_KEY`]: turning proposals off leaves evolve curation (entity /
/// link / invalidate emission) fully running — it suppresses ONLY the autonomous
/// `write_proposal` synthesis layered on top of a confirmed contradiction.
const PROPOSALS_ENABLED_KEY: &str = "proposals_enabled";

/// The `content` key carrying the M6c mandate-proposer on/off switch in a control
/// `config` event (M6c §5.5 / D8). Single-sourced so the ONE writer
/// ([`EventLog::set_mandates_enabled`]) and the reader
/// ([`EventLog::mandates_enabled`]) can never drift the key apart. INDEPENDENT of
/// [`PROPOSALS_ENABLED_KEY`] and [`EVOLVE_ENABLED_KEY`]: turning mandates off leaves
/// evolve curation and the M6b reconciliation proposer fully running — it suppresses
/// ONLY the autonomous mandate-driven `write_proposal` synthesis.
const MANDATES_ENABLED_KEY: &str = "mandates_enabled";

/// Content-key for the non-security reasoner config (mode/provider/model/base_url),
/// written by [`EventLog::set_reasoner_config`] and read by
/// [`EventLog::reasoner_config_json`] (Milestone D spec R1). A signed `config`
/// event, not a webview-writable file — egress-adjacent config stays tamper-evident.
const REASONER_CONFIG_KEY: &str = "reasoner_config";

/// Content-key for the signed cloud-enable CONSENT record binding
/// {provider, base_url_host, key_fingerprint, consented_at}, written by
/// [`EventLog::set_cloud_reasoner_consent`] and read by
/// [`EventLog::cloud_reasoner_consent_json`] (Milestone D spec R1/R5). Its
/// presence is what authorizes cloud egress, so it MUST be a signed event.
const CLOUD_REASONER_CONSENT_KEY: &str = "cloud_reasoner_consent";

/// Signed `config` key for the opt-in multilingual language pack (rung 2). Its presence +
/// `migration == Complete` is the SOLE authority for "load the multilingual model" (invariant I2);
/// `InProgress` records a consented-but-unfinished re-embed the daemon RESUMES on boot (I6). Written
/// only by [`EventLog::set_language_pack_record`]; absence means the English default (I7).
const LANGUAGE_PACK_KEY: &str = "language_pack";

/// The `content` key carrying the SP3 ongoing-capture on/off switch in a control `config` event
/// (spec §6a). Single-sourced so the ONE writer ([`EventLog::set_capture_enabled`]) and the reader
/// ([`EventLog::capture_enabled`]) can never drift the key apart, exactly like
/// [`MANDATES_ENABLED_KEY`]. UNLIKE mandates the DEFAULT is CLOSED (`false` when never set) —
/// capture must never run for a user who never consented (critic Critical C1 / I10).
const CAPTURE_ENABLED_KEY: &str = "capture_enabled";

/// The `content` key carrying the SP3 one-time backfill (historical-sweep) consent in a control
/// `config` event (spec §6a). Default CLOSED: a user who declined history at Connect — or never
/// connected — never has their pre-[`CAPTURE_ENABLED_AT_KEY`] backlog imported (critic Major M4).
const BACKFILL_CONSENTED_KEY: &str = "backfill_consented";

/// The `content` key carrying the wall-clock instant (supplied by the daemon — core stays
/// clock-free) at which capture most recently flipped ON (spec §6a). A VALUED (i64) key read via
/// [`EventLog::latest_config_value`], NOT a bool flag, so it has no [`ConfigFlag`] variant. The
/// sweeper's forward-only window is `mtime >= capture_enabled_at`; the disable path leaves it
/// sticky (capture is off, so it is not consulted).
const CAPTURE_ENABLED_AT_KEY: &str = "capture_enabled_at";

/// The `content` key carrying the Rung-3 Phase-2 conflict-detection on/off switch (spec §3.6).
/// Single-sourced (one writer [`EventLog::set_conflict_detect_enabled`], one reader
/// [`EventLog::conflict_detect_enabled`]). DEFAULT CLOSED — detection never runs for a user who
/// never consented (invariant I3), exactly like [`CAPTURE_ENABLED_KEY`].
const CONFLICT_DETECT_ENABLED_KEY: &str = "conflict_detect_enabled";

/// A typed identifier for a control-`config` key, mapping to the private `*_KEY` consts. Used by
/// `EventLog::explicitly_set` (and the capture getters) so callers (e.g. the desktop
/// `prime_switches`) reference a compile-checked variant instead of a stringly-typed key that could
/// drift on a rename (M2). `#[cfg(unix)]` is unnecessary — config flags exist on all platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFlag {
    /// The evolve on/off switch ([`EVOLVE_ENABLED_KEY`]).
    Evolve,
    /// The M6b reconciliation-proposer on/off switch ([`PROPOSALS_ENABLED_KEY`]).
    Proposals,
    /// The M6c mandate-proposer on/off switch ([`MANDATES_ENABLED_KEY`]).
    Mandates,
    /// The non-security reasoner config record ([`REASONER_CONFIG_KEY`]).
    ReasonerConfig,
    /// The signed cloud-enable consent record ([`CLOUD_REASONER_CONSENT_KEY`]).
    CloudReasonerConsent,
    /// The signed opt-in multilingual language-pack record ([`LANGUAGE_PACK_KEY`]).
    LanguagePack,
    /// The SP3 ongoing-capture on/off switch ([`CAPTURE_ENABLED_KEY`]). Default CLOSED.
    CaptureEnabled,
    /// The SP3 one-time backfill consent ([`BACKFILL_CONSENTED_KEY`]). Default CLOSED.
    BackfillConsented,
    /// The Rung-3 Phase-2 conflict-detection on/off switch ([`CONFLICT_DETECT_ENABLED_KEY`]). Default CLOSED.
    ConflictDetect,
}

impl ConfigFlag {
    /// The private content-key const this flag is stored under.
    fn key(self) -> &'static str {
        match self {
            ConfigFlag::Evolve => EVOLVE_ENABLED_KEY,
            ConfigFlag::Proposals => PROPOSALS_ENABLED_KEY,
            ConfigFlag::Mandates => MANDATES_ENABLED_KEY,
            ConfigFlag::ReasonerConfig => REASONER_CONFIG_KEY,
            ConfigFlag::CloudReasonerConsent => CLOUD_REASONER_CONSENT_KEY,
            ConfigFlag::LanguagePack => LANGUAGE_PACK_KEY,
            ConfigFlag::CaptureEnabled => CAPTURE_ENABLED_KEY,
            ConfigFlag::BackfillConsented => BACKFILL_CONSENTED_KEY,
            ConfigFlag::ConflictDetect => CONFLICT_DETECT_ENABLED_KEY,
        }
    }
}

/// The instruction channel (`system`) for the M6b whole-file rewrite reasoner call
/// (spec §5.5). The data channel is [`crate::reconcile::build_rewrite_prompt`]'s
/// fenced frame; this system line states the engine's intent so the prompt itself
/// stays purely about the (untrusted, fenced) file body.
#[cfg(unix)]
const RECONCILE_SYSTEM: &str =
    "You correct a file so it is consistent with an engine-established fact. \
     Output only the full corrected file as JSON {\"corrected_content\": ...}.";

/// The instruction channel (`system`) for the M6c mandate synthesis reasoner call
/// (spec §5.2). The data channel is [`crate::mandate::build_recipe_prompt`]'s fenced
/// frame (the trusted recipe above, the untrusted sources fenced below); this system
/// line states the engine's intent so the prompt stays purely about the synthesis.
/// Mirrors [`RECONCILE_SYSTEM`].
#[cfg(unix)]
const MANDATE_SYSTEM: &str =
    "You synthesize a file from sources per a user recipe. \
     Output only the full synced content as JSON {\"synced_content\": ...}.";

/// `(event_id, arm_score)` pair returned by each retrieval arm. Used as the
/// common type for both the vector arm (cosine distance, lower=better) and the
/// keyword arm (BM25 score, lower=better) before fusion.
type ArmHit = (String, f32);

/// Pair of live arm results (vector arm, keyword arm) returned by
/// [`resolve_arms`] after applying §10 graceful degradation.
type ArmPair = (Vec<ArmHit>, Vec<ArmHit>);
const POISON: &str = "event log mutex poisoned";

/// Number of bytes in a little-endian `f32`. Used to size and validate the
/// `embedding` BLOB encoding in the `vectors` table.
const F32_BYTES: usize = std::mem::size_of::<f32>();

/// Event types whose `content["text"]` is fed to the embedder. `page` does not
/// exist until M4 but is listed here so the seam is forward-compatible.
/// Composed from the canonical `*_EVENT_TYPE` consts in `graph` so this array
/// and the stamp sites cannot drift.
const EMBEDDABLE_EVENT_TYPES: &[&str] = &[
    crate::graph::MEMORY_EVENT_TYPE,
    crate::graph::PAGE_EVENT_TYPE,
    crate::graph::FILE_INGESTED_EVENT_TYPE,
    // A captured session's title text must be recallable (SP3). The DELETED
    // tombstone is deliberately NOT here — tombstones are never embedded.
    crate::graph::SESSION_CAPTURED_EVENT_TYPE,
];

/// Metadata for one captured coding-agent session — the input to
/// [`EventLog::capture_session`]. All string fields are owned; timestamps are
/// caller-supplied (the engine reads no clock inside this pure logic). `path` is
/// the on-disk location of the session body (`<data_dir>/sessions/…`), recorded
/// as metadata ONLY: `capture_session` records the event; the `.md` file store
/// is a later task (A7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    /// Stable per-session identity key (the fold's grouping key).
    pub session_id: String,
    /// Human-readable session title (embedded so it is recallable).
    pub title: String,
    /// The project/repo the session ran against.
    pub project: String,
    /// The coding agent that produced the session (e.g. `claude-code`).
    pub tool: String,
    /// Caller-supplied start timestamp (Unix seconds; never a clock read here).
    pub started_at: i64,
    /// Caller-supplied end timestamp (Unix seconds; never a clock read here).
    pub ended_at: i64,
    /// On-disk path of the session body (metadata only; file store is A7).
    pub path: String,
    /// SHA-256 of the session body — the dedup/supersede decision key.
    pub sha256: String,
    /// Approximate body size in bytes (metadata only).
    pub approx_bytes: u64,
}

/// One CURRENT captured session, folded from the log: the latest
/// (`seq`-max) `session_captured` for a `session_id` not retired by a
/// `supersede` and not tombstoned by a `session_deleted`. Mirrors
/// [`crate::graph::Page`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentSession {
    /// The current `session_captured` event's id.
    pub event_id: String,
    /// The session's stable identity key.
    pub session_id: String,
    /// Human-readable session title.
    pub title: String,
    /// The project/repo the session ran against.
    pub project: String,
    /// The coding agent that produced the session.
    pub tool: String,
    /// Start timestamp (Unix seconds).
    pub started_at: i64,
    /// End timestamp (Unix seconds).
    pub ended_at: i64,
    /// On-disk path of the session body.
    pub path: String,
    /// SHA-256 of the session body.
    pub sha256: String,
    /// Approximate body size in bytes.
    pub approx_bytes: u64,
}

/// One CURRENT remembered note, folded from the log: a `memory`-kind event
/// ([`EventLog::remember`] / the corrected note of [`EventLog::supersede_note`])
/// that is NOT itself retired by a `supersede`. The projection behind
/// [`EventLog::current_notes`] and the Memory-browser notes list (SP3 §7/§9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentNote {
    /// The note event's id.
    pub event_id: String,
    /// The note text (`content["text"]`; empty if the payload lacks it).
    pub text: String,
    /// Creation time (the event `ts` parsed to Unix seconds; 0 if absent/unparseable).
    pub created_at: i64,
    /// Always `None` in the current-only fold (a superseded note is EXCLUDED, so any
    /// note returned is a live head). Carried to mirror `NoteWire`'s shape for a
    /// possible future edit-history view; this projection never populates it.
    pub superseded_by: Option<String>,
}

/// One OPEN conflict proposal for the read surface (spec §3.5). Both refs are still current
/// (withdrawn proposals are absent). Ungated (portable data type). `#![deny(missing_docs)]`
/// requires a `///` on every pub field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictProposalRow {
    /// The `conflict_proposal` event id.
    pub id: String,
    /// The first ref as recorded by the proposer (by convention the OLDER side, ordered upstream by
    /// ingest ts); this projection passes it through, it does not enforce the ordering.
    pub a_ref: crate::index::ConflictRef,
    /// The second ref as recorded by the proposer (by convention the NEWER side, ordered upstream by
    /// ingest ts); this projection passes it through, it does not enforce the ordering.
    pub b_ref: crate::index::ConflictRef,
    /// Advisory winner label ("newer"/"older"/"unclear") — the engine resolves by ts.
    pub winner_hint: String,
    /// Coarse confidence band ("high"/"med") — the raw numeric confidence is never persisted.
    pub confidence_band: String,
    /// The CONTENT-FREE templated reason (`conflict::templated_why`); never memory text.
    pub why: String,
    /// Wall-clock instant detection recorded this proposal (Unix seconds).
    pub detected_at: i64,
}

/// One conflict-detection SUBJECT: a memory appended after the cursor. A `memory` event yields
/// ONE `Note` subject; a `session_captured` event yields one `Passage` subject per live
/// (non-retired) passage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSubject {
    /// The source event's `seq` (the cursor's first coordinate).
    pub seq: i64,
    /// This subject's within-`seq` id — its `passage_id` for a passage, `0` for a note. The
    /// cursor's second coordinate advances to `within_seq_id + 1` once this subject is judged.
    pub within_seq_id: usize,
    /// The typed reference this subject searches the fights index for.
    pub subject: crate::index::ConflictRef,
}

/// One of the four deterministic resolution actions (spec §1). NO LLM in the path. Portable data type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveAction {
    /// Retire the FROZEN older side (a_ref) — reversible.
    RetireOlder,
    /// Retire the FROZEN newer side (b_ref) — reversible.
    RetireNewer,
    /// Both memories coexist — never re-proposed, dropped from the read surface.
    KeepBoth,
    /// Snooze the pair — re-opens on a material change to a member (§3.1).
    Dismiss,
}

/// The outcome of a [`EventLog::resolve_conflict`] call (spec §2.1). Portable data type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// The action was applied for the first time; carries the id of the terminal marker just appended.
    Applied(String),
    /// Idempotent no-op success: the proposal was already resolved by the SAME action, OR a torn-write
    /// roll-forward completed the missing `conflict_resolved`. No fail-loud primitive was (re-)called.
    NoOp,
}

/// Map a `ResolveAction` to its terminal `ResolutionKind` (the two retire actions map to the two retire
/// kinds; KeepBoth/Dismiss map to their own).
fn action_kind(a: ResolveAction) -> ResolutionKind {
    match a {
        ResolveAction::RetireOlder => ResolutionKind::RetireOlder,
        ResolveAction::RetireNewer => ResolutionKind::RetireNewer,
        ResolveAction::KeepBoth => ResolutionKind::KeepBoth,
        ResolveAction::Dismiss => ResolutionKind::Dismiss,
    }
}

/// The `retired_event_id` recorded in `conflict_resolved` (Open-Q7): well-formed from the proposal refs on
/// BOTH the fresh and roll-forward paths. A Note → its event id; a Passage → a stable `session#passage`
/// composite (informational; the digest R-count reads the tagged retire MARKERS, not this field).
fn retired_id_of(loser: &crate::index::ConflictRef) -> String {
    match loser {
        crate::index::ConflictRef::Note { event_id } => event_id.clone(),
        crate::index::ConflictRef::Passage { session_id, passage_id } => {
            format!("{session_id}#{passage_id}")
        }
    }
}

/// What one [`EventLog::detect_conflicts_once`] cycle did (spec §3.3). All-zero + `skipped_disabled`
/// when the flag is off (I3).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConflictDetectReport {
    /// The flag was CLOSED — no scan, no model, no proposals (I3).
    pub skipped_disabled: bool,
    /// New subjects ENUMERATED since the cursor this cycle (`subjects.len()`; may exceed the number
    /// actually examined when the cycle breaks early on budget/reasoner-stop). 0 → dirty-gate
    /// short-circuit, no rebuild.
    pub scanned_subjects: usize,
    /// Judge calls made (bounded by [`crate::conflict::CONFLICT_JUDGE_PER_SWEEP`]).
    pub judged: usize,
    /// Proposals emitted.
    pub proposed: usize,
    /// Pairs not turned into a proposal without a positive verdict: the judge declined (`Ok(None)`,
    /// which DID consume a judge-budget slot), OR a side's text could not be resolved at judge time
    /// (no judge call, no budget consumed).
    pub dropped: usize,
    /// Reasoner transport/decode failures (`Err`) — the cycle stops + retries next time (I6).
    pub reasoner_errors: usize,
    /// The per-cycle judge budget was hit (backlog drips to the next cycle).
    pub budget_hit: bool,
    /// The open-proposal ceiling was hit (stop proposing; surface one quiet count).
    pub ceiling_hit: bool,
    /// Pairs abandoned this run after `CONFLICT_PAIR_ERROR_BUDGET` consecutive reasoner errors (§3.3) — a
    /// bounded dropped counter on ONE pair, never a frozen sweep, never a hidden sibling conflict.
    pub poison_skipped: usize,
}

/// The serialized, signed event log.
pub struct EventLog {
    inner: Mutex<Store>,
    key: SigningKey,
    highwater: Option<Box<dyn HighWaterStore>>,
    /// In-memory ANN index over the active model's vectors. `None` until
    /// [`EventLog::rebuild_indexes`] builds it. Never persisted — rebuilt from
    /// the encrypted log on open (zero plaintext index on disk). Guarded by its
    /// own `Mutex` so a rebuild never blocks log appends. The boxed trait is
    /// `Send + Sync` (the [`VectorIndex`] bound guarantees it), so `EventLog`
    /// stays `Send + Sync` and shareable as `Arc<EventLog>`.
    vector_index: Mutex<Option<Box<dyn VectorIndex>>>,
    /// In-memory ANN index over `entity`-event vectors ONLY, for entity
    /// resolution (spec §6). Physically separate from `vector_index` so recall
    /// can never surface an entity node and resolution can never match a memory.
    /// `None` until [`EventLog::rebuild_entity_index`]; rebuilt from the encrypted
    /// log on open (zero plaintext index on disk, like the recall index).
    entity_index: Mutex<Option<Box<dyn VectorIndex>>>,
    /// In-memory ANN index over captured-session BODY-PASSAGE vectors ONLY, for
    /// cross-session conflict retrieval (Rung-3 §7.1). Physically separate from
    /// both `vector_index` (recall) and `entity_index` (resolution) so building it
    /// never perturbs recall. Keyed by `(session_id, passage_ix)`. `None` until
    /// [`EventLog::rebuild_conflict_index`]; rebuilt from the `session_passage_vectors`
    /// table (zero plaintext index on disk, like the other two).
    conflict_index: Mutex<Option<Box<dyn VectorIndex>>>,
    /// The actuator rename mutex (spec §9). DISTINCT from `inner` (the SQLite
    /// append serializer): the M6a `execute_write` (T4) will hold THIS across its
    /// entire re-canonicalize → re-check → base-guard → temp-write → finalize-rename
    /// window, so a second write cannot interleave its base read against a
    /// half-applied first write. It guards `()` because it serializes a
    /// code-section, not in-memory state. T3 only CREATES it; T4 acquires it.
    // `dead_code`-allowed: first read by `execute_write` (T4); the field +
    // accessor are the forward seam T3 lands so T4 can lock without a
    // visibility change.
    #[allow(dead_code)]
    rename_lock: Mutex<()>,
}

/// Canonicalize a write target that may not exist yet (a Create): canonicalize the
/// PARENT directory (resolving `..`/symlinks ABOVE the target) and re-join the final
/// component verbatim, so the not-yet-created leaf is preserved and a symlinked final
/// component is NOT followed (its identity is checked separately, NOFOLLOW, at write
/// time). Returns `None` if the target has no parent or the parent does not resolve
/// (e.g. a missing intermediate dir). Single-sourced so `propose_write`'s Step-2 Create
/// arm and `add_mandate`'s grant-time canonicalization agree byte-for-byte. NOT
/// `#[cfg(unix)]`: `add_mandate` is portable and calls this on every platform.
fn canonicalize_target_or_parent(target: &std::path::Path) -> Option<std::path::PathBuf> {
    match target.parent() {
        Some(parent) => match std::fs::canonicalize(parent) {
            Ok(real_parent) => target.file_name().map(|name| real_parent.join(name)),
            Err(_) => None,
        },
        None => None,
    }
}

/// Read an OPEN file descriptor to end-of-file via `rustix::io::read`, returning
/// all bytes. Borrows the fd (does not consume it) so the same fd is reused for
/// the `fstat` identity check, this content re-hash, and (for Delete) the audit
/// bytes — all from ONE fd-relative open, never re-resolving the path string
/// (`execute_write`'s base guard, spec §9 step 3). Mirrors the EINTR-retry +
/// short-read loop discipline of `actuator::atomic_write`'s write loop.
#[cfg(unix)]
fn read_fd_to_end(fd: &std::os::fd::OwnedFd) -> Result<Vec<u8>, BossclawError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match rustix::io::read(fd, &mut chunk) {
            Ok(0) => break, // EOF
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            // EINTR → retry; any other errno is fatal.
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(BossclawError::Io(std::io::Error::other(e.to_string()))),
        }
    }
    Ok(buf)
}

/// Pack an optional [`crate::actuator::FileId`] into the three nullable SQLite
/// INTEGER columns (`post_dev`/`post_ino`/`post_size`) the undo store persists.
/// `None` → all-NULL (a Delete-undo, which leaves no post-write file). The `as i64`
/// casts are the lossless round-trip [`unpack_post_identity`] reverses (compared for
/// equality, never arithmetic), so the cast contract lives in exactly these two fns.
#[cfg(unix)]
fn pack_post_identity(
    id: Option<crate::actuator::FileId>,
) -> (Option<i64>, Option<i64>, Option<i64>) {
    id.map_or((None, None, None), |f| {
        (Some(f.dev as i64), Some(f.ino as i64), Some(f.size as i64))
    })
}

/// Inverse of [`pack_post_identity`]: rebuild a [`crate::actuator::FileId`] from the
/// three nullable columns. They are written together (Create/Edit) or all-NULL
/// (Delete), so any-one-`None` ⇒ `None`.
#[cfg(unix)]
fn unpack_post_identity(
    dev: Option<i64>,
    ino: Option<i64>,
    size: Option<i64>,
) -> Option<crate::actuator::FileId> {
    match (dev, ino, size) {
        (Some(d), Some(i), Some(s)) => Some(crate::actuator::FileId {
            dev: d as u64,
            ino: i as u64,
            size: s as u64,
        }),
        _ => None,
    }
}

/// One open (unresolved, non-terminally-rejected) `write_proposal`, projected for callers
/// outside the crate (e.g. the desktop Review queue). Mirrors the per-proposal fields of
/// `append_write_proposal_with` (`content`) plus the lineage off `model_meta`.
/// `#[cfg(unix)]` like the confirm/apply API it feeds (M1).
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingProposal {
    /// The proposal event id (the ULID).
    pub id: String,
    /// Canonical target path (`content["target"]`).
    pub target: String,
    /// `"edit"` / `"create"` / `"delete"` (`content["op"]`; the M6b reconciler emits `"edit"`).
    pub op: String,
    /// Hex sha256 of the proposed bytes (`content["new_content_hash"]`).
    pub new_content_hash: String,
    /// Plain-English "Why" (`content["rationale"]`).
    pub rationale: String,
    /// The resolved contradiction `{src, relation, dst}` (`content["inducing_key"]`).
    pub inducing_key: serde_json::Value,
    /// Lineage event ids (`model_meta.source_event_ids`); empty if absent.
    pub source_event_ids: Vec<String>,
    /// The proposer's producer stamp (`model_meta.model_id`): `"m6b-reconciler"` for an M6b
    /// reconcile proposal, `"m6c-mandate-proposer"` for an M6c mandate proposal; empty when
    /// `model_meta` is absent. The desktop sweep auto-applies iff this is exactly the M6c stamp.
    pub producer: String,
    /// The propose-time verdict summary `{requires_loud_modal, taint, allowed, base_content_hash}`
    /// (`content["verdict_summary"]`).
    pub verdict_summary: serde_json::Value,
    /// Hex sha256 of the target file's bytes AT PROPOSE TIME
    /// (`content["verdict_summary"]["base_content_hash"]`; `None` for a Create). The anti-clobber
    /// fingerprint: apply fails closed if the live file no longer hashes to this.
    pub base_content_hash: Option<String>,
}

#[cfg(unix)]
impl PendingProposal {
    /// Single-sourced fail-loud default for a proposal's loud-modal hint: an absent or garbled
    /// `verdict_summary["requires_loud_modal"]` defaults to `true` (fail-loud). Used by both
    /// `ProposalSummary::from_pending` (Task 6) and `proposal_preview` (Task 7) so they cannot drift (m2).
    pub fn requires_loud_modal(&self) -> bool {
        self.verdict_summary
            .get("requires_loud_modal")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }
}

/// One applied write attributed to a mandate (M6c), projected for the desktop Mandate-activity
/// list. Built by [`EventLog::mandate_writes`] via a join — an applied write is stamped with the
/// actuator producer, not the proposer, so the discriminator is the resolved proposal's producer.
/// `#[cfg(unix)]` like the rest of the mandate/confirm surface.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MandateWriteRecord {
    /// The `file_written` event id (also the handle Undo passes to `undo_write`).
    pub file_written_id: String,
    /// Canonical target path written (`content["target"]`).
    pub target: String,
    /// RFC-3339 time the write was recorded (`ts`).
    pub written_at: String,
    /// True iff a LATER `file_written` carries `undo_of == this.file_written_id`.
    pub undone: bool,
}

impl EventLog {
    /// Open (creating if needed) an event log at `path`, encrypted with `dek`,
    /// signing with `key`.
    pub fn open(path: &Path, dek: &[u8; 32], key: SigningKey) -> Result<Self, BossclawError> {
        let store = Store::open(path, dek)?;
        store.exec(
            "CREATE TABLE IF NOT EXISTS events (
                seq        INTEGER PRIMARY KEY AUTOINCREMENT,
                id         TEXT NOT NULL UNIQUE,
                ts         TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload    TEXT NOT NULL,
                prev_hash  TEXT NOT NULL,
                hash       TEXT NOT NULL UNIQUE
            )",
        )?;
        // Tier-A derived vectors. One row per (event, model); the embedding is
        // little-endian f32 bytes. Keyed on (event_id, model_id) so re-deriving
        // under the same model is an idempotent upsert and different models can
        // coexist for the same event without colliding.
        store.exec(
            "CREATE TABLE IF NOT EXISTS vectors (
                event_id  TEXT NOT NULL,
                model_id  TEXT NOT NULL,
                dim       INTEGER NOT NULL,
                embedding BLOB NOT NULL,
                PRIMARY KEY(event_id, model_id)
            )",
        )?;
        // FTS5 full-text index (contentless — the event log is the content of
        // record). The `fts` virtual table stores only the indexed tokens; the
        // `fts_map` side-table maps FTS rowids back to event_ids because a
        // contentless FTS5 table cannot expose a readable payload column.
        //
        // Both tables live INSIDE the SQLCipher DB, so their on-disk bytes are
        // encrypted alongside every other table. `PRAGMA temp_store = MEMORY`
        // below ensures that FTS5 index-merge temporary files are never written to
        // disk as plaintext — they stay in process memory.
        store.exec(
            "CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(body, content='')",
        )?;
        store.exec(
            "CREATE TABLE IF NOT EXISTS fts_map (
                rowid    INTEGER PRIMARY KEY,
                event_id TEXT NOT NULL UNIQUE
            )",
        )?;
        // Bi-temporal graph projection (Tier-A; spec §5.6). One `edges` row per
        // `link` event (PK = the link's ULID); `invalidate` closes rows by
        // setting valid_to/invalidated_at. `nodes` = distinct endpoints. Both are
        // a deterministic fold over link/invalidate events, rebuilt by
        // `rebuild_graph`. Timestamps are stored normalized (fixed-width UTC) so
        // SQL TEXT comparison equals chronological comparison.
        store.exec(
            "CREATE TABLE IF NOT EXISTS edges (
                edge_id          TEXT PRIMARY KEY,
                src              TEXT NOT NULL,
                relation         TEXT NOT NULL,
                dst              TEXT NOT NULL,
                valid_from       TEXT NOT NULL,
                valid_to         TEXT,
                ingested_at      TEXT NOT NULL,
                invalidated_at   TEXT,
                invalidated_by   TEXT,
                origin           TEXT NOT NULL DEFAULT 'manual',
                confidence_milli INTEGER
            )",
        )?;
        store.exec(
            "CREATE TABLE IF NOT EXISTS nodes (
                node_id TEXT PRIMARY KEY,
                kind    TEXT NOT NULL
            )",
        )?;
        // Entity projection (Tier-A; spec §4). One row per `entity` event,
        // id = "entity:<event ulid>". A deterministic fold over entity events,
        // rebuilt by `rebuild_graph`. The label is a property, never the id.
        store.exec(
            "CREATE TABLE IF NOT EXISTS entities (
                entity_id   TEXT PRIMARY KEY,
                label       TEXT NOT NULL,
                aliases     TEXT NOT NULL,
                entity_type TEXT NOT NULL
            )",
        )?;
        // Page projection (Tier-A; spec §4). At most one CURRENT page per topic;
        // a deterministic fold over `page`/`supersede` events, rebuilt by
        // `rebuild_graph`. `text` is the rendered body (also the embedded text).
        store.exec(
            "CREATE TABLE IF NOT EXISTS pages (
                topic_id      TEXT PRIMARY KEY,
                page_event_id TEXT NOT NULL,
                title         TEXT NOT NULL,
                text          TEXT NOT NULL
            )",
        )?;
        // Folder-grant projection (Tier-A; M5a). One row per granted root; a
        // deterministic fold over `grant`/`revoke` events, rebuilt by `rebuild_graph`.
        // `ingest_all` iterates active (revoked = 0) grants only.
        store.exec(
            "CREATE TABLE IF NOT EXISTS grants (
                canonical_root TEXT PRIMARY KEY,
                granted_at     TEXT NOT NULL,
                revoked        INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        // Folder WRITE-grant projection (Tier-A; M6a). Structurally SEPARATE from
        // `grants` (read) so a read grant can never authorize a write. A deterministic
        // fold over `write_grant`/`write_revoke` events, rebuilt by `rebuild_graph`.
        store.exec(
            "CREATE TABLE IF NOT EXISTS write_grants (
                canonical_root TEXT PRIMARY KEY,
                granted_at     TEXT NOT NULL,
                revoked        INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        // Mandate projection (Tier-A; M6c §4.1). A signed, bounded standing goal:
        // keep `target` == `recipe(sources under source_scope)`. A deterministic fold
        // over `mandate_grant`/`mandate_revoke` events, rebuilt by `rebuild_graph`.
        // The PRIMARY KEY is the `mandate_grant` event id (the mandate's identity), NOT
        // a content path — so the identity is a real, readable ground-truth event id
        // (load-bearing for taint: citing it never trips the fail-closed "unreadable
        // source ⇒ external" rule). `revoked` rows stay in the log forever (sticky).
        store.exec(
            "CREATE TABLE IF NOT EXISTS mandates (
                mandate_grant_id TEXT PRIMARY KEY,
                target           TEXT NOT NULL,
                source_scope     TEXT NOT NULL,
                recipe           TEXT NOT NULL,
                granted_at       TEXT NOT NULL,
                revoked          INTEGER NOT NULL DEFAULT 0
            )",
        )?;
        // Ingested-file projection (Tier-A; M5a). At most one CURRENT file_ingested per
        // canonical_path; a deterministic fold over file_ingested/supersede events,
        // rebuilt by `rebuild_graph`. `content_hash` is the dedup key; `grant_root` lets
        // recall exclude files under a now-revoked grant.
        store.exec(
            "CREATE TABLE IF NOT EXISTS files (
                canonical_path TEXT PRIMARY KEY,
                file_event_id  TEXT NOT NULL,
                content_hash   TEXT NOT NULL,
                grant_root     TEXT NOT NULL
            )",
        )?;
        // N-deep recoverable-undo store (M6a, T5 — spec §7.3). Lives INSIDE the
        // SQLCipher DB, so the captured pre-bytes are encrypted at rest automatically
        // (no plaintext sidecar). NOT a Tier-A fold: it is recovery convenience, never
        // authoritative — tampering can lose undo ability but cannot forge a signed
        // write (writes are signed; `undo_write` re-verifies pre_bytes against the
        // recorded hash before restoring). Therefore `rebuild_graph` does NOT touch it
        // (like `summarize_cursor`/`evolve_cursor`).
        //
        // KEYED BY ITS OWN `undo_id` (a fresh ULID), NOT the `file_written` id: the
        // crash-safe ordering (W8) captures + COMMITS this row BEFORE the FS mutate,
        // but the `file_written` event id is not minted until the post-mutate append
        // (the append chokepoint, `append_event_in_tx`, mints the event id itself —
        // it is NOT weakened to accept a pre-set id). So `file_written_id` starts NULL
        // and is BACKFILLED after the append. `undo_write(file_written_id)` looks the
        // row up by that backfilled column. `pre_bytes` = the bytes to restore (Edit ⇒
        // old content; Delete ⇒ deleted content; Create ⇒ NULL → undo removes the file).
        // `post_dev`/`post_ino`/`post_size` record the identity the write LEFT on
        // disk (Create/Edit), captured after the mutate and backfilled with
        // `file_written_id`. They are NULL for a Delete (no post-write file). `undo_write`
        // re-asserts the CURRENT target still has this identity before restoring — so a
        // foreign-process inode swap BETWEEN the write and the undo is caught
        // (fail-closed), closing the same-name/different-inode divergence at undo time.
        store.exec(
            "CREATE TABLE IF NOT EXISTS undo_state (
                undo_id           TEXT PRIMARY KEY,
                file_written_id   TEXT,
                canonical_target  TEXT NOT NULL,
                op                TEXT NOT NULL,
                pre_bytes         BLOB,
                base_content_hash TEXT NOT NULL,
                post_dev          INTEGER,
                post_ino          INTEGER,
                post_size         INTEGER,
                created_at        TEXT NOT NULL
            )",
        )?;
        // M6b proposal-bytes side table (spec §5.6/§7/Q-3): the corrected whole-file
        // bytes a `write_proposal` proposes are NOT stored in the signed event — they
        // live HERE, keyed by the proposal's event id, with `content_hash` recorded in
        // the signed event. SECURITY INVARIANT: this is an audit/worklist CACHE, never an
        // authorization source — at confirm the bytes are re-hashed against the recorded
        // hash and re-gated through the full M6a path. Lives inside the same SQLCipher
        // `Store` as `undo_state` (encrypted at rest; model output over untrusted input).
        store.exec(
            "CREATE TABLE IF NOT EXISTS proposal_bytes (
                proposal_id  TEXT PRIMARY KEY,
                content      BLOB NOT NULL,
                content_hash TEXT NOT NULL,
                created_at   TEXT NOT NULL
            )",
        )?;
        // M6c synthesis cache (spec §5.2/§8, finding B + F). Caches the expected file
        // bytes synthesized ONCE per source-state so they can be reused across ticks
        // (an LLM is not bit-exact, so re-synthesizing every tick would defeat
        // convergence). Keyed by (mandate, sources_hash). SECURITY (finding B): the row
        // carries `source_event_ids_at_synth` — the EXACT engine-gathered source ids read
        // at synthesis time — ALONGSIDE the bytes, so on a later cache HIT the proposal's
        // taint lineage is `synth ∪ current-in-scope` (taint is monotone) and a tainted
        // source that LEFT scope between synth and the hit can never silently drop out of
        // the lineage. The bytes and the provenance that produced them therefore travel
        // together in the SAME row. NEVER an authorization source — confirm re-gates the
        // bytes through the full M6a path. Lives inside the same SQLCipher `Store` as
        // `proposal_bytes` (encrypted at rest; bytes are derived from possibly-sensitive
        // source files). NOT a Tier-A fold: it is convergence/efficiency cache, never
        // authoritative, so `rebuild_graph` does NOT touch it.
        store.exec(
            "CREATE TABLE IF NOT EXISTS mandate_synthesis_cache (
                mandate_grant_id          TEXT NOT NULL,
                sources_hash              TEXT NOT NULL,
                expected_hash             TEXT NOT NULL,
                expected_bytes            BLOB NOT NULL,
                source_event_ids_at_synth BLOB NOT NULL,
                created_at                TEXT NOT NULL,
                PRIMARY KEY(mandate_grant_id, sources_hash)
            )",
        )?;
        // Summarize progress high-water-mark (spec §6 / F1) — sibling of
        // evolve_cursor. NOT a fold: losing it only re-derives the dirty set
        // (idempotent via the cited-set check). Single row.
        store.exec(
            "CREATE TABLE IF NOT EXISTS summarize_cursor (
                id INTEGER PRIMARY KEY CHECK (id = 0),
                last_seq INTEGER NOT NULL
            )",
        )?;
        // Entity-resolution vectors (Tier-A derived; spec §6). Separate from
        // `vectors` so the resolution index NEVER mixes with the recall index —
        // recall must exclude entity-kind, resolution searches only entity-kind.
        store.exec(
            "CREATE TABLE IF NOT EXISTS entity_vectors (
                entity_id TEXT NOT NULL,
                model_id  TEXT NOT NULL,
                dim       INTEGER NOT NULL,
                embedding BLOB NOT NULL,
                PRIMARY KEY(entity_id, model_id)
            )",
        )?;
        // Session-passage vectors (Rung-3 Phase-1 §7.1; Tier-A derived). The
        // restart-safe SOURCE for the conflict index — the daemon embeds each
        // captured session's body chunks here at capture time. Separate from
        // both `vectors` (recall) and `entity_vectors` (resolution) so the
        // conflict index NEVER mixes with either.
        // model_id-scoped, but the capture-time skip-gate (`session_passages_absent`) is
        // model-AGNOSTIC: after a model/language-pack swap, existing sessions' passages are NOT
        // auto-repopulated (a same-`sha` re-capture finds the OLD model's rows and skips), so they
        // stay under the old model_id until a future re-embed/backfill hook — a deferred Phase-1
        // limitation (that hook is out of scope here).
        store.exec(
            "CREATE TABLE IF NOT EXISTS session_passage_vectors (
                session_captured_event_id TEXT NOT NULL,
                passage_ix                INTEGER NOT NULL,
                model_id                  TEXT NOT NULL,
                dim                       INTEGER NOT NULL,
                embedding                 BLOB NOT NULL,
                PRIMARY KEY(session_captured_event_id, passage_ix, model_id)
            )",
        )?;
        // Evolve-loop progress (re-derivable progress state — NOT a Tier-A fold,
        // spec §4). Single row (id pinned to 0), advanced after each committed
        // batch. Losing it only re-processes events (idempotent: an active
        // edge-key is skipped and a resolved entity is reused), never corrupts.
        store.exec(
            "CREATE TABLE IF NOT EXISTS evolve_cursor (
                id       INTEGER PRIMARY KEY CHECK (id = 0),
                last_seq INTEGER NOT NULL
            )",
        )?;
        // Conflict-detection progress (Rung-3 Phase-2 §3.2 — re-derivable progress state, NOT a
        // Tier-A fold). Single row (id pinned to 0). `(last_seq, subject_offset)` advances
        // subject-by-subject: all subjects of the event at `last_seq` with within-seq id
        // < subject_offset are judged. Losing it only re-searches (idempotent: an already-open
        // pair is never re-proposed).
        store.exec(
            "CREATE TABLE IF NOT EXISTS conflict_cursor (
                id             INTEGER PRIMARY KEY CHECK (id = 0),
                last_seq       INTEGER NOT NULL,
                subject_offset INTEGER NOT NULL
            )",
        )?;
        // Rung-3 Phase-3 (§3.3): per-pair CONSECUTIVE reasoner-error counter. Re-derivable progress state
        // (NOT a Tier-A fold): losing it only re-tries a poison pair. Keyed by `unordered_pair_key`.
        store.exec(
            "CREATE TABLE IF NOT EXISTS conflict_pair_errors (
                pair_key           TEXT PRIMARY KEY,
                consecutive_errors INTEGER NOT NULL
            )",
        )?;
        // Route FTS5 merge temporaries to memory, preventing any plaintext index
        // spill to the filesystem. This is a connection-level setting; it must be
        // re-applied on every open.
        store.exec("PRAGMA temp_store = MEMORY")?;
        // Verify the pragma actually took effect. SQLCipher builds compiled
        // with certain options can silently ignore temp_store; if that happened
        // FTS5 index-merge files would be written as plaintext to the OS temp
        // directory and the no-plaintext-on-disk guarantee would be void. We
        // surface the failure loudly at open rather than letting it slip past
        // the security test's dir-scan (which only covers the DB directory).
        let temp_store_val: i64 = store
            .conn()
            .query_row("PRAGMA temp_store", [], |r| r.get(0))?;
        if temp_store_val != 2 {
            return Err(BossclawError::Store(format!(
                "PRAGMA temp_store = MEMORY did not take effect (got {temp_store_val}, want 2); \
                 FTS5 index-merge files would spill to the OS temp dir as plaintext"
            )));
        }
        Ok(Self {
            inner: Mutex::new(store),
            key,
            highwater: None,
            vector_index: Mutex::new(None),
            entity_index: Mutex::new(None),
            conflict_index: Mutex::new(None),
            rename_lock: Mutex::new(()),
        })
    }

    /// The actuator rename mutex (spec §9). `execute_write` (T4) acquires this to
    /// serialize its TOCTOU-critical re-check → write → finalize window across
    /// threads. Exposed `pub(crate)` so the in-crate actuator engine can lock it
    /// without widening the field's visibility.
    #[cfg(unix)]
    #[allow(dead_code)]
    pub(crate) fn rename_lock(&self) -> &Mutex<()> {
        &self.rename_lock
    }

    /// Append an event. `id`, `ts`, `prev_hash`, `hash`, `signature` are assigned
    /// here; the caller supplies `event_type`, `content`, `model_meta`,
    /// `signed_by_did`, optional `valid_time`.
    pub fn append(&self, event: Event) -> Result<String, BossclawError> {
        Self::reject_empty_tier_b(&event)?;
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let tx = conn.unchecked_transaction()?;
        let id = self.append_event_in_tx(&tx, event)?;
        tx.commit()?;
        Ok(id)
    }

    /// Atomically append `first` then `second` in ONE transaction (spec §3.7 /
    /// F5). Used to emit `supersede`+`page` together so there is never a durable
    /// orphan supersede (both commit or neither). `second` chains onto `first`
    /// because the chain-tip read is SQL inside the shared tx, so it sees the
    /// uncommitted `first`. Returns `(first_id, second_id)`.
    pub fn append_pair(&self, first: Event, second: Event) -> Result<(String, String), BossclawError> {
        Self::reject_empty_tier_b(&first)?;
        Self::reject_empty_tier_b(&second)?;
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let tx = conn.unchecked_transaction()?;
        let id1 = self.append_event_in_tx(&tx, first)?;
        let id2 = self.append_event_in_tx(&tx, second)?;
        tx.commit()?;
        Ok((id1, id2))
    }

    /// The Tier-B non-empty-`source_event_ids` guard (a `model_meta: Some` event
    /// must carry real lineage). Factored so both `append` and `append_pair`
    /// enforce it before opening a transaction.
    fn reject_empty_tier_b(event: &Event) -> Result<(), BossclawError> {
        if let Some(meta) = &event.model_meta {
            if meta.source_event_ids.is_empty() {
                return Err(BossclawError::Chain(
                    "Tier-B event requires non-empty source_event_ids".into(),
                ));
            }
        }
        Ok(())
    }

    /// True iff the event `id` (read within `tx`) carries the external taint stamp
    /// (read via [`crate::ingest::is_external`], single-sourced). The append
    /// chokepoint uses this to propagate taint to Tier-B descendants. Fail-closed: a
    /// source that cannot be read or parsed is treated as external (spec §7).
    fn source_is_external_in_tx(tx: &rusqlite::Transaction<'_>, id: &str) -> bool {
        let payload: Option<String> = match tx
            .query_row("SELECT payload FROM events WHERE id = ?1", rusqlite::params![id], |r| r.get(0))
            .optional()
        {
            Ok(p) => p,
            Err(e) => {
                log::warn!("taint-check: source {id} read failed, treating as external (fail-closed): {e}");
                None
            }
        };
        match payload.map(|p| serde_json::from_str::<crate::event::Event>(&p)) {
            Some(Ok(ev)) => crate::ingest::is_external(&ev),
            _ => true, // fail-closed: missing or unparseable source
        }
    }

    /// Assign id/ts/prev_hash, hash, sign, and INSERT `event` within `tx`. The
    /// chain tip is read via SQL inside `tx`, so consecutive calls in one tx chain
    /// correctly (the second sees the first's uncommitted insert). Returns the id.
    fn append_event_in_tx(
        &self,
        tx: &rusqlite::Transaction<'_>,
        mut event: Event,
    ) -> Result<String, BossclawError> {
        // Eager external-taint propagation (extraction-from-files D2): a Tier-B event
        // whose lineage touches ANY external source inherits the taint, stamped into the
        // signed content BEFORE hashing. is_external stays O(1) + transitive (a tainted
        // derived fact is itself stamped, so its descendants inherit). append_event_in_tx
        // is the SOLE INSERT path → no Tier-B event can bypass it.
        let tainted = event.model_meta.as_ref().is_some_and(|m| {
            m.source_event_ids.iter().any(|src| Self::source_is_external_in_tx(tx, src))
        });
        if tainted {
            if let Some(obj) = event.content.as_object_mut() {
                obj.insert("origin".to_string(),
                    serde_json::Value::String(crate::graph::EXTERNAL_ORIGIN.to_string()));
            }
        }
        let prev_hash: String = tx
            .query_row("SELECT hash FROM events ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
            .unwrap_or_else(|_| GENESIS.to_string());
        event.id = Ulid::new().to_string();
        event.ts = Utc::now().to_rfc3339();
        event.prev_hash = prev_hash;
        event.hash = None;
        event.signature = None;
        let hash = compute_hash(&event)?;
        let hash_hex = hex::encode(hash);
        let sig = sign_hash(&hash, &self.key);
        event.hash = Some(hash_hex.clone());
        event.signature = Some(sig);
        let payload = serde_json::to_string(&event)?;
        tx.execute(
            "INSERT INTO events (id, ts, event_type, payload, prev_hash, hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![event.id, event.ts, event.event_type, payload, event.prev_hash, hash_hex],
        )?;
        Ok(event.id)
    }

    /// Read a full `Event` by id (None if absent). Public read for tests + M6's walk.
    pub fn event_by_id(&self, id: &str) -> Result<Option<crate::event::Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let payload: Option<String> = store
            .conn()
            .query_row("SELECT payload FROM events WHERE id = ?1", rusqlite::params![id], |r| r.get(0))
            .optional()?;
        Ok(payload.map(|p| serde_json::from_str(&p)).transpose()?)
    }

    /// The append `seq` of the event with `event_id`, or `None` if no such event. A thin indexed lookup
    /// (`events.id` is unique); used by the Rung-3 conflict-cursor rewind (§3.2) to map an un-retired
    /// memory's event id back to its cursor coordinate.
    pub fn seq_of_event(&self, event_id: &str) -> Result<Option<i64>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        Ok(store
            .conn()
            .query_row("SELECT seq FROM events WHERE id = ?1", [event_id], |r| r.get::<_, i64>(0))
            .optional()?)
    }

    /// Number of events in the log.
    pub fn count(&self) -> Result<i64, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let n = store.conn().query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
        Ok(n)
    }

    /// Re-verify the whole chain: every row's hash recomputes from its canonical
    /// bytes + prev_hash, links to the prior row, and its signature verifies.
    pub fn verify_chain(&self) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt =
            conn.prepare("SELECT payload, prev_hash, hash FROM events ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        Self::verify_rows(rows, GENESIS.to_string(), &self.key)
    }

    /// Verify only the tail of the chain after a trusted cursor event.
    ///
    /// The cursor event and all events before it are trusted without re-checking.
    /// Only the events whose `seq` is greater than the cursor's `seq` are
    /// verified (hash recomputation, chain link, and signature).
    ///
    /// # Arguments
    /// * `from_event_id` — `None` verifies the whole chain (identical to
    ///   [`verify_chain`]). `Some(id)` verifies only the tail after the trusted
    ///   cursor event identified by `id`.
    ///
    /// # Errors
    /// * [`BossclawError::Chain`] if `from_event_id` is `Some` and the cursor
    ///   event is not found in the log.
    /// * [`BossclawError::Chain`] if any post-cursor row fails the link check,
    ///   hash recomputation, or signature verification.
    pub fn verify_chain_since(
        &self,
        from_event_id: Option<&str>,
    ) -> Result<(), BossclawError> {
        let cursor_id = match from_event_id {
            None => return self.verify_chain(),
            Some(id) => id,
        };

        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();

        // Look up the cursor row; its hash is the trusted starting point.
        let result = conn
            .query_row(
                "SELECT seq, hash FROM events WHERE id = ?1",
                rusqlite::params![cursor_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;

        let (cursor_seq, cursor_hash) = result.ok_or_else(|| {
            BossclawError::Chain(format!(
                "verify_chain_since: cursor event {cursor_id} not found"
            ))
        })?;

        // Scan only events strictly after the trusted cursor.
        let mut stmt = conn.prepare(
            "SELECT payload, prev_hash, hash FROM events WHERE seq > ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![cursor_seq], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;

        Self::verify_rows(rows, cursor_hash, &self.key)
    }

    /// Shared per-row verification loop used by both [`verify_chain`] and
    /// [`verify_chain_since`].
    ///
    /// For each row (in the order produced by `rows`), this function:
    /// 1. Checks that `prev_hash` equals `expected_prev` (chain link).
    /// 2. Deserialises the payload into an [`Event`].
    /// 3. Recomputes the canonical hash and compares it with the stored value.
    /// 4. Verifies the Ed25519 signature over the hash bytes.
    /// 5. Advances `expected_prev` to the current row's hash.
    ///
    /// Returns `Ok(())` when every row passes; propagates the first failure as
    /// [`BossclawError::Chain`].
    fn verify_rows(
        rows: impl Iterator<Item = Result<(String, String, String), rusqlite::Error>>,
        mut expected_prev: String,
        key: &SigningKey,
    ) -> Result<(), BossclawError> {
        for row in rows {
            let (payload, prev_hash, hash_hex) = row?;
            if prev_hash != expected_prev {
                return Err(BossclawError::Chain(format!(
                    "broken link: expected prev {expected_prev}, got {prev_hash}"
                )));
            }
            let event: Event = serde_json::from_str(&payload)?;
            let hash_bytes = compute_hash(&event)?;
            let recomputed = hex::encode(hash_bytes);
            if recomputed != hash_hex {
                return Err(BossclawError::Chain(format!(
                    "hash mismatch at {}: stored {hash_hex}, recomputed {recomputed}",
                    event.id
                )));
            }
            let sig = event
                .signature
                .as_deref()
                .ok_or_else(|| BossclawError::Chain("missing signature".into()))?;
            verify_hash(&hash_bytes, sig, &key.verifying_key())?;
            expected_prev = hash_hex;
        }
        Ok(())
    }

    /// Open an event log and immediately build the in-memory recall indexes.
    ///
    /// Convenience constructor that calls [`EventLog::open`] then
    /// [`EventLog::rebuild_indexes`] in one step, so the returned `EventLog` is
    /// **recall-ready**: [`EventLog::recall`] and [`EventLog::vector_search`]
    /// work without a separate `rebuild_indexes` call.
    ///
    /// # Lifecycle
    ///
    /// The in-memory vector index reflects the state of the `vectors` table at
    /// the moment [`EventLog::rebuild_indexes`] last ran (either here during
    /// open, or in a later explicit call). **After appending new events and
    /// deriving their vectors, call `rebuild_indexes(embedder)` again to make
    /// those events recallable via the semantic (vector) arm.** Until then,
    /// [`EventLog::recall`] degrades gracefully to keyword-only for the new
    /// events — this is the spec §10 intentional behaviour, but it must be
    /// explicit rather than a silent surprise.
    ///
    /// An incremental single-event `index_event` path (so appends don't require
    /// a full rebuild) is deferred to M7: the desktop decides the
    /// rebuild-vs-incremental policy once startup cost is profiled at scale.
    ///
    /// # Errors
    ///
    /// Propagates any error from [`EventLog::open`] (wrong key, I/O) or from
    /// [`EventLog::rebuild_indexes`] (embed failure, SQL error).
    ///
    /// # Note on highwater
    ///
    /// If you need both recall-ready open and truncation detection, open with
    /// [`EventLog::open_with_highwater`] then call `rebuild_indexes(embedder)`
    /// separately. A combined constructor is deferred to M7.
    pub fn open_with_recall(
        path: &Path,
        dek: &[u8; 32],
        key: SigningKey,
        embedder: &dyn Embedder,
    ) -> Result<Self, BossclawError> {
        let log = Self::open(path, dek, key)?;
        log.rebuild_indexes(embedder)?;
        log.rebuild_graph()?; // graph (+ its recall boost) live on open; persisted edges survive reopen
        Ok(log)
    }

    /// Open with a high-water store; checks truncation immediately.
    pub fn open_with_highwater(
        path: &Path,
        dek: &[u8; 32],
        key: SigningKey,
        highwater: Box<dyn HighWaterStore>,
    ) -> Result<Self, BossclawError> {
        let mut log = Self::open(path, dek, key)?;
        if let Some(mark) = highwater.load()? {
            let live = log.count()?;
            if live < mark.count {
                return Err(BossclawError::Truncation(format!(
                    "live count {live} < high-water {} (tail deleted)",
                    mark.count
                )));
            }
        }
        log.highwater = Some(highwater);
        Ok(log)
    }

    /// Return the active embedding model configuration, parsed from the latest
    /// `config` event that CARRIES the model fields.
    ///
    /// A model-config event has `event_type = "config"` and a `content` object
    /// with `active_model_id`, `dim`, and `schema_version`. Config events are
    /// scanned newest-first and the first one that successfully parses as an
    /// [`ActiveModel`] wins; configs that carry only other control keys (e.g. a
    /// control `config` setting just `evolve_enabled`, Rev 2 F2-sec(c)) are
    /// SKIPPED rather than erroring — the on/off switch and the active model are
    /// independent control keys that may be set in separate config events.
    ///
    /// Returns `Ok(None)` if no `config` event carries the model fields.
    pub fn active_model(&self) -> Result<Option<ActiveModel>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let event: Event = serde_json::from_str(&row?)?;
            // Tolerant: a config lacking the model fields is a different control
            // config (e.g. evolve_enabled-only). Skip it, do not error.
            if let Ok(model) = serde_json::from_value::<ActiveModel>(event.content) {
                return Ok(Some(model));
            }
        }
        Ok(None)
    }

    /// Return every event in chain order (M1: full scan; M2 adds `since`).
    pub fn stream_all(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare("SELECT payload FROM events ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Derive and store the Tier-A vector for a single event, if embeddable.
    ///
    /// If [`embeddable_text`] yields `Some(text)`, the text is embedded as a
    /// one-item batch and upserted into the `vectors` table under
    /// `(event.id, embedder.model_id())` (INSERT OR REPLACE), returning
    /// `Ok(true)`. Non-embeddable events store nothing and return `Ok(false)`.
    /// Embedder failures propagate as `Err`.
    ///
    /// Production calls this AFTER [`EventLog::append`] has committed and MAY
    /// ignore the returned `Err`: vector derivation is best-effort (spec §10),
    /// and a missing vector is repaired later by
    /// [`EventLog::rederive_pending`]. The append itself is never blocked on
    /// embedding success.
    pub fn derive_vector(
        &self,
        embedder: &dyn Embedder,
        event: &Event,
    ) -> Result<bool, BossclawError> {
        let text = match embeddable_text(event) {
            Some(t) => t,
            None => return Ok(false),
        };
        let embedding = embed_one(embedder, &text)?;
        let blob = vec_to_blob(&embedding);
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT OR REPLACE INTO vectors (event_id, model_id, dim, embedding)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![event.id, embedder.model_id(), embedder.dim() as i64, blob],
        )?;
        Ok(true)
    }

    /// Derive + upsert one pending event's vector under `embedder.model_id()`. Returns `Ok(true)`
    /// if a vector was written, `Ok(false)` if the event has no embeddable text (legitimately
    /// vector-less), or `Err` if the embedder failed. Shared by [`EventLog::rederive_pending`]
    /// (which swallows the `Err` — best-effort) and [`EventLog::reembed_prepare`] (which tolerates
    /// it here and catches the resulting shortfall with a strict completeness scan).
    fn embed_and_upsert(&self, embedder: &dyn Embedder, event: &Event) -> Result<bool, BossclawError> {
        let text = match embeddable_text(event) {
            Some(t) => t,
            None => return Ok(false),
        };
        let embedding = embed_one(embedder, &text)?;
        let blob = vec_to_blob(&embedding);
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT OR REPLACE INTO vectors (event_id, model_id, dim, embedding)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![event.id, embedder.model_id(), embedder.dim() as i64, blob],
        )?;
        Ok(true)
    }

    /// Backfill every embeddable event that has no vector for this model.
    ///
    /// This is both the initial backfill and the spec §10 retry hook: it finds
    /// events of an embeddable type that lack a `vectors` row for
    /// `embedder.model_id()` (in `seq` order) and derives them. The store
    /// `Mutex` is held only to collect the pending rows and (separately) to
    /// upsert each result — never across [`Embedder::embed`], so the single
    /// store mutex cannot deadlock against the embedder.
    ///
    /// BEST-EFFORT: an individual embed failure is logged via [`log::warn!`]
    /// and skipped; the backfill continues. Returns the number of vectors
    /// successfully derived.
    pub fn rederive_pending(&self, embedder: &dyn Embedder) -> Result<usize, BossclawError> {
        let pending = self.collect_pending(embedder.model_id())?;
        let mut derived = 0usize;
        for event in pending {
            match self.embed_and_upsert(embedder, &event) {
                Ok(true) => derived += 1,
                Ok(false) => log::warn!(
                    "rederive_pending: event {} (type={}) has no embeddable text; skipping (malformed content)",
                    event.id, event.event_type,
                ),
                Err(e) => log::warn!("rederive_pending: skipping event {} (embed failed): {e}", event.id),
            }
        }
        Ok(derived)
    }

    /// All stored vectors for `model_id`, as `(event_id, vector)` pairs ordered
    /// by `event_id ASC`.
    ///
    /// This is the active-model-filtered read: only vectors derived under the
    /// given `model_id` are returned, so cross-model comparison is impossible by
    /// construction. The `event_id ASC` ordering is mandatory — the T5
    /// deterministic index rebuild depends on a stable row order.
    pub fn vectors_for_model(
        &self,
        model_id: &str,
    ) -> Result<Vec<(String, Vec<f32>)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT event_id, embedding FROM vectors WHERE model_id = ?1 ORDER BY event_id ASC",
        )?;
        let rows = stmt.query_map([model_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (event_id, blob) = row?;
            out.push((event_id, blob_to_vec(&blob)?));
        }
        Ok(out)
    }

    /// Rebuild the in-memory vector index AND the FTS5 keyword index from the
    /// encrypted log for the active embedding model.
    ///
    /// **Vector rebuild:** reads every persisted vector for `embedder.model_id()`
    /// (via [`EventLog::vectors_for_model`], which returns rows `ORDER BY
    /// event_id ASC`), builds a fresh [`HnswIndex`] sized to the exact row
    /// count, and **serially** adds each `(event_id, vector)`.  Serial
    /// insertion over a deterministic row order is what makes the index
    /// reproducible across re-opens (spec F2).  The finished index replaces any
    /// previous one.  Because only `model_id`-matching rows are read, the index
    /// can only ever contain active-model vectors — cross-model bleed is
    /// impossible by construction (spec C4).
    ///
    /// **FTS rebuild:** wipes `fts` and `fts_map` entirely, then re-populates
    /// from every `memory`/`page` event (the same embeddable types that feed the
    /// vector index), scanned `ORDER BY seq ASC`.  No embedder is needed for
    /// this half — FTS indexes the raw event text.
    ///
    /// Both rebuilds are idempotent: calling this method twice leaves the indexes
    /// in the same state as calling it once.
    ///
    /// Emits [`log::info!`] timing lines so rebuild cost is visible before the
    /// recall benchmark (T9).
    pub fn rebuild_indexes(&self, embedder: &dyn Embedder) -> Result<(), BossclawError> {
        // ── Vector index rebuild ──────────────────────────────────────────────
        let vec_started = Instant::now();
        let rows = self.vectors_for_model(embedder.model_id())?;
        let vec_count = rows.len();
        let mut index = HnswIndex::with_capacity(vec_count);
        for (event_id, vec) in rows {
            index.add(&event_id, &vec);
        }
        let boxed: Box<dyn VectorIndex> = Box::new(index);
        *self.vector_index.lock().expect(POISON) = Some(boxed);
        log::info!(
            "rebuilt vector index: {vec_count} vectors in {}ms",
            vec_started.elapsed().as_millis()
        );

        // ── FTS5 keyword index rebuild ────────────────────────────────────────
        let fts_started = Instant::now();
        // Collect the events to index before taking the store lock (same
        // pattern as collect_pending — never hold the lock across I/O or
        // expensive work).
        let events_to_index = self.collect_embeddable_events_ordered()?;
        let fts_count = events_to_index.len();

        {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            // Wipe the existing FTS index so this call is fully idempotent.
            // A contentless FTS5 table does not support plain `DELETE FROM
            // fts`; the FTS5 `delete-all` auxiliary command is the correct
            // API for clearing all indexed content.
            conn.execute_batch(
                "INSERT INTO fts(fts) VALUES('delete-all'); DELETE FROM fts_map;",
            )?;
        }

        // Re-populate row by row. Each keyword_add call takes and releases the
        // lock internally, keeping the lock-hold time minimal.
        for (event_id, text) in events_to_index {
            self.keyword_add(&event_id, &text)?;
        }

        log::info!(
            "rebuilt fts index: {fts_count} entries in {}ms",
            fts_started.elapsed().as_millis()
        );
        Ok(())
    }

    /// Search the in-memory vector index for the `k` nearest `(event_id,
    /// distance)` pairs to `query_vec`, ascending by distance.
    ///
    /// Returns [`BossclawError::InvalidInput`] if the index has not been built
    /// yet (no [`EventLog::rebuild_indexes`] call since open) — recall cannot run
    /// against a missing index. Tombstoned ids are excluded by the index itself.
    ///
    /// T7's `recall()` will embed the query text and then call this.
    pub fn vector_search(
        &self,
        query_vec: &[f32],
        k: usize,
    ) -> Result<Vec<(String, f32)>, BossclawError> {
        let guard = self.vector_index.lock().expect(POISON);
        match guard.as_ref() {
            Some(index) => Ok(index.search(query_vec, k)),
            None => Err(BossclawError::InvalidInput(
                "vector index not built — call rebuild_indexes".into(),
            )),
        }
    }

    /// The number of vectors in the recall index, or `0` if it has not been built
    /// yet (no [`EventLog::rebuild_indexes`] call since open).
    ///
    /// Task 6 asserts recall stays byte-untouched by comparing this count before
    /// and after building the separate conflict index — the recall index must
    /// never change.
    // consumed by Task 6 (recall-untouched assertion); no non-test reader yet.
    #[allow(dead_code)]
    pub(crate) fn vector_index_len(&self) -> usize {
        let guard = self.vector_index.lock().expect(POISON);
        guard.as_ref().map_or(0, |ix| ix.len())
    }

    /// Index an event in the FTS5 keyword index.
    ///
    /// The `event_id` / `text` pair is inserted into the `fts` virtual table
    /// (body column) and the corresponding rowid is recorded in `fts_map` so
    /// that keyword searches can return `event_id` values.
    ///
    /// **Idempotent by event_id:** if `event_id` already has a row in
    /// `fts_map` this method returns `Ok(())` immediately — no duplicate FTS
    /// entry is created.  Both the `fts` insert and the `fts_map` insert are
    /// performed in a single transaction to keep them consistent.
    pub fn keyword_add(&self, event_id: &str, text: &str) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();

        // Open the transaction first so the dedup check AND both inserts are
        // one atomic unit. The process-wide Mutex serializes all callers, so
        // the rowid captured immediately after the fts insert is unambiguous —
        // no other writer can have interleaved between the two statements.
        let tx = conn.unchecked_transaction()?;

        // Dedup check inside the transaction — eliminates the TOCTOU window
        // that would exist between a pre-tx read and the subsequent writes.
        let exists = tx
            .query_row(
                "SELECT 1 FROM fts_map WHERE event_id = ?1",
                rusqlite::params![event_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            // tx drops here, rolling back (nothing was written).
            return Ok(());
        }

        tx.execute("INSERT INTO fts(body) VALUES (?1)", rusqlite::params![text])?;
        // last_insert_rowid is read from the same transaction object immediately
        // after the fts insert; Transaction derefs to Connection so the call is
        // identical in shape to conn.last_insert_rowid().
        let rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO fts_map(rowid, event_id) VALUES (?1, ?2)",
            rusqlite::params![rowid, event_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Search the FTS5 keyword index for events whose body matches `query`.
    ///
    /// The raw query string is escaped via [`keyword::escape_fts_query`] before
    /// being passed to FTS5's `MATCH` operator, so user-supplied strings
    /// containing FTS5 operators or unbalanced quotes cannot alter query
    /// semantics or cause a parse error.
    ///
    /// Returns up to `k` `(event_id, score)` pairs ordered by BM25 rank
    /// (lower BM25 score = more relevant; T7's RRF fusion will normalise by
    /// rank position rather than raw score).
    ///
    /// An empty or whitespace-only `query` returns `Ok(vec![])` immediately —
    /// passing an empty string to FTS5 `MATCH` is a parse error, so we guard
    /// against it here.
    pub fn keyword_search(
        &self,
        query: &str,
        k: usize,
    ) -> Result<Vec<(String, f32)>, BossclawError> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let escaped = keyword::escape_fts_query(query);
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT m.event_id, bm25(fts) AS score
             FROM fts
             JOIN fts_map m ON m.rowid = fts.rowid
             WHERE fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![escaped, k as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)? as f32))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Hybrid recall: embed the query, run BOTH retrieval arms, fuse by
    /// reciprocal rank, apply recency + pin boosts, rerank, and return the top-`k`
    /// [`Hit`]s with provenance. This is the heart of M2 (spec §5.7).
    ///
    /// # Pipeline
    /// 1. **Embed** `query` (one-item batch) → query vector.
    /// 2. **Two arms**, each fetching [`FUSION_FETCH`] candidates (≥ `k`, so
    ///    fusion sees enough tail to reorder): the vector arm
    ///    ([`EventLog::vector_search`]) and the keyword arm
    ///    ([`EventLog::keyword_search`]).
    /// 3. **Fuse** both arms (tie-aware RRF) → base score per id, while recording
    ///    which arm(s) surfaced each id (its [`RecallSource`]s).
    /// 4. **Boost** multiplicatively using **f64** throughout to avoid f32
    ///    precision underflow: recency `*= 1 + RECENCY_WEIGHT * exp(-age/HALF_LIFE)`
    ///    (age = now − the event's `ts`), and pin `*= PIN_MULTIPLIER` for ids in
    ///    `opts.pinned`. The recency tilt narrows but does not necessarily close
    ///    every adjacent-rank gap; it reorders candidates with equal or near-equal
    ///    fused base scores.
    /// 5. **Rerank** through the [`Reranker`] seam (v1: [`NoopReranker`]).
    /// 6. **Sort** by final score DESC, with **`ts` DESC** as the explicit
    ///    recency tie-break and **`event_id` DESC** as the final deterministic
    ///    backstop, then return the top `k`.
    ///
    /// ## Why the explicit `ts`-DESC tie-break is required
    ///
    /// The recency multiplier `1 + RECENCY_WEIGHT * exp(-age/HALF_LIFE_SECS)` is
    /// computed in f64 and then stored in `Hit.score` as f32. For events that are
    /// only milliseconds apart (common in tests), the f64 delta is on the order of
    /// 1e-11, which underflows to exactly `0.0` when cast to f32 — leaving two
    /// candidates with bit-identical f32 scores. A sort that breaks those ties by
    /// HashMap iteration order (random per process via hashbrown's random seed)
    /// would be non-deterministic ~30 % of runs. The `ts`-DESC comparator makes
    /// "newer wins ties" a hard guarantee independent of float precision.
    ///
    /// # Graceful degradation (spec §10)
    /// Recall is robust to a missing or unbuilt index. Arm resolution is handled
    /// by [`resolve_arms`]: a failing vector arm (embed error OR index not built —
    /// [`BossclawError::InvalidInput`]) is logged and recall degrades to
    /// **keyword-only**; a failing keyword arm degrades to **vector-only**; only
    /// when both fail is `Err` returned.
    ///
    /// # Lifecycle note
    /// The semantic (vector) arm reflects the index state at the last
    /// [`EventLog::rebuild_indexes`] / [`EventLog::open_with_recall`] call.
    /// **Events appended after that call are not yet in the vector index** and
    /// will only surface via the keyword arm until `rebuild_indexes(embedder)`
    /// is called again. This is intentional spec §10 graceful degradation, not a
    /// bug — but callers should be aware of the gap.
    pub fn recall(
        &self,
        embedder: &dyn Embedder,
        query: &str,
        k: usize,
        opts: &RecallOptions,
    ) -> Result<Vec<Hit>, BossclawError> {
        // ── Run both arms, applying spec §10 graceful degradation. ──
        let vector_result = embed_one(embedder, query)
            .and_then(|qv| self.vector_search(&qv, FUSION_FETCH));
        let keyword_result = self.keyword_search(query, FUSION_FETCH);
        let (vector_arm, keyword_arm) = resolve_arms(vector_result, keyword_result)?;

        // ── Provenance: which arm(s) surfaced each id (vector before keyword for
        //    a stable evidence order). Membership sets keep this O(1) per id. ──
        let vector_set: std::collections::HashSet<&String> =
            vector_arm.iter().map(|(id, _)| id).collect();
        let keyword_set: std::collections::HashSet<&String> =
            keyword_arm.iter().map(|(id, _)| id).collect();

        // ── Fuse both arms → base RRF score (f32) per id. Tie-aware: candidates
        //    with an identical arm score share a rank, so identical-text events get
        //    an EQUAL base, making the ts-DESC comparator below the deterministic
        //    tie-break (both arms rank lower scores first → lower_is_better=true). ──
        let fused = fuse_scored_arms(&[
            (vector_arm.as_slice(), true),
            (keyword_arm.as_slice(), true),
        ]);

        // ── Recency boost needs each candidate's ts; fetch them in one query. ──
        let candidate_ids: Vec<String> = fused.keys().cloned().collect();
        let timestamps = self.candidate_timestamps(&candidate_ids)?;
        // Per-candidate event_type (F2): needed to set Hit.kind AND to filter
        // pages. Same single-lock id-IN pattern as candidate_timestamps.
        let kinds = self.candidate_event_types(&candidate_ids)?;
        // Current page ids (for the superseded-page exclusion). Gated behind a
        // page-candidate check: the `pages` projection SELECT is skipped entirely
        // on the common case where no page is in the fusion candidate set (Fix A).
        let current_page_ids: std::collections::HashSet<String> =
            if kinds.values().any(|k| k == crate::graph::PAGE_EVENT_TYPE) {
                self.current_pages()?.into_iter().map(|p| p.page_event_id).collect()
            } else {
                std::collections::HashSet::new()
            };
        // Current file ids whose grant is still active (for the file-version +
        // revoked-grant exclusion). Gated: skipped entirely unless a file is in the
        // fusion candidate set (mirrors the page gate).
        let current_file_ids: std::collections::HashSet<String> =
            if kinds.values().any(|k| k == crate::graph::FILE_INGESTED_EVENT_TYPE) {
                self.current_files_active()?
            } else {
                std::collections::HashSet::new()
            };
        // Session/note exclusion sets (A3). Both come from ONE fold over the
        // session/supersede event stream: `fold_sessions` already scans every
        // `supersede` event (they are shared across the page/file/session/note
        // folds), so its `superseded` set is the COMPLETE retired-id universe —
        // deriving both here costs a single scan, versus a separate
        // `superseded_event_ids()` query plus a `current_sessions` fold. Gated
        // like the page/file sets above: a recall with no session- or memory-kind
        // candidate (e.g. pages only) pays nothing. `current_session_event_ids`
        // is the INCLUSION set (only the current fold head survives);
        // `superseded_ids` is an EXCLUSION set (see the memory arm below).
        let (current_session_event_ids, superseded_ids, retired_note_ids): (
            std::collections::HashSet<String>,
            std::collections::HashSet<String>,
            std::collections::HashSet<String>,
        ) = if kinds.values().any(|k| {
            k == crate::graph::SESSION_CAPTURED_EVENT_TYPE || k == crate::graph::MEMORY_EVENT_TYPE
        }) {
            let fold = fold_sessions(&self.session_events_ordered()?);
            (
                fold.current.into_iter().map(|cs| cs.event_id).collect(),
                fold.superseded,
                fold.retired_notes,
            )
        } else {
            (
                std::collections::HashSet::new(),
                std::collections::HashSet::new(),
                std::collections::HashSet::new(),
            )
        };
        let now = Utc::now();
        let pinned: std::collections::HashSet<&String> = opts.pinned.iter().collect();

        // ── Graph-proximity seeds: explicit, else auto-seed from the top fused
        //    base score(s). Then BFS current-edge neighbors (best-effort: a graph
        //    error degrades to no boost, never failing recall — spec §6/§10). ──
        let seeds: Vec<String> = if !opts.graph_seeds.is_empty() {
            opts.graph_seeds.clone()
        } else {
            // Intra-result reinforcement (spec §7 / Rev 2): auto-seed expands from
            // the single top-1 hit (M3's GRAPH_AUTO_SEED_TOPK) to the top
            // GRAPH_REINFORCE_TOPK fused hits — a memory linked to ANY of the
            // result set's strong hits gets the proximity tilt, not only neighbors
            // of the single strongest hit.
            let mut by_score: Vec<(&String, &f32)> = fused.iter().collect();
            by_score.sort_by(|a, b| {
                // id desc = deterministic tie-break only (not semantically meaningful).
                b.1.total_cmp(a.1).then_with(|| b.0.cmp(a.0))
            });
            by_score.into_iter().take(GRAPH_REINFORCE_TOPK).map(|(id, _)| id.clone()).collect()
        };
        let graph_hops = self
            .current_neighbors_with_hops(&seeds, GRAPH_MAX_HOPS)
            .unwrap_or_else(|e| {
                log::warn!("recall: graph-proximity boost skipped: {e}");
                HashMap::new()
            });

        // ── Assemble hits: compute the full-precision (f64) boosted score, store
        //    it alongside the Hit so the sort comparator can use it without
        //    re-computing. Hit.score is set from the f64 value (truncated to f32
        //    for the public field) so callers get a reasonably precise score. ──
        let scored: Vec<(Hit, f64)> = fused
            .into_iter()
            .map(|(id, base_score)| {
                // Carry base score in f64 to avoid sub-millisecond recency deltas
                // underflowing when cast to f32 (see doc comment above).
                let mut score_f64 = base_score as f64;

                // Recency tilt: multiplicative, bounded by (1 + RECENCY_WEIGHT).
                // A candidate with no parseable ts gets factor 1.0 (no boost).
                if let Some(ts) = timestamps.get(&id) {
                    let age_secs = (now - *ts).num_milliseconds() as f64 / 1000.0;
                    let decay = (-age_secs / HALF_LIFE_SECS).exp();
                    score_f64 *= 1.0 + RECENCY_WEIGHT as f64 * decay;
                }

                // Pin: hard multiplicative boost for explicitly-pinned ids.
                if pinned.contains(&id) {
                    score_f64 *= PIN_MULTIPLIER as f64;
                }

                // Graph-proximity tilt: a current-edge neighbour of a seed is
                // boosted by 1 + GRAPH_WEIGHT * GRAPH_HOP_DECAY^(hops-1).
                if let Some(&hop) = graph_hops.get(&id) {
                    let decay = (GRAPH_HOP_DECAY as f64).powi(hop as i32 - 1);
                    score_f64 *= 1.0 + GRAPH_WEIGHT as f64 * decay;
                }

                let mut sources = Vec::new();
                if vector_set.contains(&id) {
                    sources.push(RecallSource::Vector);
                }
                if keyword_set.contains(&id) {
                    sources.push(RecallSource::Keyword);
                }
                // Per-hit event type (F2). Read before `id` is moved into the Hit;
                // a candidate missing from the map (race with deletion) gets an
                // empty kind, which the page-filter treats as a non-page (kept).
                let kind = kinds.get(&id).cloned().unwrap_or_default();
                let hit = Hit { event_id: id, score: score_f64 as f32, sources, kind };
                (hit, score_f64)
            })
            .collect();

        // ── Rerank (v1: identity). Split scored into (Hit, f64) components;
        //    keep the id→f64 map for the sort comparator. ──
        let reranker = NoopReranker;
        let mut id_to_score: std::collections::HashMap<String, f64> =
            HashMap::with_capacity(scored.len());
        let hits_only: Vec<Hit> = scored
            .into_iter()
            .map(|(h, s)| {
                id_to_score.insert(h.event_id.clone(), s);
                h
            })
            .collect();
        let mut hits = reranker.rerank(query, hits_only);

        // ── Sort: score_f64 DESC → ts DESC (newer wins) → event_id DESC (backstop).
        //    The ts-DESC key is the explicit recency tie-break that survives f32
        //    underflow (see doc comment). event_id DESC is the final deterministic
        //    backstop for candidates that genuinely share a ts (e.g. same-millisecond
        //    appends in tests). ──
        hits.sort_by(|a, b| {
            let sa = id_to_score.get(&a.event_id).copied().unwrap_or(0.0);
            let sb = id_to_score.get(&b.event_id).copied().unwrap_or(0.0);
            sb.total_cmp(&sa)
                .then_with(|| {
                    let ta = timestamps.get(&a.event_id);
                    let tb = timestamps.get(&b.event_id);
                    tb.cmp(&ta) // newer (larger DateTime) first
                })
                .then_with(|| b.event_id.cmp(&a.event_id)) // lexicographic DESC backstop
        });
        // F2/F4: drop pages/files that must not surface — BEFORE truncate(k) so a
        // superseded or revoked entry can never crowd out a valid lower-ranked
        // candidate. Uses single-sourced event-type discriminators.
        hits.retain(|h| {
            if h.kind == crate::graph::PAGE_EVENT_TYPE {
                if opts.exclude_pages {
                    return false; // one-way rule (F3)
                }
                return current_page_ids.contains(&h.event_id); // only the CURRENT page
            }
            if h.kind == crate::graph::FILE_INGESTED_EVENT_TYPE {
                if opts.exclude_files {
                    return false; // one-way rule for files — keeps external text out of evolve context (Task 9)
                }
                // Keep only the CURRENT version for its path AND only if the grant is
                // still active (never-forget storage ≠ must-surface).
                return current_file_ids.contains(&h.event_id);
            }
            if h.kind == crate::graph::SESSION_CAPTURED_EVENT_TYPE {
                // Deleted/superseded sessions are gone for EVERY caller — this arm
                // is UNCONDITIONAL (no RecallOptions gate), so the evolve and
                // snapshot recall paths are covered with no options change. Keep
                // only the current fold head; a deleted session (tombstoned) or a
                // superseded capture is not in `current`, so it is dropped even
                // after a reopen re-adds its persisted vector (rebuild-proof).
                // Do NOT "symmetrize" this to the memory arm's exclusion shape:
                // `!superseded_ids.contains(&h.event_id)` would MISS owner-deleted
                // sessions — the `session_deleted` tombstone keys on session_id,
                // not event id, so a deleted session's capture event is absent
                // from the supersede set. Inclusion-by-fold-head is the only
                // shape that covers both retirement paths here.
                return current_session_event_ids.contains(&h.event_id);
            }
            if h.kind == crate::graph::MEMORY_EVENT_TYPE {
                // EXCLUSION, not inclusion: `memory` is the shared kind of EVERY
                // ground-truth note, so an inclusion set would drop every
                // non-superseded memory. `superseded_ids` over-covers (it also
                // holds session/file supersede targets), but this arm consults it
                // only for memory-kind ids, so the over-breadth is inert. A rung-3
                // retired note (distinct `note_retired` marker, reversible) is also
                // dropped here — fold-time, no index rebuild — so retire/unretire
                // take effect immediately (the vector stays in the HNSW index).
                return !superseded_ids.contains(&h.event_id)
                    && !retired_note_ids.contains(&h.event_id);
            }
            true // every other kind always survives
        });
        hits.truncate(k);
        Ok(hits)
    }

    /// Collect, under a single short-lived lock, the events of an embeddable
    /// type that have no `vectors` row for `model_id`, in `seq` order. Returns
    /// owned `Event`s so the lock is released before any embedding happens.
    ///
    /// The SQL `IN (...)` filter is built from [`EMBEDDABLE_EVENT_TYPES`] so
    /// there is a single authoritative list — the Rust const and the SQL clause
    /// cannot drift independently.
    fn collect_pending(&self, model_id: &str) -> Result<Vec<Event>, BossclawError> {
        // SP3 embed gate: never re-vectorize a retired note/session or an
        // owner-deleted session. Computed BEFORE the store lock below because it
        // scans the log (via `session_events_ordered`) under the same lock, and
        // re-entering it here would deadlock. A deleted session keeps its OLD-model
        // vector in the `vectors` table, so without this gate a language migration
        // (which re-embeds every "pending" event under the new model) would mint it
        // a fresh vector and resurrect it in the rebuilt index.
        let excluded = self.embed_excluded_event_ids()?;
        // Build `?2,?3,...` placeholders (one per embeddable type; ?1 = model_id).
        let placeholders: String = EMBEDDABLE_EVENT_TYPES
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT e.payload FROM events e
             LEFT JOIN vectors v ON v.event_id = e.id AND v.model_id = ?1
             WHERE v.event_id IS NULL AND e.event_type IN ({placeholders})
             ORDER BY e.seq ASC"
        );
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        // Bind model_id first (?1), then each embeddable type (?2, ?3, …).
        // `&&str` coerces to `&dyn ToSql`; `&model_id` produces `&&str` from
        // `&str`, and `t` from `EMBEDDABLE_EVENT_TYPES` is already `&&str`.
        let params: Vec<&dyn rusqlite::ToSql> =
            std::iter::once(&model_id as &dyn rusqlite::ToSql)
                .chain(
                    EMBEDDABLE_EVENT_TYPES
                        .iter()
                        .map(|t| t as &dyn rusqlite::ToSql),
                )
                .collect();
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            // Skip a gone event (superseded note/session or deleted session).
            if excluded.contains(&ev.id) {
                continue;
            }
            out.push(ev);
        }
        Ok(out)
    }

    /// Collect `(event_id, text)` for every embeddable event, in `seq ASC`
    /// order, under a single short-lived lock.
    ///
    /// Used by [`EventLog::rebuild_indexes`] to populate the FTS keyword index.
    /// Only events whose `content["text"]` is a non-empty string are returned;
    /// events with missing or non-string `text` are silently skipped (their
    /// vectors would also be absent — see `embeddable_text`).
    fn collect_embeddable_events_ordered(&self) -> Result<Vec<(String, String)>, BossclawError> {
        let placeholders: String = EMBEDDABLE_EVENT_TYPES
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, payload FROM events WHERE event_type IN ({placeholders}) ORDER BY seq ASC"
        );
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = EMBEDDABLE_EVENT_TYPES
            .iter()
            .map(|t| t as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (event_id, payload) = row?;
            let event: Event = serde_json::from_str(&payload)?;
            if let Some(text) = embeddable_text(&event) {
                out.push((event_id, text));
            }
        }
        Ok(out)
    }

    /// Fetch the ingestion timestamp of each id in `ids`, parsed to
    /// [`DateTime<Utc>`], under a single short-lived lock.
    ///
    /// Used by [`EventLog::recall`] for the recency boost. The SQL is a single
    /// `SELECT id, ts FROM events WHERE id IN (...)` with one placeholder per id
    /// (matching the dynamic-`IN` pattern used by the other collectors). Ids not
    /// found in the log, or rows whose `ts` is not valid RFC 3339, are simply
    /// absent from the returned map — recall treats a missing ts as "no recency
    /// boost" rather than failing the whole query.
    ///
    /// An empty `ids` short-circuits to an empty map (an empty `IN ()` clause is
    /// a SQL syntax error).
    fn candidate_timestamps(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, DateTime<Utc>>, BossclawError> {
        let mut out = std::collections::HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let placeholders: String = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT id, ts FROM events WHERE id IN ({placeholders})");
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, ts) = row?;
            // A malformed ts is non-fatal: skip it so the candidate just misses
            // the recency boost (factor 1.0) instead of failing the whole recall.
            match DateTime::parse_from_rfc3339(&ts) {
                Ok(parsed) => {
                    out.insert(id, parsed.with_timezone(&Utc));
                }
                Err(e) => {
                    log::warn!("recall: event {id} has unparseable ts {ts:?}: {e}");
                }
            }
        }
        Ok(out)
    }

    /// `id → event_type` for the given ids, one parameterized query (F2). Ids not
    /// present are simply absent from the map. Mirrors [`Self::candidate_timestamps`]'
    /// single-lock IN-query so recall can tag each [`Hit`] with its kind and filter
    /// pages.
    fn candidate_event_types(
        &self,
        ids: &[String],
    ) -> Result<std::collections::HashMap<String, String>, BossclawError> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let placeholders: String = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT id, event_type FROM events WHERE id IN ({placeholders})");
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (id, t) = row?;
            out.insert(id, t);
        }
        Ok(out)
    }

    /// Switch the active embedding model, re-embed all events, GC stale vectors,
    /// and rebuild the in-memory indexes.
    ///
    /// # Steps (order is load-bearing for resumability)
    ///
    /// 1. **Config event** — append a `config` event naming `embedder` as the
    ///    new active model. The `schema_version` is inherited from the most
    ///    recent existing config, or [`SCHEMA_VERSION`] if no config exists yet.
    ///
    /// 2. **Re-embed** — [`EventLog::rederive_pending`] backfills every event
    ///    that lacks a vector for `embedder.model_id()`. Best-effort: individual
    ///    embed failures are logged and skipped.
    ///
    /// 3. **GC** — `DELETE FROM vectors WHERE model_id != embedder.model_id()`.
    ///    All rows for every other model are removed. The count of removed rows is
    ///    recorded in [`ReembedStats::gc_removed`].
    ///
    /// 4. **Rebuild** — [`EventLog::rebuild_indexes`] rebuilds both the ANN
    ///    vector index and the FTS5 keyword index under the new model.
    ///
    /// # Integrity note
    ///
    /// The active-model switch is recorded as a `config` event that is
    /// Ed25519-signed and hash-chained (M1), so a forged or replayed
    /// model-switch is tamper-evident via `verify_chain` / `verify_chain_since`.
    /// Surfacing a model-switch to the user as a recall-integrity alert is
    /// deferred to the desktop (M7).
    ///
    /// # Resumability
    ///
    /// A crash between the config switch (step 1) and the GC (step 3) is
    /// correctness-safe: recall is active-model-filtered, so stale rows for
    /// the old model are simply ignored. Re-running `reembed_migration` (or
    /// the next migration) completes the GC, making this operation
    /// idempotent/resumable. A second run re-embeds 0 (nothing pending) and
    /// GCs 0 (stale rows already removed), leaving one consistent active model.
    ///
    /// A crash after the GC (step 3) but before `rebuild_indexes` completes
    /// (step 4) is also safe: the `vectors` table already contains only the new
    /// model's rows (no data loss), and the in-memory index is simply stale or
    /// absent. Recovery is a single call to `rebuild_indexes(embedder)`, which
    /// is also what a normal reopen + rebuild does — no special handling needed.
    ///
    /// # Lock discipline
    ///
    /// The single-store [`Mutex`] is never held across [`Embedder::embed`]
    /// calls. Re-embedding is delegated to [`EventLog::rederive_pending`] which
    /// already implements that discipline. The GC `DELETE` is a short, bounded
    /// operation and holds the lock only for that one statement.
    ///
    /// # Returns
    ///
    /// [`ReembedStats`] carrying `reembedded`, `gc_removed`, and `elapsed_ms`
    /// — the §15 time-budget observability signal.
    pub fn reembed_migration(
        &self,
        embedder: &dyn Embedder,
    ) -> Result<ReembedStats, BossclawError> {
        let migration_start = Instant::now();

        // Step 1: append a config event selecting the new active model.
        // Reuse the existing schema_version if a config already exists.
        let schema_version = self
            .active_model()?
            .map(|m| m.schema_version)
            .unwrap_or(SCHEMA_VERSION);

        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "active_model_id": embedder.model_id(),
                "dim": embedder.dim() as u32,
                "schema_version": schema_version,
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: "did:wba:bossclaw-migration".to_string(),
            signature: None,
        })?;

        // Step 2: re-embed every event missing a vector for the new model.
        let reembedded = self.rederive_pending(embedder)?;

        // Step 3: GC — delete all vectors for every model OTHER than the new one.
        // Hold the lock only for this short DELETE statement.
        let gc_removed = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.execute(
                "DELETE FROM vectors WHERE model_id != ?1",
                rusqlite::params![embedder.model_id()],
            )?;
            conn.changes() as usize
        };

        // Step 4: rebuild the in-memory ANN + FTS indexes under the new model.
        self.rebuild_indexes(embedder)?;

        // Count total events BEFORE stopping the clock so `elapsed_ms` spans
        // the whole operation including this query.
        let total_events = self.count()?;
        let elapsed_ms = migration_start.elapsed().as_millis();

        // Avoid division by zero; an idempotent re-run (reembedded == 0) or a
        // migration on an empty store are both valid. Report the throughput label
        // as "reembedded/sec" (not "events/sec") so a 0-reembed idempotent run
        // prints "0 reembedded/sec" without ambiguity.
        let reembedded_per_sec = if elapsed_ms > 0 {
            reembedded as f64 / (elapsed_ms as f64 / 1000.0)
        } else {
            f64::INFINITY
        };
        log::info!(
            "re-embed migration: {} vectors re-embedded in {}ms ({:.0} reembedded/sec); \
             gc_removed={} total_events={} model={}",
            reembedded,
            elapsed_ms,
            reembedded_per_sec,
            gc_removed,
            total_events,
            embedder.model_id(),
        );

        Ok(ReembedStats { reembedded, gc_removed, elapsed_ms })
    }

    /// STAGE 1 of a crash-safe language migration (invariant I5): re-embed every embeddable event
    /// AND every entity under `embedder.model_id()`, writing the new-id rows ALONGSIDE the existing
    /// old-id rows (nothing is deleted here). Reports progress as `(done, total)` over embeddable
    /// events. Returns `Ok(())` only when EVERY embeddable-with-text event has a new-id vector; a
    /// shortfall (an embed failure) returns `Err` with NO GC, so recall keeps serving the old model
    /// and the migration is retryable. Idempotent: a re-run derives only the still-missing rows.
    ///
    /// The store `Mutex` is never held across [`Embedder::embed`] (each upsert takes it briefly),
    /// matching [`EventLog::rederive_pending`]'s lock discipline.
    pub fn reembed_prepare(
        &self,
        embedder: &dyn Embedder,
        on_progress: &mut dyn FnMut(u64, u64),
    ) -> Result<(), BossclawError> {
        // Total embeddable-with-text events (the completeness denominator — events lacking text are
        // legitimately vector-less and must NOT count against completeness).
        //
        // SP3: drop retired/deleted events from BOTH the denominator and the completeness scan so
        // they agree with `collect_pending`'s embed gate. Otherwise a deleted session (skipped by
        // `collect_pending`, so never re-embedded under the new model) would still be counted as
        // "missing a vector" below and block the migration from EVER completing.
        let excluded = self.embed_excluded_event_ids()?;
        let embeddable: Vec<(String, String)> = self
            .collect_embeddable_events_ordered()?
            .into_iter()
            .filter(|(id, _)| !excluded.contains(id))
            .collect();
        let total = embeddable.len() as u64;
        on_progress(0, total);

        // Re-embed the pending memory/page/file vectors under the new id.
        let pending = self.collect_pending(embedder.model_id())?;
        let mut done = (total as usize).saturating_sub(pending.len()) as u64;
        for event in pending {
            // A single embed failure is tolerated here; the completeness scan below turns it into
            // the all-or-nothing `Err`.
            if let Ok(true) = self.embed_and_upsert(embedder, &event) {
                done += 1;
                on_progress(done, total);
            }
        }

        // Re-embed entity resolution vectors under the new id (U8). The entities projection must be
        // current, so rebuild it first (cheap; deterministic fold over entity events).
        self.rebuild_graph()?;
        self.rederive_entity_vectors_pending(embedder)?;

        // Completeness scan (I5): every embeddable-with-text event MUST now have a new-id vector.
        // Reuse the `embeddable` list collected above so this shares ONE scan with the denominator.
        let missing = self.count_missing_vectors(embedder.model_id(), &embeddable)?;
        if missing > 0 {
            return Err(BossclawError::Store(format!(
                "re-embed incomplete: {missing} of {total} embeddable events still lack a vector \
                 under {} — no vectors were garbage-collected; retry",
                embedder.model_id()
            )));
        }
        Ok(())
    }

    /// STAGE 2 of a crash-safe language migration: after the signed record has been flipped to
    /// `Complete` (the commit point — done by the daemon between prepare and this call), GC every
    /// `vectors` AND `entity_vectors` row for a model OTHER than `embedder.model_id()`, then rebuild
    /// the in-memory recall + entity indexes under the new model. Returns [`ReembedStats`]. Safe to
    /// re-run (idempotent GC of already-removed rows).
    pub fn reembed_finalize_gc(&self, embedder: &dyn Embedder) -> Result<ReembedStats, BossclawError> {
        let started = Instant::now();
        let gc_removed = self.gc_stale_vectors(embedder.model_id())?;
        self.rebuild_indexes(embedder)?;
        self.rebuild_entity_index(embedder)?;
        Ok(ReembedStats {
            reembedded: self.vectors_for_model(embedder.model_id())?.len(),
            gc_removed,
            elapsed_ms: started.elapsed().as_millis(),
        })
    }

    /// GC every `vectors` + `entity_vectors` row whose `model_id` differs from `keep_model_id`.
    /// Returns the number of `vectors` rows removed. Idempotent. Used by [`EventLog::reembed_finalize_gc`]
    /// and by the daemon's boot sweep after a crash between the record-flip and the GC.
    pub fn gc_stale_vectors(&self, keep_model_id: &str) -> Result<usize, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        // Both DELETEs commit together or not at all: a crash between them must never leave the
        // recall (`vectors`) and entity-resolution (`entity_vectors`) tables under different models.
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM vectors WHERE model_id != ?1", rusqlite::params![keep_model_id])?;
        let removed = tx.changes() as usize;
        tx.execute("DELETE FROM entity_vectors WHERE model_id != ?1", rusqlite::params![keep_model_id])?;
        tx.commit()?;
        Ok(removed)
    }

    /// Count how many of `embeddable` still lack a `vectors` row for `model_id`. `embeddable` is the
    /// authoritative embeddable-with-text list `(event_id, text)`, passed IN so this scan and the
    /// caller's completeness denominator share ONE `collect_embeddable_events_ordered` pass (they
    /// cannot drift, and the scan is not run twice per migration).
    fn count_missing_vectors(&self, model_id: &str, embeddable: &[(String, String)]) -> Result<usize, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT 1 FROM vectors WHERE event_id = ?1 AND model_id = ?2",
        )?;
        let mut missing = 0usize;
        for (event_id, _text) in embeddable {
            let has: bool = stmt.exists(rusqlite::params![event_id, model_id])?;
            if !has {
                missing += 1;
            }
        }
        Ok(missing)
    }

    /// Re-derive resolution vectors under `embedder.model_id()` for every entity in the current
    /// projection that lacks one (U8). Reads the label from the `entities` table; idempotent upsert.
    pub fn rederive_entity_vectors_pending(&self, embedder: &dyn Embedder) -> Result<usize, BossclawError> {
        // Collect (entity_id, label) for entities missing a vector under the new model, releasing
        // the lock before embedding (same discipline as collect_pending).
        let pending: Vec<(String, String)> = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            let mut stmt = conn.prepare(
                "SELECT e.entity_id, e.label FROM entities e
                 LEFT JOIN entity_vectors v ON v.entity_id = e.entity_id AND v.model_id = ?1
                 WHERE v.entity_id IS NULL
                 ORDER BY e.entity_id ASC",
            )?;
            let rows = stmt.query_map([embedder.model_id()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            out
        };
        let mut derived = 0usize;
        for (entity_id, label) in pending {
            self.derive_entity_vector(embedder, &entity_id, &label)?;
            derived += 1;
        }
        Ok(derived)
    }

    /// Record the active embedding model as a signed `config` event so
    /// [`active_model`](Self::active_model) becomes truthful. Mirrors the config
    /// write inside [`reembed_migration`](Self::reembed_migration) but signed by
    /// this log's own DID (not the migration DID) and without the re-embed/GC —
    /// callers that have just embedded under `model_id` use this to stamp the
    /// model at vector-birth. Reuses the existing `schema_version` if a config
    /// already exists. Returns the event id.
    pub fn set_active_model(&self, model_id: &str, dim: u32) -> Result<String, BossclawError> {
        let schema_version = self
            .active_model()?
            .map(|m| m.schema_version)
            .unwrap_or(SCHEMA_VERSION);
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "active_model_id": model_id,
                "dim": dim,
                "schema_version": schema_version,
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// Append a signed Tier-B `link` event connecting `src` —`relation`→ `dst`.
    ///
    /// `valid_time` (optional, RFC 3339) is the world-clock start; absent means
    /// "valid from when we learned it" (the event's ingestion `ts`). If
    /// `source_event_ids` is empty it defaults to `[src, dst]` so the Tier-B
    /// non-empty-provenance rule is satisfied honestly (the two endpoints justify
    /// the link). Returns the new event id (which is also the edge's identity).
    ///
    /// The `edges` table is NOT updated here — call [`EventLog::rebuild_graph`]
    /// to refresh `neighbors`/`as_of`/the recall boost (same "rebuild after
    /// append" lifecycle as [`EventLog::rebuild_indexes`]).
    pub fn link(
        &self,
        src: &str,
        relation: &str,
        dst: &str,
        valid_time: Option<&str>,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        self.append_graph_event("link", MANUAL_LINK_PRODUCER, src, relation, dst, valid_time, source_event_ids)
    }

    /// Append a signed Tier-B MACHINE `link` carrying its `confidence` as an
    /// INTEGER `confidence_milli` (0..=1000) in the signed CONTENT (spec §4/§7;
    /// Rev 2 F3 — never a raw `f32`, never in `ModelMeta`). For the M4a reasoner:
    /// a NON-MANUAL producer, so `source_event_ids` MUST be non-empty (the F2
    /// taint guard rejects an empty set — an empty default would launder taint
    /// past the §5.11 lineage walk).
    ///
    /// `confidence` is clamped to `[0.0, 1.0]` then quantized to integer milli
    /// (`(c.clamp(0.0,1.0) * 1000.0).round() as i64`) so the JCS-canonical signed
    /// bytes have ONE deterministic form — a float would risk
    /// [`EventLog::verify_chain`] breaking across `serde_jcs` versions on this
    /// append-only signed store. The value projects to `edges.confidence_milli`
    /// and gates the recall boost (spec §7): a machine edge below
    /// [`crate::extract::TRUST_MIN`] is recorded + queryable but does NOT tilt
    /// recall. The `producer` MUST NOT be [`MANUAL_LINK_PRODUCER`] (a machine link
    /// is, by definition, non-manual — that is what makes `origin = "machine"`).
    /// Returns the new edge event's id.
    ///
    /// The `edges` table is NOT updated here — call [`EventLog::rebuild_graph`].
    pub fn link_machine(
        &self,
        src: &str,
        relation: &str,
        dst: &str,
        confidence: f32,
        producer: &str,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        if source_event_ids.is_empty() {
            return Err(BossclawError::InvalidInput(
                "machine link requires explicit non-empty source_event_ids (the cheat-sheet \
                 read-set) — an empty default would launder taint past the §5.11 lineage walk"
                    .into(),
            ));
        }
        // A machine link is, by definition, NON-manual — that is what makes
        // origin = "machine" and keeps its confidence. A manual producer would
        // silently fold as a manual edge with confidence discarded. The producer
        // is engine-internal (never user input), so a debug_assert is the right
        // guard: it catches a wiring mistake in tests/dev without a release cost.
        debug_assert!(
            producer != MANUAL_LINK_PRODUCER,
            "link_machine producer must be non-manual"
        );
        // Integer milli (Rev 2 F3): clamp to [0,1] then quantize — single-sourced
        // in extract so the encode side and the trust-gate threshold can never
        // diverge. ONE canonical JCS form, no f32/f64 ambiguity in SIGNED content.
        let confidence_milli = crate::extract::to_confidence_milli(confidence);
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: "link".to_string(),
            content: serde_json::json!({
                "src": src,
                "relation": relation,
                "dst": dst,
                "confidence_milli": confidence_milli,
            }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(),
                prompt_hash: String::new(),
                source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// Append a signed Tier-B `invalidate` event retiring the edge-key
    /// `(src, relation, dst)`. `valid_time` (optional) is when the fact stopped
    /// being true in the world. Same `source_event_ids` defaulting and lifecycle
    /// as [`EventLog::link`].
    pub fn invalidate(
        &self,
        src: &str,
        relation: &str,
        dst: &str,
        valid_time: Option<&str>,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        self.append_graph_event("invalidate", MANUAL_LINK_PRODUCER, src, relation, dst, valid_time, source_event_ids)
    }

    /// Append a signed Tier-B `entity` event minting a stable `entity:<ulid>`
    /// node carrying `{label, aliases, entity_type}` (spec §4). Returns the
    /// namespaced node id `entity:<event id>` (NOT the bare event id) — the form
    /// links reference.
    ///
    /// `entity` is a NON-MANUAL producer: `source_event_ids` MUST be non-empty
    /// (the memory/-ies that introduced the entity). An empty source set is
    /// rejected (the M3 F2 taint guard, parent §5.11) — defaulting here would
    /// erase the inducing memory from the lineage the actuator walks fail-closed.
    ///
    /// The `entities` table is NOT updated here — call [`EventLog::rebuild_graph`]
    /// to refresh it (same append→rebuild lifecycle as [`EventLog::link`]).
    pub fn entity(
        &self,
        label: &str,
        aliases: &[String],
        entity_type: &str,
        producer: &str,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        if source_event_ids.is_empty() {
            // entity is never the manual producer; an empty source set is always
            // a taint-laundering reject (mirrors `append_graph_event`'s F2 arm).
            return Err(BossclawError::InvalidInput(
                "entity event requires explicit non-empty source_event_ids (the inducing \
                 memory) — an empty default would erase it from the §5.11 lineage walk"
                    .into(),
            ));
        }
        let event_id = self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: ENTITY_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "label": label,
                "aliases": aliases,
                "entity_type": entity_type,
            }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(),
                prompt_hash: String::new(),
                source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(entity_node_id(&event_id))
    }

    /// Every entity, `ORDER BY entity_id ASC` (deterministic). Tier-A read.
    pub fn all_entities(&self) -> Result<Vec<crate::graph::Entity>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT entity_id, label, aliases, entity_type \
             FROM entities ORDER BY entity_id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let aliases_json: String = r.get(2)?;
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                aliases_json,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (entity_id, label, aliases_json, entity_type) = row?;
            // aliases is stored as a JSON array string; a malformed value degrades
            // to empty rather than failing the read (best-effort, matches the fold).
            let aliases: Vec<String> =
                serde_json::from_str(&aliases_json).unwrap_or_default();
            out.push(crate::graph::Entity { entity_id, label, aliases, entity_type });
        }
        Ok(out)
    }

    /// Append a signed Tier-B `page` (summary) event for `topic_id` (spec §4).
    /// `claims` are the structured `{text, cites:[event_id]}` items; `cites` MUST
    /// be sorted+deduped and `claims` capped to `MAX_CLAIMS_PER_PAGE` by the
    /// caller BEFORE this (F7 — canonicalization). NON-MANUAL producer: empty
    /// `source_event_ids` rejected (F4). `text` is the rendered body (also the
    /// embedded text). Returns the page event id.
    // The explicit-args shape mirrors [`EventLog::entity`]; a params struct would
    // add indirection without safety benefit (same rationale as `append_graph_event`).
    #[allow(clippy::too_many_arguments)]
    pub fn page(
        &self,
        topic_id: &str,
        title: &str,
        text: &str,
        claims: &[serde_json::Value],
        tags: &[String],
        producer: &str,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        if source_event_ids.is_empty() {
            return Err(BossclawError::InvalidInput(
                "page event requires explicit non-empty source_event_ids (the cited memories)".into(),
            ));
        }
        self.append(Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::PAGE_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "topic_id": topic_id, "title": title, "text": text,
                "claims": claims, "tags": tags,
            }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(), prompt_hash: String::new(),
                source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        })
    }

    /// Append a signed Tier-B `supersede` retiring page id `supersedes` (spec §4).
    /// Machine producer → empty `source_event_ids` rejected (F4). Prefer
    /// [`EventLog::emit_page`] which pairs this with the replacement atomically.
    pub fn supersede(
        &self,
        supersedes: &str,
        producer: &str,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        if source_event_ids.is_empty() {
            return Err(BossclawError::InvalidInput(
                "supersede event requires explicit non-empty source_event_ids".into(),
            ));
        }
        self.append(Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::SUPERSEDE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "supersedes": supersedes }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(), prompt_hash: String::new(),
                source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        })
    }

    /// Emit a dossier for a topic, atomically superseding its prior page (F5).
    /// When `prior_page_id` is `Some`, `supersede`+`page` go through `append_pair`
    /// (no orphan supersede); when `None` (first page), just the `page`. Returns
    /// `(page_event_id, superseded)`. The caller guarantees `claims` are already
    /// floor-verified, cap-applied, and `cites`-sorted (F6/F7).
    // Explicit-args shape mirrors [`EventLog::page`]; a params struct would add
    // indirection without safety benefit (same rationale as `append_graph_event`).
    #[allow(clippy::too_many_arguments)]
    pub fn emit_page(
        &self,
        topic_id: &str,
        title: &str,
        text: &str,
        claims: &[serde_json::Value],
        tags: &[String],
        producer: &str,
        source_event_ids: &[String],
        prior_page_id: Option<&str>,
    ) -> Result<(String, bool), BossclawError> {
        if source_event_ids.is_empty() {
            return Err(BossclawError::InvalidInput("page requires non-empty source_event_ids".into()));
        }
        let page_ev = Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::PAGE_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "topic_id": topic_id, "title": title, "text": text,
                "claims": claims, "tags": tags,
            }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(), prompt_hash: String::new(),
                source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        };
        match prior_page_id {
            None => Ok((self.append(page_ev)?, false)),
            Some(prior) => {
                let supersede_ev = Event {
                    id: String::new(), ts: String::new(), valid_time: None,
                    event_type: crate::graph::SUPERSEDE_EVENT_TYPE.to_string(),
                    content: serde_json::json!({ "supersedes": prior }),
                    model_meta: Some(ModelMeta {
                        model_id: producer.to_string(), prompt_hash: String::new(),
                        source_event_ids: source_event_ids.to_vec(),
                    }),
                    prev_hash: String::new(), hash: None,
                    signed_by_did: self.signer_did(), signature: None,
                };
                let (_s, p) = self.append_pair(supersede_ev, page_ev)?;
                Ok((p, true))
            }
        }
    }

    /// Append a signed Tier-B `write_proposal` stamped with the M6b reconciler producer.
    /// Thin wrapper over [`Self::append_write_proposal_with`] — see it for argument detail.
    #[cfg(unix)]
    #[allow(clippy::too_many_arguments)]
    pub fn append_write_proposal(
        &self, target_canonical: &str, op: &str, new_content_hash: &str, byte_size: u64,
        rationale: &str, inducing_key: &serde_json::Value, verdict_summary: &serde_json::Value,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        self.append_write_proposal_with(
            target_canonical, op, new_content_hash, byte_size, rationale,
            inducing_key, verdict_summary, source_event_ids,
            crate::graph::M6B_PROPOSER_PRODUCER,
        )
    }

    /// Append a signed Tier-B `write_proposal` stamped with `producer` as its
    /// `model_meta.model_id` (M6b passes `M6B_PROPOSER_PRODUCER`; M6c mandates pass
    /// `M6C_PROPOSER_PRODUCER`). `source_event_ids` MUST be the engine-gathered lineage
    /// (Task 3), non-empty. Bytes are NOT in the event (Task 5 side table). The producer
    /// string is provenance ONLY — it is independent of the taint lineage.
    #[cfg(unix)]
    #[allow(clippy::too_many_arguments)]
    pub fn append_write_proposal_with(
        &self, target_canonical: &str, op: &str, new_content_hash: &str, byte_size: u64,
        rationale: &str, inducing_key: &serde_json::Value, verdict_summary: &serde_json::Value,
        source_event_ids: &[String], producer: &str,
    ) -> Result<String, BossclawError> {
        let content = serde_json::json!({
            "target": target_canonical, "op": op,
            "new_content_hash": new_content_hash, "byte_size": byte_size,
            "rationale": rationale, "inducing_key": inducing_key, "verdict_summary": verdict_summary,
        });
        self.append(self.build_proposer_event(producer, crate::graph::WRITE_PROPOSAL_EVENT_TYPE, content, source_event_ids))
    }

    /// Append a signed Rung-3 `conflict_proposal` (spec §3.5). Content: `{ a_ref, b_ref,
    /// winner_hint, confidence_band, why, detected_at }` — typed stable refs only, NO memory
    /// bodies (I7). `winner_hint`/`confidence_band` are the coarsened forms; `why` MUST be the
    /// CONTENT-FREE `conflict::templated_why` output (never model text). `source_event_ids` is the
    /// referenced memories' lineage (note event id / session capture event id). Mirrors the
    /// `#[cfg(unix)]` `build_proposer_event` shape used by the write-proposal family.
    #[cfg(unix)]
    #[allow(clippy::too_many_arguments)]
    pub fn append_conflict_proposal(
        &self,
        a_ref: &crate::index::ConflictRef,
        b_ref: &crate::index::ConflictRef,
        winner_hint: &str,
        confidence_band: &str,
        why: &str,
        detected_at: i64,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        let content = serde_json::json!({
            "a_ref": a_ref.to_json(),
            "b_ref": b_ref.to_json(),
            "winner_hint": winner_hint,
            "confidence_band": confidence_band,
            "why": why,
            "detected_at": detected_at,
        });
        self.append(self.build_proposer_event(
            crate::graph::CONFLICT_PROPOSER_PRODUCER,
            crate::graph::CONFLICT_PROPOSAL_EVENT_TYPE,
            content,
            source_event_ids,
        ))
    }

    /// Every `conflict_proposal` whose BOTH refs STILL resolve to a current memory — the OPEN set.
    /// Auto-withdraw (I-gc, no withdrawal event) is exact for a NOTE ref (retire/delete/edit ⇒ a new
    /// event id, so it leaves `current_notes`) and for a WHOLE-session delete. A PASSAGE ref is
    /// current only when its session is current, it is not in `retired_passages`, AND its ordinal
    /// still exists in the current head capture — so a same-`session_id` supersede that re-chunks to
    /// fewer passages (emitting no `passage_retired`) withdraws a proposal on the vanished ordinal.
    /// Oldest first (`events_of_types` is `seq ASC`). Shared by `pending_conflict_proposals` (Task 7)
    /// and `is_conflict_proposal_suppressed`. `#[cfg(unix)]` (feeds the append/projection/sweep family).
    #[cfg(unix)]
    fn open_conflict_proposals(&self) -> Result<Vec<OpenConflictProposal>, BossclawError> {
        use crate::index::ConflictRef;
        // Current membership: notes by event id; sessions by session_id; retired passages.
        let current_note_ids: std::collections::HashSet<String> =
            self.current_notes()?.into_iter().map(|n| n.event_id).collect();
        let fold = fold_sessions(&self.session_events_ordered()?);
        let current_sessions: std::collections::HashSet<&str> =
            fold.current.iter().map(|cs| cs.session_id.as_str()).collect();
        // Passage count of each CURRENT session's head capture. A Passage ref is only "current" if its
        // ordinal still EXISTS in that head: a same-`session_id` supersede that re-chunks to fewer
        // passages emits no `passage_retired`, so a dropped ordinal would otherwise stay open.
        // `session_passage_count` is model-agnostic and can OVER-count in a multi-model state (an
        // accepted property — see the Task 3 caveat); over-count is CONSERVATIVE here (keeps a proposal
        // open slightly too long, never withdraws a valid one), and the default single-model config is
        // exact.
        let mut head_passage_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::with_capacity(fold.current.len());
        for cs in &fold.current {
            head_passage_counts.insert(cs.session_id.clone(), self.session_passage_count(&cs.event_id)?);
        }
        let ref_is_current = |r: &ConflictRef| -> bool {
            match r {
                ConflictRef::Note { event_id } => current_note_ids.contains(event_id),
                ConflictRef::Passage { session_id, passage_id } => {
                    current_sessions.contains(session_id.as_str())
                        && !fold.retired_passages.contains(&(session_id.clone(), *passage_id))
                        // Some() is guaranteed by the current_sessions check above; the 0 fallback is
                        // unreachable but fails safe (treat as no passages → withdraw).
                        && *passage_id < *head_passage_counts.get(session_id.as_str()).unwrap_or(&0)
                    // ordinal still exists in the current head
                }
            }
        };
        let mut out = Vec::new();
        for ev in self.events_of_types(&[crate::graph::CONFLICT_PROPOSAL_EVENT_TYPE])? {
            let (Some(a_ref), Some(b_ref)) = (
                ev.content.get("a_ref").and_then(ConflictRef::from_json),
                ev.content.get("b_ref").and_then(ConflictRef::from_json),
            ) else {
                continue; // malformed — never open
            };
            if !ref_is_current(&a_ref) || !ref_is_current(&b_ref) {
                continue; // GC: a side is gone → withdrawn
            }
            out.push(OpenConflictProposal {
                id: ev.id.clone(),
                a_ref,
                b_ref,
                winner_hint: ev.content.get("winner_hint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                confidence_band: ev.content.get("confidence_band").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                why: ev.content.get("why").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                detected_at: ev.content.get("detected_at").and_then(|v| v.as_i64()).unwrap_or(0),
            });
        }
        Ok(out)
    }

    /// Recover `(a_ref, b_ref)` for a `conflict_proposal` by id, REGARDLESS of open-ness (spec §2.3,
    /// MAJOR-1). `open_conflict_proposals` withdraws a proposal whose ref went non-current (a retire), so
    /// the idempotency/roll-forward path in [`EventLog::resolve_conflict`] cannot read refs from the open
    /// set. This reads the raw `conflict_proposal` event by id. `None` for an unknown id or a non-proposal
    /// event (a `memory` id must never resolve here). `#[cfg(unix)]`.
    #[cfg(unix)]
    pub fn conflict_proposal_by_id(
        &self,
        proposal_id: &str,
    ) -> Result<Option<(crate::index::ConflictRef, crate::index::ConflictRef)>, BossclawError> {
        use crate::index::ConflictRef;
        let Some(ev) = self.event_by_id(proposal_id)? else {
            return Ok(None);
        };
        if ev.event_type != crate::graph::CONFLICT_PROPOSAL_EVENT_TYPE {
            return Ok(None);
        }
        let (Some(a), Some(b)) = (
            ev.content.get("a_ref").and_then(ConflictRef::from_json),
            ev.content.get("b_ref").and_then(ConflictRef::from_json),
        ) else {
            return Ok(None); // malformed proposal — treat as unknown
        };
        Ok(Some((a, b)))
    }

    /// Every OPEN conflict proposal (both refs current), oldest first — the read behind a later
    /// App-only `ListConflicts` (spec §3.5). GC is inherent: `open_conflict_proposals` already
    /// drops any proposal whose referenced memory was retired/deleted/edited (I-gc). Fold-derived,
    /// so it survives restart with no cursor. `#[cfg(unix)]`.
    #[cfg(unix)]
    pub fn pending_conflict_proposals(&self) -> Result<Vec<ConflictProposalRow>, BossclawError> {
        // Rung-3 Phase-3 (§2.2 item 2, I9): a retire drops via the currency-GC in
        // `open_conflict_proposals`; KeepBoth/Dismiss retire NOTHING, so the SAME live coexist/dismissed
        // set that suppresses the finder (Task 7) must also drop them here — or the pending count /
        // `ListConflicts` nags forever. Single-sourced `resolution_exclusions()` ⇒ reader and finder
        // can never drift on `session_heads` liveness.
        let excluded = self.resolution_exclusions()?;
        Ok(self
            .open_conflict_proposals()?
            .into_iter()
            .filter(|p| {
                let pk = Self::conflict_pair_key(&p.a_ref, &p.b_ref);
                !excluded.coexist_pairs.contains(&pk) && !excluded.dismissed_pairs.contains(&pk)
            })
            .map(|p| ConflictProposalRow {
                id: p.id,
                a_ref: p.a_ref,
                b_ref: p.b_ref,
                winner_hint: p.winner_hint,
                confidence_band: p.confidence_band,
                why: p.why,
                detected_at: p.detected_at,
            })
            .collect())
    }

    /// The unordered pair key for two typed refs (sorted `pair_key`s). Two refs in either order
    /// map to the SAME key — the idempotency identity (spec §3.5). `#[cfg(unix)]` (only the sweep +
    /// idempotency predicate — both `#[cfg(unix)]` — use it). Delegates to the single source of
    /// truth ([`crate::index::ConflictRef::unordered_pair_key`]) shared with the finder so the two
    /// can never drift.
    #[cfg(unix)]
    fn conflict_pair_key(a: &crate::index::ConflictRef, b: &crate::index::ConflictRef) -> String {
        crate::index::ConflictRef::unordered_pair_key(a, b)
    }

    /// True iff an OPEN `conflict_proposal` already exists for the unordered typed pair `(a, b)`
    /// (spec §3.5). A GC-withdrawn proposal (a referenced memory gone) does NOT suppress — the
    /// pair may re-propose (so a materially-changed / re-added memory re-opens, resolved Q3).
    /// `#[cfg(unix)]` (consumes `open_conflict_proposals`).
    #[cfg(unix)]
    pub fn is_conflict_proposal_suppressed(
        &self,
        a: &crate::index::ConflictRef,
        b: &crate::index::ConflictRef,
    ) -> Result<bool, BossclawError> {
        let want = Self::conflict_pair_key(a, b);
        Ok(self
            .open_conflict_proposals()?
            .iter()
            .any(|p| Self::conflict_pair_key(&p.a_ref, &p.b_ref) == want))
    }

    /// The live coexist + dismissed PAIR exclusions (spec §2.2 / §3.1). ONE fold-derived read consumed by
    /// BOTH the finder's `open_pairs` union (Task 7) AND `pending_conflict_proposals` (Task 8), so the two
    /// honor resolution identically (resolved Open-Q1). A `coexist_allowed` pair is permanent; a
    /// `dismissed` pair is included ONLY while every session in its stored `session_heads` still has that
    /// exact current head (a re-capture advances the head → the dismissal lapses → the pair may
    /// re-propose). Notes need no head: an edit mints a new event id → a new `unordered_pair_key` the
    /// stored key no longer matches, so the stale key becomes inert. Restart-safe (pure fold, no cursor).
    #[cfg(unix)]
    #[allow(dead_code)] // consumed by the finder's open_pairs union (Task 7) + pending_conflict_proposals (Task 8)
    fn resolution_exclusions(&self) -> Result<ResolutionExclusions, BossclawError> {
        let fold = fold_sessions(&self.session_events_ordered()?);
        let head_of: std::collections::HashMap<String, String> =
            fold.current.iter().map(|cs| (cs.session_id.clone(), cs.event_id.clone())).collect();
        let mut out = ResolutionExclusions::default();
        for ev in self.events_of_types(&[
            crate::graph::COEXIST_ALLOWED_EVENT_TYPE,
            crate::graph::DISMISSED_EVENT_TYPE,
        ])? {
            let Some(pk) = ev.content.get("pair_key").and_then(|v| v.as_str()) else {
                continue; // malformed — never excludes
            };
            if ev.event_type == crate::graph::COEXIST_ALLOWED_EVENT_TYPE {
                out.coexist_pairs.insert(pk.to_string());
                continue;
            }
            // dismissed: live only while every stored session head is unchanged.
            let live = match ev.content.get("session_heads").and_then(|v| v.as_object()) {
                None => true, // no passage members (note↔note) → no head to lapse; key is inert on edit
                Some(map) => map.iter().all(|(sid, stored)| {
                    head_of.get(sid).map(String::as_str) == stored.as_str()
                }),
            };
            if live {
                out.dismissed_pairs.insert(pk.to_string());
            }
        }
        Ok(out)
    }

    /// Fold ALL terminal markers into `proposal_id -> ResolutionRecord` (spec §2.1). The idempotency +
    /// terminal-state guard in [`EventLog::resolve_conflict`] reads this over ALL proposals (NOT the open
    /// set — a retire withdrew the proposal from open, MAJOR-1). FIRST marker per proposal wins (a second,
    /// different action is rejected by the guard before it can append, so a well-formed log has at most one
    /// per id; the fold defensively keeps the earliest). `#[cfg(unix)]`.
    #[cfg(unix)]
    #[allow(dead_code)] // consumed by resolve_conflict's terminal-state guard (Task 6)
    fn resolution_markers(&self) -> Result<std::collections::HashMap<String, ResolutionRecord>, BossclawError> {
        let mut out: std::collections::HashMap<String, ResolutionRecord> = std::collections::HashMap::new();
        for ev in self.events_of_types(&[
            crate::graph::CONFLICT_RESOLVED_EVENT_TYPE,
            crate::graph::COEXIST_ALLOWED_EVENT_TYPE,
            crate::graph::DISMISSED_EVENT_TYPE,
        ])? {
            let Some(pid) = ev.content.get("proposal_id").and_then(|v| v.as_str()) else {
                continue;
            };
            if out.contains_key(pid) {
                continue; // earliest (seq ASC) wins — events_of_types is seq-ordered
            }
            let record = match ev.event_type.as_str() {
                t if t == crate::graph::CONFLICT_RESOLVED_EVENT_TYPE => {
                    let kind = match ev.content.get("action").and_then(|v| v.as_str()) {
                        Some("retire_older") => ResolutionKind::RetireOlder,
                        Some("retire_newer") => ResolutionKind::RetireNewer,
                        _ => continue, // malformed conflict_resolved — ignore
                    };
                    ResolutionRecord {
                        kind,
                        retired_event_id: ev
                            .content
                            .get("retired_event_id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    }
                }
                t if t == crate::graph::COEXIST_ALLOWED_EVENT_TYPE => {
                    ResolutionRecord { kind: ResolutionKind::KeepBoth, retired_event_id: None }
                }
                _ => ResolutionRecord { kind: ResolutionKind::Dismiss, retired_event_id: None },
            };
            out.insert(pid.to_string(), record);
        }
        Ok(out)
    }

    /// Resolve a detected `conflict_proposal` (spec §2.1). Deterministic, no LLM, no egress. Owns its
    /// idempotency via the ALL-proposals guard (a retire withdraws the proposal from the OPEN set, so the
    /// guard must NOT key off open membership — MAJOR-1). The retire actions retire the FROZEN loser
    /// (`RetireOlder`=a_ref, `RetireNewer`=b_ref — detection fixed older→a_ref at `log.rs:6453`; NO ts
    /// recompute here, since a passage's ts tracks its session head, which a re-capture can flip). A
    /// torn-write retry (loser already in the retired set, no `conflict_resolved`) rolls forward: append
    /// the missing marker, return `NoOp` — never re-call the fail-loud primitive (`Err("already retired")`,
    /// `log.rs:5214`/`:5130`). `#[cfg(unix)]`.
    #[cfg(unix)]
    pub fn resolve_conflict(
        &self,
        proposal_id: &str,
        action: ResolveAction,
    ) -> Result<ResolveOutcome, BossclawError> {
        use crate::index::ConflictRef;
        // (1) Load refs by id (open-ness independent) — unknown id ⇒ error.
        let Some((a_ref, b_ref)) = self.conflict_proposal_by_id(proposal_id)? else {
            return Err(BossclawError::InvalidInput(format!("unknown conflict proposal {proposal_id}")));
        };
        let want = action_kind(action);
        // (2) Terminal-state guard over ALL resolution markers.
        if let Some(existing) = self.resolution_markers()?.get(proposal_id) {
            if existing.kind == want {
                return Ok(ResolveOutcome::NoOp); // idempotent same-action repeat
            }
            return Err(BossclawError::InvalidInput(format!(
                "conflict proposal {proposal_id} is already resolved"
            )));
        }
        // (3) No terminal marker yet — apply.
        match action {
            ResolveAction::RetireOlder | ResolveAction::RetireNewer => {
                let loser = if matches!(action, ResolveAction::RetireOlder) { &a_ref } else { &b_ref };
                let retired_event_id = retired_id_of(loser);
                // Roll-forward gate: retired-SET membership (regardless of who retired it — §3.4). If the
                // frozen loser is ALREADY retired, the primitive would fail loud → append the missing
                // conflict_resolved instead and return no-op success.
                let fold = fold_sessions(&self.session_events_ordered()?);
                let already_retired = match loser {
                    ConflictRef::Note { event_id } => fold.retired_notes.contains(event_id),
                    ConflictRef::Passage { session_id, passage_id } => {
                        fold.retired_passages.contains(&(session_id.clone(), *passage_id))
                    }
                };
                if !already_retired {
                    // Retire the frozen loser with conflict provenance (§2.1 MAJOR-2), written FIRST.
                    match loser {
                        ConflictRef::Note { event_id } => {
                            self.retire_memory(event_id, Some(proposal_id))?;
                        }
                        ConflictRef::Passage { session_id, passage_id } => {
                            self.retire_passage(session_id, *passage_id, Some(proposal_id))?;
                        }
                    }
                }
                let marker_id = self.append_conflict_resolved(proposal_id, want, &retired_event_id)?;
                // A fresh retire → Applied; a roll-forward (loser was already retired) → NoOp success.
                if already_retired {
                    Ok(ResolveOutcome::NoOp)
                } else {
                    Ok(ResolveOutcome::Applied(marker_id))
                }
            }
            ResolveAction::KeepBoth => {
                let id = self.append_pair_terminal(
                    crate::graph::COEXIST_ALLOWED_EVENT_TYPE, proposal_id, &a_ref, &b_ref, None,
                )?;
                Ok(ResolveOutcome::Applied(id))
            }
            ResolveAction::Dismiss => {
                // Record the current head of every referenced session so the dismissal lapses on
                // re-capture (§3.1). Notes contribute no head.
                let fold = fold_sessions(&self.session_events_ordered()?);
                let head_of: std::collections::HashMap<&str, &str> =
                    fold.current.iter().map(|cs| (cs.session_id.as_str(), cs.event_id.as_str())).collect();
                let mut heads = serde_json::Map::new();
                for r in [&a_ref, &b_ref] {
                    if let ConflictRef::Passage { session_id, .. } = r {
                        if let Some(h) = head_of.get(session_id.as_str()) {
                            heads.insert(session_id.clone(), serde_json::Value::String((*h).to_string()));
                        }
                    }
                }
                let id = self.append_pair_terminal(
                    crate::graph::DISMISSED_EVENT_TYPE, proposal_id, &a_ref, &b_ref,
                    Some(serde_json::Value::Object(heads)),
                )?;
                Ok(ResolveOutcome::Applied(id))
            }
        }
    }

    /// Append a `conflict_resolved{proposal_id, action, retired_event_id}` terminal marker (§2.1). Plain
    /// signed `append` (like the retire markers — NOT `build_proposer_event`). Written AFTER the retire
    /// marker (§3.4 ordering). `#[cfg(unix)]`.
    #[cfg(unix)]
    fn append_conflict_resolved(
        &self,
        proposal_id: &str,
        kind: ResolutionKind,
        retired_event_id: &str,
    ) -> Result<String, BossclawError> {
        let action = match kind {
            ResolutionKind::RetireOlder => "retire_older",
            ResolutionKind::RetireNewer => "retire_newer",
            _ => unreachable!("append_conflict_resolved is only for retire kinds"),
        };
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::CONFLICT_RESOLVED_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "proposal_id": proposal_id, "action": action, "retired_event_id": retired_event_id,
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// Append a `coexist_allowed` / `dismissed` PAIR terminal marker with the shared shape
    /// `{proposal_id, pair_key, a_ref, b_ref}` (+ optional `session_heads` for `dismissed`). §2.1.
    /// `#[cfg(unix)]`.
    #[cfg(unix)]
    fn append_pair_terminal(
        &self,
        event_type: &str,
        proposal_id: &str,
        a_ref: &crate::index::ConflictRef,
        b_ref: &crate::index::ConflictRef,
        session_heads: Option<serde_json::Value>,
    ) -> Result<String, BossclawError> {
        let mut content = serde_json::Map::new();
        content.insert("proposal_id".to_string(), serde_json::Value::String(proposal_id.to_string()));
        content.insert(
            "pair_key".to_string(),
            serde_json::Value::String(crate::index::ConflictRef::unordered_pair_key(a_ref, b_ref)),
        );
        content.insert("a_ref".to_string(), a_ref.to_json());
        content.insert("b_ref".to_string(), b_ref.to_json());
        if let Some(h) = session_heads {
            content.insert("session_heads".to_string(), h);
        }
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: event_type.to_string(),
            content: serde_json::Value::Object(content),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// Append a signed Tier-B `write_rejected` stamped with the M6b reconciler producer.
    /// Thin wrapper over [`Self::append_write_rejected_with`].
    #[cfg(unix)]
    pub fn append_write_rejected(
        &self, target_canonical: Option<&str>, reason: &str,
        inducing_key: &serde_json::Value, source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        self.append_write_rejected_with(
            target_canonical, reason, inducing_key, source_event_ids,
            crate::graph::M6B_PROPOSER_PRODUCER,
        )
    }

    /// Append a signed Tier-B `write_rejected` (emitted INSTEAD of a proposal on synthesis/
    /// gate failure; a terminal audit marker — never resolves a proposal), stamped with
    /// `producer` as its `model_meta.model_id`.
    #[cfg(unix)]
    pub fn append_write_rejected_with(
        &self, target_canonical: Option<&str>, reason: &str,
        inducing_key: &serde_json::Value, source_event_ids: &[String], producer: &str,
    ) -> Result<String, BossclawError> {
        let content = serde_json::json!({ "target": target_canonical, "reason": reason, "inducing_key": inducing_key });
        self.append(self.build_proposer_event(producer, crate::graph::WRITE_REJECTED_EVENT_TYPE, content, source_event_ids))
    }

    /// App-facing: a human declined a proposal. Appends an M6b-reconciler-stamped
    /// `write_declined` that RESOLVES it. Thin wrapper over [`Self::decline_write_proposal_with`].
    #[cfg(unix)]
    pub fn decline_write_proposal(&self, proposal_id: &str, reason: &str) -> Result<String, BossclawError> {
        self.decline_write_proposal_with(proposal_id, reason, crate::graph::M6B_PROPOSER_PRODUCER)
    }

    /// App-facing: a human declined a proposal. Appends a `write_declined` that RESOLVES it,
    /// inheriting the proposal's lineage and stamped with `producer` as its `model_meta.model_id`.
    #[cfg(unix)]
    pub fn decline_write_proposal_with(&self, proposal_id: &str, reason: &str, producer: &str) -> Result<String, BossclawError> {
        let sources = self.source_ids_of_event(proposal_id)?.unwrap_or_default();
        if sources.is_empty() {
            return Err(BossclawError::InvalidInput("unknown or non-Tier-B proposal id".into()));
        }
        let content = serde_json::json!({ "resolves_proposal": proposal_id, "reason": reason });
        self.append(self.build_proposer_event(producer, crate::graph::WRITE_DECLINED_EVENT_TYPE, content, &sources))
    }

    /// Idempotency (§5.7): suppress a new proposal for (canonical_path, inducing_key) if
    /// EITHER an OPEN write_proposal exists for it OR a write_rejected was recorded for it.
    /// A write_proposal is OPEN until a later file_written/write_declined carries
    /// resolves_proposal == its id. Engine write_rejected never resolves a proposal.
    /// inducing_key is the RESOLVED (entity-id, relation, entity-id) — never surface forms.
    ///
    /// O(n) fold over the (low-volume) actuator events — acceptable for v1; a dedicated
    /// projection table is a future optimization.
    #[cfg(unix)]
    pub fn is_proposal_suppressed(
        &self,
        canonical_path: &str,
        inducing_key: &serde_json::Value,
    ) -> Result<bool, BossclawError> {
        let mut open_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut resolved: std::collections::HashSet<String> = std::collections::HashSet::new();
        for ev in self.events_of_types(&[
            crate::graph::WRITE_PROPOSAL_EVENT_TYPE,
            crate::graph::WRITE_REJECTED_EVENT_TYPE,
            crate::graph::WRITE_DECLINED_EVENT_TYPE,
            crate::graph::FILE_WRITTEN_EVENT_TYPE,
        ])? {
            match ev.event_type.as_str() {
                t if t == crate::graph::WRITE_PROPOSAL_EVENT_TYPE => {
                    if ev.content.get("target").and_then(|v| v.as_str()) == Some(canonical_path)
                        && ev.content.get("inducing_key") == Some(inducing_key)
                    {
                        open_ids.insert(ev.id.clone());
                    }
                }
                t if t == crate::graph::WRITE_REJECTED_EVENT_TYPE => {
                    // By design TERMINAL for this resolved (path,key): a write_rejected
                    // permanently suppresses re-attempts (never resolved, never re-opened).
                    // So T7 MUST emit write_rejected ONLY for genuine synthesis/gate
                    // failures — never for cap-elision or off-switch deferrals, which must
                    // stay retryable on a later tick.
                    if ev.content.get("target").and_then(|v| v.as_str()) == Some(canonical_path)
                        && ev.content.get("inducing_key") == Some(inducing_key)
                    {
                        return Ok(true);
                    }
                }
                _ => {
                    if let Some(rid) = ev.content.get("resolves_proposal").and_then(|v| v.as_str()) {
                        resolved.insert(rid.to_string());
                    }
                }
            }
        }
        Ok(open_ids.iter().any(|id| !resolved.contains(id)))
    }

    /// Every OPEN `write_proposal`: emitted, not yet resolved by a `file_written`/`write_declined`,
    /// and whose `(target, inducing_key)` is not terminally `write_rejected`. Oldest first
    /// (`events_of_types` returns `seq ASC`). The desktop Review queue source. `#[cfg(unix)]` (M1).
    #[cfg(unix)]
    pub fn pending_proposals(&self) -> Result<Vec<PendingProposal>, BossclawError> {
        use std::collections::{HashMap, HashSet};
        // proposal id → parsed row, in emission order.
        let mut open: Vec<PendingProposal> = Vec::new();
        let mut resolved: HashSet<String> = HashSet::new();
        // (target, inducing_key.to_string()) terminally rejected.
        let mut rejected_keys: HashSet<(String, String)> = HashSet::new();
        let mut proposal_keys: HashMap<String, (String, String)> = HashMap::new();

        for ev in self.events_of_types(&[
            crate::graph::WRITE_PROPOSAL_EVENT_TYPE,
            crate::graph::WRITE_REJECTED_EVENT_TYPE,
            crate::graph::WRITE_DECLINED_EVENT_TYPE,
            crate::graph::FILE_WRITTEN_EVENT_TYPE,
        ])? {
            match ev.event_type.as_str() {
                t if t == crate::graph::WRITE_PROPOSAL_EVENT_TYPE => {
                    // Read ALL fields via borrows of `ev.content` / `ev.model_meta` FIRST, then
                    // build the struct — never move `ev.model_meta` before reading `ev.content`
                    // (that would be a move-after-use, E0382).
                    let id = ev.id.clone();
                    let target = ev.content.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let op = ev.content.get("op").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let new_content_hash = ev.content.get("new_content_hash").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let rationale = ev.content.get("rationale").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let inducing_key = ev.content.get("inducing_key").cloned().unwrap_or(serde_json::Value::Null);
                    let verdict_summary = ev.content.get("verdict_summary").cloned().unwrap_or(serde_json::Value::Null);
                    let base_content_hash = verdict_summary.get("base_content_hash")
                        .and_then(|v| v.as_str()).map(|s| s.to_string());
                    let source_event_ids = ev.model_meta.as_ref()
                        .map(|m| m.source_event_ids.clone()).unwrap_or_default();
                    let producer = ev.model_meta.as_ref()
                        .map(|m| m.model_id.clone()).unwrap_or_default();
                    proposal_keys.insert(id.clone(), (target.clone(), inducing_key.to_string()));
                    open.push(PendingProposal {
                        id, target, op, new_content_hash, rationale,
                        inducing_key, source_event_ids, producer, base_content_hash, verdict_summary,
                    });
                }
                t if t == crate::graph::WRITE_REJECTED_EVENT_TYPE => {
                    let target = ev.content.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let key = ev.content.get("inducing_key").cloned().unwrap_or(serde_json::Value::Null);
                    rejected_keys.insert((target, key.to_string()));
                }
                _ => {
                    if let Some(rid) = ev.content.get("resolves_proposal").and_then(|v| v.as_str()) {
                        resolved.insert(rid.to_string());
                    }
                }
            }
        }

        Ok(open
            .into_iter()
            .filter(|p| {
                !resolved.contains(&p.id)
                    && match proposal_keys.get(&p.id) {
                        Some(k) => !rejected_keys.contains(k),
                        None => true,
                    }
            })
            .collect())
    }

    /// M6c mandate idempotency (spec §5.4) — decline-STICKY suppression keyed on the
    /// source-state `inducing_key` (`{mandate, target, sources_hash}`). A SMALL DELTA on
    /// [`Self::is_proposal_suppressed`]: it shares every rule EXCEPT one. For
    /// `(canonical_path, inducing_key)` it returns `true` when ANY of:
    /// - a `write_rejected` matches it — TERMINAL suppress (genuine failure), SAME as M6b;
    /// - an OPEN `write_proposal` matches it (no resolver yet) — don't double-ask, SAME as M6b;
    /// - a matching `write_proposal` was resolved by a **`write_declined`** — THE NEW RULE:
    ///   a declined sync is sticky for that source-state, so the engine won't re-nag every
    ///   tick while the file is still out of sync (M6b's predicate would re-fire here).
    ///
    /// A matching `write_proposal` resolved ONLY by a `file_written` (the human accepted)
    /// does NOT suppress — a later legitimate drift re-syncs via convergence/compare-vs-disk
    /// (Task 9). Cap-elision and the off-switch emit no event, so they stay retryable.
    /// Because suppression is keyed on `sources_hash`, a NEW source-state is a fresh key and
    /// is never suppressed by a prior state's decline.
    ///
    /// `is_proposal_suppressed` (M6b) is intentionally left UNCHANGED — only the
    /// declined-also-suppresses rule differs, captured here by tracking decline-resolvers
    /// separately from accept-resolvers (`file_written`).
    ///
    /// O(n) fold over the (low-volume) actuator events — same posture as the M6b predicate.
    #[cfg(unix)]
    pub fn is_mandate_proposal_suppressed(
        &self,
        canonical_path: &str,
        inducing_key: &serde_json::Value,
    ) -> Result<bool, BossclawError> {
        // Proposals matching (path, key); the set of ids resolved by a DECLINE (the only
        // resolver that suppresses under M6c). A `file_written` resolver is deliberately NOT
        // collected, so an accepted-then-resolved proposal is treated as closed-and-retryable.
        let mut matching_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut declined: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut resolved: std::collections::HashSet<String> = std::collections::HashSet::new();
        for ev in self.events_of_types(&[
            crate::graph::WRITE_PROPOSAL_EVENT_TYPE,
            crate::graph::WRITE_REJECTED_EVENT_TYPE,
            crate::graph::WRITE_DECLINED_EVENT_TYPE,
            crate::graph::FILE_WRITTEN_EVENT_TYPE,
        ])? {
            match ev.event_type.as_str() {
                t if t == crate::graph::WRITE_PROPOSAL_EVENT_TYPE => {
                    if ev.content.get("target").and_then(|v| v.as_str()) == Some(canonical_path)
                        && ev.content.get("inducing_key") == Some(inducing_key)
                    {
                        matching_ids.insert(ev.id.clone());
                    }
                }
                t if t == crate::graph::WRITE_REJECTED_EVENT_TYPE => {
                    // TERMINAL for this (path,key): a genuine synthesis/gate failure
                    // permanently suppresses — SAME as M6b.
                    if ev.content.get("target").and_then(|v| v.as_str()) == Some(canonical_path)
                        && ev.content.get("inducing_key") == Some(inducing_key)
                    {
                        return Ok(true);
                    }
                }
                t if t == crate::graph::WRITE_DECLINED_EVENT_TYPE => {
                    if let Some(rid) = ev.content.get("resolves_proposal").and_then(|v| v.as_str()) {
                        declined.insert(rid.to_string());
                        resolved.insert(rid.to_string());
                    }
                }
                // file_written: closes the proposal (no longer OPEN) but does NOT suppress —
                // the human accepted, so a later drift may legitimately re-sync.
                _ => {
                    if let Some(rid) = ev.content.get("resolves_proposal").and_then(|v| v.as_str()) {
                        resolved.insert(rid.to_string());
                    }
                }
            }
        }
        // Suppress a matching proposal that is either still OPEN (no resolver) OR was closed
        // by a decline. A proposal closed ONLY by a file_written is neither and does not suppress.
        Ok(matching_ids
            .iter()
            .any(|id| !resolved.contains(id) || declined.contains(id)))
    }

    /// Store the proposed corrected bytes for a `write_proposal`, keyed by its event id.
    /// The bytes live in the SQLCipher `Store` (encrypted at rest) because they are model
    /// output over untrusted input — NOT in the signed event, which records only
    /// `new_content_hash`. `INSERT OR REPLACE` so a re-proposal at the same id overwrites.
    ///
    /// SECURITY: this row is an audit/worklist CACHE, never an authorization source. It is
    /// validated against the signed-event hash at read time
    /// ([`Self::get_proposal_bytes_checked`]) and re-gated through the full M6a path at
    /// confirm — a tampered row fails closed BEFORE any write.
    #[cfg(unix)]
    pub fn put_proposal_bytes(
        &self,
        proposal_id: &str,
        content: &[u8],
        content_hash: &str,
    ) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        conn.execute(
            "INSERT OR REPLACE INTO proposal_bytes
               (proposal_id, content, content_hash, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![proposal_id, content, content_hash, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Read back the proposed bytes and verify they STILL hash to `expected_hash` (the
    /// hash the signed `write_proposal` recorded). The table NEVER authorizes; it only
    /// caches for preview, so this fails closed unless BOTH hold:
    /// - the freshly recomputed hash equals the row's stored `content_hash` (the row was
    ///   not tampered after it was written), AND
    /// - that hash equals `expected_hash` (the row matches the signed event).
    ///
    /// The recompute uses the SAME hasher the engine uses for `file_written.content_hash`
    /// (`hex::encode(Sha256::digest(..))`), so the comparison is against the canonical
    /// content hash, not a second scheme. A missing row, a tampered row, or a row that no
    /// longer matches the signed event all return `Err` — the caller never sees bytes it
    /// cannot vouch for.
    #[cfg(unix)]
    pub fn get_proposal_bytes_checked(
        &self,
        proposal_id: &str,
        expected_hash: &str,
    ) -> Result<Vec<u8>, BossclawError> {
        let fail =
            |why: &str| -> BossclawError { BossclawError::InvalidInput(format!("proposal_bytes fail-closed: {why}")) };

        let (content, stored_hash): (Vec<u8>, String) = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.query_row(
                "SELECT content, content_hash FROM proposal_bytes WHERE proposal_id = ?1",
                rusqlite::params![proposal_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| fail("no cached bytes for this proposal id (unknown or GC'd)"))?
        };

        // Recompute with the engine's canonical content hasher and gate on BOTH equalities.
        let actual = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(&content))
        };
        if actual != stored_hash {
            return Err(fail("cached bytes no longer match their stored content_hash (row tampered)"));
        }
        if actual != expected_hash {
            return Err(fail("cached bytes do not match the signed proposal's recorded hash"));
        }
        Ok(content)
    }

    /// Store synthesized expected file bytes for a mandate at one source-state, then evict
    /// the mandate's prior source-states (finding F — bounded growth: keep only the current
    /// source-state's bytes). `synth_lineage` is the EXACT engine-gathered source ids read
    /// at synthesis time; it is persisted ALONGSIDE the bytes (finding B) so a later cache
    /// HIT can union it with the then-current in-scope sources without ever dropping a
    /// tainted source that left scope. `INSERT OR REPLACE` so a re-synthesis at the same
    /// `(mandate, sources_hash)` overwrites. The lineage is JSON-encoded into a BLOB column.
    ///
    /// SECURITY: this row is a convergence/efficiency CACHE, never an authorization source —
    /// the confirm path re-gates the bytes through the full M6a path. It lives in the
    /// SQLCipher `Store` (encrypted at rest) because the bytes are derived from
    /// possibly-sensitive source files, exactly like [`Self::put_proposal_bytes`].
    #[cfg(unix)]
    pub fn put_synthesis_cache(
        &self,
        mandate_id: &str,
        sources_hash: &str,
        bytes: &[u8],
        expected_hash: &str,
        synth_lineage: &[String],
    ) -> Result<(), BossclawError> {
        let lineage_blob = serde_json::to_vec(synth_lineage)?;
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        conn.execute(
            "INSERT OR REPLACE INTO mandate_synthesis_cache
               (mandate_grant_id, sources_hash, expected_hash, expected_bytes,
                source_event_ids_at_synth, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                mandate_id,
                sources_hash,
                expected_hash,
                bytes,
                lineage_blob,
                Utc::now().to_rfc3339()
            ],
        )?;
        // Finding F — evict prior source-states for this mandate so cache growth stays
        // bounded to the current source-state's single row.
        conn.execute(
            "DELETE FROM mandate_synthesis_cache
             WHERE mandate_grant_id = ?1 AND sources_hash <> ?2",
            rusqlite::params![mandate_id, sources_hash],
        )?;
        Ok(())
    }

    /// Read back a cached synthesis row for `(mandate, sources_hash)`, or `None` on a miss.
    /// Returns the bytes, their `expected_hash`, and the synth-time lineage decoded from the
    /// JSON BLOB. Task 9's confirm path unions `source_event_ids_at_synth` with the
    /// then-current in-scope sources (finding B) and re-gates the bytes — this method only
    /// returns the stored row; it NEVER authorizes.
    #[cfg(unix)]
    pub fn get_synthesis_cache(
        &self,
        mandate_id: &str,
        sources_hash: &str,
    ) -> Result<Option<SynthCacheRow>, BossclawError> {
        let row: Option<(Vec<u8>, String, Vec<u8>)> = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.query_row(
                "SELECT expected_bytes, expected_hash, source_event_ids_at_synth
                 FROM mandate_synthesis_cache
                 WHERE mandate_grant_id = ?1 AND sources_hash = ?2",
                rusqlite::params![mandate_id, sources_hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
        };
        match row {
            None => Ok(None),
            Some((expected_bytes, expected_hash, lineage_blob)) => {
                let source_event_ids_at_synth: Vec<String> = serde_json::from_slice(&lineage_blob)?;
                Ok(Some(SynthCacheRow {
                    expected_bytes,
                    expected_hash,
                    source_event_ids_at_synth,
                }))
            }
        }
    }

    /// Shared proposer Tier-B event builder. `producer` is stamped as `model_meta.model_id`
    /// (the M6b reconciler and the M6c mandate proposer pass DIFFERENT producers — Task 9's
    /// per-mandate cap distinguishes them by this stamp). Lineage is engine-gathered; the
    /// event shape is otherwise identical regardless of producer.
    #[cfg(unix)]
    fn build_proposer_event(&self, producer: &str, event_type: &str, content: serde_json::Value, source_event_ids: &[String]) -> crate::event::Event {
        crate::event::Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: event_type.to_string(), content,
            model_meta: Some(crate::event::ModelMeta {
                model_id: producer.to_string(),
                prompt_hash: String::new(), source_event_ids: source_event_ids.to_vec(),
            }),
            prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        }
    }

    /// Every CURRENT page (one per topic), `ORDER BY topic_id ASC`. Tier-A read.
    pub fn current_pages(&self) -> Result<Vec<crate::graph::Page>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT topic_id, page_event_id, title, text FROM pages ORDER BY topic_id ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok(crate::graph::Page {
            topic_id: r.get(0)?, page_event_id: r.get(1)?, title: r.get(2)?, text: r.get(3)?,
        }))?;
        let mut out = Vec::new();
        for row in rows { out.push(row?); }
        Ok(out)
    }

    /// Grant read-access to a folder (M5a). Canonicalizes `path`, appends a
    /// ground-truth `grant` event, and refreshes the grants projection. Returns the
    /// event id. Canonicalization fails closed if the path does not exist.
    pub fn add_grant(&self, path: &std::path::Path) -> Result<String, BossclawError> {
        let canonical = std::fs::canonicalize(path)
            .map_err(|e| BossclawError::InvalidInput(format!("grant path not resolvable: {e}")))?;
        let root = canonical.to_string_lossy().to_string();
        let id = self.append(Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::GRANT_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "canonical_root": root }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        })?;
        self.rebuild_graph()?;
        Ok(id)
    }

    /// Revoke a previously-granted folder (M5a). Canonicalizes `path`, appends a
    /// ground-truth `revoke` event, and refreshes the grants projection. Ingested
    /// files under a revoked root stay in the log but are excluded from recall.
    pub fn revoke_grant(&self, path: &std::path::Path) -> Result<String, BossclawError> {
        let canonical = std::fs::canonicalize(path)
            .map_err(|e| BossclawError::InvalidInput(format!("revoke path not resolvable: {e}")))?;
        let root = canonical.to_string_lossy().to_string();
        let id = self.append(Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::REVOKE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "canonical_root": root }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        })?;
        self.rebuild_graph()?;
        Ok(id)
    }

    /// Every grant (active and revoked), `ORDER BY canonical_root ASC`. Tier-A read.
    pub fn grants(&self) -> Result<Vec<crate::graph::Grant>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT canonical_root, granted_at, revoked FROM grants ORDER BY canonical_root ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok(crate::graph::Grant {
            canonical_root: r.get(0)?, granted_at: r.get(1)?, revoked: r.get::<_, i64>(2)? != 0,
        }))?;
        let mut out = Vec::new();
        for row in rows { out.push(row?); }
        Ok(out)
    }

    /// Grant WRITE-access to a folder (M6a). Canonicalizes `path`, appends a
    /// ground-truth `write_grant` event, and refreshes the write-grants projection.
    /// Returns the event id. Canonicalization fails closed if the path does not exist.
    /// Independent of [`add_grant`](Self::add_grant): a write grant is a distinct
    /// event type and projection, so granting write never grants read (or vice-versa).
    pub fn add_write_grant(&self, path: &std::path::Path) -> Result<String, BossclawError> {
        let canonical = std::fs::canonicalize(path)
            .map_err(|e| BossclawError::InvalidInput(format!("write-grant path not resolvable: {e}")))?;
        let root = canonical.to_string_lossy().to_string();
        let id = self.append(Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::WRITE_GRANT_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "canonical_root": root }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        })?;
        self.rebuild_graph()?;
        Ok(id)
    }

    /// Revoke a previously-WRITE-granted folder (M6a). Canonicalizes `path`, appends a
    /// ground-truth `write_revoke` event, and refreshes the write-grants projection.
    /// Returns the event id. Mirrors [`revoke_grant`](Self::revoke_grant) on the write side.
    pub fn revoke_write_grant(&self, path: &std::path::Path) -> Result<String, BossclawError> {
        let canonical = std::fs::canonicalize(path)
            .map_err(|e| BossclawError::InvalidInput(format!("write-revoke path not resolvable: {e}")))?;
        let root = canonical.to_string_lossy().to_string();
        let id = self.append(Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::WRITE_REVOKE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "canonical_root": root }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        })?;
        self.rebuild_graph()?;
        Ok(id)
    }

    /// Every write-grant (active and revoked), `ORDER BY canonical_root ASC`. Tier-A
    /// read. The write-side sibling of [`grants`](Self::grants); reads a separate table.
    pub fn write_grants(&self) -> Result<Vec<crate::graph::WriteGrant>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT canonical_root, granted_at, revoked FROM write_grants ORDER BY canonical_root ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok(crate::graph::WriteGrant {
            canonical_root: r.get(0)?, granted_at: r.get(1)?, revoked: r.get::<_, i64>(2)? != 0,
        }))?;
        let mut out = Vec::new();
        for row in rows { out.push(row?); }
        Ok(out)
    }

    /// Grant a standing **mandate** (M6c §4.1/§4.2): a signed, bounded goal to keep
    /// `target` == `recipe(sources under source_scope)`. Appends a ground-truth
    /// `mandate_grant` event and refreshes the mandates projection; the returned event
    /// id IS the mandate's identity. Mirrors [`add_write_grant`](Self::add_write_grant)
    /// but with the load-bearing grant-time guards.
    ///
    /// Guards, IN ORDER (each rejects — never silently truncates or downgrades):
    /// 1. **Recipe cap (Finding D).** `recipe.len() > MAX_RECIPE_LEN` ⇒ reject, so the
    ///    signed recipe and the later prompt's recipe can never disagree.
    /// 2. **Canonicalize.** `source_scope` must resolve (it must exist); `target` is
    ///    canonicalized via its PARENT when it does not exist yet (a Create), reusing
    ///    `propose_write`'s logic so all later `starts_with` checks are on real paths.
    /// 3. **UX guard.** `target` must be under an active WRITE grant
    ///    ([`is_write_allowed`](Self::is_write_allowed)) — you cannot create a mandate
    ///    the brain could never act on. (Convenience only; the security boundary is
    ///    `propose_write`'s re-gate at propose/execute time, §4.3 #1.)
    /// 4. **LOAD-BEARING self-loop guard (Finding A).** The canonical `target` MUST be
    ///    OUTSIDE every active READ-grant root, tested with **segment-aware**
    ///    [`Path::starts_with`] on canonical paths (so `/a/b` never matches `/a/bc`).
    ///    This structurally guarantees the engine's own confirmed write to `target` can
    ///    never fire a source watcher event nor be re-ingested as a source — so the
    ///    recipe can never fold its own output back into its input (convergence holds
    ///    unconditionally, §6.4 #1).
    pub fn add_mandate(
        &self,
        target: &std::path::Path,
        source_scope: &std::path::Path,
        recipe: &str,
    ) -> Result<String, BossclawError> {
        // 1. Recipe cap (Finding D) — reject, never truncate.
        if recipe.len() > crate::graph::MAX_RECIPE_LEN {
            return Err(BossclawError::InvalidInput("recipe too long".into()));
        }
        // 2. Canonicalize source_scope (must exist) and target (else its parent — Create).
        let canon_scope = std::fs::canonicalize(source_scope).map_err(|e| {
            BossclawError::InvalidInput(format!("mandate source_scope not resolvable: {e}"))
        })?;
        let canon_target = canonicalize_target_or_parent(target).ok_or_else(|| {
            BossclawError::InvalidInput("mandate target path is not resolvable".into())
        })?;
        // 3. UX guard: target must be under an active WRITE grant.
        if !self.is_write_allowed(target)? {
            return Err(BossclawError::InvalidInput("target not write-granted".into()));
        }
        // 4. Finding A — early-reject a target inside any active READ-grant root. This is
        //    a TIGHT grant-time guard (defense in depth), NOT the unconditional convergence
        //    proof: the ultimate enforcer is execute-time `O_NOFOLLOW` + canonical-root-
        //    anchored ingest (a confirmed write follows no symlink, and ingest only adopts
        //    files whose canonical path is under a read root). Here we reject as early as
        //    possible so a misconfigured mandate never even reaches propose-time.
        //    LEAF-TIGHT: resolve an EXISTING target's final component first
        //    (`std::fs::canonicalize` follows a leaf symlink), so a symlink AT the leaf that
        //    points INTO a read root is caught — `canon_target` alone joins the raw leaf
        //    NOFOLLOW and would miss it. A not-yet-existing Create target fails canonicalize
        //    → fall back to the parent+leaf form (unchanged). The scan stays segment-aware
        //    (`Path::starts_with` on `Path`s — `/a/b` never matches `/a/bc`), active grants only.
        let resolved_target = std::fs::canonicalize(target)
            .ok()
            .or_else(|| canonicalize_target_or_parent(target))
            .ok_or_else(|| {
                BossclawError::InvalidInput("mandate target path is not resolvable".into())
            })?;
        for g in self.grants()? {
            if !g.revoked
                && resolved_target.starts_with(std::path::Path::new(&g.canonical_root))
            {
                return Err(BossclawError::InvalidInput(
                    "mandate target must be outside every read-grant root".into(),
                ));
            }
        }
        // 5. Append the ground-truth `mandate_grant` event (Tier-A, model_meta: None).
        //    The append chokepoint mints the id = the mandate identity (§4.1). Paths are
        //    stored canonical so all later `starts_with` checks are on real paths.
        let id = self.append(Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::MANDATE_GRANT_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "target": canon_target.to_string_lossy().to_string(),
                "source_scope": canon_scope.to_string_lossy().to_string(),
                "recipe": recipe,
            }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        })?;
        self.rebuild_graph()?;
        Ok(id)
    }

    /// Revoke a mandate by its `mandate_grant_id` (M6c §4.1). Appends a ground-truth
    /// `mandate_revoke` event referencing the grant id and refreshes the projection.
    /// **Sticky + fail-closed:** the fold only ever sets `revoked=1` (there is no
    /// un-revoke), and a revoke of an unknown id is harmlessly ignored by the fold —
    /// mirroring `write_revoke`. Returns the revoke event id.
    pub fn revoke_mandate(&self, mandate_grant_id: &str) -> Result<String, BossclawError> {
        let id = self.append(Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::MANDATE_REVOKE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "mandate_grant_id": mandate_grant_id }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        })?;
        self.rebuild_graph()?;
        // Purge all of this mandate's synthesis-cache rows so the cache cannot outlive the
        // mandate under a live watcher (Task 7). A revoke of an unknown id is a harmless
        // no-op DELETE — mirroring the fold, which ignores a revoke with no grant.
        {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.execute(
                "DELETE FROM mandate_synthesis_cache WHERE mandate_grant_id = ?1",
                rusqlite::params![mandate_grant_id],
            )?;
        }
        Ok(id)
    }

    /// Every ACTIVE (un-revoked) mandate, `ORDER BY granted_at ASC`. Tier-A read; the
    /// mandate sibling of [`grants`](Self::grants)/[`write_grants`](Self::write_grants).
    pub fn active_mandates(&self) -> Result<Vec<crate::graph::Mandate>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT mandate_grant_id, target, source_scope, recipe, granted_at, revoked
             FROM mandates WHERE revoked = 0 ORDER BY granted_at ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok(crate::graph::Mandate {
            mandate_grant_id: r.get(0)?,
            target: r.get(1)?,
            source_scope: r.get(2)?,
            recipe: r.get(3)?,
            granted_at: r.get(4)?,
            revoked: r.get::<_, i64>(5)? != 0,
        }))?;
        let mut out = Vec::new();
        for row in rows { out.push(row?); }
        Ok(out)
    }

    /// Is `path` authorized for WRITING under some ACTIVE write-grant? (M6a, T1).
    ///
    /// Canonicalizes the candidate's REAL path and requires **path-segment descent**
    /// from an active (`!revoked`) `write_grant` root: the canonical target must EQUAL
    /// a root or be a descendant of it **by whole path components** (via segment-aware
    /// [`Path::starts_with`]), so `/a/bc` is NOT under `/a/b`. For a not-yet-existing
    /// target (a Create), `std::fs::canonicalize` would error — so the PARENT directory
    /// is canonicalized instead and tested for membership.
    ///
    /// **Advisory only.** This is a string-segment check on a canonical path; it is
    /// WEAKER than the read side's fd-walk, which defends against an intermediate
    /// symlink being swapped between the check and the open (a TOCTOU race). The real
    /// write boundary is the execute-time fd-relative open built in a later M6a task;
    /// this predicate is a fast pre-filter, not the enforcement point.
    pub fn is_write_allowed(&self, path: &std::path::Path) -> Result<bool, BossclawError> {
        // Canonicalize the target's real path; if it does not exist yet (Create),
        // canonicalize the parent and test the parent for membership instead.
        let canonical = match std::fs::canonicalize(path) {
            Ok(c) => c,
            Err(_) => {
                let parent = path.parent().ok_or_else(|| {
                    BossclawError::InvalidInput("write target has no parent to resolve".into())
                })?;
                std::fs::canonicalize(parent).map_err(|e| {
                    BossclawError::InvalidInput(format!("write target parent not resolvable: {e}"))
                })?
            }
        };
        // Membership: descendant-by-path-components of any ACTIVE write-grant root.
        // `Path::starts_with` is segment-aware (it compares whole components), so a
        // mere string-prefix sibling like `/a/b-evil` does not match root `/a/b`.
        for g in self.write_grants()? {
            if g.revoked {
                continue;
            }
            if canonical.starts_with(std::path::Path::new(&g.canonical_root)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Build a [`Provenance`] record from a resolved cited source event, noting
    /// whether it carries the external taint stamp. Pulls `origin_path` +
    /// `ingested_at` from a `file_ingested` event's provenance block when present
    /// (so the verdict can show "this edit came from ~/x/README.md, ingested …").
    ///
    /// `is_external` here is the O(1) [`crate::ingest::is_external`] stamp read; the
    /// gate's FAIL-CLOSED rule (an UNRESOLVABLE source taints the whole proposal)
    /// is enforced by the caller, NOT here — this only describes a source that DID
    /// resolve (spec L10: `is_external` is not self-fail-closed).
    #[cfg(unix)]
    fn provenance_from_event(ev: &crate::event::Event) -> crate::actuator::Provenance {
        // A `file_ingested` event nests its origin path; other event kinds carry
        // none, so `origin_path` stays `None` for them. `ingested_at` is the EVENT's
        // `ts` (the true time we learned the content), NOT the file's `modified_at`
        // mtime — sourcing it from mtime would display a dishonest lineage time
        // (T2 review). An empty `ts` (an un-appended event in a unit test) → `None`.
        let origin_path = ev
            .content
            .get("provenance")
            .and_then(|p| p.get("canonical_path"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let ingested_at = if ev.ts.is_empty() { None } else { Some(ev.ts.clone()) };
        crate::actuator::Provenance {
            event_id: ev.id.clone(),
            kind: ev.event_type.clone(),
            origin_path,
            ingested_at,
            is_external: crate::ingest::is_external(ev),
        }
    }

    /// The PURE write gate (spec §8) — compute a [`crate::actuator::WriteVerdict`]
    /// for `p` **without mutating the filesystem**. This is the confused-deputy
    /// defense (spec §4): provenance + an engine-anchored, fail-closed taint
    /// verdict + target eligibility + an advisory diff-guard + the concurrency base
    /// hash AND identity. It never writes; execute (T4) re-checks the FS-mutable
    /// facts inside a critical section.
    ///
    /// Gate logic, IN ORDER (each step's rationale is inline):
    /// 1. **Sources** — empty `source_event_ids` is rejected. Each cited source is
    ///    resolved via [`event_by_id`](Self::event_by_id); if **ANY** is
    ///    unresolvable the WHOLE proposal is `Untrusted` (fail-closed OVER THE SET,
    ///    L10 — never `filter_map` the resolvable ones), else a [`Provenance`] is
    ///    built and `is_external` noted.
    /// 2. **Canonicalize** — Edit/Delete canonicalize the target; Create
    ///    canonicalizes the PARENT (the target is absent). Unresolvable ⇒
    ///    `reject_reason`. `allowed = is_write_allowed(target)`.
    /// 3. **op × existence** — Create-of-existing, Edit/Delete-of-absent, and a
    ///    symlink final component each set `reject_reason`.
    /// 4. **Engine-anchored taint (THE D8-FOR-WRITES FIX, L11)** — INDEPENDENTLY of
    ///    the citations, the files projection is consulted: if `target_canonical`
    ///    is a currently-tracked ingested file, its `file_ingested` event is
    ///    external by construction ⇒ `Untrusted`, UNIONED with step 1. A
    ///    confused-deputy caller citing only clean events while editing a tainted
    ///    file is caught HERE, from the target itself.
    /// 5. **Base capture (Edit/Delete)** — the current file is read for
    ///    `base_content_hash` (hex SHA-256) + `base_identity` (`dev,ino,size` via
    ///    `symlink_metadata`). Create ⇒ both `None`.
    /// 6. **Loud modal** — `requires_loud_modal = Untrusted || Delete ||
    ///    diff_flags.any()` (MONOTONIC — the diff-guard can only escalate).
    ///
    /// [`Provenance`]: crate::actuator::Provenance
    #[cfg(unix)]
    pub fn propose_write(
        &self,
        p: crate::actuator::WriteProposal,
    ) -> Result<crate::actuator::GatedProposal, BossclawError> {
        use crate::actuator::{
            classify_op_existence, diff_guard, FileId, GatedProposal, Provenance, Taint,
            WriteOp, WriteVerdict,
        };

        // A verdict accumulator. `reject_reason` short-circuits the meaning of the
        // verdict (the proposal cannot proceed) but we still return a fully-formed
        // verdict so the app can show WHY. Taint starts Clean and only escalates.
        let mut taint = Taint::Clean;
        let mut provenance: Vec<Provenance> = Vec::new();
        let mut reject_reason: Option<String> = None;

        // ── Step 1: sources (fail-closed OVER THE SET — L10) ──────────────────────
        // An empty cite list is a hard reject (a Tier-B write needs lineage, spec
        // §4 key invariant). For a NON-empty list, we resolve EVERY id: a single
        // unresolvable id taints the WHOLE proposal. We deliberately do NOT
        // `filter_map` to the resolvable subset and judge only those — that would
        // let a confused-deputy hide the inducing event behind a bogus id that
        // reads "clean" because it is simply absent.
        // Candidate external sources gathered in Step 1 but NOT escalated yet (SP5 c): a
        // mandate may authorize them against THIS target, which we can only test after Step 2
        // resolves the canonical target. Each is `(event_id, ingested canonical_path)`.
        let mut external_candidates: Vec<(String, Option<String>)> = Vec::new();
        if p.source_event_ids.is_empty() {
            reject_reason.get_or_insert_with(|| "source_event_ids is empty".to_string());
        } else {
            for src in &p.source_event_ids {
                match self.event_by_id(src)? {
                    Some(ev) => {
                        let prov = Self::provenance_from_event(&ev);
                        if prov.is_external {
                            // DEFER escalation: record the candidate + its ingested path. The
                            // `origin_path` on the provenance is the source's canonical path
                            // (from the files projection), the exact stored form we compare
                            // against `m.source_scope` — never re-canonicalize a live path (M2).
                            external_candidates.push((prov.event_id.clone(), prov.origin_path.clone()));
                        }
                        provenance.push(prov);
                    }
                    None => {
                        // Unresolvable cited source ⇒ fail closed over the set (target-independent).
                        taint = Taint::Untrusted;
                    }
                }
            }
        }

        // ── Step 2: canonicalize target (Create ⇒ parent) + eligibility ───────────
        // Reuse the SAME parent-canonicalize logic `is_write_allowed` documents:
        // for Create the target is absent, so we resolve and key off the PARENT
        // (via the shared [`canonicalize_target_or_parent`] helper, also used by
        // `add_mandate` so the two agree). Edit/Delete canonicalize the target itself.
        let canonical: Option<std::path::PathBuf> = match p.op {
            WriteOp::Create => canonicalize_target_or_parent(&p.target),
            WriteOp::Edit | WriteOp::Delete => std::fs::canonicalize(&p.target).ok(),
        };
        if canonical.is_none() {
            reject_reason
                .get_or_insert_with(|| "write target path is not resolvable".to_string());
        }
        // `is_write_allowed` already canonicalizes (target, else parent) internally,
        // so it is correct for all three ops; advisory only (the fd-relative open at
        // execute time is the real boundary, §6 #1). FAIL CLOSED on Err: surface a
        // reject_reason rather than masking the error as a silent `allowed=false`
        // (T2 review) — a swallowed error must never read as a benign deny.
        let allowed = match self.is_write_allowed(&p.target) {
            Ok(a) => a,
            Err(e) => {
                reject_reason
                    .get_or_insert_with(|| format!("write-grant check failed: {e}"));
                false
            }
        };

        // ── Step 1.5: escalate deferred external candidates unless a mandate authorizes them ──
        // (SP5 change c, SECURITY-CRITICAL.) An external cited source does NOT taint iff some
        // ACTIVE mandate `m` has `m.target == canonical_target` AND the source's ingested
        // canonical_path is segment-aware UNDER `m.source_scope`. FAIL-CLOSED ORDERING (M2/L1):
        //   • if the target is unresolvable (`canonical == None`) → escalate EVERY candidate;
        //   • else escalate each candidate UNLESS authorized.
        // `active_mandates()` is read ONCE here, inside this gate evaluation (an in-flight revoke
        // is caught by the apply-time re-gate). Both sides of the containment test are STORED
        // canonical forms (scope canonical-at-grant; source canonical-from-projection) compared
        // with segment-aware `Path::starts_with` — never re-canonicalize a live (symlinkable) path.
        if !external_candidates.is_empty() {
            match &canonical {
                None => {
                    // Unresolvable target ⇒ cannot authorize anything ⇒ taint ALL (never skip).
                    taint = Taint::Untrusted;
                }
                Some(canonical_target) => {
                    let canonical_target_str = canonical_target.to_string_lossy().to_string();
                    let mandates = self.active_mandates()?;
                    for (_src_id, src_canonical) in &external_candidates {
                        let authorized = match src_canonical {
                            // A candidate with no recorded ingested path cannot be proven
                            // in-scope → fail closed (taint).
                            None => false,
                            Some(src_path) => mandates.iter().any(|m| {
                                m.target == canonical_target_str
                                    && std::path::Path::new(src_path)
                                        .starts_with(std::path::Path::new(&m.source_scope))
                            }),
                        };
                        if !authorized {
                            taint = Taint::Untrusted;
                        }
                    }
                }
            }
        }

        // ── Step 3: op × existence matrix ─────────────────────────────────────────
        // One `symlink_metadata` probe (NOFOLLOW semantics: it describes the final
        // component itself, so a symlink there is seen as a symlink). A missing
        // target reads as "does not exist". The pure classifier holds the matrix.
        // The same probe yields the target's ON-DISK identity `(dev,ino,size)`,
        // reused by BOTH the step-4 inode anchor and the step-5 base_identity.
        let final_meta = std::fs::symlink_metadata(&p.target);
        let exists = final_meta.is_ok();
        let is_symlink = final_meta
            .as_ref()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        let target_identity: Option<FileId> = final_meta.as_ref().ok().map(|m| {
            use std::os::unix::fs::MetadataExt;
            FileId { dev: m.dev(), ino: m.ino(), size: m.size() }
        });
        if let Some(reason) = classify_op_existence(p.op, exists, is_symlink).reject_reason() {
            reject_reason.get_or_insert(reason);
        }

        // ── Step 4: engine-anchored taint (D8-for-writes — L11, SECURITY-CRITICAL) ─
        // INDEPENDENTLY of the caller's citations, ask the engine "is this target a
        // file I currently track as ingested?". If so, its `file_ingested` event is
        // external BY CONSTRUCTION, so the write is tainted — even if the caller
        // cited only clean events. This is the floor that a confused-deputy
        // cite-around cannot bypass, because it is derived from the TARGET, never
        // the citation list. Unioned with step 1's result (taint only escalates).
        //
        // The anchor matches on PATH **OR** INODE IDENTITY: a hardlink alias of a
        // tracked file has a DIFFERENT canonical path but the SAME `(dev,ino)`, so a
        // path-only match would miss it and launder the taint (T2 review Critical).
        // Both resolve to the same `file_event_id`, so the provenance de-dupe below
        // (keyed on the event id) is correct for either match.
        if let Some(real) = &canonical {
            let real_str = real.to_string_lossy().to_string();
            let by_path = self.current_file_for_path(&real_str)?;
            let anchored = match by_path {
                Some(rec) => Some(rec),
                None => match target_identity {
                    Some(id) => self.tracked_file_with_identity(id.dev, id.ino)?,
                    None => None,
                },
            };
            if let Some(rec) = anchored {
                taint = Taint::Untrusted;
                // Surface the engine-anchored provenance too, de-duped against any
                // identical id the caller already cited (so the same file_ingested
                // event is not listed twice).
                if !provenance.iter().any(|pr| pr.event_id == rec.file_event_id) {
                    if let Some(ev) = self.event_by_id(&rec.file_event_id)? {
                        provenance.push(Self::provenance_from_event(&ev));
                    }
                }
            }
        }

        // ── Step 5: base hash + identity + (display) diff for Edit/Delete ─────────
        // Create leaves both `None` (there is no base). For Edit/Delete we read the
        // CURRENT bytes for the concurrency hash and stat the final component (via
        // symlink_metadata, NOT following a link) for the (dev,ino,size) anchor the
        // execute-time guard re-asserts (L12). Best-effort: an unreadable base does
        // not crash the gate — it simply leaves the field `None` (execute fails
        // closed later if it cannot re-establish the anchor).
        let (base_content_hash, base_identity) = match p.op {
            WriteOp::Create => (None, None),
            WriteOp::Edit | WriteOp::Delete => {
                let hash = std::fs::read(&p.target).ok().map(|bytes| {
                    use sha2::{Digest, Sha256};
                    hex::encode(Sha256::digest(&bytes))
                });
                // Reuse the single `symlink_metadata` identity captured in step 3
                // (no second stat) — the (dev,ino,size) anchor the execute-time
                // guard re-asserts (L12).
                (hash, target_identity)
            }
        };

        // ── Step 6: advisory diff-guard + the MONOTONIC loud-modal verdict ────────
        // For Delete there is no new content to scan, so the diff-guard sees empty
        // bytes (no flags) — but Delete forces the loud modal anyway via the rule
        // below, so the modal is never downgraded by an empty Delete diff.
        let diff_flags = match p.op {
            WriteOp::Delete => crate::actuator::DiffFlags::default(),
            WriteOp::Create | WriteOp::Edit => diff_guard(&p.new_content),
        };
        let requires_loud_modal = matches!(taint, Taint::Untrusted)
            || matches!(p.op, WriteOp::Delete)
            || diff_flags.any();

        let verdict = WriteVerdict {
            target_canonical: canonical,
            allowed,
            taint,
            provenance,
            diff_flags,
            base_content_hash,
            base_identity,
            requires_loud_modal,
            reject_reason,
        };
        Ok(GatedProposal { proposal: p, verdict })
    }

    /// Execute a gated write **inside the actuator rename critical section** (spec
    /// §9) — the confused-deputy defense's TOCTOU close. Re-derives every FS-mutable
    /// security fact AT EXECUTE (never trusting the propose-time verdict blindly),
    /// mutates atomically, and appends the SOLE-CONSTRUCTOR `file_written` event.
    /// Returns the appended event's id.
    ///
    /// The whole window — re-canonicalize → re-check grant → base guard → mutate →
    /// append — is serialized by [`rename_lock`](Self::rename_lock), so a second
    /// `execute_write` cannot interleave its base read against a half-applied first
    /// write. Anything diverged since propose ⇒ **fail-closed reject** (the app must
    /// re-propose); failure leaves the filesystem unchanged.
    ///
    /// Steps (spec §9):
    /// 1. **Re-check the verdict + re-canonicalize.** A verdict carrying a
    ///    `reject_reason`, or not `allowed`, is refused outright. The target (parent,
    ///    for Create) is re-canonicalized and re-run through `is_write_allowed` — a
    ///    grant revoked since propose ⇒ fail-closed.
    /// 2. **Open the parent dir fd** via the writable careful-open (fd-relative anchor).
    /// 3. **Base guard (Edit/Delete):** open the target fd-relative (NOFOLLOW),
    ///    `fstat` it, and require BOTH `(dev,ino,size) == verdict.base_identity` AND a
    ///    re-hash of the current bytes `== verdict.base_content_hash`. Either diverged
    ///    ⇒ fail-closed (closes the content-change race AND the same-content/
    ///    different-inode swap). Create's no-clobber is enforced by `atomic_write`.
    /// 4. **Re-derive engine target-provenance (L11, defense-in-depth):** using the
    ///    re-canonicalized path + the target's freshly-`fstat`'d `(dev,ino)`, re-run
    ///    the path/inode anchor; if the target is a tracked ingested file, capture its
    ///    `file_ingested` id so the recorded sources carry it (verdict ≡ persisted
    ///    stamp). NOT trusted from the verdict — re-derived here.
    /// 5. **Mutate atomically:** Create/Edit → `atomic_write` (no-clobber iff Create);
    ///    Delete → fd-relative `unlinkat` (**hard-delete, NO OS trash** — spec W5).
    /// 6. **Append** the Tier-B `file_written` with `source_event_ids = dedup(caller
    ///    cites ∪ {re-derived file_ingested id})`. The append chokepoint stamps it
    ///    `origin:"external"` automatically whenever a source is external, so a write
    ///    to a tracked file is taint-stamped by construction.
    #[cfg(unix)]
    pub fn execute_write(
        &self,
        confirmed: crate::actuator::GatedProposal,
        acknowledged_loud: bool,
    ) -> Result<String, BossclawError> {
        // The public entry: a normal (non-undo) write carries no `undo_of` and
        // resolves no proposal (the recorded `file_written` is byte-identical to M6a).
        // The caller's loud acknowledgement is threaded to the engine loud-gate.
        self.execute_write_inner(confirmed, None, None, acknowledged_loud)
    }

    /// As [`execute_write`](Self::execute_write), but records that this write RESOLVES
    /// the M6b `write_proposal` whose id is `resolves_proposal` — stamping
    /// `content["resolves_proposal"]` on the `file_written`. The write itself (re-checks,
    /// base guard, atomic mutate, undo capture, sole-constructor append) is identical;
    /// only the recorded provenance gains the back-reference. Carries no `undo_of`.
    ///
    /// SECURITY (SP5): this TRUSTS `confirmed.verdict` without recomputing it. Callers MUST obtain
    /// `confirmed` from a `propose_write()` run against the CURRENT filesystem + grants +
    /// active_mandates immediately prior, so `execute_write_inner`'s loud-gate judges a FRESH
    /// verdict. The desktop `apply_proposal` does exactly this (re-proposes, then passes the user's
    /// `acknowledged_loud`); the autonomous mandate sweep passes `acknowledged_loud = false` so a
    /// fresh-loud verdict fails closed. Never pass a stored/aged verdict — a propose-time-clean
    /// verdict that is now loud (e.g. a mandate revoked between propose and apply) would otherwise
    /// be applied unchecked.
    #[cfg(unix)]
    pub fn execute_write_resolving(
        &self,
        confirmed: crate::actuator::GatedProposal,
        resolves_proposal: &str,
        acknowledged_loud: bool,
    ) -> Result<String, BossclawError> {
        self.execute_write_inner(confirmed, None, Some(resolves_proposal), acknowledged_loud)
    }

    /// The shared execute path for both [`execute_write`](Self::execute_write) (the
    /// public, `undo_of = None` entry) and [`undo_write`](Self::undo_write) (which
    /// passes `Some(original_file_written_id)` so the recorded `file_written` carries
    /// `undo_of`). Factoring it keeps the public `execute_write` signature exactly as
    /// the spec dictates while letting undo stamp the discriminator + lineage — all
    /// re-checks, the base guard, the durable undo capture (W8), the atomic mutate,
    /// and the sole-constructor append run identically for a write and its undo.
    #[cfg(unix)]
    fn execute_write_inner(
        &self,
        confirmed: crate::actuator::GatedProposal,
        undo_of: Option<&str>,
        resolves_proposal: Option<&str>,
        acknowledged_loud: bool,
    ) -> Result<String, BossclawError> {
        use crate::actuator::WriteOp;
        use std::os::unix::ffi::OsStrExt;

        let crate::actuator::GatedProposal { proposal, verdict } = confirmed;

        // A single fail-closed reject helper so every divergence reads the same way.
        let reject = |why: &str| -> BossclawError {
            BossclawError::InvalidInput(format!("execute_write fail-closed: {why}"))
        };

        // ── Step 1a: the verdict itself must permit the write ─────────────────────
        // A reject_reason (op×existence, unresolvable target, empty sources) or a
        // not-`allowed` verdict means the proposal never cleared the gate; refuse
        // BEFORE acquiring the lock or touching the FS.
        if let Some(reason) = &verdict.reject_reason {
            return Err(reject(&format!("verdict carries reject_reason: {reason}")));
        }
        if !verdict.allowed {
            return Err(reject("verdict.allowed is false (target not under an active write grant)"));
        }

        // ── Step 1a.5: ENGINE-ENFORCED loud-gate (SP5 change d, SECURITY-CRITICAL) ─
        // A loud write (Untrusted ∪ Delete ∪ secret/value-shaped) is refused unless the caller
        // passed `acknowledged_loud == true`. This makes "a loud write needs an explicit ack" an
        // engine INVARIANT for every caller — desktop apply (threads the user's value), the
        // autonomous sweep (passes false ⇒ a loud mandate write can never auto-apply), and any
        // future caller. The ONLY sanctioned ack-without-UI path is `undo_write` (a hash-verified
        // inverse of an already-approved write), which passes true with a documented exemption.
        if verdict.requires_loud_modal && !acknowledged_loud {
            return Err(reject(&format!("{LOUD_ACK_REQUIRED_MSG} (refused fail-closed)")));
        }

        // The whole TOCTOU-critical window is held under the rename mutex (spec §9).
        let _rename_guard = self.rename_lock().lock().expect(POISON);

        // ── Step 1b: re-canonicalize + re-check the grant (a revoke since propose
        // ── must fail closed). For Create the target is absent ⇒ key off the PARENT.
        let (parent_dir, final_name): (std::path::PathBuf, std::ffi::OsString) = match proposal.op {
            WriteOp::Create => {
                let parent = proposal
                    .target
                    .parent()
                    .ok_or_else(|| reject("create target has no parent"))?;
                let real_parent = std::fs::canonicalize(parent)
                    .map_err(|e| reject(&format!("create parent not resolvable: {e}")))?;
                let name = proposal
                    .target
                    .file_name()
                    .ok_or_else(|| reject("create target has no final component"))?
                    .to_os_string();
                (real_parent, name)
            }
            WriteOp::Edit | WriteOp::Delete => {
                let real = std::fs::canonicalize(&proposal.target)
                    .map_err(|e| reject(&format!("target not resolvable: {e}")))?;
                let parent = real
                    .parent()
                    .ok_or_else(|| reject("target has no parent"))?
                    .to_path_buf();
                let name = real
                    .file_name()
                    .ok_or_else(|| reject("target has no final component"))?
                    .to_os_string();
                (parent, name)
            }
        };
        // The canonical target path (parent/name) — the recorded `target` + the L11
        // path-anchor key. For Create the file does not exist yet, so it is built
        // from the real parent + the proposed name; for Edit/Delete it is the
        // canonicalized real path.
        let canonical_target = parent_dir.join(&final_name);

        // Re-check the write-grant against the freshly-resolved real path. A grant
        // revoked between propose and execute makes this false ⇒ fail-closed.
        if !self
            .is_write_allowed(&canonical_target)
            .map_err(|e| reject(&format!("write-grant re-check errored: {e}")))?
        {
            return Err(reject("target is no longer under an active write grant"));
        }

        // ── Step 2: open the parent dir fd (the fd-relative anchor) ───────────────
        let dir_fd = crate::actuator::open_dir_for_write(&parent_dir)
            .map_err(|e| reject(&format!("parent dir careful-open failed: {e}")))?;

        // ── Step 3: base guard (Edit/Delete) — identity AND content, fd-relative ──
        // Open the target fd-relative from the dir_fd with NOFOLLOW (so a final
        // component swapped to a symlink AFTER propose is refused here, not followed),
        // fstat it, and require BOTH the (dev,ino,size) identity AND a re-hash of the
        // current bytes to equal the verdict's. This single open is reused for the
        // identity check, the content re-hash, and the delete bytes (if any).
        //
        // IMPORTANT (honest scope): this proves the IDENTITY OF THE OPEN FD. The
        // mutate in step 5 acts BY NAME (`renameat`/`unlinkat` re-resolve
        // `final_name` against `dir_fd`), which is a different operation — so the
        // guard's fd identity does NOT transfer to the mutate. `rename_lock`
        // serializes bossclaw's own writers, but a FOREIGN process can still swap
        // `final_name` to a different inode in the guard→mutate window. Step 5 adds a
        // fail-closed pre-mutate re-stat to NARROW that window (it cannot eliminate
        // it — the statat→mutate gap remains, exactly like the macOS create residual,
        // spec §9). `guard_identity` (the value asserted here) is the comparison
        // anchor for that re-stat.
        let (pre_bytes, guard_identity): (Option<Vec<u8>>, Option<crate::actuator::FileId>) =
            match proposal.op {
                WriteOp::Create => (None, None),
                WriteOp::Edit | WriteOp::Delete => {
                    let want_identity = verdict
                        .base_identity
                        .ok_or_else(|| reject("edit/delete verdict missing base_identity"))?;
                    let want_hash = verdict
                        .base_content_hash
                        .as_deref()
                        .ok_or_else(|| reject("edit/delete verdict missing base_content_hash"))?;

                    // Fd-relative NOFOLLOW open of the existing target.
                    let target_fd = rustix::fs::openat(
                        &dir_fd,
                        final_name.as_bytes(),
                        rustix::fs::OFlags::RDONLY
                            | rustix::fs::OFlags::NOFOLLOW
                            | rustix::fs::OFlags::CLOEXEC,
                        rustix::fs::Mode::empty(),
                    )
                    .map_err(|e| reject(&format!("target re-open failed: {e}")))?;

                    let st = rustix::fs::fstat(&target_fd)
                        .map_err(|e| reject(&format!("target fstat failed: {e}")))?;
                    // It must still be a regular file (a swap to a dir/fifo/device is a
                    // divergence, and reading it could hang or mislead the guard).
                    if rustix::fs::FileType::from_raw_mode(st.st_mode)
                        != rustix::fs::FileType::RegularFile
                    {
                        return Err(reject("target is no longer a regular file"));
                    }
                    // Identity half (L12): dev/ino/size must match the propose-time stat.
                    // The `as u64` casts mirror the propose-time capture (ingest.rs), so
                    // the two sides are compared on the same widths.
                    let now_identity = crate::actuator::FileId {
                        dev: st.st_dev as u64,
                        ino: st.st_ino as u64,
                        size: st.st_size as u64,
                    };
                    if now_identity != want_identity {
                        return Err(reject(
                            "base identity (dev,ino,size) diverged since propose (TOCTOU swap)",
                        ));
                    }
                    // Content half: re-hash the CURRENT bytes through the same fd.
                    let current = read_fd_to_end(&target_fd)
                        .map_err(|e| reject(&format!("target re-read failed: {e}")))?;
                    let now_hash = {
                        use sha2::{Digest, Sha256};
                        hex::encode(Sha256::digest(&current))
                    };
                    if now_hash != want_hash {
                        return Err(reject("base content hash diverged since propose"));
                    }
                    // The bytes we just read ARE the pre-mutate content — the undo
                    // pre-bytes (T5). For BOTH Edit and Delete we keep them so step 4.5
                    // can durably capture them to the undo store BEFORE the FS mutate
                    // (W8). For an Edit they are the old content (undo = restore them);
                    // for a Delete they are the deleted content (undo = recreate from
                    // them). `now_identity` is carried out as the step-5 re-stat anchor.
                    (Some(current), Some(now_identity))
                }
            };

        // ── Step 4: re-derive engine target-provenance (L11, defense-in-depth) ────
        // Do NOT trust verdict.provenance: re-derive from the re-canonicalized path
        // OR the target's freshly-stat'd (dev,ino) (the hardlink-alias path). If the
        // target is a tracked ingested file, its `file_ingested` id is unioned into
        // the recorded sources so the append chokepoint stamps the write external —
        // making the persisted stamp identical to the verdict (L11).
        let mut sources: Vec<String> = proposal.source_event_ids.clone();
        let real_str = canonical_target.to_string_lossy().to_string();
        // DELIBERATE: both anchors read the FULL files projection (`current_files`),
        // NOT `current_files_active`. Including tracked files whose grant was revoked
        // is the SAFE direction here: this step can only ADD an external source, and
        // taint is monotone (it only escalates Clean→Untrusted), so a revoked-grant
        // tracked file still correctly taints the write. A future "optimization" to
        // `current_files_active` would WEAKEN taint coverage — do not make it.
        let anchored = match self.current_file_for_path(&real_str)? {
            Some(rec) => Some(rec),
            // Inode fallback only applies to an existing target (Edit/Delete); a
            // Create has no on-disk inode yet, so there is nothing to alias.
            None => match (proposal.op, verdict.base_identity) {
                (WriteOp::Edit | WriteOp::Delete, Some(id)) => {
                    self.tracked_file_with_identity(id.dev, id.ino)?
                }
                _ => None,
            },
        };
        if let Some(rec) = anchored {
            if !sources.contains(&rec.file_event_id) {
                sources.push(rec.file_event_id);
            }
        }
        // The Tier-B non-empty invariant is structurally guaranteed (the caller's
        // cites were validated non-empty at propose), but assert it again here so the
        // sole constructor never emits a Tier-B event with empty lineage.
        if sources.is_empty() {
            return Err(reject("file_written would have empty source_event_ids"));
        }

        let op_str = match proposal.op {
            WriteOp::Create => "create",
            WriteOp::Edit => "edit",
            WriteOp::Delete => "delete",
        };

        // ── Step 4.5: durably capture the undo pre-bytes BEFORE mutating (W8) ─────
        // Crash-safety ordering (spec §7.3/§9 step 4): the undo row is INSERTed and
        // COMMITTED to the encrypted store BEFORE the FS mutation is observable, so a
        // crash after the mutate always leaves recoverable pre-bytes. The row is keyed
        // by its OWN fresh ULID with `file_written_id` NULL; the real event id is
        // backfilled AFTER the post-mutate append (step 6.5). The append chokepoint
        // mints the event id itself and is NOT weakened to accept a pre-set id — the
        // backfill is how the two are bound without touching the chokepoint.
        //
        // ONLY a normal write captures a frame: an UNDO (`undo_of.is_some()`) is a
        // recovery action, not a new user write, so it neither captures a new frame nor
        // GCs (spec §7.3 "recovery convenience"). This keeps the per-target frame stack
        // a pure record of forward writes — a LIFO walk of undos cannot evict the very
        // frames it is walking. (v1 has no redo, so an undo needs no recovery point.)
        //
        // `record_hash` is the hash the undo will later re-verify the pre_bytes
        // against (W9). For Create there are no pre-bytes (pre_bytes = NULL → undo
        // removes the file); the recorded hash is the new content's hash, but it is
        // never used to gate a Create-undo (which simply deletes). For Edit/Delete it
        // is `verdict.base_content_hash` (the guard just re-confirmed the on-disk bytes
        // hash to this), so the captured pre_bytes provably hash to it.
        let undo_id = Ulid::new().to_string();
        let capture_frame = undo_of.is_none();
        if capture_frame {
            let record_hash: String = match (&verdict.base_content_hash, proposal.op) {
                (Some(h), _) => h.clone(),
                // Create has no base; record the would-be new-content hash so the column
                // is non-NULL (it is never used to gate a Create-undo).
                (None, WriteOp::Create) => {
                    use sha2::{Digest, Sha256};
                    hex::encode(Sha256::digest(&proposal.new_content))
                }
                (None, _) => {
                    return Err(reject("edit/delete missing base_content_hash for undo capture"))
                }
            };
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "INSERT INTO undo_state
                   (undo_id, file_written_id, canonical_target, op, pre_bytes, base_content_hash, created_at)
                 VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    undo_id,
                    real_str,
                    op_str,
                    pre_bytes,      // Option<Vec<u8>> → NULL for Create, bytes otherwise
                    record_hash,
                    Utc::now().to_rfc3339(),
                ],
            )?;
            tx.commit()?; // ← pre-bytes durable on disk before any FS mutation below.

            // TEST-ONLY crash-ordering seam (W8): fires immediately AFTER the undo row
            // is durably committed and immediately BEFORE the FS mutate. The in-crate
            // test installs a probe that asserts the row is readable at this instant,
            // proving the durable-before-mutate ordering. Compiled out of non-test builds.
            #[cfg(test)]
            undo_test_hooks::fire_pre_mutate(&undo_id);
        }

        // ── Step 5: mutate atomically (failure leaves the FS unchanged) ───────────
        // The mutate acts BY NAME (`renameat`/`unlinkat` re-resolve `final_name`
        // against `dir_fd`), so the step-3 fd-identity guard does not transfer here.
        // For Edit/Delete we pass `guard_identity` so a fail-closed `(dev,ino)`
        // re-stat runs immediately before the by-name mutate, narrowing the
        // foreign-process guard→mutate window (it cannot be eliminated — the
        // re-stat→mutate gap remains, like the macOS create residual; spec §9).
        match proposal.op {
            WriteOp::Create => crate::actuator::atomic_write(
                &dir_fd,
                &final_name,
                &proposal.new_content,
                true,           // no_clobber: a Create must not overwrite a racing file
                None,           // Create has no base identity (the no-clobber handles a racer)
            )
            .map_err(|e| reject(&format!("atomic create failed: {e}")))?,
            WriteOp::Edit => crate::actuator::atomic_write(
                &dir_fd,
                &final_name,
                &proposal.new_content,
                false,          // overwrite is intended for an Edit
                guard_identity, // re-stat-before-rename: fail closed on a name swap
            )
            .map_err(|e| reject(&format!("atomic edit failed: {e}")))?,
            WriteOp::Delete => {
                // Hard-delete, fd-relative, NO OS trash (spec L1/W5). The step-3 guard
                // proved the IDENTITY OF AN OPEN FD; `unlinkat` acts BY NAME, so it is
                // a different operation. Re-stat the name's (dev,ino) against the guard
                // identity immediately before unlinking and fail closed on a swap —
                // narrowing (not eliminating) the foreign-process guard→unlink window.
                if let Some(expected) = guard_identity {
                    crate::actuator::restat_identity_matches(&dir_fd, &final_name, expected)
                        .map_err(|e| reject(&format!("pre-unlink re-stat: {e}")))?;
                }
                rustix::fs::unlinkat(&dir_fd, final_name.as_bytes(), rustix::fs::AtFlags::empty())
                    .map_err(|e| reject(&format!("unlinkat (hard-delete) failed: {e}")))?;
            }
        }

        // ── Step 5.5: capture the POST-write identity (Create/Edit) for the undo ──
        // The identity the write LEFT on disk: `undo_write` re-asserts the CURRENT
        // target still has it before restoring, so a foreign-process inode swap BETWEEN
        // the write and a later undo is caught (W9 "identity diverged ⇒ fail-closed").
        // Opened fd-relative NOFOLLOW from the same dir_fd (never re-resolving the path
        // string). A Delete leaves no file, so its post-identity is None. Only computed
        // when a frame was captured (a non-undo write); an undo records no frame.
        let post_identity: Option<crate::actuator::FileId> = if capture_frame {
            match proposal.op {
                WriteOp::Delete => None,
                WriteOp::Create | WriteOp::Edit => {
                    let written_fd = rustix::fs::openat(
                        &dir_fd,
                        final_name.as_bytes(),
                        rustix::fs::OFlags::RDONLY
                            | rustix::fs::OFlags::NOFOLLOW
                            | rustix::fs::OFlags::CLOEXEC,
                        rustix::fs::Mode::empty(),
                    )
                    .map_err(|e| reject(&format!("post-write re-open failed: {e}")))?;
                    let st = rustix::fs::fstat(&written_fd)
                        .map_err(|e| reject(&format!("post-write fstat failed: {e}")))?;
                    Some(crate::actuator::FileId {
                        dev: st.st_dev as u64,
                        ino: st.st_ino as u64,
                        size: st.st_size as u64,
                    })
                }
            }
        } else {
            None
        };

        // ── Step 6: append the SOLE-CONSTRUCTOR Tier-B `file_written` event ───────
        // content shape (spec §7.2).
        //
        // CONVENTION (Delete): `content_hash` is non-optional in the schema, so a
        // Delete sets `content_hash == prev_content_hash` (BOTH the deleted file's
        // hash) and `byte_size: 0`. There is no post-state content for a delete, so
        // CONSUMERS MUST branch on `op == "delete"`: do not read `content_hash` as a
        // "new content" hash for a delete — it is the removed file's hash.
        let (content_hash, prev_content_hash, byte_size): (String, Option<String>, u64) =
            match proposal.op {
                WriteOp::Create => {
                    use sha2::{Digest, Sha256};
                    (
                        hex::encode(Sha256::digest(&proposal.new_content)),
                        None, // a Create has no prior content
                        proposal.new_content.len() as u64,
                    )
                }
                WriteOp::Edit => {
                    use sha2::{Digest, Sha256};
                    (
                        hex::encode(Sha256::digest(&proposal.new_content)),
                        // prev = the base hash the guard just re-confirmed.
                        verdict.base_content_hash.clone(),
                        proposal.new_content.len() as u64,
                    )
                }
                WriteOp::Delete => {
                    // The deleted file's hash (== verdict.base_content_hash, which the
                    // guard re-confirmed against the freshly-read `pre_bytes`).
                    let h = verdict
                        .base_content_hash
                        .clone()
                        .ok_or_else(|| reject("delete missing base_content_hash for record"))?;
                    (h.clone(), Some(h), 0)
                }
            };

        // `op_str` was already computed in step 4.5 (the undo capture). Reuse it so
        // the recorded `op` and the captured `undo_state.op` cannot drift.
        //
        // Build content as a JSON OBJECT (the chokepoint only stamps `origin` when
        // `content.as_object_mut()` is Some — spec §6 M1). `prev_content_hash` is
        // omitted entirely for Create (skip-if-None via not inserting it). `undo_of`
        // is present iff this write IS an undo (spec §7.2) — the undo discriminator.
        // Keep a copy of the canonical target for the post-append undo GC (the
        // original `real_str` is moved into the content map just below).
        let gc_target = real_str.clone();
        let mut content = serde_json::Map::new();
        content.insert("target".to_string(), serde_json::Value::String(real_str));
        content.insert("op".to_string(), serde_json::Value::String(op_str.to_string()));
        content.insert("content_hash".to_string(), serde_json::Value::String(content_hash));
        if let Some(prev) = prev_content_hash {
            content.insert("prev_content_hash".to_string(), serde_json::Value::String(prev));
        }
        content.insert("byte_size".to_string(), serde_json::Value::Number(byte_size.into()));
        if let Some(undone) = undo_of {
            content.insert("undo_of".to_string(), serde_json::Value::String(undone.to_string()));
        }
        // M6b: stamp the back-reference to the resolved `write_proposal` iff this write
        // was confirmed via `execute_write_resolving` (skip-if-None, exactly like
        // `prev_content_hash`/`undo_of` above). Absent when None ⇒ the M6a `file_written`
        // content is byte-identical to before M6b (the frozen vector is unperturbed).
        if let Some(resolved) = resolves_proposal {
            content.insert("resolves_proposal".to_string(), serde_json::Value::String(resolved.to_string()));
        }

        let event = Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::FILE_WRITTEN_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object(content),
            // ALWAYS Tier-B (spec L9/W6): the sole constructor unconditionally sets
            // model_meta, so no Tier-A `file_written` can dodge the taint stamp.
            model_meta: Some(crate::event::ModelMeta {
                model_id: crate::graph::ACTUATOR_PRODUCER.to_string(),
                prompt_hash: String::new(), // no prompt — explicit caller (spec W10)
                source_event_ids: sources,
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        };
        // `append` re-checks the Tier-B non-empty invariant and runs the chokepoint
        // (which stamps `origin:"external"` if any source is external) BEFORE hashing.
        let file_written_id = self.append(event)?;

        // ── Step 6.5: bind the durable undo row to the now-minted event id + GC ───
        // Backfill `file_written_id` (NULL until now — the append chokepoint minted the
        // id) AND the post-write identity, so `undo_write` can look the row up by the
        // event id and re-assert the target identity. Then GC older rows so at most
        // UNDO_DEPTH remain per canonical_target (spec L3 §7.3). Skipped for an undo
        // (no frame was captured). Both run AFTER the mutate + append succeeded; a crash
        // before this leaves an orphan undo row (harmless — keyed by its own id, never
        // surfaced without a backfilled file_written_id; re-running adds a fresh row).
        if capture_frame {
            self.bind_and_gc_undo(&undo_id, &file_written_id, &gc_target, post_identity)?;
        }

        Ok(file_written_id)
    }

    /// Backfill an `undo_state` row's `file_written_id` (NULL until the append minted
    /// the id) and GC older rows so at most [`UNDO_DEPTH`] remain per
    /// `canonical_target` (spec L3 §7.3). One transaction: the bind + the GC commit
    /// together. GC orders by `created_at` ASC then `rowid` ASC (a stable tiebreak for
    /// rows minted in the same RFC-3339 second) and deletes all but the newest
    /// `UNDO_DEPTH` rows for that target — so the oldest pre-bytes drop out first.
    #[cfg(unix)]
    fn bind_and_gc_undo(
        &self,
        undo_id: &str,
        file_written_id: &str,
        canonical_target: &str,
        post_identity: Option<crate::actuator::FileId>,
    ) -> Result<(), BossclawError> {
        let (post_dev, post_ino, post_size) = pack_post_identity(post_identity);
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE undo_state
               SET file_written_id = ?1, post_dev = ?2, post_ino = ?3, post_size = ?4
             WHERE undo_id = ?5",
            rusqlite::params![file_written_id, post_dev, post_ino, post_size, undo_id],
        )?;
        // Keep the newest UNDO_DEPTH rows for this target; delete the rest. The
        // subquery selects the survivors (newest by created_at, rowid as tiebreak);
        // anything for this target NOT in that set is GC'd. `rowid` is SQLite's
        // implicit monotonic insert key, so it disambiguates same-second rows.
        tx.execute(
            "DELETE FROM undo_state
             WHERE canonical_target = ?1
               AND undo_id NOT IN (
                   SELECT undo_id FROM undo_state
                   WHERE canonical_target = ?1
                   ORDER BY created_at DESC, rowid DESC
                   LIMIT ?2
               )",
            rusqlite::params![canonical_target, UNDO_DEPTH as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Recoverable N-deep undo of a recorded `file_written` (M6a, T5 — spec §7.3/W9).
    /// Re-establishes the PRE-write state of `file_written_id`'s target by rebuilding a
    /// [`crate::actuator::WriteProposal`] from the captured `undo_state` row and running
    /// it through the **FULL** [`propose_write`](Self::propose_write) →
    /// [`execute_write`](Self::execute_write) path against the **CURRENT** grants +
    /// identity. The undo is itself recorded as a `file_written` carrying
    /// `undo_of: <file_written_id>` and `source_event_ids = [<file_written_id>]`.
    ///
    /// Returns the undo event's id.
    ///
    /// **The undo RE-GATES (W9) — it is NOT a privileged direct write:**
    /// - The rebuilt proposal goes through `propose_write` (so a target no longer under
    ///   an active write grant, or whose identity diverged, is caught) AND through
    ///   `execute_write`'s critical-section re-checks (grant re-check, base guard).
    ///   Anything diverged ⇒ **fail-closed**.
    /// - Before writing, the restored `pre_bytes` are **hash-verified against the
    ///   recorded `base_content_hash`** — a tampered undo row (bytes whose hash no
    ///   longer matches) **fails closed**, so the recovery store cannot be turned into
    ///   an injection vector.
    ///
    /// **The inverse op (spec §7.2):**
    /// - undo of an **Edit** ⇒ an Edit restoring the old `pre_bytes`.
    /// - undo of a **Create** ⇒ a Delete of the created file (`pre_bytes` is NULL).
    /// - undo of a **Delete** ⇒ a Create of the file from the deleted `pre_bytes`.
    #[cfg(unix)]
    pub fn undo_write(&self, file_written_id: &str) -> Result<String, BossclawError> {
        use crate::actuator::{WriteOp, WriteProposal};

        let fail = |why: &str| -> BossclawError {
            BossclawError::InvalidInput(format!("undo_write fail-closed: {why}"))
        };

        // ── Load the undo_state frame + its target's stack top in ONE query ───────
        // `undo_id` + `rowid` identify the frame in the per-target stack; `rowid` is
        // SQLite's monotonic insert order, so the MAX-rowid frame for a target is the
        // stack top (the most recent forward write). The correlated `top_rowid`
        // subquery fetches that top in the SAME row read, so the load and the LIFO
        // top-of-stack check are one lock + one round trip (and a consistent snapshot).
        #[allow(clippy::type_complexity)]
        let (undo_id, rowid, top_rowid, orig_op, canonical_target, pre_bytes, base_content_hash, post_id): (
            String,
            i64,
            i64,
            String,
            String,
            Option<Vec<u8>>,
            String,
            Option<crate::actuator::FileId>,
        ) = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.query_row(
                "SELECT undo_id, rowid,
                        (SELECT MAX(rowid) FROM undo_state u2
                          WHERE u2.canonical_target = u1.canonical_target) AS top_rowid,
                        op, canonical_target, pre_bytes, base_content_hash,
                        post_dev, post_ino, post_size
                 FROM undo_state u1 WHERE file_written_id = ?1",
                rusqlite::params![file_written_id],
                |r| {
                    let post = unpack_post_identity(r.get(7)?, r.get(8)?, r.get(9)?);
                    Ok((
                        r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                        r.get(6)?, post,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                fail("no undo record for this file_written id (GC'd past N-deep, or unknown id)")
            })?
        };

        // ── This frame must be the TOP of its target's undo stack (LIFO) ──────────
        // The undo store is an N-deep stack PER target (spec L3/§7.3): only the most
        // recent forward write can be undone (you cannot undo write 5 while 6.. are
        // still applied — their inodes have moved on). After this frame is popped, the
        // next undo targets the new top. Enforcing top-of-stack is what makes the
        // recorded post-identity check below sound across a multi-step LIFO walk.
        if rowid != top_rowid {
            return Err(fail(
                "not the most recent write to this target — undo is LIFO (undo newer writes first)",
            ));
        }

        // ── Re-assert the target identity has NOT diverged since the write (W9) ───
        // The undo store recorded the identity the write LEFT on disk (Create/Edit).
        // If a foreign process swapped the target for a different inode AFTER the write
        // but BEFORE this undo, the current on-disk identity will differ — fail closed
        // rather than clobber a file that is no longer the one we wrote. A Delete-undo
        // has no recorded post identity (the file was removed), so the recreate's
        // no-clobber handles a racing file instead. `symlink_metadata` (NOFOLLOW) so a
        // symlink swapped in at the name reads as a different inode and is rejected.
        if let Some(want) = post_id {
            use std::os::unix::fs::MetadataExt;
            let now = std::fs::symlink_metadata(&canonical_target).map_err(|e| {
                fail(&format!(
                    "undo target no longer stat-able (diverged/removed since the write): {e}"
                ))
            })?;
            let now_id = crate::actuator::FileId {
                dev: now.dev(),
                ino: now.ino(),
                size: now.size(),
            };
            if now_id != want {
                return Err(fail(
                    "target identity (dev,ino,size) diverged since the write (foreign swap) — refusing undo",
                ));
            }
        }

        // ── Verify the captured pre_bytes still hash to the recorded base hash (W9) ─
        // A tampered undo row (pre_bytes whose hash != base_content_hash) must fail
        // closed BEFORE any FS write, so the recovery store can never be used to inject
        // content. Create has NULL pre_bytes (undo = delete), so there is nothing to
        // verify — the recorded hash there is the (unused) new-content hash.
        if let Some(bytes) = &pre_bytes {
            use sha2::{Digest, Sha256};
            let actual = hex::encode(Sha256::digest(bytes));
            if actual != base_content_hash {
                return Err(fail(
                    "captured pre_bytes hash != recorded base_content_hash (tampered undo row)",
                ));
            }
        }

        // ── Rebuild the inverse-op proposal (spec §7.2) ──────────────────────────
        let target = std::path::PathBuf::from(&canonical_target);
        let (undo_op, new_content): (WriteOp, Vec<u8>) = match orig_op.as_str() {
            // Undo an Edit ⇒ Edit restoring the old bytes.
            "edit" => (
                WriteOp::Edit,
                pre_bytes.ok_or_else(|| fail("edit undo row missing pre_bytes"))?,
            ),
            // Undo a Create ⇒ Delete the created file (no content needed).
            "create" => (WriteOp::Delete, Vec::new()),
            // Undo a Delete ⇒ Create the file from the deleted bytes.
            "delete" => (
                WriteOp::Create,
                pre_bytes.ok_or_else(|| fail("delete undo row missing pre_bytes"))?,
            ),
            other => return Err(fail(&format!("unknown undo op '{other}'"))),
        };

        // The undo's lineage cites the original write id (spec §7.2 / R4): the append
        // chokepoint re-stamps taint via this source, so undoing a write to a tainted
        // file is itself stamped external by construction.
        let proposal = WriteProposal {
            target,
            new_content,
            op: undo_op,
            source_event_ids: vec![file_written_id.to_string()],
            rationale: format!("undo of {file_written_id}"),
        };

        // ── RE-GATE through the full propose path, then execute carrying undo_of ──
        // `propose_write` recomputes eligibility + base hash/identity against the
        // CURRENT filesystem; `execute_write_inner` re-checks the grant + base guard
        // inside the critical section. A diverged grant ⇒ fail-closed here. The undo
        // itself records NO new frame (`undo_of.is_some()`), so the stack is a pure
        // record of forward writes.
        let gated = self.propose_write(proposal)?;
        if let Some(reason) = &gated.verdict.reject_reason {
            return Err(fail(&format!("re-gated undo proposal rejected: {reason}")));
        }
        // An undo records NO frame and resolves NO M6b proposal (it carries `undo_of`, not
        // `resolves_proposal`). It passes `acknowledged_loud = true` as the SOLE sanctioned
        // ack-without-UI exemption (SP5 change d): an undo is a hash-verified inverse-restore of
        // `pre_bytes` already validated against the recorded base_content_hash — the inverse of an
        // already-approved write, never fresh untrusted content. Its re-gate is loud (the inverse
        // cites the original external file_written), so without this exemption every undo of a
        // tainted-file write would fail closed.
        let undo_event_id = self.execute_write_inner(gated, Some(file_written_id), None, true)?;

        // ── Pop this frame + hand the stack top off to the previous frame ─────────
        // The undo succeeded, so this frame is consumed: delete it. Then, if a previous
        // forward-write frame for this target remains (the new top), update its recorded
        // post-identity to the inode this undo just left on disk — so a subsequent LIFO
        // undo's identity check compares against the live inode, not the now-gone one
        // the previous write originally produced. A Create-undo removed the file, so the
        // new identity is None (no file); the recreate's no-clobber would guard a racer.
        let new_post: Option<crate::actuator::FileId> = {
            use std::os::unix::fs::MetadataExt;
            std::fs::symlink_metadata(&canonical_target).ok().map(|m| crate::actuator::FileId {
                dev: m.dev(),
                ino: m.ino(),
                size: m.size(),
            })
        };
        self.pop_and_handoff_undo(&undo_id, &canonical_target, new_post)?;

        Ok(undo_event_id)
    }

    /// LIFO stack maintenance after a successful undo (M6a, T5): delete the consumed
    /// frame `undo_id`, then update the NEW top frame for `canonical_target` (the
    /// remaining MAX-rowid frame) so its recorded post-identity is `new_post` — the
    /// inode the undo just left on disk. One transaction. If no previous frame remains,
    /// only the delete happens.
    #[cfg(unix)]
    fn pop_and_handoff_undo(
        &self,
        undo_id: &str,
        canonical_target: &str,
        new_post: Option<crate::actuator::FileId>,
    ) -> Result<(), BossclawError> {
        let (dev, ino, size) = pack_post_identity(new_post);
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM undo_state WHERE undo_id = ?1",
            rusqlite::params![undo_id],
        )?;
        // Re-point the new top (if any) at the live inode. Scoped to a single rowid so
        // only the top frame is touched.
        tx.execute(
            "UPDATE undo_state SET post_dev = ?1, post_ino = ?2, post_size = ?3
             WHERE rowid = (SELECT MAX(rowid) FROM undo_state WHERE canonical_target = ?4)",
            rusqlite::params![dev, ino, size, canonical_target],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Every CURRENT file (one per path), `ORDER BY canonical_path ASC`. Tier-A read.
    pub fn current_files(&self) -> Result<Vec<crate::graph::FileRecord>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT canonical_path, file_event_id, content_hash, grant_root FROM files ORDER BY canonical_path ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok(crate::graph::FileRecord {
            canonical_path: r.get(0)?, file_event_id: r.get(1)?, content_hash: r.get(2)?, grant_root: r.get(3)?,
        }))?;
        let mut out = Vec::new();
        for row in rows { out.push(row?); }
        Ok(out)
    }

    /// The CURRENT file record for `canonical_path`, or `None`. The dedup-decision
    /// lookup used by ingest.
    pub(crate) fn current_file_for_path(&self, canonical_path: &str) -> Result<Option<crate::graph::FileRecord>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let row = conn.query_row(
            "SELECT canonical_path, file_event_id, content_hash, grant_root FROM files WHERE canonical_path = ?1",
            rusqlite::params![canonical_path],
            |r| Ok(crate::graph::FileRecord {
                canonical_path: r.get(0)?, file_event_id: r.get(1)?, content_hash: r.get(2)?, grant_root: r.get(3)?,
            }),
        ).optional()?;
        Ok(row)
    }

    /// Reverse accessor: map a `file_ingested` event id → the projection's CURRENT
    /// FileRecord for it (the live on-disk path). Returns None if that id is no longer
    /// the current file at its path (superseded by a re-ingest) or never tracked.
    ///
    /// M6b reconciliation-proposer plumbing (§5.3): the proposer holds a lineage file
    /// id and needs the live path to read + rewrite. `pub` so the M6b integration tests
    /// in `tests/reconcile.rs` exercise it directly, mirroring the other `pub` read
    /// accessors ([`current_files`](Self::current_files)).
    #[cfg(unix)]
    pub fn current_path_for_file_event(
        &self,
        file_event_id: &str,
    ) -> Result<Option<crate::graph::FileRecord>, BossclawError> {
        for rec in self.current_files()? {
            if rec.file_event_id == file_event_id {
                return Ok(Some(rec));
            }
        }
        Ok(None)
    }

    /// Returns Some(FileRecord) iff the lineage file id is still the CURRENT tracked file
    /// at its path AND the on-disk target is a regular file (not a symlink/dir). The
    /// freshness guard (M6b §5.3).
    ///
    /// M6b reconciliation-proposer plumbing: the proposer only rewrites a target that is
    /// still the live, un-superseded, regular-file version of what it reasoned about —
    /// a superseded id, a re-pointed symlink, or a vanished path all fail closed. `pub`
    /// for the same reason as [`current_path_for_file_event`](Self::current_path_for_file_event).
    #[cfg(unix)]
    pub fn is_reconcilable_target(
        &self,
        lineage_file_id: &str,
    ) -> Result<Option<crate::graph::FileRecord>, BossclawError> {
        let Some(rec) = self.current_path_for_file_event(lineage_file_id)? else { return Ok(None) };
        match self.current_file_for_path(&rec.canonical_path)? {
            Some(cur) if cur.file_event_id == lineage_file_id => {}
            _ => return Ok(None),
        }
        match std::fs::symlink_metadata(&rec.canonical_path) {
            Ok(m) if m.file_type().is_file() => Ok(Some(rec)),
            _ => Ok(None),
        }
    }

    /// Engine-gathered lineage for a reconciliation proposal (M6b D8, §5.4):
    /// union( the retired edge's own source_event_ids , the inducing read_set ).
    /// Deliberately EXCLUDES the endpoints' entity lineage (over-reach: an entity accretes
    /// lineage from every memory that mentioned it). The model's citations are NEVER consulted.
    ///
    /// # Caller contract (anti-laundering — load-bearing)
    /// - `retired_edge_id` MUST be a bare link-EVENT id (an [`Edge::edge_id`](crate::graph::Edge::edge_id)
    ///   from [`fold_edges`](crate::graph::fold_edges)), NEVER an `entity:<ulid>` node id.
    ///   The internal `source_ids_of_event` read does NOT strip the `entity:` prefix,
    ///   so an entity id would silently resolve to `None` and DROP the asserting file's lineage —
    ///   a laundering hole. Pass the edge id, not an endpoint.
    /// - Callers MUST propagate the `Err` (use `?`), NEVER `.unwrap_or_default()`. The error is the
    ///   fail-closed signal for a corrupt/unparseable event payload; swallowing it would convert a
    ///   hard failure into a silently-empty lineage (taint laundered to nothing). This is the
    ///   contract the evolve-loop caller (Task 7) relies on.
    #[cfg(unix)]
    pub fn reconciliation_lineage(
        &self,
        retired_edge_id: &str,
        read_set: &[String],
    ) -> Result<Vec<String>, BossclawError> {
        let mut lineage: Vec<String> = Vec::new();
        if let Some(ids) = self.source_ids_of_event(retired_edge_id)? {
            lineage.extend(ids);
        }
        lineage.extend(read_set.iter().cloned());
        lineage.sort();
        lineage.dedup();
        Ok(lineage)
    }

    /// The CURRENT tracked file whose ON-DISK identity is `(dev, ino)`, or `None`.
    /// The INODE-keyed sibling of [`current_file_for_path`](Self::current_file_for_path),
    /// for the write gate's engine anchor (M6a, T2 review).
    ///
    /// **Why identity, not just path (the hardlink-alias close):** a hardlink is a
    /// second directory entry with a DIFFERENT name but the SAME inode.
    /// `std::fs::canonicalize` collapses symlinks and `..` but does NOT collapse a
    /// hardlink to a canonical name — so a path-only anchor MISSES a write made
    /// through a hardlink alias of a tracked external file, laundering the taint.
    /// This stats each current tracked path (via `symlink_metadata`, never
    /// following a link) and returns the record whose `(st_dev, st_ino)` equals the
    /// target's. Cross-device hardlinks are impossible, so `(dev, ino)` is a sound
    /// identity within the grant tree. A tracked path that no longer stats (since
    /// deleted/moved) is skipped — it cannot be the live target's alias.
    #[cfg(unix)]
    pub(crate) fn tracked_file_with_identity(
        &self,
        dev: u64,
        ino: u64,
    ) -> Result<Option<crate::graph::FileRecord>, BossclawError> {
        use std::os::unix::fs::MetadataExt;
        for rec in self.current_files()? {
            match std::fs::symlink_metadata(&rec.canonical_path) {
                Ok(m) if m.dev() == dev && m.ino() == ino => return Ok(Some(rec)),
                _ => continue, // mismatch, or no longer stat-able → not this alias
            }
        }
        Ok(None)
    }

    /// Event ids of CURRENT files whose grant root is still ACTIVE (revoked = 0).
    /// Used by recall to drop stale-version AND revoked-grant file hits.
    pub(crate) fn current_files_active(&self) -> Result<std::collections::HashSet<String>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT f.file_event_id FROM files f \
             JOIN grants g ON g.canonical_root = f.grant_root \
             WHERE g.revoked = 0",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = std::collections::HashSet::new();
        for row in rows { out.insert(row?); }
        Ok(out)
    }

    /// Shared builder for `link`/`invalidate`. The `[src, dst]` convenience
    /// default for `source_event_ids` is gated to the manual producer only.
    ///
    /// **SECURITY (taint, parent §5.11):** the `[src, dst]` default is for
    /// MANUAL (engine/test) links only — there the two endpoints genuinely ARE
    /// the whole justification. A non-manual producer (the M4 reasoner) MUST
    /// pass its real read-set; defaulting there would erase the inducing event
    /// from the lineage the actuator walks fail-closed.
    ///
    // The `producer` parameter is required by the F2 security gate; the remaining
    // args are the event's intrinsic fields. A params struct would add indirection
    // without safety benefit for this private, two-call-site helper.
    #[allow(clippy::too_many_arguments)]
    fn append_graph_event(
        &self,
        event_type: &str,
        producer: &str,
        src: &str,
        relation: &str,
        dst: &str,
        valid_time: Option<&str>,
        source_event_ids: &[String],
    ) -> Result<String, BossclawError> {
        let sources = match (producer == MANUAL_LINK_PRODUCER, source_event_ids.is_empty()) {
            (true, true) => vec![src.to_string(), dst.to_string()],
            (false, true) => {
                // Caller-argument-policy rejection → InvalidInput (not Chain, which
                // is for hash/chain-integrity failures). NB: M1's analogous empty-
                // source guard in `append` uses Chain (pre-existing; a candidate for
                // a later unify — do NOT change M1 here).
                return Err(BossclawError::InvalidInput(
                    "non-manual graph link requires explicit source_event_ids (no [src,dst] \
                     default — would launder taint past the §5.11 lineage walk)".into(),
                ));
            }
            (_, false) => source_event_ids.to_vec(),
        };
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: valid_time.map(String::from),
            event_type: event_type.to_string(),
            content: serde_json::json!({ "src": src, "relation": relation, "dst": dst }),
            model_meta: Some(ModelMeta {
                model_id: producer.to_string(),
                prompt_hash: String::new(),
                source_event_ids: sources,
            }),
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// The DID stamped on engine-authored events (`link`/`invalidate`). v1 uses a
    /// fixed engine identity; M4/M7 will thread the user's real DID through here.
    ///
    /// Note: `signed_by_did` is informational here (not verified against `key` at
    /// append). A fixed engine DID keeps the M3 surface small; threading the user
    /// DID is M4/M7 (carried, security I3).
    ///
    /// Returns an owned `String` (not the `&'static str` const) because M4/M7 will
    /// make this dynamic — the user's real DID, looked up per call.
    pub(crate) fn signer_did(&self) -> String {
        ENGINE_SIGNER_DID.to_string()
    }

    /// Append a signed `memory`-type event carrying `text`, stamped `origin = external`
    /// (the taint model, single-sourced via [`crate::graph::EXTERNAL_ORIGIN`]) so a
    /// remembered note is recallable (`memory` ∈ `EMBEDDABLE_EVENT_TYPES`) yet never
    /// auto-trusted downstream (`is_external` stays true). Derives + persists the note's
    /// vector so a subsequent [`EventLog::rebuild_indexes`] + `recall` surfaces it.
    /// Rejects empty/blank text with [`BossclawError::InvalidInput`] (no empty events).
    /// Tier-A (`model_meta: None`), signed by the engine DID like every ground-truth write.
    ///
    /// Not atomic: `append` commits before `derive_vector_for` runs, so if vector
    /// derivation fails the memory event is already durable (keyword-recallable) even
    /// though `Err` is returned. A caller retry could thus double-write; dedup is
    /// intentionally deferred to a later SP1 task.
    pub fn remember(&self, embedder: &dyn Embedder, text: &str) -> Result<String, BossclawError> {
        Self::reject_blank_note_text(text)?;
        let id = self.append(self.external_note_event(text))?;
        self.derive_vector_for(embedder, &id)?;
        Ok(id)
    }

    /// Reject empty/blank note text with [`BossclawError::InvalidInput`] — the
    /// single source of the "no empty note" rule shared by [`EventLog::remember`]
    /// and [`EventLog::supersede_note`] so the two checks can never drift.
    fn reject_blank_note_text(text: &str) -> Result<(), BossclawError> {
        if text.trim().is_empty() {
            return Err(BossclawError::InvalidInput("cannot remember empty or blank text".into()));
        }
        Ok(())
    }

    /// Build the signed, external-tainted `memory` Event carrying `text` — the
    /// exact content shape shared by [`EventLog::remember`] and the corrected note
    /// of [`EventLog::supersede_note`]. Stamped `origin = external` (the taint
    /// model, single-sourced via [`crate::graph::EXTERNAL_ORIGIN`]) so the note is
    /// recallable (`memory` ∈ `EMBEDDABLE_EVENT_TYPES`) yet never auto-trusted.
    /// Tier-A (`model_meta: None`), engine-signed. Does NOT validate `text`:
    /// callers reject blank text via [`EventLog::reject_blank_note_text`] first.
    fn external_note_event(&self, text: &str) -> Event {
        Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::MEMORY_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "text": text,
                "origin": crate::graph::EXTERNAL_ORIGIN,
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        }
    }

    /// Supersede a [`EventLog::remember`] note: validates `target_event_id` is a
    /// CURRENT note — an existing [`crate::graph::MEMORY_EVENT_TYPE`] event that is
    /// not itself already retired by a `supersede` (supersede chains have a single
    /// head) — then appends an atomic ground-truth pair: a
    /// [`crate::graph::SUPERSEDE_EVENT_TYPE`] link retiring the old note plus a
    /// fresh corrected note (same external-tainted, Tier-A shape as `remember`,
    /// via [`EventLog::external_note_event`]). Returns the NEW note's event id.
    ///
    /// Rejects (all [`BossclawError::InvalidInput`]): blank `text` (same check as
    /// `remember`), a target that does not exist, a non-`memory` target (e.g. a
    /// `session_captured` event), and an already-superseded target.
    ///
    /// Recall does not yet exclude superseded notes — that projection lands in a
    /// later SP3 task; this is the write primitive only. Not atomic across the
    /// vector: `append_pair` commits before `derive_vector_for` runs, matching
    /// `remember` and `capture_session`.
    pub fn supersede_note(
        &self,
        embedder: &dyn Embedder,
        target_event_id: &str,
        text: &str,
    ) -> Result<String, BossclawError> {
        Self::reject_blank_note_text(text)?;

        // Target must be a CURRENT note: it exists, is memory-kind, and is not
        // already the head-below of a supersede.
        let target = self.event_by_id(target_event_id)?.ok_or_else(|| {
            BossclawError::InvalidInput(format!(
                "cannot supersede {target_event_id}: no such event"
            ))
        })?;
        if target.event_type != crate::graph::MEMORY_EVENT_TYPE {
            return Err(BossclawError::InvalidInput(format!(
                "cannot supersede {target_event_id}: not a remembered note (event_type = {})",
                target.event_type
            )));
        }
        if self.superseded_event_ids()?.contains(target_event_id) {
            return Err(BossclawError::InvalidInput(format!(
                "cannot supersede {target_event_id}: already superseded (supersede chain heads only)"
            )));
        }

        // Atomic ground-truth pair (mirrors capture_session's changed-sha arm):
        // retire the old note, then append the corrected note.
        let supersede = ground_truth_supersede_event(target_event_id, self.signer_did());
        let corrected = self.external_note_event(text);
        let (_supersede_id, new_id) = self.append_pair(supersede, corrected)?;
        self.derive_vector_for(embedder, &new_id)?;
        Ok(new_id)
    }

    /// Retire a [`EventLog::remember`] note (rung-3 "Retire older") via a DISTINCT
    /// [`crate::graph::NOTE_RETIRED_EVENT_TYPE`] marker — reversible, no replacement.
    /// A distinct marker (not a `supersede`) is what lets [`EventLog::unretire`]
    /// reverse a retire WITHOUT ever reversing an edit: a bare supersede is
    /// byte-identical to an edit, so retiring through the supersede machinery would
    /// let an unretire resurrect edited-away content. Validation mirrors
    /// [`EventLog::supersede_note`] (exists, memory-kind, not already superseded)
    /// EXCEPT the blank-text check (there is no replacement text), plus a
    /// not-already-retired check. Returns the `note_retired` event's id.
    ///
    /// I5 completeness caveat (documented): retire excludes the note from recall,
    /// the Library list, and the embed-rebuild gate, but does NOT yet remove
    /// already-minted entities/edges or dequeue it from extraction — that
    /// edge-invalidation is resolution-time (Phase 3 / Task 9), not a Phase-1 blocker.
    pub fn retire_memory(
        &self,
        target_event_id: &str,
        source_proposal_id: Option<&str>,
    ) -> Result<String, BossclawError> {
        self.assert_retirable_note(target_event_id)?;
        // Base marker (byte-identical to today when `source_proposal_id` is None). The retire FOLD
        // keys on `retires` only (fold_sessions, log.rs `.get("retires")`), so the additive
        // `via`/`proposal_id` fields never disturb it — they exist ONLY to make the §3.4 digest
        // R-count conflict-scoped AND torn-write-safe (same marker type, no distinct event).
        let mut content = serde_json::Map::new();
        content.insert("retires".to_string(), serde_json::Value::String(target_event_id.to_string()));
        if let Some(pid) = source_proposal_id {
            content.insert("via".to_string(), serde_json::Value::String("conflict".to_string()));
            content.insert("proposal_id".to_string(), serde_json::Value::String(pid.to_string()));
        }
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::NOTE_RETIRED_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object(content),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// Reverse a prior [`EventLog::retire_memory`] (rung-3): appends a
    /// [`crate::graph::UNRETIRE_EVENT_TYPE`] marker that removes the note from
    /// `fold_sessions().retired_notes`, restoring it to recall and the Library list.
    /// Only reverses a RETIRE — an `unretire` removes solely from `retired_notes`,
    /// never from `superseded`, so it can never resurrect an edited-away note.
    /// Returns [`BossclawError::InvalidInput`] if `retired_event_id` is not currently
    /// retired (validated before appending). Returns the `unretire` event's id.
    pub fn unretire(&self, retired_event_id: &str) -> Result<String, BossclawError> {
        self.assert_note_retired(retired_event_id)?;
        let marker_id = self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::UNRETIRE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "unretires": retired_event_id }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        // Rung-3 Phase-3 (§3.2): the note is current again but the cursor swept past it. Rewind (marker
        // written first, so a torn write is benign). A note is one subject at within-seq id 0.
        if let Some(seq) = self.seq_of_event(retired_event_id)? {
            self.rewind_conflict_cursor(seq, 0)?;
        }
        Ok(marker_id)
    }

    /// Rung-3 §7.2: retire a SINGLE session passage (by `session_id` +
    /// `passage_id`) via a DISTINCT [`crate::graph::PASSAGE_RETIRED_EVENT_TYPE`]
    /// marker — the next [`EventLog::rebuild_conflict_index`] excludes exactly
    /// that passage from [`EventLog::conflict_search`], leaving its siblings and
    /// the recall / resolution indexes byte-untouched. Reversible via
    /// [`EventLog::unretire_passage`]; the append-only marker also survives a
    /// same-sha re-capture (an A2 dedup no-op), so a sweeper cycle never resurrects
    /// the passage.
    ///
    /// Validation is model-AGNOSTIC (so it holds before ANY recall rebuild and
    /// regardless of the active model): resolve the session's CURRENT fold-head
    /// capture event id via `fold.current`, then reject a `passage_id` at/above the
    /// [`EventLog::session_passage_count`] for that capture. Also rejects an unknown
    /// session and (I6) an already-retired passage. Returns the marker event's id.
    pub fn retire_passage(
        &self,
        session_id: &str,
        passage_id: usize,
        source_proposal_id: Option<&str>,
    ) -> Result<String, BossclawError> {
        let fold = fold_sessions(&self.session_events_ordered()?);
        let cs = fold
            .current
            .iter()
            .find(|cs| cs.session_id == session_id)
            .ok_or_else(|| {
                BossclawError::InvalidInput(format!(
                    "cannot retire passage: no current session {session_id}"
                ))
            })?;
        let n = self.session_passage_count(&cs.event_id)?;
        if passage_id >= n {
            return Err(BossclawError::InvalidInput(format!(
                "cannot retire passage {passage_id} of {session_id}: out of range ({n})"
            )));
        }
        if fold.retired_passages.contains(&(session_id.to_string(), passage_id)) {
            return Err(BossclawError::InvalidInput(format!(
                "cannot retire passage {passage_id} of {session_id}: already retired"
            )));
        }
        // Base marker (byte-identical to today when `source_proposal_id` is None). The retire FOLD
        // keys on `session_id`+`passage_id` only, so the additive `via`/`proposal_id` fields never
        // disturb it — they exist ONLY to make the §3.4 digest R-count conflict-scoped AND
        // torn-write-safe (same marker type, no distinct event).
        let mut content = serde_json::Map::new();
        content.insert("session_id".to_string(), serde_json::Value::String(session_id.to_string()));
        content.insert("passage_id".to_string(), serde_json::json!(passage_id));
        if let Some(pid) = source_proposal_id {
            content.insert("via".to_string(), serde_json::Value::String("conflict".to_string()));
            content.insert("proposal_id".to_string(), serde_json::Value::String(pid.to_string()));
        }
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::PASSAGE_RETIRED_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object(content),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// Reverse a prior [`EventLog::retire_passage`] (Rung-3 §7.3): appends an
    /// [`crate::graph::UNRETIRE_EVENT_TYPE`] marker carrying `{session_id,
    /// passage_id}`, which the fold removes from `retired_passages` — restoring the
    /// passage to [`EventLog::conflict_search`] and NOTHING else (it never touches
    /// `superseded`/`retired_notes`, so it can never resurrect edited-away content).
    /// Rejects (I6) a passage that is not currently retired. Returns the marker id.
    ///
    /// Phase-1 note: passage-unretire is CORE-ONLY — there is deliberately no
    /// proto/server unretire-passage op (the wire `Request::Unretire` is note-only).
    /// The Phase-3 conflict-resolution UI wires this; it exists now so the retire
    /// action is provably reversible.
    pub fn unretire_passage(
        &self,
        session_id: &str,
        passage_id: usize,
    ) -> Result<String, BossclawError> {
        let fold = fold_sessions(&self.session_events_ordered()?);
        if !fold.retired_passages.contains(&(session_id.to_string(), passage_id)) {
            return Err(BossclawError::InvalidInput(format!(
                "cannot unretire passage {passage_id} of {session_id}: not retired"
            )));
        }
        let marker_id = self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::UNRETIRE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "session_id": session_id, "passage_id": passage_id }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        // Rung-3 Phase-3 (§3.2): rewind to (current-head capture seq, passage_id). Resolve the head via
        // the post-append fold so the un-retired passage is included.
        let fold = fold_sessions(&self.session_events_ordered()?);
        if let Some(cs) = fold.current.iter().find(|cs| cs.session_id == session_id) {
            if let Some(seq) = self.seq_of_event(&cs.event_id)? {
                self.rewind_conflict_cursor(seq, passage_id)?;
            }
        }
        Ok(marker_id)
    }

    /// Validate `target_event_id` is a retirable CURRENT note — the same
    /// exists/memory-kind/not-already-superseded checks
    /// [`EventLog::supersede_note`] applies (blank-text excepted: retire has no
    /// replacement), PLUS not already retired by a live `note_retired` marker.
    /// All failures are [`BossclawError::InvalidInput`].
    ///
    /// The superseded + retired checks read BOTH sets off a single
    /// [`fold_sessions`] pass: `fold.superseded` IS the complete superseded-id
    /// universe (identical to [`EventLog::superseded_event_ids`] — the recall arm
    /// documents the same equivalence), and `fold.retired_notes` the retired one,
    /// so one scan over the session/supersede stream answers both.
    fn assert_retirable_note(&self, target_event_id: &str) -> Result<(), BossclawError> {
        let target = self.event_by_id(target_event_id)?.ok_or_else(|| {
            BossclawError::InvalidInput(format!(
                "cannot retire {target_event_id}: no such event"
            ))
        })?;
        if target.event_type != crate::graph::MEMORY_EVENT_TYPE {
            return Err(BossclawError::InvalidInput(format!(
                "cannot retire {target_event_id}: not a remembered note (event_type = {})",
                target.event_type
            )));
        }
        let fold = fold_sessions(&self.session_events_ordered()?);
        if fold.superseded.contains(target_event_id) {
            return Err(BossclawError::InvalidInput(format!(
                "cannot retire {target_event_id}: already superseded"
            )));
        }
        if fold.retired_notes.contains(target_event_id) {
            return Err(BossclawError::InvalidInput(format!(
                "cannot retire {target_event_id}: already retired"
            )));
        }
        Ok(())
    }

    /// Validate `retired_event_id` is currently retired (present in
    /// `fold_sessions().retired_notes`), the precondition for [`EventLog::unretire`].
    /// Returns [`BossclawError::InvalidInput`] otherwise.
    fn assert_note_retired(&self, retired_event_id: &str) -> Result<(), BossclawError> {
        if !fold_sessions(&self.session_events_ordered()?)
            .retired_notes
            .contains(retired_event_id)
        {
            return Err(BossclawError::InvalidInput(format!(
                "cannot unretire {retired_event_id}: not retired"
            )));
        }
        Ok(())
    }

    /// The set of event ids retired by a `supersede` event, across ALL folds:
    /// page/file/session/note supersedes share [`crate::graph::SUPERSEDE_EVENT_TYPE`]
    /// and each targets exactly one (disjoint) id. Reads only supersede events
    /// (`events_of_types` filters on `event_type` — never a whole-log scan). Used
    /// by [`EventLog::supersede_note`] to reject superseding an already-retired note.
    fn superseded_event_ids(&self) -> Result<HashSet<String>, BossclawError> {
        let events = self.events_of_types(&[crate::graph::SUPERSEDE_EVENT_TYPE])?;
        Ok(events
            .iter()
            .filter_map(|e| {
                e.content.get("supersedes").and_then(|v| v.as_str()).map(String::from)
            })
            .collect())
    }

    /// Event ids that the model-facing BATCH paths must treat as GONE — the union
    /// of (1) every id retired by a `supersede`: superseded notes, session heads,
    /// and page/file versions (`session_events_ordered` pulls ALL supersede
    /// events, which are shared across the folds — and gating retired page/file
    /// versions out of re-embedding is beneficial, not inert: only the CURRENT
    /// version of anything should ever re-vectorize) and (2) every
    /// `session_captured` event whose `session_id` carries a `session_deleted`
    /// tombstone (the tombstone keys on `session_id`, so the capture event id is
    /// NOT in the supersede set), and (3) every rung-3 retired note (a live
    /// `note_retired` marker, reversible via `unretire`) — so a retired note never
    /// re-vectorizes on rebuild/migration either. One
    /// [`EventLog::session_events_ordered`] scan feeds all three. Used to gate
    /// re-embedding ([`EventLog::collect_pending`]) AND the migration completeness
    /// denominator ([`EventLog::reembed_prepare`]) so a deleted/retired item
    /// neither re-vectorizes on rebuild/migration nor (by being counted "missing")
    /// blocks a migration from ever completing.
    fn embed_excluded_event_ids(&self) -> Result<HashSet<String>, BossclawError> {
        let events = self.session_events_ordered()?;
        let fold = fold_sessions(&events);
        let (deleted, mut excluded) = (fold.deleted, fold.superseded);
        excluded.extend(fold.retired_notes);
        for ev in &events {
            if ev.event_type == crate::graph::SESSION_CAPTURED_EVENT_TYPE {
                if let Some(sid) = ev.content.get("session_id").and_then(|v| v.as_str()) {
                    if deleted.contains(sid) {
                        excluded.insert(ev.id.clone());
                    }
                }
            }
        }
        Ok(excluded)
    }

    /// Derive + persist the vector for a just-appended event id (M5a ingest convenience).
    /// Best-effort: a non-embeddable or text-less event is a no-op.
    pub(crate) fn derive_vector_for(&self, embedder: &dyn Embedder, event_id: &str) -> Result<(), BossclawError> {
        let payload: Option<String> = {
            let store = self.inner.lock().expect(POISON);
            store.conn().query_row(
                "SELECT payload FROM events WHERE id = ?1", rusqlite::params![event_id],
                |r| r.get::<_, String>(0),
            ).optional()?
        };
        if let Some(p) = payload {
            let ev: Event = serde_json::from_str(&p)?;
            self.derive_vector(embedder, &ev)?;
        }
        Ok(())
    }

    /// All `session_captured` + `session_deleted` + `supersede` + Rung-3 retire
    /// markers (`note_retired` / `passage_retired` / `unretire`), in chain
    /// (`seq ASC`) order — the input to [`fold_sessions`]. The `supersede` rows
    /// are shared with the page/file folds; cross-fold safety holds because a
    /// supersede targets a disjoint event id (a session supersede references a
    /// session event, never a page/file event, and vice-versa). The retire
    /// markers fold into their OWN sets, strictly disjoint from `superseded`.
    fn session_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        self.events_of_types(&[
            crate::graph::SESSION_CAPTURED_EVENT_TYPE,
            crate::graph::SESSION_DELETED_EVENT_TYPE,
            crate::graph::SUPERSEDE_EVENT_TYPE,
            crate::graph::NOTE_RETIRED_EVENT_TYPE,
            crate::graph::PASSAGE_RETIRED_EVENT_TYPE,
            crate::graph::UNRETIRE_EVENT_TYPE,
        ])
    }

    /// Record a captured coding-agent session as a signed, external-tainted,
    /// embeddable `session_captured` event (SP3, spec §4b/§7a). Mirrors the M5a
    /// file-ingest dedup/supersede decision, keyed on `session_id`:
    ///
    /// - **tombstoned** (`session_deleted` seen for this id) → [`BossclawError::InvalidInput`]
    ///   (I9: an owner-deleted session can never be recaptured);
    /// - **same id + same `sha256`** → no-op, returns the EXISTING current event id;
    /// - **same id + different `sha256`** → an atomic ground-truth `supersede`+`session_captured`
    ///   pair (the body changed), returning the NEW event id;
    /// - **new id** → a plain append.
    ///
    /// The event's `content["text"]` (title-derived) is embedded like [`EventLog::remember`]
    /// so the title is recallable; `content["origin"]` is [`crate::graph::EXTERNAL_ORIGIN`]
    /// so the session body is never auto-trusted. Tier-A (`model_meta: None`), signed by the
    /// engine DID. The `.md` body file itself is NOT written here (that is task A7): only the
    /// event is recorded; `meta.path` is metadata.
    ///
    /// Not atomic across the vector: `append`/`append_pair` commits before
    /// `derive_vector_for` runs, matching [`EventLog::remember`] and the M5a ingest path.
    pub fn capture_session(&self, embedder: &dyn Embedder, meta: &SessionMeta) -> Result<String, BossclawError> {
        let events = self.session_events_ordered()?;
        let fold = fold_sessions(&events);

        // I9: a tombstoned session is gone forever — never recapturable.
        if fold.deleted.contains(&meta.session_id) {
            return Err(BossclawError::InvalidInput(format!(
                "session {} was deleted and cannot be recaptured (I9)",
                meta.session_id
            )));
        }

        match fold.current.iter().find(|cs| cs.session_id == meta.session_id) {
            // Same id + same bytes → no-op (return the existing current event id).
            // The sweeper's steady-state hot path: no event is even constructed.
            Some(prev) if prev.sha256 == meta.sha256 => Ok(prev.event_id.clone()),
            // Same id + changed bytes → atomic supersede + new capture.
            Some(prev) => {
                let supersede = ground_truth_supersede_event(&prev.event_id, self.signer_did());
                let event = session_captured_event(meta, self.signer_did());
                let (_s, new_id) = self.append_pair(supersede, event)?;
                self.derive_vector_for(embedder, &new_id)?;
                Ok(new_id)
            }
            // New session → plain append.
            None => {
                let event = session_captured_event(meta, self.signer_did());
                let new_id = self.append(event)?;
                self.derive_vector_for(embedder, &new_id)?;
                Ok(new_id)
            }
        }
    }

    /// Owner-commanded deletion of a captured session (SP3, I7): appends a signed,
    /// non-embeddable `session_deleted` tombstone so [`EventLog::current_sessions`]
    /// (and, later, recall) treat the session as gone. Append-only — the original
    /// `session_captured` event stays in the log forever; the tombstone shadows it.
    /// Returns [`BossclawError::InvalidInput`] if no CURRENT session has that id
    /// (nothing to delete: already gone, superseded away, or never captured).
    pub fn delete_session(&self, session_id: &str) -> Result<String, BossclawError> {
        let events = self.session_events_ordered()?;
        let fold = fold_sessions(&events);
        if !fold.current.iter().any(|cs| cs.session_id == session_id) {
            return Err(BossclawError::InvalidInput(format!(
                "cannot delete session {session_id}: no current captured session with that id"
            )));
        }
        // A tombstone carries no embeddable text and is never given a vector.
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::SESSION_DELETED_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "session_id": session_id }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// The CURRENT captured sessions: the latest non-superseded, non-tombstoned
    /// `session_captured` per `session_id` (SP3 §4b). A deterministic fold over
    /// the log via [`fold_sessions`] — the data source a later recall task filters
    /// against so deleted/superseded sessions never surface.
    pub fn current_sessions(&self) -> Result<Vec<CurrentSession>, BossclawError> {
        let events = self.session_events_ordered()?;
        Ok(fold_sessions(&events).current)
    }

    /// The CURRENT remembered notes (SP3 §7/§9): every `memory`-kind event
    /// ([`EventLog::remember`] / the corrected note of [`EventLog::supersede_note`])
    /// NOT retired by a `supersede` AND NOT retired by a live rung-3 `note_retired`
    /// marker (an `unretire` restores it), newest-first. A deterministic read (no
    /// vector, no embedder) backing the Memory-browser notes list — mirrors
    /// [`EventLog::current_sessions`]. "Note" is defined by event-kind EXACTLY as
    /// [`EventLog::supersede_note`] validates its target (memory-kind), so the list
    /// and the edit primitive can never disagree on what counts as a note. Reads
    /// only `memory` + `supersede` + `note_retired` + `unretire` events (never a
    /// whole-log scan).
    pub fn current_notes(&self) -> Result<Vec<CurrentNote>, BossclawError> {
        let events = self.events_of_types(&[
            crate::graph::MEMORY_EVENT_TYPE,
            crate::graph::SUPERSEDE_EVENT_TYPE,
            crate::graph::NOTE_RETIRED_EVENT_TYPE,
            crate::graph::UNRETIRE_EVENT_TYPE,
        ])?;
        Ok(fold_notes(&events))
    }

    /// The tombstoned (owner-deleted) `session_id`s — the same set
    /// [`EventLog::capture_session`] consults to enforce I9. A deterministic,
    /// sorted read over the session fold (no vector, no embedder). The sweeper
    /// (SP3 A9) uses this to EXCLUDE a deleted session's still-present transcript
    /// from re-capture at decision time, so it never re-renders — and never
    /// transiently rewrites the `.md` of — a session the owner deleted (I9). The
    /// engine's `capture_session` reject remains the durable backstop; this is
    /// the cheap pre-filter that keeps the per-sweep cap for live sessions.
    pub fn deleted_session_ids(&self) -> Result<Vec<String>, BossclawError> {
        let events = self.session_events_ordered()?;
        let mut ids: Vec<String> = fold_sessions(&events).deleted.into_iter().collect();
        ids.sort();
        Ok(ids)
    }

    /// Rebuild the persisted `edges`/`nodes` tables as a deterministic fold over
    /// every `link`/`invalidate` event (`ORDER BY seq ASC`). Tier-A: byte-
    /// identical across rebuilds (spec §4/§9). Wipes both tables and re-inserts
    /// under one transaction. Cheap (graph events are few). Call after appending
    /// `link`/`invalidate` events to refresh `neighbors`/`as_of`/the recall boost.
    ///
    /// **Lifecycle:** graph queries and the recall boost reflect the `edges`
    /// table as of the last `rebuild_graph` / [`EventLog::open_with_recall`].
    /// After appending `link`/`invalidate` events WITHIN a session, call
    /// `rebuild_graph` again — the same append→rebuild lifecycle as
    /// [`EventLog::rebuild_indexes`].
    pub fn rebuild_graph(&self) -> Result<(), BossclawError> {
        let started = Instant::now();
        let events = self.graph_events_ordered()?;
        let edges = crate::graph::fold_edges(&events);
        // F4: a signed link/invalidate with malformed content is silently
        // dropped by the fold (it never becomes an edge). Surface the count so
        // malformed-but-signed events are not invisible.
        let malformed = events
            .iter()
            .filter(|e| crate::graph::parse_link_content(&e.content).is_none())
            .count();
        if malformed > 0 {
            log::warn!(
                "rebuild_graph: {malformed} link/invalidate event(s) had malformed content \
                 and were skipped"
            );
        }

        // Fold entity events → entities projection + the set of entity node ids
        // (used to label node kind "entity" rather than "external").
        let entity_events = self.entity_events_ordered()?;
        let entities = crate::graph::fold_entities(&entity_events);
        // Set of entity node ids → used to mark node kind "entity" (overrides
        // the "external" default for ids the edges reference).
        let entity_ids: HashSet<String> =
            entities.iter().map(|e| e.entity_id.clone()).collect();

        // Fold page/supersede events → current dossier per topic (F9).
        let page_events = self.page_and_supersede_events_ordered()?;
        let pages = crate::graph::fold_pages(&page_events);

        // Fold grant/revoke events → current grants projection (M5a).
        let grant_events = self.grant_events_ordered()?;
        let grants = crate::graph::fold_grants(&grant_events);

        // Fold write_grant/write_revoke events → current write-grants projection (M6a).
        // SEPARATE event stream + fold from the read grants above: a `grant`/`revoke`
        // event cannot reach this projection (the query filters on the write types).
        let write_grant_events = self.write_grant_events_ordered()?;
        let write_grants = crate::graph::fold_write_grants(&write_grant_events);

        // Fold mandate_grant/mandate_revoke events → current mandates projection (M6c).
        // SEPARATE event stream + fold from the grants above; keyed on the grant event
        // id (the mandate identity), with a sticky `revoked` flag.
        let mandate_events = self.mandate_events_ordered()?;
        let mandates = crate::graph::fold_mandates(&mandate_events);

        // Fold file_ingested/supersede events → current file per path (M5a).
        let file_events = self.file_and_supersede_events_ordered()?;
        let files = crate::graph::fold_files(&file_events);

        let memory_ids = self.memory_page_ids()?;

        // Distinct endpoints → nodes (BTreeMap = deterministic node order).
        let mut node_kinds: BTreeMap<String, String> = BTreeMap::new();
        for e in &edges {
            for endpoint in [&e.src, &e.dst] {
                node_kinds.entry(endpoint.clone()).or_insert_with(|| {
                    if entity_ids.contains(endpoint) {
                        ENTITY_NODE_KIND.to_string()
                    } else if memory_ids.contains(endpoint) {
                        MEMORY_NODE_KIND.to_string()
                    } else {
                        EXTERNAL_NODE_KIND.to_string()
                    }
                });
            }
        }

        let edge_count = edges.len();
        let node_count = node_kinds.len();
        let entity_count = entities.len();
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM edges", [])?;
        tx.execute("DELETE FROM nodes", [])?;
        tx.execute("DELETE FROM entities", [])?;
        tx.execute("DELETE FROM pages", [])?;
        tx.execute("DELETE FROM grants", [])?;
        tx.execute("DELETE FROM write_grants", [])?;
        tx.execute("DELETE FROM mandates", [])?;
        tx.execute("DELETE FROM files", [])?;
        for e in &edges {
            tx.execute(
                "INSERT INTO edges
                   (edge_id, src, relation, dst, valid_from, valid_to,
                    ingested_at, invalidated_at, invalidated_by, origin, confidence_milli)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    e.edge_id, e.src, e.relation, e.dst, e.valid_from, e.valid_to,
                    e.ingested_at, e.invalidated_at, e.invalidated_by, e.origin, e.confidence_milli
                ],
            )?;
        }
        for (node_id, kind) in &node_kinds {
            tx.execute(
                "INSERT INTO nodes (node_id, kind) VALUES (?1, ?2)",
                rusqlite::params![node_id, kind],
            )?;
        }
        for e in &entities {
            tx.execute(
                "INSERT INTO entities (entity_id, label, aliases, entity_type)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    e.entity_id,
                    e.label,
                    // JSON array string — serde_json Vec<String> serialization is
                    // deterministic (array order preserved), so the stored string
                    // is byte-stable across rebuilds (byte-identical-rebuild holds).
                    serde_json::to_string(&e.aliases)?,
                    e.entity_type
                ],
            )?;
        }
        for p in &pages {
            tx.execute(
                "INSERT INTO pages (topic_id, page_event_id, title, text) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![p.topic_id, p.page_event_id, p.title, p.text],
            )?;
        }
        for g in &grants {
            tx.execute(
                "INSERT INTO grants (canonical_root, granted_at, revoked) VALUES (?1, ?2, ?3)",
                rusqlite::params![g.canonical_root, g.granted_at, g.revoked as i64],
            )?;
        }
        for g in &write_grants {
            tx.execute(
                "INSERT INTO write_grants (canonical_root, granted_at, revoked) VALUES (?1, ?2, ?3)",
                rusqlite::params![g.canonical_root, g.granted_at, g.revoked as i64],
            )?;
        }
        for m in &mandates {
            tx.execute(
                "INSERT INTO mandates
                   (mandate_grant_id, target, source_scope, recipe, granted_at, revoked)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    m.mandate_grant_id,
                    m.target,
                    m.source_scope,
                    m.recipe,
                    m.granted_at,
                    m.revoked as i64
                ],
            )?;
        }
        for f in &files {
            tx.execute(
                "INSERT INTO files (canonical_path, file_event_id, content_hash, grant_root)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![f.canonical_path, f.file_event_id, f.content_hash, f.grant_root],
            )?;
        }
        tx.commit()?;
        log::info!(
            "rebuilt graph: {edge_count} edges, {node_count} nodes, \
             {entity_count} entities in {}ms",
            started.elapsed().as_millis()
        );
        Ok(())
    }

    /// All `link`/`invalidate` events, payload-parsed, in chain (`seq ASC`) order.
    fn graph_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events
             WHERE event_type IN ('link', 'invalidate') ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// All `entity` events, payload-parsed, in chain (`seq ASC`) order.
    ///
    /// Used by [`EventLog::rebuild_graph`] to fold entity events into the
    /// `entities` projection. Parameterised query only — no string interpolation.
    fn entity_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = 'entity' ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// All `page` + `supersede` events, payload-parsed, in chain (`seq ASC`)
    /// order — the input to [`crate::graph::fold_pages`].
    fn page_and_supersede_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type IN ('page','supersede') ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows { out.push(serde_json::from_str(&row?)?); }
        Ok(out)
    }

    /// All `grant`/`revoke` events, payload-parsed, in chain (`seq ASC`) order.
    fn grant_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type IN ('grant', 'revoke') ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// All `write_grant`/`write_revoke` events, payload-parsed, in chain (`seq ASC`)
    /// order. The WRITE-side sibling of [`grant_events_ordered`]: it selects ONLY the
    /// write event types, so a read `grant`/`revoke` can never feed the write fold.
    fn write_grant_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type IN ('write_grant', 'write_revoke') ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// All `mandate_grant`/`mandate_revoke` events, payload-parsed, in chain (`seq ASC`)
    /// order (M6c). The mandate sibling of [`write_grant_events_ordered`]: it selects ONLY
    /// the mandate event types, so a grant/write-grant event can never feed the mandate fold.
    fn mandate_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type IN ('mandate_grant', 'mandate_revoke') ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// All `file_ingested`/`supersede` events, payload-parsed, in chain (`seq ASC`)
    /// order. (Page supersedes are included but harmless — `fold_files` only retires
    /// `file_ingested` ids.)
    fn file_and_supersede_events_ordered(&self) -> Result<Vec<Event>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type IN ('file_ingested', 'supersede') ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows { out.push(serde_json::from_str(&row?)?); }
        Ok(out)
    }

    /// Set of event ids whose type is `memory`/`page` — used to label node kinds.
    fn memory_page_ids(&self) -> Result<HashSet<String>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt =
            conn.prepare("SELECT id FROM events WHERE event_type IN ('memory', 'page')")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// Every edge, `ORDER BY edge_id ASC` (deterministic). Tier-A read.
    pub fn all_edges(&self) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        self.query_edges(
            "SELECT edge_id, src, relation, dst, valid_from, valid_to, \
                ingested_at, invalidated_at, invalidated_by, origin, confidence_milli \
             FROM edges ORDER BY edge_id ASC",
            &[],
        )
    }

    /// Every node, `ORDER BY node_id ASC`.
    pub fn all_nodes(&self) -> Result<Vec<crate::graph::Node>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare("SELECT node_id, kind FROM nodes ORDER BY node_id ASC")?;
        let rows = stmt.query_map([], |r| {
            Ok(crate::graph::Node { node_id: r.get(0)?, kind: r.get(1)? })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Current edges touching `node` in either direction (`invalidated_at IS
    /// NULL`). The result includes:
    ///
    /// - **Outgoing** edges where `src == node`.
    /// - **Incoming** (backlink) edges where `dst == node`.
    /// - **Self-loops** where `src == dst == node` (appear exactly once, not
    ///   twice — `OR` on a single row is still one row).
    ///
    /// Caller can filter for backlinks with `.iter().filter(|e| e.dst == node)`.
    /// `ORDER BY edge_id ASC` for deterministic output.
    pub fn neighbors(&self, node: &str) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        self.query_edges(
            "SELECT edge_id, src, relation, dst, valid_from, valid_to, \
                ingested_at, invalidated_at, invalidated_by, origin, confidence_milli \
             FROM edges \
             WHERE (src = ?1 OR dst = ?1) AND invalidated_at IS NULL \
             ORDER BY edge_id ASC",
            &[&node as &dyn rusqlite::ToSql],
        )
    }

    /// Bi-temporal edge query for `node` (spec §5). Both `AsOf` axes are optional
    /// `WHERE` filters layered on the persisted edges:
    /// - `valid_time` t → `valid_from <= t AND (valid_to IS NULL OR t < valid_to)`
    ///   ("true in the world at t").
    /// - `known_as_of` t → `ingested_at <= t AND (invalidated_at IS NULL OR
    ///   t < invalidated_at)` ("known at t").
    ///
    /// When BOTH axes are `None`, returns the current graph (`invalidated_at IS
    /// NULL`), identical to [`EventLog::neighbors`]. Query timestamps are
    /// normalized with [`crate::graph::normalize_ts`] so TEXT comparison is
    /// chronological. `ORDER BY edge_id ASC`.
    pub fn as_of(
        &self,
        node: &str,
        as_of: &crate::graph::AsOf,
    ) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        let mut sql = String::from(
            "SELECT edge_id, src, relation, dst, valid_from, valid_to, \
                ingested_at, invalidated_at, invalidated_by, origin, confidence_milli \
             FROM edges WHERE (src = ?1 OR dst = ?1)",
        );

        // F1 (clippy `redundant_closure` trap): normalize_ts takes `&str` but
        // the closure arg is `&String`; `.as_str()` makes the deref explicit so
        // clippy does NOT suggest `.map(normalize_ts)` (which would compile-fail).
        let valid = as_of.valid_time.as_ref().map(|t| crate::graph::normalize_ts(t.as_str()));
        let known = as_of.known_as_of.as_ref().map(|t| crate::graph::normalize_ts(t.as_str()));

        // Owned, normalized param strings kept alive for the bind slice below.
        let mut owned: Vec<String> = Vec::new();

        // SQL params are 1-indexed; ?1 is `node`, so the k-th owned timestamp
        // binds to ?{owned.len()+2}. Both `+2` sites below share this invariant.
        match (&valid, &known) {
            (None, None) => sql.push_str(" AND invalidated_at IS NULL"),
            _ => {
                if let Some(t) = &valid {
                    let i = owned.len() + 2; // ?1 is node
                    sql.push_str(&format!(
                        " AND valid_from <= ?{i} AND (valid_to IS NULL OR ?{i} < valid_to)"
                    ));
                    owned.push(t.clone());
                }
                if let Some(t) = &known {
                    let i = owned.len() + 2;
                    sql.push_str(&format!(
                        " AND ingested_at <= ?{i} AND (invalidated_at IS NULL OR ?{i} < invalidated_at)"
                    ));
                    owned.push(t.clone());
                }
            }
        }
        sql.push_str(" ORDER BY edge_id ASC");

        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + owned.len());
        params.push(&node as &dyn rusqlite::ToSql);
        for t in &owned {
            params.push(t as &dyn rusqlite::ToSql);
        }
        self.query_edges(&sql, &params)
    }

    /// Run a SELECT that returns the full edge column list (in the fixed order
    /// used by [`EventLog::all_edges`]) and map rows to [`crate::graph::Edge`].
    /// Shared by `all_edges`, `neighbors`, and `as_of` so the column→field
    /// mapping is single-sourced.
    fn query_edges(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<crate::graph::Edge>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| {
            Ok(crate::graph::Edge {
                edge_id: r.get(0)?,
                src: r.get(1)?,
                relation: r.get(2)?,
                dst: r.get(3)?,
                valid_from: r.get(4)?,
                valid_to: r.get(5)?,
                ingested_at: r.get(6)?,
                invalidated_at: r.get(7)?,
                invalidated_by: r.get(8)?,
                origin: r.get(9)?,
                confidence_milli: r.get(10)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Map every node within `max_hops` of any `seed` (over CURRENT edges,
    /// treated as undirected for relatedness) to its shortest hop distance
    /// (1..=max_hops). Seeds themselves are excluded. Used by the recall
    /// graph-proximity boost. A seed with no current edges contributes nothing.
    fn current_neighbors_with_hops(
        &self,
        seeds: &[String],
        max_hops: u32,
    ) -> Result<HashMap<String, u32>, BossclawError> {
        let mut hops: HashMap<String, u32> = HashMap::new();
        let mut frontier: HashSet<String> = seeds.iter().cloned().collect();
        let mut visited: HashSet<String> = seeds.iter().cloned().collect();
        for hop in 1..=max_hops {
            if frontier.is_empty() {
                break;
            }
            let next = self.current_adjacent(&frontier)?;
            let mut new_frontier: HashSet<String> = HashSet::new();
            for id in next {
                if visited.insert(id.clone()) {
                    hops.insert(id.clone(), hop);
                    new_frontier.insert(id);
                }
            }
            frontier = new_frontier;
        }
        Ok(hops)
    }

    /// Distinct opposite endpoints of CURRENT edges incident to any id in
    /// `frontier` (undirected: returns both `dst` where `src ∈ frontier` and
    /// `src` where `dst ∈ frontier`). Empty `frontier` → empty set.
    ///
    /// **Trust gate (spec §7 / Rev 2 M4+F3):** only edges that pass
    /// `origin = 'manual' OR (origin = 'machine' AND confidence_milli >= ?)`
    /// contribute the proximity boost — manual edges always, machine edges only
    /// when their integer `confidence_milli` clears the threshold derived from
    /// [`crate::extract::TRUST_MIN`] (= 600). Low-confidence machine edges are
    /// still recorded + queryable (never-forget), but do NOT tilt recall. The
    /// threshold is an INTEGER **bound as a SQL parameter** (never `format!`-ed
    /// into the SQL) — both the F3 signing-integrity contract and SQLi hygiene.
    /// `confidence_milli` is NULL for manual edges, so the `origin = 'manual'` arm
    /// matches them regardless (NULL never satisfies `>= ?`, which is why the OR
    /// is structured this way).
    ///
    /// Both `IN` clauses share the same `?1..?n` placeholders (the id list bound
    /// ONCE — n params, not 2n, which would exceed the statement's parameter
    /// count); the trust threshold is bound ONCE at `?{n+1}` and referenced in
    /// both halves of the `UNION`.
    fn current_adjacent(
        &self,
        frontier: &HashSet<String>,
    ) -> Result<HashSet<String>, BossclawError> {
        if frontier.is_empty() {
            return Ok(HashSet::new());
        }
        let ids: Vec<&String> = frontier.iter().collect();
        let placeholders: String =
            (0..ids.len()).map(|i| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
        // Trust threshold bound as the parameter AFTER the id placeholders.
        let trust_param = format!("?{}", ids.len() + 1);
        let trust = format!(
            "(origin = 'manual' OR (origin = 'machine' AND confidence_milli >= {trust_param}))"
        );
        // dst where src ∈ frontier  UNION  src where dst ∈ frontier (current +
        // trust-gated only). Both IN clauses reference the SAME ?1..?n
        // placeholders; the trust threshold is the SAME ?{n+1} in both halves.
        let sql = format!(
            "SELECT dst AS other FROM edges \
               WHERE invalidated_at IS NULL AND {trust} AND src IN ({placeholders}) \
             UNION \
             SELECT src AS other FROM edges \
               WHERE invalidated_at IS NULL AND {trust} AND dst IN ({placeholders})"
        );
        // Integer trust threshold derived from the documented f32 TRUST_MIN via the
        // SAME single-sourced quantizer as the encode side (Rev 2 F3 / review I1):
        // TRUST_MIN stays an f32 used ONLY to derive this integer = 600, and encode
        // ⇄ threshold can never diverge.
        let trust_min_milli = crate::extract::to_confidence_milli(crate::extract::TRUST_MIN);
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| *id as &dyn rusqlite::ToSql).collect();
        params.push(&trust_min_milli as &dyn rusqlite::ToSql);
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
        let mut out = HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// Derive + store the resolution vector for an `entity` node under
    /// `(entity_id, model_id)` in a dedicated `entity_vectors` table. Separate
    /// from `vectors` (which feeds recall) so the two indexes never bleed. The
    /// `text` is the entity's label (+ optionally aliases) — what future mentions
    /// are matched against. Idempotent upsert.
    pub fn derive_entity_vector(
        &self,
        embedder: &dyn Embedder,
        entity_id: &str,
        text: &str,
    ) -> Result<(), BossclawError> {
        let embedding = embed_one(embedder, text)?;
        let blob = vec_to_blob(&embedding);
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT OR REPLACE INTO entity_vectors (entity_id, model_id, dim, embedding)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![entity_id, embedder.model_id(), embedder.dim() as i64, blob],
        )?;
        Ok(())
    }

    /// Rebuild the in-memory entity-resolution index from `entity_vectors` for
    /// the active model (zero plaintext index on disk; rebuilt on open — same
    /// mechanism as [`EventLog::rebuild_indexes`]). Serial insertion over
    /// `entity_id ASC` for reproducibility.
    pub fn rebuild_entity_index(&self, embedder: &dyn Embedder) -> Result<(), BossclawError> {
        let rows = self.entity_vectors_for_model(embedder.model_id())?;
        let mut index = HnswIndex::with_capacity(rows.len());
        for (entity_id, vec) in rows {
            index.add(&entity_id, &vec);
        }
        let boxed: Box<dyn VectorIndex> = Box::new(index);
        *self.entity_index.lock().expect(POISON) = Some(boxed);
        Ok(())
    }

    /// All entity vectors for `model_id` as `(entity_id, vector)` pairs, ordered
    /// `entity_id ASC` (deterministic rebuild order). Mirrors
    /// [`EventLog::vectors_for_model`] but over the `entity_vectors` table.
    pub fn entity_vectors_for_model(
        &self,
        model_id: &str,
    ) -> Result<Vec<(String, Vec<f32>)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT entity_id, embedding FROM entity_vectors WHERE model_id = ?1 \
             ORDER BY entity_id ASC",
        )?;
        let rows = stmt.query_map([model_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, blob) = row?;
            out.push((id, blob_to_vec(&blob)?));
        }
        Ok(out)
    }

    /// Rung-3 Phase-1 (§7.1): embed + persist a captured session's body passages
    /// under `event_id`, ONE row per chunk `ix`, into the dedicated
    /// `session_passage_vectors` table (the conflict index's restart-safe source).
    /// Mirrors [`EventLog::derive_entity_vector`] — same blob encoding, `model_id`
    /// and `dim` taken from `embedder`. A SEPARATE table from `vectors` (recall)
    /// and `entity_vectors` (resolution), so persisting here never perturbs the
    /// recall index. Idempotent upsert per `(event_id, ix, model_id)`.
    pub fn store_session_passages(
        &self,
        embedder: &dyn Embedder,
        event_id: &str,
        chunks: &[String],
    ) -> Result<(), BossclawError> {
        // Embed first (no store lock needed), then upsert each — the same
        // "embed, then lock, then insert" ordering as `derive_entity_vector`.
        let mut rows: Vec<(usize, Vec<u8>)> = Vec::with_capacity(chunks.len());
        for (ix, chunk) in chunks.iter().enumerate() {
            rows.push((ix, vec_to_blob(&embed_one(embedder, chunk)?)));
        }
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        // ALL-OR-NOTHING: one transaction over the whole passage set, not N implicit
        // ones. A mid-loop failure must never leave a PARTIAL set — `session_passages_absent`'s
        // "any row ⇒ done" gate would then treat that partial as complete forever. Also collapses
        // N fsyncs into one.
        let tx = conn.unchecked_transaction()?;
        for (ix, blob) in &rows {
            tx.execute(
                "INSERT OR REPLACE INTO session_passage_vectors \
                 (session_captured_event_id, passage_ix, model_id, dim, embedding) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![event_id, *ix as i64, embedder.model_id(), embedder.dim() as i64, blob],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// All session-passage vectors for `model_id` as `(event_id, passage_ix,
    /// vector)` triples, ordered `session_captured_event_id, passage_ix ASC`.
    /// Mirrors [`EventLog::entity_vectors_for_model`] but over
    /// `session_passage_vectors` and carrying the passage index (the tuple shape
    /// the Task-6/7 conflict index consumes).
    pub fn session_passages_for_model(
        &self,
        model_id: &str,
    ) -> Result<Vec<(String, usize, Vec<f32>)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT session_captured_event_id, passage_ix, embedding \
             FROM session_passage_vectors WHERE model_id = ?1 \
             ORDER BY session_captured_event_id, passage_ix ASC",
        )?;
        let rows = stmt.query_map([model_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (event_id, ix, blob) = row?;
            out.push((event_id, ix as usize, blob_to_vec(&blob)?));
        }
        Ok(out)
    }

    /// True when NO passage rows exist for `event_id` (existence probe over any
    /// model). The capture path uses this to persist passages on the FIRST
    /// capture and SKIP re-embedding on a same-`sha` recapture that already has
    /// rows.
    pub fn session_passages_absent(&self, event_id: &str) -> Result<bool, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let present: Option<i64> = store
            .conn()
            .query_row(
                "SELECT 1 FROM session_passage_vectors \
                 WHERE session_captured_event_id = ?1 LIMIT 1",
                rusqlite::params![event_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(present.is_none())
    }

    /// Count of `session_passage_vectors` rows for `event_id`, over ANY model
    /// (model-AGNOSTIC — a retire targets a logical `(session_id, passage_id)`
    /// position, independent of the active model). [`EventLog::retire_passage`]
    /// uses this to range-check a `passage_id` BEFORE any recall rebuild, so it
    /// deliberately does NOT scope by `model_id`.
    ///
    /// Safety of the un-scoped count: with multiple models embedding the same
    /// capture, `COUNT(*)` can only OVER-count (it sums a passage across model_ids);
    /// it can never under-count. An over-permissive range check merely admits an
    /// INERT `passage_retired` marker for a `passage_id` that matches nothing at
    /// model-scoped [`EventLog::rebuild_conflict_index`] time — harmless. Tightening
    /// this to model-scoped would instead wrongly REJECT valid passages (a capture
    /// whose passages were embedded only under a since-swapped model), so it MUST
    /// stay model-agnostic.
    pub fn session_passage_count(&self, event_id: &str) -> Result<usize, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let n: i64 = store.conn().query_row(
            "SELECT COUNT(*) FROM session_passage_vectors \
             WHERE session_captured_event_id = ?1",
            rusqlite::params![event_id],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Search the entity-resolution index for the `k` nearest `(entity_id,
    /// distance)` pairs to `mention`'s embedding. ONLY entity nodes are searched
    /// (the index holds only `entity_vectors`). Returns [`BossclawError::InvalidInput`]
    /// if the entity index was never built.
    pub fn entity_search(
        &self,
        embedder: &dyn Embedder,
        mention: &str,
        k: usize,
    ) -> Result<Vec<(String, f32)>, BossclawError> {
        let query = embed_one(embedder, mention)?;
        let guard = self.entity_index.lock().expect(POISON);
        match guard.as_ref() {
            Some(index) => Ok(index.search(&query, k)),
            None => Err(BossclawError::InvalidInput(
                "entity index not built — call rebuild_entity_index".into(),
            )),
        }
    }

    /// Rebuild the SEPARATE conflict index (Rung-3 §7.1) from the
    /// `session_passage_vectors` table: one vector per CURRENT session's live
    /// passage, keyed by `(session_id, passage_ix)`. Mirrors
    /// [`EventLog::rebuild_entity_index`] but writes ONLY `conflict_index` — the
    /// recall `vector_index` and the resolution `entity_index` are byte-untouched.
    ///
    /// A passage row is keyed on its capture `event_id`; the index is keyed on the
    /// session's stable `session_id`, so rows are mapped through the fold head
    /// (`fold.current`). Rows whose `event_id` is NOT a current head (superseded /
    /// tombstoned / orphaned captures) are SKIPPED, and any `(session_id,
    /// passage_ix)` in `fold.retired_passages` is EXCLUDED at build time (a retired
    /// passage is simply never added).
    pub fn rebuild_conflict_index(&self, embedder: &dyn Embedder) -> Result<(), BossclawError> {
        let fold = fold_sessions(&self.session_events_ordered()?);
        // event_id → session_id for the CURRENT (non-superseded, non-tombstoned)
        // sessions; rows outside this map are not part of any current session.
        let sid_of: std::collections::HashMap<&str, &str> = fold
            .current
            .iter()
            .map(|cs| (cs.event_id.as_str(), cs.session_id.as_str()))
            .collect();
        let rows = self.session_passages_for_model(embedder.model_id())?;
        let mut index = HnswIndex::with_capacity(rows.len());
        for (event_id, ix, vec) in rows {
            let Some(&session_id) = sid_of.get(event_id.as_str()) else {
                continue; // orphaned / superseded / tombstoned capture — skip
            };
            // `.to_string()` allocates a throwaway key, so gate it behind the common
            // no-retirements case — only pay the alloc when retirements actually exist.
            if !fold.retired_passages.is_empty()
                && fold.retired_passages.contains(&(session_id.to_string(), ix))
            {
                continue; // retired passage — excluded at rebuild time
            }
            // `encode_chunk_key`'s param is named `event_id`, but here we key by
            // session_id BY DESIGN: an A5-validated session_id is `[A-Za-z0-9_-]`
            // (see bossclawd `valid_session_id`), so it can never contain
            // CHUNK_KEY_SEP (`\u{1f}`) — the key round-trips losslessly through
            // `decode_chunk_key` in `conflict_search`.
            index.add(&crate::index::encode_chunk_key(session_id, ix), &vec);
        }
        // Note arm (Rung-3 Phase-2 §2): add each CURRENT, non-superseded, non-retired memory
        // note's body vector under a DISTINCT note key so notes and passages share ONE fights
        // index without colliding. `current_notes` already applies the supersede + `note_retired`
        // exclusion, so no extra filtering is needed here. Empty-body notes cannot contradict on
        // content and are skipped (a zero-length body would embed to a meaningless vector).
        //
        // I4 / design open-question #2: we RE-EMBED every current note each rebuild rather than
        // persisting note vectors in a dedicated `note_conflict_vectors` table (like
        // `session_passage_vectors`). This is acceptable at Phase-2 scale because the embedder is a
        // STATIC model2vec (a token-vector lookup + mean-pool, NOT a transformer forward pass), so
        // per-note cost is a cheap table lookup. The `note_conflict_vectors` table is DEFERRED (add
        // only if this cost proves material). The `log::debug!` below is the trip-wire that makes
        // the re-embed count observable before any production-enable.
        let mut notes_embedded = 0usize;
        for note in self.current_notes()? {
            if note.text.trim().is_empty() {
                continue;
            }
            let vec = embed_one(embedder, &note.text)?;
            index.add(&crate::index::encode_note_key(&note.event_id), &vec);
            notes_embedded += 1;
        }
        log::debug!("rebuild_conflict_index: re-embedded {notes_embedded} note bodies (I4 trip-wire)");
        let boxed: Box<dyn VectorIndex> = Box::new(index);
        *self.conflict_index.lock().expect(POISON) = Some(boxed);
        Ok(())
    }

    /// Search the SEPARATE conflict index (Rung-3 §7.1) for the `k` nearest
    /// `(session_id, passage_ix, distance)` triples to the query vector `qv`.
    /// Mirrors [`EventLog::entity_search`] but takes a pre-computed query vector and
    /// returns a plain `Vec`.
    ///
    /// Callers MUST call [`EventLog::rebuild_conflict_index`] first. An unbuilt
    /// index yields an EMPTY result BY DESIGN — "no index → no hits" is a deliberate
    /// non-fatal policy for this detector (in release, "no index built yet" simply
    /// flags no conflicts), whereas entity resolution errors on an unbuilt index. A
    /// `debug_assert` guards the "called before rebuild" bug in dev/test, where it
    /// is always a mistake. Keys that fail to decode are dropped — since Rung-3 Phase-2
    /// the UNIFIED index also holds NOTE keys (`encode_note_key`), which `decode_chunk_key`
    /// returns `None` for; that intentional drop is what preserves this method's passage-only
    /// tuple contract for the memharness caller. NOTE the behavioral consequence: `index.search`
    /// returns the `k` nearest keys BEFORE this passage filter, so a note ranking in the top-`k`
    /// consumes a slot — `conflict_search(qv, k)` can therefore return FEWER than `k` passages
    /// once the index is note-populated. Callers needing typed note+passage hits use
    /// [`EventLog::conflict_search_refs`].
    pub fn conflict_search(&self, qv: &[f32], k: usize) -> Vec<(String, usize, f32)> {
        let guard = self.conflict_index.lock().expect(POISON);
        debug_assert!(
            guard.is_some(),
            "conflict_search called before rebuild_conflict_index"
        );
        let Some(index) = guard.as_ref() else {
            return Vec::new();
        };
        index
            .search(qv, k)
            .into_iter()
            .filter_map(|(key, score)| {
                crate::index::decode_chunk_key(&key).map(|(sid, ix)| (sid.to_string(), ix, score))
            })
            .collect()
    }

    /// Typed sibling of [`EventLog::conflict_search`] (Rung-3 Phase-2 §2): returns the `k` nearest
    /// `(ConflictRef, distance)` pairs over the UNIFIED conflict index, decoding both note keys and
    /// passage chunk keys. `conflict_search` (passage-only tuples) is left byte-identical for the
    /// harness caller. Same empty-when-unbuilt policy + `debug_assert` as `conflict_search`.
    pub fn conflict_search_refs(&self, qv: &[f32], k: usize) -> Vec<(crate::index::ConflictRef, f32)> {
        let guard = self.conflict_index.lock().expect(POISON);
        debug_assert!(guard.is_some(), "conflict_search_refs called before rebuild_conflict_index");
        let Some(index) = guard.as_ref() else {
            return Vec::new();
        };
        index
            .search(qv, k)
            .into_iter()
            .filter_map(|(key, score)| crate::index::ConflictRef::decode_key(&key).map(|r| (r, score)))
            .collect()
    }

    /// Run ONE conflict-detection cycle (spec §3.3). Gated on the owner flag (I3). Advances the
    /// `(seq, subject_offset)` cursor past each FULLY-judged subject, so a budget-truncated or
    /// reasoner-interrupted cycle resumes exactly (I6). `passage_text(session_id, passage_id)`
    /// supplies a passage's real text (the daemon reads the `.md`); note text comes from core.
    /// `resolution_excluded_refs` is EMPTY in Phase 2 (Phase 3 fills it). `detected_at` stamps each
    /// proposal. `#[cfg(unix)]` (uses `append_conflict_proposal` / the idempotency fold).
    #[cfg(unix)]
    pub fn detect_conflicts_once(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
        passage_text: &dyn Fn(&str, usize) -> Option<String>,
        resolution_excluded_refs: &std::collections::HashSet<String>,
        detected_at: i64,
    ) -> Result<ConflictDetectReport, BossclawError> {
        use crate::conflict::{
            bound_judge_text, confidence_band, decide_conflict_sweep, templated_why, winner_str,
            FinderInput, CANDIDATE_SIM_MIN, CONFLICT_JUDGE_PER_SWEEP, CONFLICT_OPEN_CEILING,
            CONFLICT_PAIR_ERROR_BUDGET, CONFLICT_SCAN_BOUND, CONFLICT_SEARCH_K,
            MAX_CANDIDATE_PAIRS_PER_SUBJECT,
        };
        use crate::index::ConflictRef;
        let mut report = ConflictDetectReport::default();

        // De-conflict with the extract path (spec §8): Rung-3 memory-level detection (this method) and
        // the evolve/extract path's EDGE-level reconciliation (`ProposedRetraction` →
        // `reconcile_confirmed_contradiction`) are two INDEPENDENT, complementary axes. There is no
        // reverse "memory-claim → invalidated-edge" index and Phase 2 adds none; de-dup happens only
        // WITHIN rung-3 via the proposal-idempotency fold (`is_conflict_proposal_suppressed`).

        // (1) Gate FIRST — no scan, no rebuild, no model when CLOSED (I3).
        if !self.conflict_detect_enabled()? {
            report.skipped_disabled = true;
            return Ok(report);
        }

        // (2) Dirty-gate: nothing new since the cursor → no rebuild, no model (I4).
        let (cursor_seq, cursor_off) = self.conflict_cursor()?;
        let subjects =
            self.unprocessed_conflict_subjects_since(cursor_seq, cursor_off, CONFLICT_SCAN_BOUND)?;
        if subjects.is_empty() {
            return Ok(report);
        }
        report.scanned_subjects = subjects.len();

        // (3) Rebuild the unified fights index so BOTH sides of any conflict are present + current.
        self.rebuild_conflict_index(embedder)?;

        // (4) One-shot lookups: the passage-vector map, the session fold, and the OPEN pair set.
        let mut passage_vec: std::collections::HashMap<(String, usize), Vec<f32>> =
            std::collections::HashMap::new();
        let fold = fold_sessions(&self.session_events_ordered()?);
        let head_of: std::collections::HashMap<String, String> = fold
            .current
            .iter()
            .map(|cs| (cs.session_id.clone(), cs.event_id.clone()))
            .collect();
        let session_of_event: std::collections::HashMap<String, String> = fold
            .current
            .iter()
            .map(|cs| (cs.event_id.clone(), cs.session_id.clone()))
            .collect();
        for (event_id, ix, vec) in self.session_passages_for_model(embedder.model_id())? {
            if let Some(sid) = session_of_event.get(&event_id) {
                passage_vec.insert((sid.clone(), ix), vec);
            }
        }
        let opens = self.open_conflict_proposals()?;
        let mut open_pairs: std::collections::HashSet<String> =
            opens.iter().map(|p| Self::conflict_pair_key(&p.a_ref, &p.b_ref)).collect();
        // Rung-3 Phase-3 (§2.2 item 1, I9): a kept-both / live-dismissed pair must never be re-proposed.
        // Union its `unordered_pair_key` into `open_pairs` (the SAME space the finder screens against) so the
        // pure finder needs zero reshape. NOT the single-ref `resolution_excluded_refs` param, which feeds
        // `decide_conflict_sweep`'s single-ref `excluded_refs` and would silently never match a pair key.
        let resolution = self.resolution_exclusions()?;
        open_pairs.extend(resolution.coexist_pairs.iter().cloned());
        open_pairs.extend(resolution.dismissed_pairs.iter().cloned());
        let mut open_count = opens.len();

        // Text / lineage / ts / kind resolvers for a ref (notes from core; passages via the closure).
        let ref_text = |r: &ConflictRef| -> Option<String> {
            match r {
                ConflictRef::Note { event_id } => self
                    .event_by_id(event_id)
                    .ok()
                    .flatten()
                    .and_then(|e| e.content.get("text").and_then(|t| t.as_str()).map(str::to_string))
                    .map(|t| bound_judge_text(&t).to_string()),
                ConflictRef::Passage { session_id, passage_id } => {
                    passage_text(session_id, *passage_id).map(|t| bound_judge_text(&t).to_string())
                }
            }
        };
        let ref_source_event = |r: &ConflictRef| -> Option<String> {
            match r {
                ConflictRef::Note { event_id } => Some(event_id.clone()),
                ConflictRef::Passage { session_id, .. } => head_of.get(session_id).cloned(),
            }
        };
        let ref_ts = |r: &ConflictRef| -> i64 {
            ref_source_event(r)
                .and_then(|id| self.event_by_id(&id).ok().flatten())
                .and_then(|e| DateTime::parse_from_rfc3339(&e.ts).ok().map(|d| d.timestamp()))
                .unwrap_or(0)
        };
        let ref_kind = |r: &ConflictRef| -> &'static str {
            match r {
                ConflictRef::Note { .. } => "note",
                ConflictRef::Passage { .. } => "passage",
            }
        };

        // (5) SUBJECT-BY-SUBJECT (the anti-stall fix). The cursor advances to (seq, within+1) after
        //     EACH fully-judged subject, so a crash / reasoner-stop mid-cycle resumes exactly (I6).
        let mut budget_left = CONFLICT_JUDGE_PER_SWEEP;
        for cs in &subjects {
            let subject = &cs.subject;
            let qv = match subject {
                ConflictRef::Note { event_id } => {
                    match self.event_by_id(event_id)?.and_then(|e| {
                        e.content.get("text").and_then(|t| t.as_str()).map(str::to_string)
                    }) {
                        Some(t) if !t.trim().is_empty() => embed_one(embedder, &t)?,
                        _ => {
                            self.set_conflict_cursor(cs.seq, cs.within_seq_id + 1)?;
                            continue;
                        }
                    }
                }
                ConflictRef::Passage { session_id, passage_id } => {
                    match passage_vec.get(&(session_id.clone(), *passage_id)) {
                        Some(v) => v.clone(),
                        None => {
                            // A transiently-unavailable passage vector is best-effort SKIPPED: advance
                            // the cursor past it rather than stall the whole cycle (a re-embed under a
                            // new model repopulates it and a fresh capture re-enqueues it downstream).
                            self.set_conflict_cursor(cs.seq, cs.within_seq_id + 1)?;
                            continue;
                        }
                    }
                }
            };
            let mut excluded_refs = resolution_excluded_refs.clone();
            excluded_refs.insert(subject.pair_key());
            // +1 for the guaranteed self-match slot: the subject's own vector is in the rebuilt index
            // at distance ~0, always returned then dropped by excluded_refs; +1 keeps a full `budget`
            // of REAL candidates (never-skip). decide_conflict_sweep still caps at
            // MAX_CANDIDATE_PAIRS_PER_SUBJECT == CONFLICT_JUDGE_PER_SWEEP, so pairs.len() <= budget.
            let neighbors = self.conflict_search_refs(&qv, CONFLICT_SEARCH_K + 1);
            let pairs = decide_conflict_sweep(&FinderInput {
                subject,
                neighbors: &neighbors,
                sim_min: CANDIDATE_SIM_MIN,
                excluded_refs: &excluded_refs,
                open_pairs: &open_pairs,
                max_pairs: MAX_CANDIDATE_PAIRS_PER_SUBJECT,
            });
            if pairs.len() > budget_left {
                report.budget_hit = true;
                break;
            }
            let mut subject_blocked = false; // an outstanding SUB-budget pair error holds the cursor (I6)
            for (a, b) in pairs {
                let (older, newer) = if ref_ts(&a) <= ref_ts(&b) { (a, b) } else { (b, a) };
                let pk = Self::conflict_pair_key(&older, &newer);
                // §3.3: a pair already at the budget is POISON — stop judging it entirely (consume no
                // budget, count nothing, do not re-error). It no longer holds the cursor
                // (`subject_blocked` stays false), so a deterministically-erroring pair can't stall the sweep.
                if self.conflict_pair_error_count(&pk)? >= CONFLICT_PAIR_ERROR_BUDGET {
                    continue;
                }
                let (Some(ot), Some(nt)) = (ref_text(&older), ref_text(&newer)) else {
                    report.dropped += 1;
                    continue;
                };
                report.judged += 1;
                budget_left -= 1;
                match crate::conflict::judge_pair(reasoner, &ot, &nt) {
                    Ok(Some(v)) => {
                        // Any successful judge (contradiction or not) clears the pair's error streak (§3.3).
                        self.reset_conflict_pair_error(&pk)?;
                        // [I7 FIX — do NOT use the plan's `log::debug!("... {}", v.why)`. This runs as
                        // a daemon under launchd/systemd, which capture BOTH stdout and stderr to
                        // persistent log files, so neither `log::` nor `eprintln!` is ephemeral. The
                        // model's raw `v.why` is attacker-influenceable free text, so it is DISCARDED
                        // ENTIRELY — never logged/printed. Only structured, non-sensitive verdict
                        // fields are traced. The persisted `why` (below) is the content-free template.]
                        log::debug!(
                            "conflict verdict: winner={} confidence={}",
                            winner_str(v.winner),
                            v.confidence
                        );
                        if open_count >= CONFLICT_OPEN_CEILING {
                            report.ceiling_hit = true;
                            continue;
                        }
                        // Durable authoritative re-check against PERSISTED proposals before the signed
                        // append — an independent-source guard (the in-memory open_pairs pre-filter in
                        // decide_conflict_sweep is the fast path that already screened candidates and
                        // saved judge calls; this backstops an open_pairs-maintenance bug on the
                        // highest-stakes write).
                        if self.is_conflict_proposal_suppressed(&older, &newer)? {
                            continue;
                        }
                        let why = templated_why(
                            winner_str(v.winner),
                            confidence_band(v.confidence),
                            ref_kind(&older),
                            ref_kind(&newer),
                        );
                        let sources: Vec<String> = [ref_source_event(&older), ref_source_event(&newer)]
                            .into_iter()
                            .flatten()
                            .collect();
                        self.append_conflict_proposal(
                            &older,
                            &newer,
                            winner_str(v.winner),
                            confidence_band(v.confidence),
                            &why,
                            detected_at,
                            &sources,
                        )?;
                        open_pairs.insert(pk);
                        open_count += 1;
                        report.proposed += 1;
                    }
                    Ok(None) => {
                        // A successful judge that declined still clears the streak (§3.3).
                        self.reset_conflict_pair_error(&pk)?;
                        report.dropped += 1;
                    }
                    Err(_) => {
                        // Pair-granular (§3.3): skip ONLY this pair; keep judging the subject's other pairs so
                        // a poison pair never hides a real sibling conflict. Persist the consecutive-error
                        // count so a DETERMINISTIC poison pair is bounded across cycles/restarts.
                        report.reasoner_errors += 1;
                        let n = self.bump_conflict_pair_error(&pk)?;
                        if n >= CONFLICT_PAIR_ERROR_BUDGET {
                            report.poison_skipped += 1; // give up on this pair; it no longer holds the cursor
                        } else {
                            subject_blocked = true; // sub-budget: retry this subject next cycle (I6)
                        }
                        continue;
                    }
                }
            }
            if subject_blocked {
                // A transient outage → do NOT advance past this subject; next cycle re-judges it. Once every
                // erroring pair reaches the budget (poison_skipped), `subject_blocked` stays false → advance.
                break;
            }
            self.set_conflict_cursor(cs.seq, cs.within_seq_id + 1)?;
        }
        Ok(report)
    }

    /// Resolve one entity `mention` against the existing entity nodes (spec §6):
    /// embed → search the entity index → convert distance to cosine similarity →
    /// [`crate::extract::resolve_decision`]; for the mid-band, ask `reasoner` to
    /// adjudicate and collapse its answer to a final [`crate::extract::ResolveDecision::Merge`]
    /// (a chosen candidate) or [`crate::extract::ResolveDecision::Mint`] (`"none"` / unknown id).
    ///
    /// The adjudication call is the ONLY model use here; merge/mint short-circuit
    /// without a model call (cheap + deterministic at the thresholds).
    pub fn resolve_mention(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
        mention: &str,
    ) -> Result<crate::extract::ResolveDecision, BossclawError> {
        use crate::extract::ResolveDecision;
        // DistCosine returns distance in [0, 2]; similarity = 1 - distance.
        let candidates: Vec<(String, f32)> = self
            .entity_search(embedder, mention, crate::extract::GRAPH_CONTEXT_K)?
            .into_iter()
            .map(|(id, dist)| (id, 1.0 - dist))
            .collect();
        match crate::extract::resolve_decision(&candidates) {
            ResolveDecision::Adjudicate(ids) => {
                let decided = self.adjudicate_entity(reasoner, mention, &ids)?;
                match decided {
                    Some(id) => Ok(ResolveDecision::Merge(id)),
                    None => Ok(ResolveDecision::Mint),
                }
            }
            other => Ok(other),
        }
    }

    /// Ask `reasoner` which of `candidate_ids` (if any) the `mention` refers to.
    /// Returns `Some(id)` for a chosen candidate that is actually in the list,
    /// `None` for `"none"` OR any id the model invented (defensive: a hallucinated
    /// id must not become a merge target). Uses the adjudication schema.
    fn adjudicate_entity(
        &self,
        reasoner: &dyn crate::reason::Reasoner,
        mention: &str,
        candidate_ids: &[String],
    ) -> Result<Option<String>, BossclawError> {
        let system = "You resolve entity coreference. Answer ONLY with the JSON the schema \
                      describes: the id of the candidate the mention refers to, or \"none\".";
        let prompt = crate::extract::build_adjudication_prompt(mention, candidate_ids);
        let answer = reasoner.complete_json(system, &prompt, &crate::reason::adjudication_schema())?;
        let chosen = answer.get("match").and_then(|m| m.as_str()).unwrap_or("none");
        if chosen == "none" {
            return Ok(None);
        }
        // Fail-closed: only accept an id the model was actually offered.
        Ok(candidate_ids.iter().find(|id| id.as_str() == chosen).cloned())
    }

    // ── Evolve loop (spec §8, Task 7) ────────────────────────────────────────

    /// Read the evolve cursor (the last processed `seq`); `0` if never set (the
    /// table is empty on a fresh store → no memory has been processed). The
    /// cursor is persistent progress state, NOT a fold (spec §4).
    pub fn evolve_cursor(&self) -> Result<i64, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let seq = conn
            .query_row("SELECT last_seq FROM evolve_cursor WHERE id = 0", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        Ok(seq)
    }

    /// Set the evolve cursor to `last_seq` (idempotent upsert of the single row).
    /// Persistent progress state — NOT rebuilt from events (spec §4). Losing it
    /// only re-processes events idempotently; it never corrupts the log.
    pub fn set_evolve_cursor(&self, last_seq: i64) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO evolve_cursor (id, last_seq) VALUES (0, ?1)
             ON CONFLICT(id) DO UPDATE SET last_seq = ?1",
            rusqlite::params![last_seq],
        )?;
        Ok(())
    }

    /// Read the conflict cursor `(last_seq, subject_offset)`; `(0, 0)` if never set. All subjects of
    /// the event at `last_seq` with within-seq id `< subject_offset` are fully judged (a note is one
    /// subject at id 0; a capture's passages use `passage_id`). Persistent progress state, NOT a fold
    /// (spec §3.2) — losing it only re-searches idempotently.
    pub fn conflict_cursor(&self) -> Result<(i64, usize), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let row: Option<(i64, i64)> = store
            .conn()
            .query_row(
                "SELECT last_seq, subject_offset FROM conflict_cursor WHERE id = 0",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(s, o)| (s, o as usize)).unwrap_or((0, 0)))
    }

    /// Advance the conflict cursor to `(last_seq, subject_offset)` (idempotent single-row upsert).
    pub fn set_conflict_cursor(
        &self,
        last_seq: i64,
        subject_offset: usize,
    ) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO conflict_cursor (id, last_seq, subject_offset) VALUES (0, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET last_seq = ?1, subject_offset = ?2",
            rusqlite::params![last_seq, subject_offset as i64],
        )?;
        Ok(())
    }

    /// Read a pair's consecutive-error count (0 if absent). §3.3.
    fn conflict_pair_error_count(&self, pair_key: &str) -> Result<usize, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let n: Option<i64> = store
            .conn()
            .query_row("SELECT consecutive_errors FROM conflict_pair_errors WHERE pair_key = ?1", [pair_key], |r| r.get(0))
            .optional()?;
        Ok(n.unwrap_or(0) as usize)
    }

    /// Increment a pair's consecutive-error count, returning the NEW value (§3.3).
    fn bump_conflict_pair_error(&self, pair_key: &str) -> Result<usize, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO conflict_pair_errors (pair_key, consecutive_errors) VALUES (?1, 1)
             ON CONFLICT(pair_key) DO UPDATE SET consecutive_errors = consecutive_errors + 1",
            [pair_key],
        )?;
        let n: i64 = store.conn().query_row(
            "SELECT consecutive_errors FROM conflict_pair_errors WHERE pair_key = ?1", [pair_key], |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Reset a pair's consecutive-error count to 0 (on any successful judge of that pair — §3.3).
    fn reset_conflict_pair_error(&self, pair_key: &str) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute("DELETE FROM conflict_pair_errors WHERE pair_key = ?1", [pair_key])?;
        Ok(())
    }

    /// Rewind the conflict cursor to the lexicographic `min` of its current position and `(seq, within)`
    /// (§3.2). MONOTONE — only ever moves the cursor BACK (never advances), so a re-examination is
    /// re-scheduled without ever skipping unrelated newer subjects. Idempotent upsert via
    /// [`Self::set_conflict_cursor`], so it works on a brain that never ran detection (cursor defaults to
    /// `(0, 0)`). Caller appends the unretire marker FIRST; a torn write here leaves the cursor un-rewound
    /// (a benign delay — the memory is current but re-examined only at the next natural sweep past it).
    fn rewind_conflict_cursor(&self, seq: i64, within: usize) -> Result<(), BossclawError> {
        let current = self.conflict_cursor()?;
        let target = (seq, within);
        if target < current {
            self.set_conflict_cursor(seq, within)?;
        }
        Ok(())
    }

    /// Set the evolve on/off switch by appending a control `config` event whose
    /// content is `{ "evolve_enabled": <enabled> }` (Rev 2 F2-sec(b)).
    ///
    /// This is the ONLY writer of the [`EVOLVE_ENABLED_KEY`] key — the off-switch
    /// is a PRIVILEGE, not arbitrary data, so it has a typed setter (the precedent
    /// is the active-model config written by [`EventLog::reembed_migration`]).
    /// Control config must not be written through a generic `append` in v1.
    /// The change is Ed25519-signed + hash-chained like every event, so a forged
    /// or replayed flip is tamper-evident via `verify_chain`. (M7 additionally
    /// verifies the signer DID == the resolved user owner before honoring it;
    /// `signed_by_did` is unverified today — spec §16 / M3 §12.1.)
    ///
    /// Carries NO model fields, so it never disturbs [`EventLog::active_model`]
    /// (which skips configs lacking `active_model_id`/`dim`/`schema_version`).
    pub fn set_evolve_enabled(&self, enabled: bool) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            // Explicit map so the key is the named const (json!{} cannot take a
            // const identifier as an object key).
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(EVOLVE_ENABLED_KEY.to_string(), serde_json::Value::Bool(enabled));
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// Persist the non-security reasoner config (mode/provider/model/base_url)
    /// as a signed `config` event (spec R1). CLONES the
    /// [`EventLog::set_evolve_enabled`] mechanism exactly — Ed25519-signed +
    /// hash-chained (so a forged/replayed write is tamper-evident via
    /// `verify_chain`), the only writer of [`REASONER_CONFIG_KEY`]. Webview writes
    /// route through a command, not a file — egress-adjacent config stays
    /// tamper-evident. Carries no model fields, so it never disturbs
    /// [`EventLog::active_model`].
    pub fn set_reasoner_config(&self, config: serde_json::Value) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            // Explicit map so the key is the named const (json!{} cannot take a
            // const identifier as an object key).
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(REASONER_CONFIG_KEY.to_string(), config);
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// Persist the signed cloud-enable consent record, binding
    /// {provider, base_url_host, key_fingerprint, consented_at}, as a signed
    /// `config` event (spec R1/R5). CLONES the [`EventLog::set_evolve_enabled`]
    /// mechanism exactly — Ed25519-signed + hash-chained (tamper-evident via
    /// `verify_chain`), the only writer of [`CLOUD_REASONER_CONSENT_KEY`]. Its
    /// presence is what authorizes egress, so it MUST be a signed event; written
    /// ONLY by the enable flow after the R5 test-key call succeeds (Task 12).
    /// Carries no model fields, so it never disturbs [`EventLog::active_model`].
    pub fn set_cloud_reasoner_consent(&self, record: serde_json::Value) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            // Explicit map so the key is the named const (json!{} cannot take a
            // const identifier as an object key).
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(CLOUD_REASONER_CONSENT_KEY.to_string(), record);
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// Persist the signed opt-in language-pack record (invariant I2). CLONES the
    /// [`EventLog::set_cloud_reasoner_consent`] mechanism exactly — Ed25519-signed + hash-chained
    /// (tamper-evident via `verify_chain`), the only writer of [`LANGUAGE_PACK_KEY`]. Carries no
    /// model fields, so it never disturbs [`EventLog::active_model`].
    pub fn set_language_pack_record(&self, record: &LanguagePackRecord) -> Result<(), BossclawError> {
        let value = serde_json::to_value(record)?;
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            // Explicit map so the key is the named const (json!{} cannot take a const identifier
            // as an object key).
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(LANGUAGE_PACK_KEY.to_string(), value);
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// The newest signed language-pack record, or `None` if never set (English default — I7).
    /// STICKY: the first `config` event (newest-first) carrying the key wins, mirroring
    /// [`EventLog::cloud_reasoner_consent_json`].
    pub fn language_pack_record(&self) -> Result<Option<LanguagePackRecord>, BossclawError> {
        match self.latest_config_value(LANGUAGE_PACK_KEY)? {
            Some(v) => Ok(Some(serde_json::from_value(v)?)),
            None => Ok(None),
        }
    }

    /// Whether the evolve loop is enabled (spec §8 off-switch / Rev 2 F2-sec(a)).
    ///
    /// STICKY / fail-closed semantics: config events are scanned newest-first and
    /// the FIRST one that carries an explicit `evolve_enabled` bool wins. Because
    /// [`EventLog::set_evolve_enabled`] is the only writer of the key, this is
    /// exactly "the latest EXPLICIT value": once an explicit `false` exists with
    /// no LATER explicit `true`, the loop stays disabled — a flag-LESS newer
    /// config (e.g. an active-model switch) does NOT silently re-arm the loop.
    /// Default-open (`true`) ONLY when the flag was never set at all.
    ///
    /// Honored BEFORE any model call in [`EventLog::evolve_once`].
    pub fn evolve_enabled(&self) -> Result<bool, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            if let Some(flag) = ev.content.get(EVOLVE_ENABLED_KEY).and_then(|v| v.as_bool()) {
                return Ok(flag); // newest explicit flag wins → sticky
            }
        }
        Ok(true) // flag never set → default open
    }

    /// The newest signed `reasoner_config` value, or `None` if never set
    /// (default: Local reasoner, no cloud — spec R1). STICKY: the first `config`
    /// event (newest-first) carrying the key wins.
    pub fn reasoner_config_json(&self) -> Result<Option<serde_json::Value>, BossclawError> {
        self.latest_config_value(REASONER_CONFIG_KEY)
    }

    /// The newest signed cloud-enable consent record, or `None` if never set
    /// (default-CLOSED: no egress — spec R1/R5). STICKY: the first `config` event
    /// (newest-first) carrying the key wins. Unlike [`EventLog::evolve_enabled`]
    /// (default-OPEN), absence here means egress is FORBIDDEN.
    pub fn cloud_reasoner_consent_json(&self) -> Result<Option<serde_json::Value>, BossclawError> {
        self.latest_config_value(CLOUD_REASONER_CONSENT_KEY)
    }

    /// Shared scan returning the newest `config` event's value under `key`, or
    /// `None` when no `config` event carries it (default-CLOSED). CLONES the
    /// locking + SQL idiom of [`EventLog::evolve_enabled`] exactly — config events
    /// scanned newest-first, deserialized, first hit wins (sticky). Backs both
    /// [`EventLog::reasoner_config_json`] and [`EventLog::cloud_reasoner_consent_json`].
    fn latest_config_value(&self, key: &str) -> Result<Option<serde_json::Value>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            if let Some(v) = ev.content.get(key) {
                return Ok(Some(v.clone())); // newest config carrying the key wins → sticky
            }
        }
        Ok(None) // key never set → default closed
    }

    /// Set the M6b reconciliation-proposer on/off switch by appending a control
    /// `config` event whose content is `{ "proposals_enabled": <enabled> }`
    /// (M6b §5.3). CLONES the [`EventLog::set_evolve_enabled`] mechanism exactly —
    /// the only writer of [`PROPOSALS_ENABLED_KEY`], a signed + hash-chained
    /// control config (so a forged/replayed flip is tamper-evident via
    /// `verify_chain`). INDEPENDENT of the evolve switch: this gates ONLY the
    /// autonomous `write_proposal` synthesis, never the curation loop. Carries no
    /// model fields, so it never disturbs [`EventLog::active_model`].
    pub fn set_proposals_enabled(&self, enabled: bool) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            // Explicit map so the key is the named const (json!{} cannot take a
            // const identifier as an object key).
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(PROPOSALS_ENABLED_KEY.to_string(), serde_json::Value::Bool(enabled));
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// Whether the M6b reconciliation proposer is enabled (M6b §5.3 off-switch).
    ///
    /// STICKY / fail-closed semantics IDENTICAL to [`EventLog::evolve_enabled`]:
    /// config events are scanned newest-first and the FIRST one carrying an
    /// explicit `proposals_enabled` bool wins. Because [`EventLog::set_proposals_enabled`]
    /// is the only writer of the key, this is exactly "the latest EXPLICIT value":
    /// once an explicit `false` exists with no LATER explicit `true`, proposals stay
    /// off — a flag-LESS newer config (e.g. an active-model switch, or an
    /// `evolve_enabled` flip) does NOT silently re-arm proposals. Default-open
    /// (`true`) ONLY when the flag was never set at all.
    ///
    /// Read once near the top of [`EventLog::evolve_once`] (after the evolve
    /// off-switch); when `false`, the confirmed-contradiction loop still emits every
    /// `invalidate` but skips the reconciliation synthesis entirely.
    pub fn proposals_enabled(&self) -> Result<bool, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            if let Some(flag) = ev.content.get(PROPOSALS_ENABLED_KEY).and_then(|v| v.as_bool()) {
                return Ok(flag); // newest explicit flag wins → sticky
            }
        }
        Ok(true) // flag never set → default open
    }

    /// Was a config flag ever EXPLICITLY set (regardless of its value)? Scans `config` events for
    /// a bool under the flag's key, returning true on the first hit. Distinguishes the engine's
    /// never-set default-open from a user's explicit choice — the desktop `prime_switches` needs to
    /// avoid clobbering a user `true` on every launch (SP4 change-b). A typed `ConfigFlag` (not a
    /// raw key string) keeps the const single-sourced (M2/MIN-4).
    pub fn explicitly_set(&self, flag: ConfigFlag) -> Result<bool, BossclawError> {
        let key = flag.key();
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            if ev.content.get(key).and_then(|v| v.as_bool()).is_some() {
                return Ok(true); // some config event carries this key as a bool ⇒ explicit
            }
        }
        Ok(false)
    }

    /// Set the M6c mandate-proposer on/off switch by appending a control
    /// `config` event whose content is `{ "mandates_enabled": <enabled> }`
    /// (M6c §5.5 / D8). CLONES the [`EventLog::set_evolve_enabled`] mechanism exactly —
    /// the only writer of [`MANDATES_ENABLED_KEY`], a signed + hash-chained
    /// control config (so a forged/replayed flip is tamper-evident via
    /// `verify_chain`). INDEPENDENT of the evolve and proposals switches: this gates
    /// ONLY the autonomous mandate-driven `write_proposal` synthesis, never the
    /// curation loop. Carries no model fields, so it never disturbs
    /// [`EventLog::active_model`].
    pub fn set_mandates_enabled(&self, enabled: bool) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            // Explicit map so the key is the named const (json!{} cannot take a
            // const identifier as an object key).
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(MANDATES_ENABLED_KEY.to_string(), serde_json::Value::Bool(enabled));
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// Whether the M6c mandate proposer is enabled (M6c §5.5 / D8 off-switch).
    ///
    /// STICKY / fail-closed semantics IDENTICAL to [`EventLog::evolve_enabled`]:
    /// config events are scanned newest-first and the FIRST one carrying an
    /// explicit `mandates_enabled` bool wins. Because [`EventLog::set_mandates_enabled`]
    /// is the only writer of the key, this is exactly "the latest EXPLICIT value":
    /// once an explicit `false` exists with no LATER explicit `true`, mandates stay
    /// off — a flag-LESS newer config (e.g. an active-model switch, or a
    /// `proposals_enabled` flip) does NOT silently re-arm mandates. Default-open
    /// (`true`) ONLY when the flag was never set at all.
    pub fn mandates_enabled(&self) -> Result<bool, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT payload FROM events WHERE event_type = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map([CONFIG_EVENT_TYPE], |r| r.get::<_, String>(0))?;
        for row in rows {
            let ev: Event = serde_json::from_str(&row?)?;
            if let Some(flag) = ev.content.get(MANDATES_ENABLED_KEY).and_then(|v| v.as_bool()) {
                return Ok(flag); // newest explicit flag wins → sticky
            }
        }
        Ok(true) // flag never set → default open
    }

    /// Whether ongoing session capture is enabled (spec §6a — critic Critical C1's default-CLOSED
    /// flag). STICKY / fail-closed, reusing [`EventLog::latest_config_value`]'s newest-first scan:
    /// the newest `config` event carrying an explicit `capture_enabled` bool wins. UNLIKE
    /// [`EventLog::mandates_enabled`] (default-OPEN), the default here is CLOSED — a never-set flag
    /// returns `false`, so capture never runs for a user who never consented (I10) even if the boot
    /// force-off cascade never ran. Because [`EventLog::set_capture_enabled`] is the only writer of
    /// the key and always writes a bool, the value is never absent-but-present.
    pub fn capture_enabled(&self) -> Result<bool, BossclawError> {
        Ok(self
            .latest_config_value(ConfigFlag::CaptureEnabled.key())?
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Whether the one-time historical-backfill consent was granted (spec §6a — critic Major M4).
    /// Default CLOSED, mirroring [`EventLog::capture_enabled`]: a user who declined history at
    /// Connect — or never connected — reads `false` here, so the sweeper never imports the backlog
    /// that predates [`EventLog::capture_enabled_at`]. Disabling capture clears this back to `false`
    /// (the consent is one-time and spent — see [`EventLog::set_capture_enabled`]), so a later
    /// forward-only re-enable cannot silently resurrect the backlog.
    pub fn backfill_consented(&self) -> Result<bool, BossclawError> {
        Ok(self
            .latest_config_value(ConfigFlag::BackfillConsented.key())?
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// The wall-clock instant capture most recently flipped ON, or `None` if capture was never
    /// enabled (spec §6a). Written by [`EventLog::set_capture_enabled`] on every ON transition; the
    /// disable path leaves it sticky (capture is off, so it is not consulted). Backs the sweeper's
    /// forward-only window (`mtime >= capture_enabled_at`).
    pub fn capture_enabled_at(&self) -> Result<Option<i64>, BossclawError> {
        Ok(self
            .latest_config_value(CAPTURE_ENABLED_AT_KEY)?
            .and_then(|v| v.as_i64()))
    }

    /// Flip the SP3 capture flags by appending ONE signed + hash-chained control `config` event
    /// (spec §6a). ATOMIC by construction — `capture_enabled` and its `capture_enabled_at` timestamp
    /// can never be persisted apart. `at` is supplied by the daemon so core stays clock-free
    /// (mirrors how the sweeper passes time in).
    ///
    /// Semantics (the C1 + M4 resolution):
    /// - `enabled` is written explicitly every call → [`EventLog::explicitly_set`]
    ///   `(ConfigFlag::CaptureEnabled)` becomes true (what the boot cascade keys off).
    /// - On enable (`enabled == true`) the `capture_enabled_at` timestamp is (re)recorded.
    /// - `backfill_consented` is written **true** ONLY when enabling WITH `backfill` (the Connect
    ///   checkbox). Enabling WITHOUT backfill (the Integrations toggle) does NOT touch the key, so a
    ///   just-granted Connect consent survives and a never-granted one stays `false` (forward-only,
    ///   M4).
    /// - DISABLING (`enabled == false`) clears `backfill_consented` to **false**: the historical
    ///   consent is one-time and spent, so a later forward-only re-enable must NOT silently re-import
    ///   the declined backlog (M4). `at` is inert on disable (no ON transition to timestamp).
    ///
    /// The single writer of all three keys, so the readers above can never drift the shape apart.
    pub fn set_capture_enabled(
        &self,
        enabled: bool,
        backfill: bool,
        at: i64,
    ) -> Result<(), BossclawError> {
        // Explicit map so keys are the named consts (json!{} cannot take a const identifier as an
        // object key). One event ⇒ the enabled bit and its timestamp land atomically.
        let mut content = serde_json::Map::new();
        content.insert(CAPTURE_ENABLED_KEY.to_string(), serde_json::Value::Bool(enabled));
        if enabled {
            content.insert(CAPTURE_ENABLED_AT_KEY.to_string(), serde_json::Value::from(at));
            if backfill {
                content.insert(BACKFILL_CONSENTED_KEY.to_string(), serde_json::Value::Bool(true));
            }
        } else {
            // Spend the one-time historical consent so a later forward-only enable cannot
            // resurrect the declined backlog (critic M4).
            content.insert(BACKFILL_CONSENTED_KEY.to_string(), serde_json::Value::Bool(false));
        }
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object(content),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// Whether Rung-3 conflict detection is enabled (spec §3.6). STICKY / fail-closed via
    /// [`EventLog::latest_config_value`]'s newest-first scan; DEFAULT CLOSED (a never-set flag
    /// reads `false`), so the sweep never runs for a user who never consented (I3).
    pub fn conflict_detect_enabled(&self) -> Result<bool, BossclawError> {
        Ok(self
            .latest_config_value(ConfigFlag::ConflictDetect.key())?
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Flip the conflict-detection switch by appending ONE signed + hash-chained control `config`
    /// event `{ "conflict_detect_enabled": <enabled> }`. The ONLY writer of the key (so the reader
    /// can never drift the shape). Carries no model fields → never disturbs `active_model`. Mirrors
    /// [`EventLog::set_mandates_enabled`].
    pub fn set_conflict_detect_enabled(&self, enabled: bool) -> Result<(), BossclawError> {
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: CONFIG_EVENT_TYPE.to_string(),
            // Explicit map so the key is the named const (json!{} cannot take a
            // const identifier as an object key).
            content: serde_json::Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(CONFLICT_DETECT_ENABLED_KEY.to_string(), serde_json::Value::Bool(enabled));
                m
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })?;
        Ok(())
    }

    /// The `(seq, id, text)` of each unprocessed extractable event strictly after
    /// the cursor, in `seq ASC` order, capped at `limit` (the per-tick batch).
    ///
    /// Extractable subjects are `memory` events (user-authored notes) and
    /// `file_ingested` events (imported file text). Derived events — `entity`,
    /// `link`, `page` — are NEVER subjects: facts derived from a subject inherit
    /// the subject's `source_event_ids` rather than being re-extracted themselves.
    ///
    /// Returns owned data so the store lock is released before any model/embedder
    /// call (lock discipline).
    fn unprocessed_extractable_since(
        &self,
        cursor: i64,
        limit: usize,
    ) -> Result<Vec<(i64, String, String)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT seq, id, payload FROM events
             WHERE event_type IN (?1, ?2) AND seq > ?3 ORDER BY seq ASC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![MEMORY_EVENT_TYPE, crate::graph::FILE_INGESTED_EVENT_TYPE, cursor, limit as i64],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, id, payload) = row?;
            let ev: Event = serde_json::from_str(&payload)?;
            if let Some(text) = ev.content.get("text").and_then(|t| t.as_str()) {
                out.push((seq, id, text.to_string()));
            }
        }
        Ok(out)
    }

    /// New conflict subjects at or after the cursor position, `(seq ASC, within_seq_id ASC)`, from
    /// at most `limit` source EVENTS (a capture expands to its passages, so the returned Vec may
    /// exceed `limit`). For the IN-PROGRESS event (`seq == cursor_seq`) subjects with within-seq id
    /// `< subject_offset` are skipped (already judged); newer events skip nothing. Notes: only
    /// CURRENT memory events. Passages: only a capture that is the CURRENT head for its `session_id`
    /// (not superseded / tombstoned) and only its non-retired passages, ordered by `passage_id`
    /// (retiring a passage skips it but does NOT renumber siblings, so `subject_offset` stays valid
    /// across cycles).
    ///
    /// This is a best-effort forward-looking snapshot; its correctness rests on the cursor being
    /// forward-only, idempotent (an already-open pair is never re-proposed downstream), and advanced
    /// by a SINGLE caller (the sweeper). A memory superseded / retired / capture-superseded between
    /// this events-read and the later folds is only ever DROPPED (safe — a phantom conflict is worse
    /// than a missed one), and anything newly appended lands beyond the `LIMIT` window and is caught
    /// next cycle. Cost: a full `fold_sessions` + `current_notes` per call regardless of `limit`
    /// (acceptable at Phase-2 scale; a later sweeper may compute the fold once and pass it in).
    pub fn unprocessed_conflict_subjects_since(
        &self,
        cursor_seq: i64,
        subject_offset: usize,
        limit: usize,
    ) -> Result<Vec<ConflictSubject>, BossclawError> {
        use crate::index::ConflictRef;
        // Source events at OR after the cursor's seq (the in-progress event is re-scanned so its
        // not-yet-judged subjects resume), oldest first, bounded.
        let rows: Vec<(i64, String, String)> = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            let mut stmt = conn.prepare(
                "SELECT seq, id, event_type FROM events
                 WHERE event_type IN (?1, ?2) AND seq >= ?3 ORDER BY seq ASC LIMIT ?4",
            )?;
            let mapped = stmt.query_map(
                rusqlite::params![
                    MEMORY_EVENT_TYPE,
                    crate::graph::SESSION_CAPTURED_EVENT_TYPE,
                    cursor_seq,
                    limit as i64
                ],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            )?;
            let mut out = Vec::new();
            for row in mapped {
                out.push(row?);
            }
            out
        };
        // Fold once for the current-head + retired-passage checks (deterministic).
        let fold = fold_sessions(&self.session_events_ordered()?);
        let current_note_ids: std::collections::HashSet<String> =
            self.current_notes()?.into_iter().map(|n| n.event_id).collect();

        let mut subjects = Vec::new();
        for (seq, id, etype) in rows {
            // Skip the already-judged prefix ONLY for the in-progress event; newer events skip 0.
            let skip_below = if seq == cursor_seq { subject_offset } else { 0 };
            if etype == MEMORY_EVENT_TYPE {
                // A note is ONE subject at within-seq id 0 — included iff not already judged.
                if skip_below == 0 && current_note_ids.contains(&id) {
                    subjects.push(ConflictSubject {
                        seq,
                        within_seq_id: 0,
                        subject: ConflictRef::Note { event_id: id },
                    });
                }
                continue;
            }
            // session_captured: only the CURRENT head for its session_id contributes passages.
            let Some(cs) = fold.current.iter().find(|cs| cs.event_id == id) else {
                continue; // superseded by a newer capture, or tombstoned — not a subject
            };
            let sid = cs.session_id.clone();
            let n = self.session_passage_count(&id)?;
            for pid in skip_below..n {
                if fold.retired_passages.contains(&(sid.clone(), pid)) {
                    continue; // retired passage — never a subject
                }
                subjects.push(ConflictSubject {
                    seq,
                    within_seq_id: pid,
                    subject: ConflictRef::Passage { session_id: sid.clone(), passage_id: pid },
                });
            }
        }
        Ok(subjects)
    }

    /// Every `Event` whose `event_type` is in `types`, in `seq ASC` (append) order.
    /// A parameterized `WHERE event_type IN (?1, ?2, ...)` over the events table — no
    /// value interpolation. Returns owned `Event`s so the store lock is released before
    /// the caller folds over them. Used by the M6b pending-proposal projection.
    pub(crate) fn events_of_types(
        &self,
        types: &[&str],
    ) -> Result<Vec<Event>, BossclawError> {
        if types.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: String =
            (0..types.len()).map(|i| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT payload FROM events WHERE event_type IN ({placeholders}) ORDER BY seq ASC"
        );
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            types.iter().map(|t| t as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str::<Event>(&row?)?);
        }
        Ok(out)
    }

    /// The CURRENT active edge-keys `(src, relation, dst)` from the folded `edges`
    /// table (`invalidated_at IS NULL`). These endpoints are already RESOLVED
    /// `entity:<ulid>` ids (the fold stores whatever a `link` carried, and the
    /// evolve loop only ever emits links on resolved ids — Rev 2 F4), so a
    /// retraction must be remapped to resolved ids BEFORE it is confirmed against
    /// this set. Used both to confirm retractions and to seed within-tick dedup.
    fn active_edge_keys(&self) -> Result<Vec<(String, String, String)>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT src, relation, dst FROM edges WHERE invalidated_at IS NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Fetch the `content["text"]` of each id in `ids` (memory/page events), in
    /// the caller's order (recall rank), skipping ids with no text. Turns recalled
    /// EVENT ids into the Pass-A cheat-sheet text. Parameterized `IN (...)` — no
    /// string interpolation of values.
    fn texts_for_ids(&self, ids: &[String]) -> Result<Vec<String>, BossclawError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: String =
            (0..ids.len()).map(|i| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, payload FROM events WHERE id IN ({placeholders})");
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        // Preserve the caller's id order (recall rank), not SQL row order.
        let mut by_id: HashMap<String, String> = HashMap::new();
        for row in rows {
            let (id, payload) = row?;
            let ev: Event = serde_json::from_str(&payload)?;
            if let Some(t) = ev.content.get("text").and_then(|t| t.as_str()) {
                by_id.insert(id, t.to_string());
            }
        }
        Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
    }

    /// Read the summarize cursor (0 if unset). Sibling of `evolve_cursor`, the
    /// summarize-phase high-water-mark over `seq` (spec §6 / M4b F1). NOT a fold:
    /// losing it only re-derives the dirty set, which idempotency (F6) makes a
    /// safe no-op for already-current topics.
    fn summarize_cursor(&self) -> Result<i64, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let v = conn
            .query_row("SELECT last_seq FROM summarize_cursor WHERE id = 0", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        Ok(v)
    }

    /// Persist the summarize cursor (spec §6 / M4b F1). Single-row upsert.
    fn set_summarize_cursor(&self, seq: i64) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO summarize_cursor (id, last_seq) VALUES (0, ?1)
             ON CONFLICT(id) DO UPDATE SET last_seq = ?1",
            rusqlite::params![seq],
        )?;
        Ok(())
    }

    /// The distinct `entity:`-prefixed endpoints of `link`/`invalidate`/`entity`
    /// events with `seq > cursor` — the dirty topic set (spec §6 / M4b F1).
    /// Non-entity endpoints (bare mentions passed through by `map_mention`) are
    /// excluded. Returns `(max_seq_scanned, entity_ids)`; `max_seq_scanned`
    /// stays at `cursor` when nothing matched. Parameterised query only.
    fn dirty_entities_since(&self, cursor: i64) -> Result<(i64, Vec<String>), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(
            "SELECT seq, event_type, payload FROM events
             WHERE seq > ?1 AND event_type IN ('link','invalidate','entity') ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![cursor], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        let mut max_seq = cursor;
        let mut seen = std::collections::BTreeSet::new();
        for row in rows {
            let (seq, etype, payload) = row?;
            max_seq = seq;
            let ev: Event = serde_json::from_str(&payload)?;
            if etype == "entity" {
                seen.insert(crate::graph::entity_node_id(&ev.id));
            } else if let Some((src, _r, dst, _c)) = crate::graph::parse_link_content(&ev.content) {
                for endpoint in [src, dst] {
                    if endpoint.starts_with(crate::graph::ENTITY_NODE_PREFIX) {
                        seen.insert(endpoint);
                    }
                }
            }
        }
        Ok((max_seq, seen.into_iter().collect()))
    }

    /// Like [`EventLog::texts_for_ids`], but DROPS any `page`-typed id by
    /// construction — the one-way rule enforced at fact-set materialization
    /// (spec §7 / M4b F3). A page id reaching the fact-set is a contract
    /// violation, never silently summarized back into a summary. File-typed rows
    /// are included (Door C): file text may feed a dossier; the dossier inherits
    /// external taint via D8 (engine gather lineage). One `IN (?,...)`
    /// query (mirrors [`EventLog::texts_for_ids`]); caller id order is preserved
    /// on the way out (the gather sorts lineage before calling, but preserving
    /// order is a defensive courtesy and costs nothing).
    fn fact_texts_for_ids(&self, ids: &[String]) -> Result<Vec<(String, String)>, BossclawError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: String =
            (0..ids.len()).map(|i| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
        let sql =
            format!("SELECT id, event_type, payload FROM events WHERE id IN ({placeholders})");
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        // Build a by-id map; skip page-typed rows (F3: a summary never feeds
        // summary-generation). File-typed rows are NO LONGER skipped (Door C):
        // file text may feed a dossier; the dossier inherits the external taint
        // via D8 (engine lineage).
        let mut by_id: HashMap<String, String> = HashMap::new();
        for row in rows {
            let (id, etype, payload) = row?;
            if etype == crate::graph::PAGE_EVENT_TYPE {
                continue;
            }
            let ev: Event = serde_json::from_str(&payload)?;
            if let Some(t) = ev.content.get("text").and_then(|t| t.as_str()) {
                by_id.insert(id, t.to_string());
            }
        }
        // Preserve the caller's id order, same as texts_for_ids.
        Ok(ids.iter().filter_map(|id| by_id.get(id).map(|t| (id.clone(), t.clone()))).collect())
    }

    /// Read an event's `model_meta.source_event_ids` by its `entity:<ulid>` node
    /// id (M4b lineage gather): strip the `entity:` prefix to recover the entity
    /// event id, then read its lineage. `None` if the event is absent or carries
    /// no `model_meta`. One parameterised query.
    fn source_ids_of_entity(&self, entity_id: &str) -> Result<Option<Vec<String>>, BossclawError> {
        let event_id = entity_id
            .strip_prefix(crate::graph::ENTITY_NODE_PREFIX)
            .unwrap_or(entity_id);
        self.source_ids_of_event(event_id)
    }

    /// Read an event's `model_meta.source_event_ids` by event id (M4b lineage
    /// gather). `None` if the event is absent or carries no `model_meta`. One
    /// parameterised query.
    fn source_ids_of_event(&self, event_id: &str) -> Result<Option<Vec<String>>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let payload: Option<String> = conn
            .query_row(
                "SELECT payload FROM events WHERE id = ?1",
                rusqlite::params![event_id],
                |r| r.get(0),
            )
            .optional()?;
        match payload {
            None => Ok(None),
            Some(p) => {
                let ev: Event = serde_json::from_str(&p)?;
                Ok(ev.model_meta.map(|m| m.source_event_ids))
            }
        }
    }

    /// The current page's `(page_event_id, sorted+deduped cited-source ids)` for a
    /// topic, or `None` if the topic has no current page (M4b F6 idempotency key).
    /// The cited set is the union of the page event's claim `cites` — the exact
    /// value the summarize phase compares against to skip an unchanged dossier.
    fn current_page_for_topic(
        &self,
        topic_id: &str,
    ) -> Result<Option<(String, Vec<String>)>, BossclawError> {
        // The current page id from the projection (small, parameterised read).
        let page_event_id: Option<String> = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.query_row(
                "SELECT page_event_id FROM pages WHERE topic_id = ?1",
                rusqlite::params![topic_id],
                |r| r.get(0),
            )
            .optional()?
        };
        let page_event_id = match page_event_id {
            None => return Ok(None),
            Some(id) => id,
        };
        // Parse the page event's claims → the sorted+deduped union of cites.
        let payload: Option<String> = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.query_row(
                "SELECT payload FROM events WHERE id = ?1",
                rusqlite::params![page_event_id],
                |r| r.get(0),
            )
            .optional()?
        };
        let mut cites: Vec<String> = Vec::new();
        if let Some(p) = payload {
            let ev: Event = serde_json::from_str(&p)?;
            if let Some(claims) = ev.content.get("claims").and_then(|c| c.as_array()) {
                for claim in claims {
                    if let Some(arr) = claim.get("cites").and_then(|c| c.as_array()) {
                        cites.extend(arr.iter().filter_map(|v| v.as_str().map(String::from)));
                    }
                }
            }
        }
        cites.sort();
        cites.dedup();
        Ok(Some((page_event_id, cites)))
    }

    /// Gather the bounded fact-set for one topic entity (spec §3.3, `Tight`
    /// reach): its current edges (as `src -relation-> dst` lines) + the memory
    /// texts in the lineage of the entity event and those edges. NEVER includes a
    /// page (M4b F3, via [`EventLog::fact_texts_for_ids`]).
    ///
    /// `pub` (not `pub(crate)`) so the hermetic evolve tests can compute the
    /// EXACT fact-set the summarize phase will gather and script the matching
    /// compose turn (the `ScriptedReasoner` keys on the precise prompt). It is a
    /// pure read over already-signed events — exposing it leaks no write path.
    pub fn gather_fact_set(
        &self,
        entity: &crate::graph::Entity,
    ) -> Result<crate::summarize::FactSet, BossclawError> {
        let neighbors = self.neighbors(&entity.entity_id).unwrap_or_default(); // current edges
        let edges: Vec<String> = neighbors
            .iter()
            .map(|e| format!("{} -{}-> {}", e.src, e.relation, e.dst))
            .collect();
        // Lineage memory ids = union of source_event_ids on the entity event + the
        // edge (link) events, resolved through the page-dropping reader (F3).
        let mut lineage: Vec<String> = Vec::new();
        if let Some(ids) = self.source_ids_of_entity(&entity.entity_id)? {
            lineage.extend(ids);
        }
        for e in &neighbors {
            if let Some(ids) = self.source_ids_of_event(&e.edge_id)? {
                lineage.extend(ids);
            }
        }
        lineage.sort();
        lineage.dedup();
        let memories = self.fact_texts_for_ids(&lineage)?;
        Ok(crate::summarize::FactSet { entity: entity.clone(), edges, memories, source_ids: lineage })
    }

    /// The summarize phase of one tick (spec §3, §6 / M4b). For each dirty topic
    /// (≤ [`crate::extract::SUMMARY_BATCH`], deterministic `entity_id` order):
    /// gather the fact-set → compose → citation floor → assemble → (idempotency
    /// F6) emit only when the cited-source SET differs from the current page's →
    /// [`EventLog::emit_page`] (atomic supersede, F5). Per-topic `continue` on any
    /// gather/compose/parse/emit error (F4) — extraction already committed, so a
    /// topic-A failure must never block topic B or the cursor advance. Advances
    /// `summarize_cursor` to the scanned tip ONLY when the dirty set fully drained
    /// this tick (F1); otherwise the overflow re-scans next tick (idempotent).
    fn summarize_topics(
        &self,
        reasoner: &dyn crate::reason::Reasoner,
        report: &mut EvolveReport,
    ) -> Result<(), BossclawError> {
        let cursor = self.summarize_cursor()?;
        let (max_seq, dirty) = self.dirty_entities_since(cursor)?;
        let drained = dirty.len() <= crate::extract::SUMMARY_BATCH;
        let entities = self.all_entities()?;
        for topic_id in dirty.iter().take(crate::extract::SUMMARY_BATCH) {
            let entity = match entities.iter().find(|e| &e.entity_id == topic_id) {
                Some(e) => e.clone(),
                None => continue, // a dirty endpoint with no folded entity (rare) → skip.
                // Intentional F1 safe: the cursor may advance past this entity
                // permanently — it has no entity record so it cannot be
                // summarized. A future mint or repair event emits a NEW event
                // with a seq PAST the advanced cursor, re-dirtying the topic
                // for the next tick. Nothing is silently dropped forever.
            };
            let facts = match self.gather_fact_set(&entity) {
                Ok(f) if f.fact_count() >= crate::summarize::PAGE_MIN_FACTS => f,
                _ => continue, // too thin, or a gather error → skip this topic (F4)
            };
            let raw = match reasoner.complete_json(
                crate::summarize::SUMMARIZE_SYSTEM,
                &crate::summarize::build_compose_prompt(&facts),
                &crate::summarize::compose_schema(),
            ) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("summarize: compose failed for {topic_id}, skipping: {e}");
                    continue;
                }
            };
            let draft = match crate::summarize::parse_draft(&raw) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("summarize: malformed draft for {topic_id}, skipping: {e}");
                    continue;
                }
            };
            let floored = crate::summarize::citation_floor(&draft, &facts);
            // F4: the empty-floor path NEVER reaches emit_page/append (an empty
            // source set would hit the Tier-B reject and is not an emit anyway).
            let rendered = match crate::summarize::assemble(&floored) {
                Some(r) => r,
                None => continue,
            };
            // Idempotency (F6): compare the cited-source SET against the current
            // page; an unchanged grounding set emits nothing (no supersede churn).
            let prior = self.current_page_for_topic(topic_id)?;
            if let Some((_pid, prior_cites)) = &prior {
                if prior_cites == &rendered.cites {
                    continue;
                }
            }
            // Canonicalize each claim's own `cites` for F7 signed content. This
            // is NOT removable and NOT a duplicate of assemble(): assemble() sorts
            // the cites UNION to produce `source_event_ids` (the page-level set),
            // while this block sorts each INDIVIDUAL claim's `cites` array (the
            // per-claim attribution stored in `content.claims[].cites`). Removing
            // this leaves per-claim cites in raw model order → JCS-canonical
            // signing becomes non-deterministic. The cap precedes signing.
            let claims_json: Vec<serde_json::Value> = floored
                .claims
                .iter()
                .map(|c| {
                    let mut cites = c.cites.clone();
                    cites.sort();
                    cites.dedup();
                    serde_json::json!({ "text": c.text, "cites": cites })
                })
                .collect();
            let claims_capped =
                &claims_json[..claims_json.len().min(crate::summarize::MAX_CLAIMS_PER_PAGE)];
            let prior_id = prior.as_ref().map(|(id, _)| id.as_str());
            match self.emit_page(
                topic_id,
                &rendered.title,
                &rendered.text,
                claims_capped,
                &[],
                reasoner.model_id(),
                &facts.source_ids, // D8: engine gather lineage (taint anchor), not model cites
                prior_id,
            ) {
                Ok((_pid, superseded)) => {
                    report.pages_emitted += 1;
                    if superseded {
                        report.pages_superseded += 1;
                    }
                }
                Err(e) => {
                    log::warn!("summarize: emit_page failed for {topic_id}, skipping: {e}");
                    continue;
                }
            }
        }
        // Refresh the projection only if the phase changed the page set.
        if report.pages_emitted > 0 || report.pages_superseded > 0 {
            self.rebuild_graph()?;
        }
        // F1: advance the cursor only when the dirty set fully drained this tick.
        if drained && max_seq > cursor {
            self.set_summarize_cursor(max_seq)?;
        }
        Ok(())
    }

    /// Map a proposed mention to its resolved `entity:<ulid>` if known, else pass
    /// the raw string through (a relation endpoint the model named but resolution
    /// did not cover — kept as an opaque node id, never silently dropped). Pure
    /// helper over the per-memory `mention_to_id` map (Rev 2 F4).
    fn map_mention(
        mention_to_id: &HashMap<String, String>,
        mention: &str,
    ) -> String {
        mention_to_id.get(mention).cloned().unwrap_or_else(|| mention.to_string())
    }

    /// Fold a [`EventLog::resolve_or_mint`] outcome `(entity_id, minted)` into the
    /// tick counters, returning the id. Single-sourced so the resolve loop counts a
    /// mint in exactly one place (no per-call-site duplication).
    fn count_mint(
        report: &mut EvolveReport,
        minted_this_tick: &mut bool,
        outcome: (String, bool),
    ) -> String {
        let (id, minted) = outcome;
        if minted {
            report.entities_minted += 1;
            *minted_this_tick = true;
        }
        id
    }

    /// Run ONE evolve tick (spec §3, §8 / Rev 2 F1/F4/F5/F6): for each unprocessed
    /// `memory` (≤ [`EVOLVE_BATCH`]): recall context → Pass A propose → resolve
    /// EVERY distinct mention (entities ∪ relations ∪ retractions) to a stable
    /// `entity:<ulid>` → augment with the resolved-entity neighborhood → Pass B
    /// (pure fail-closed span floor + ONE model critique that can only subtract,
    /// then cardinality-gated retraction confirmation against the CURRENT graph
    /// on RESOLVED ids) → emit `entity`/`invalidate`/`link` events through
    /// [`EventLog::append`] (the single serialized writer — the loop is NOT
    /// privileged) → advance the cursor after the batch commits.
    ///
    /// Idempotency: an active edge-key is skipped (seeded from the graph and
    /// updated WITHIN the tick, Rev 2 F5, so two memories asserting the same edge
    /// in one tick emit only once); a resolved/just-minted entity is reused.
    ///
    /// Resource fail-safes (Rev 2 F6): at most [`MAX_ENTITIES_PER_MEMORY`]
    /// entities are accepted per memory, the source text is truncated to
    /// [`crate::extract::MAX_INPUT_TEXT_BYTES`] before the model sees it, and the
    /// entity index is rebuilt ONCE after the batch (not per memory).
    ///
    /// Degrade-never-break (spec §10): the off-switch short-circuits to a no-op
    /// BEFORE any model call; a reasoner/graph error on a memory logs + STOPS the
    /// batch (the cursor does not advance past an unprocessed memory) so the
    /// memory retries next tick — recall + storage are untouched.
    pub fn evolve_once(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
    ) -> Result<EvolveReport, BossclawError> {
        let mut report = EvolveReport::default();
        // Off-switch is checked BEFORE any model call (Rev 2 F2-sec).
        if !self.evolve_enabled()? {
            report.skipped_disabled = true;
            return Ok(report);
        }
        // M6b proposer off-switch (§5.3), read ONCE per tick. INDEPENDENT of the
        // evolve switch above: when false, curation (invalidate/link) still runs but
        // the reconciliation synthesis after each confirmed contradiction is skipped.
        let proposals_on = self.proposals_enabled()?;
        let cursor = self.evolve_cursor()?;
        let batch = self.unprocessed_extractable_since(cursor, EVOLVE_BATCH)?;
        // Within-tick active-key set (Rev 2 F5): seed from the current graph, then
        // grow as this tick emits — so a duplicate edge across two memories in the
        // SAME tick is skipped, not double-emitted.
        let mut active_keys: HashSet<(String, String, String)> =
            self.active_edge_keys()?.into_iter().collect();
        let mut last_committed_seq = cursor;
        // Whether any mint happened → rebuild the entity index ONCE after the
        // batch (Rev 2 F6), instead of O(memories) rebuilds inside the loop.
        let mut minted_this_tick = false;
        // Tick-scoped mention→id cache (mention surface form → resolved id). Since
        // the entity index is rebuilt only AFTER the batch (F6), a mention minted
        // by an EARLIER memory in this tick is not yet in the index; this cache
        // lets a LATER memory in the same tick reuse that mint instead of minting a
        // duplicate — which is also what lets within-tick edge dedup (F5) land
        // (two memories asserting the same edge resolve to the same key).
        let mut tick_mint_cache: HashMap<String, String> = HashMap::new();

        for (seq, mem_id, full_text) in batch {
            // F6: bound the text handed to the reasoner (the on-disk memory is
            // untouched; only the extraction copy is truncated).
            let text = crate::extract::truncate_for_reasoner(&full_text).to_string();

            // ── 1. recall context (M2). entity-kind is excluded from recall by
            //    construction (separate index); `exclude_pages: true` drops pages.
            //    Door B OPEN: `exclude_files: false` — external file text CAN now
            //    serve as extraction context. Any file hit in the read-set taints
            //    the derived fact via the append chokepoint (extraction-from-files
            //    D2), and the Pass-A cheat-sheet is fenced (extract.rs §3) so
            //    external context cannot inject instructions. The read-set is EVENT
            //    ids only (never entity:<ulid>), spec §16. ──
            let recalled: Vec<String> = self
                .recall(
                    embedder,
                    &text,
                    crate::extract::GRAPH_CONTEXT_K,
                    &RecallOptions { exclude_pages: true, exclude_files: false, ..Default::default() },
                )
                .map(|hits| {
                    hits.into_iter()
                        .filter(|h| h.event_id != mem_id) // never feed the source back as context
                        .map(|h| h.event_id)
                        .collect()
                })
                .unwrap_or_default();
            // Count file-derived (`is_external`) snippets in this memory's recall payload
            // so Phase 2b can disclose "N file-derived snippets sent to the cloud" (spec R4).
            // Read-only: loading + classifying recalled events never alters evolve's behavior.
            let tainted_this_memory = recalled
                .iter()
                .filter(|id| {
                    self.event_by_id(id)
                        .ok()
                        .flatten()
                        .map(|ev| crate::ingest::is_external(&ev))
                        .unwrap_or(false)
                })
                .count();
            report.tainted_recall_snippets += tainted_this_memory;
            let recalled_texts = self.texts_for_ids(&recalled)?;
            let read_set: Vec<String> = {
                let mut v = vec![mem_id.clone()];
                v.extend(recalled.iter().cloned());
                v
            };

            // ── 2. Pass A — propose. A reasoner error makes THIS memory a no-op
            //    (stop the batch; the cursor stays at last_committed_seq so the
            //    memory retries next tick) — spec §10. ──
            let proposals = match crate::extract::propose(reasoner, &text, &recalled_texts) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("evolve: Pass A failed for memory {mem_id}, stopping batch: {e}");
                    break;
                }
            };

            // ── 3. resolve EVERY distinct mention across entities ∪ relations ∪
            //    retractions to a stable entity:<ulid> (Rev 2 F4). An entity
            //    mention that resolves Mint becomes a signed `entity` event;
            //    relation/retraction endpoints the model named but did not list as
            //    entities are still resolved so they remap to graph-key ids. ──
            let mut mention_to_id: HashMap<String, String> = HashMap::new();
            // Resolve EVERY distinct mention to a stable entity id in ONE pass.
            // The work list is, in order: entity proposals (capped at
            // MAX_ENTITIES_PER_MEMORY, F6) with their declared type, then every
            // relation/retraction endpoint with the neutral UNRESOLVED_ENTITY_TYPE
            // (a bare endpoint the model named but did not list in entities[]).
            // First-seen wins, so an endpoint that is also a declared entity keeps
            // its real type. Folding both into one loop means the mint-count + the
            // resolve call appear exactly once (no duplication).
            let resolve_work = proposals
                .entities
                .iter()
                .take(MAX_ENTITIES_PER_MEMORY)
                .map(|e| (e.mention.clone(), e.entity_type.clone()))
                .chain(
                    proposals
                        .relations
                        .iter()
                        .flat_map(|r| [r.src.clone(), r.dst.clone()])
                        .chain(
                            proposals
                                .retractions
                                .iter()
                                .flat_map(|r| [r.src.clone(), r.dst.clone()]),
                        )
                        .map(|m| (m, UNRESOLVED_ENTITY_TYPE.to_string())),
                );
            for (mention, entity_type) in resolve_work {
                if mention_to_id.contains_key(&mention) {
                    continue; // first-seen wins (declared type beats the endpoint default)
                }
                let outcome = self.resolve_or_mint(
                    embedder,
                    reasoner,
                    &mention,
                    &entity_type,
                    &read_set,
                    &mut tick_mint_cache,
                )?;
                let id = Self::count_mint(&mut report, &mut minted_this_tick, outcome);
                mention_to_id.insert(mention, id);
            }

            // ── 4. augment: the neighborhood of the resolved entity ids (the
            //    second half of the cheat sheet) as `src -relation-> dst` lines. ──
            let neighborhood = self.neighborhood_lines(&mention_to_id)?;

            // ── 5. Pass B — model-driven critique over a pure fail-closed floor
            //    (Rev 2 F1): the floor keeps only span-verified relations; the
            //    model may DROP or down-confidence but NEVER add an edge the floor
            //    didn't support. Bounded by MAX_REFLECT total passes (Pass A +
            //    this critique = 2). A reasoner error → no-op this memory. ──
            // The MAX_REFLECT bound (Pass A propose + one Pass B critique = 2) is
            // enforced at COMPILE time: this tick runs exactly those two model
            // passes, so a future tightening of MAX_REFLECT below 2 must fail the
            // build rather than silently under-run the reflexion contract.
            const _: () = assert!(MAX_REFLECT >= 2, "evolve runs Pass A + one critique");
            let refined = match crate::extract::critique_with_reasoner(
                reasoner, &text, &proposals, &neighborhood,
            ) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("evolve: Pass B failed for memory {mem_id}, stopping batch: {e}");
                    break;
                }
            };

            // ── 6. remap refined relations'/retractions' endpoints to resolved
            //    ids (Rev 2 F4) BEFORE confirming retractions / emitting links. ──
            let remapped_retractions: Vec<crate::extract::ProposedRetraction> = refined
                .retractions
                .iter()
                .map(|r| crate::extract::ProposedRetraction {
                    src: Self::map_mention(&mention_to_id, &r.src),
                    relation: r.relation.clone(),
                    dst: Self::map_mention(&mention_to_id, &r.dst),
                    reason: r.reason.clone(),
                    confidence: r.confidence,
                })
                .collect();
            // Confirm against the CURRENT active edges (resolved ids). active_keys
            // already holds resolved-id keys; only materialize the slice when there
            // is actually a retraction to confirm (retractions are rare — avoid the
            // per-memory clone of the whole active set otherwise).
            let confirmed = if remapped_retractions.is_empty() {
                Vec::new()
            } else {
                let active_now: Vec<(String, String, String)> =
                    active_keys.iter().cloned().collect();
                crate::extract::confirm_retractions(&remapped_retractions, &active_now)
            };

            // ── 6a. invalidate confirmed contradictions FIRST (so the fold closes
            //    the old interval before any replacement opens). Drop the retired
            //    key from the within-tick active set. ──
            for r in &confirmed {
                self.invalidate(&r.src, &r.relation, &r.dst, None, &read_set)?;
                active_keys.remove(&(r.src.clone(), r.relation.clone(), r.dst.clone()));
                report.invalidates_emitted += 1;

                // ── 6a′. M6b reconciliation (best-effort, gated). The committed
                //    invalidate above is NEVER unwound by a reconciliation failure:
                //    the attempt is wrapped so any Err is logged and the loop
                //    proceeds (mirrors the per-memory `continue` discipline). The
                //    backward walk MUST run HERE, while the retired edge is still
                //    active (the end-of-tick `rebuild_graph` has not yet folded it
                //    closed), because it relocates the edge via `neighbors` (an
                //    active-edge lookup). On Unix only — the actuator/gate path
                //    (M6a) is `#[cfg(unix)]`. ──
                #[cfg(unix)]
                if proposals_on {
                    if let Err(e) = self.reconcile_confirmed_contradiction(
                        reasoner, r, &read_set, &mut report,
                    ) {
                        log::warn!(
                            "evolve: reconciliation for ({} -{}-> {}) failed (invalidate kept): {e}",
                            r.src, r.relation, r.dst
                        );
                    }
                }
            }

            // ── 6b. emit confirmed relations as machine links on RESOLVED ids,
            //    skipping any (src, relation, dst) ALREADY active — including ones
            //    emitted earlier in THIS tick (Rev 2 F5). ──
            for rel in &refined.relations {
                let s = Self::map_mention(&mention_to_id, &rel.src);
                let d = Self::map_mention(&mention_to_id, &rel.dst);
                let key = (s.clone(), rel.relation.clone(), d.clone());
                if active_keys.contains(&key) {
                    continue; // already asserted → emit nothing (idempotent)
                }
                self.link_machine(
                    &s, &rel.relation, &d, rel.confidence, reasoner.model_id(), &read_set,
                )?;
                active_keys.insert(key);
                report.links_emitted += 1;
            }

            report.memories_processed += 1;
            last_committed_seq = seq;
        }

        // ── 7. rebuild the entity index ONCE after the batch (Rev 2 F6) so the
        //    next tick can resolve this tick's mints, and refresh the graph so
        //    the folded `edges`/`entities` reflect the just-emitted events. ──
        if minted_this_tick {
            self.rebuild_entity_index(embedder)?;
        }
        if report.links_emitted > 0
            || report.invalidates_emitted > 0
            || report.entities_minted > 0
        {
            self.rebuild_graph()?;
        }

        // ── 8. advance the cursor to the last fully-processed memory's seq, only
        //    after the batch committed (a stopped batch leaves it where it was). ──
        if last_committed_seq > cursor {
            self.set_evolve_cursor(last_committed_seq)?;
        }

        // ── 9. summarize phase (M4b): AFTER extraction committed + the graph is
        //    folded, (re)summarize the dirty topics into dossier `page` events.
        //    Best-effort and self-contained — its per-topic `continue` (F4) never
        //    unwinds the already-committed extraction work; it manages its own
        //    `summarize_cursor` (F1) independent of the extraction cursor above. ──
        self.summarize_topics(reasoner, &mut report)?;

        // ── 10. M6c mandate phase (§5.1/§5.5/§5.6). A NEW top-level phase, AFTER summarize:
        //    the brain's standing sync goals are evaluated last, so a mandate synthesizes
        //    over a graph + dossiers that already reflect THIS tick's curation. Skipped
        //    wholesale when the INDEPENDENT mandate off-switch is engaged (sticky +
        //    fail-closed, §5.6); the evolve off-switch already returned earlier. The whole
        //    phase is `#[cfg(unix)]` — the actuator/gate surface (M6a) is Unix-only.
        //
        //    Best-effort isolation mirrors the M6b reconciliation wiring EXACTLY: each
        //    mandate is wrapped so an `Err` is logged and the loop CONTINUES — a single
        //    mandate failure NEVER unwinds the committed graph/summarize work nor aborts the
        //    tick. The off-switch is RE-READ per mandate (security M1, fast-kill): flipping
        //    it mid-phase stops further proposals within the same tick. The shared
        //    `report.proposals_emitted` counter bounds M6b + M6c proposals together. ──
        #[cfg(unix)]
        {
            if self.mandates_enabled()? {
                for m in self.active_mandates()? {
                    // Re-read the off-switch per mandate so a flip mid-phase fast-kills the
                    // rest of the tick (sticky + fail-closed). A read error aborts the phase
                    // (fail-closed) but never unwinds committed work.
                    if !self.mandates_enabled()? {
                        break;
                    }
                    if let Err(e) = self.run_mandate(&m, reasoner, &mut report) {
                        log::warn!(
                            "evolve: mandate {} (target {}) failed (committed work kept): {e}",
                            m.mandate_grant_id, m.target
                        );
                    }
                }
            }
        }

        Ok(report)
    }

    /// M6c mandate phase — gather + cached-or-synth DECISION for ONE mandate `m`
    /// (spec §5.1/§5.2, findings E/G, Task 9a). Returns a [`MandateAction`]; it appends
    /// NO event and runs NO write gate (Task 9b turns the action into a gated
    /// `write_proposal` / `write_rejected` and wires it into `evolve_once`).
    ///
    /// Algorithm:
    /// 1. **Gather** `current_files()` whose canonical path is segment-aware UNDER
    ///    `m.source_scope` (the same `Path::starts_with` discipline as `is_write_allowed`
    ///    / `add_mandate`'s read-root scan) AND not equal to `m.target` (the output is
    ///    never its own source). Over [`crate::graph::MAX_SOURCES_PER_MANDATE`] → `Elide`
    ///    (directory-bomb guard, finding E). Empty in-scope set → `Elide` (never
    ///    synthesize from nothing, finding G).
    /// 2. **sources_hash** = sha256 over the SORTED `(canonical_path, content_hash)` pairs
    ///    (deterministic — sort before hashing).
    /// 3. **cached-or-synth**: a cache HIT (`get_synthesis_cache`) reuses its bytes +
    ///    `source_event_ids_at_synth` with NO LLM call. A miss reads each source's ON-DISK
    ///    bytes, fences EACH via [`crate::extract::push_fenced_source`]; if the combined
    ///    fenced length would exceed [`crate::extract::MAX_INPUT_TEXT_BYTES`] → `Elide` (do
    ///    NOT truncate, finding E). Otherwise it calls the reasoner EXACTLY as
    ///    `reconcile_confirmed_contradiction` does the rewrite turn (same `complete_json`,
    ///    same structured schema, parse-failure → the `?`-propagated `Reject`); an empty
    ///    `synced_content` → `Reject("empty synthesis")` (finding G). On success it caches
    ///    the bytes + the engine-gathered source ids just read.
    /// 4. **Compare vs ON-DISK** (`std::fs::read(&m.target)`, NEVER the stale `files`
    ///    projection hash): equal → `Elide`. Op = `Create` if the target is absent, else
    ///    `Edit` (M6c never deletes).
    /// 5. **Lineage (finding B — anti-laundering union)**: the cache/synth source ids ∪
    ///    the CURRENT in-scope `file_event_id`s, deduped, fed to
    ///    [`crate::mandate::mandate_lineage`]. EVERY id is engine-gathered; see the
    ///    invariant comment at the union site.
    #[cfg(unix)]
    pub fn mandate_phase_for(
        &self,
        m: &crate::graph::Mandate,
        reasoner: &dyn crate::reason::Reasoner,
    ) -> Result<MandateAction, BossclawError> {
        use sha2::{Digest, Sha256};

        // ── 1. Gather in-scope sources: segment-aware UNDER source_scope, target excluded.
        //    `Path::starts_with` compares whole components, so a string-prefix sibling of
        //    the scope (e.g. `/a/bc` under `/a/b`) does NOT match — the same discipline as
        //    `is_write_allowed` / the `add_mandate` read-root scan. ──
        let scope = std::path::Path::new(&m.source_scope);
        let target = std::path::Path::new(&m.target);
        let in_scope: Vec<crate::graph::FileRecord> = self
            .current_files()?
            .into_iter()
            .filter(|rec| {
                let p = std::path::Path::new(&rec.canonical_path);
                p.starts_with(scope) && p != target
            })
            .collect();

        // Directory-bomb guard (finding E): too many sources → Elide (retryable, no event).
        if in_scope.len() > crate::graph::MAX_SOURCES_PER_MANDATE {
            return Ok(MandateAction::Elide);
        }
        // Never synthesize from nothing (finding G): empty in-scope set → Elide.
        if in_scope.is_empty() {
            return Ok(MandateAction::Elide);
        }

        // ── 2. sources_hash: sha256 over the SORTED (canonical_path, content_hash) pairs.
        //    Sort first so the digest is independent of `current_files()` row order
        //    (it already orders by path, but we sort explicitly to make the determinism
        //    a property of THIS code, not of the query). A NUL byte separates fields and
        //    pairs so no concatenation collision can occur. ──
        let mut pairs: Vec<(&str, &str)> = in_scope
            .iter()
            .map(|rec| (rec.canonical_path.as_str(), rec.content_hash.as_str()))
            .collect();
        pairs.sort_unstable();
        let mut hasher = Sha256::new();
        for (path, content_hash) in &pairs {
            hasher.update(path.as_bytes());
            hasher.update([0x00]);
            hasher.update(content_hash.as_bytes());
            hasher.update([0x00]);
        }
        let sources_hash = hex::encode(hasher.finalize());

        // The CURRENT in-scope source event ids — engine-gathered, used in BOTH the
        // synth-time cache lineage and the step-5 anti-laundering union.
        let current_source_ids: Vec<String> =
            in_scope.iter().map(|rec| rec.file_event_id.clone()).collect();

        // ── 3. cached-or-synth → (expected_bytes, synth_lineage_ids). ──
        let (expected_bytes, synth_lineage_ids) =
            match self.get_synthesis_cache(&m.mandate_grant_id, &sources_hash)? {
                // Cache HIT: reuse the stored bytes + the exact ids read at synth time. NO
                // LLM call (convergence + cost: the source-state is byte-identical).
                Some(row) => (row.expected_bytes, row.source_event_ids_at_synth),
                // Cache MISS: read each source's ON-DISK bytes and fence EACH as untrusted
                // DATA into one combined block (mirrors how the reconcile rewrite turn fences
                // the live file body). FAIL CLOSED on any unusable in-scope source — see the
                // per-source guards below. The hash already pinned the FULL in-scope set, so
                // synthesizing from fewer sources than that set names would cache PARTIAL
                // bytes under the full-set `sources_hash`; because a skipped source's
                // content_hash is unchanged when it becomes readable again, the cache would
                // return those partial bytes FOREVER → permanent staleness, convergence
                // silently broken. So we synthesize ONLY when EVERY in-scope source reads
                // cleanly as UTF-8 — guaranteeing the cached bytes correspond to exactly the
                // set `sources_hash` names. An unusable source elides (retryable next tick).
                None => {
                    let mut fenced = String::new();
                    for rec in &in_scope {
                        // Read-Err (e.g. the file vanished after the projection listed it):
                        // do NOT skip, synthesize, or cache — Elide and retry once every
                        // in-scope source is readable again (the source-state is unchanged,
                        // so a later tick recomputes the same `sources_hash`).
                        let Ok(bytes) = std::fs::read(&rec.canonical_path) else {
                            return Ok(MandateAction::Elide);
                        };
                        // Require valid UTF-8 ON DISK NOW. A lossy view would synthesize from
                        // replacement chars (also defeating Elide-convergence), so non-UTF-8
                        // also fails closed → Elide. The fence neutralizes any embedded
                        // terminator; the reasoner sees DATA, never instructions.
                        let Ok(text) = String::from_utf8(bytes) else {
                            return Ok(MandateAction::Elide);
                        };
                        crate::extract::push_fenced_source(&mut fenced, &text);
                    }
                    // Over-cap input (finding E): do NOT truncate — Elide and surface nothing
                    // (stays retryable). Truncating would synthesize from a partial view and
                    // could silently drop a source's content.
                    if fenced.len() > crate::extract::MAX_INPUT_TEXT_BYTES {
                        return Ok(MandateAction::Elide);
                    }
                    // Synthesize. Mirrors `reconcile_confirmed_contradiction`'s rewrite turn
                    // EXACTLY: the same `complete_json` trait method, the same structured
                    // schema, and a parse/transport failure propagates via `?` (→ the caller
                    // sees the `Err`; Task 9b leaves the mandate retryable). The trusted
                    // recipe is the frame; the sources are fenced below.
                    let prompt = crate::mandate::build_recipe_prompt(&m.recipe, &fenced);
                    let out = reasoner.complete_json(
                        MANDATE_SYSTEM,
                        &prompt,
                        &crate::mandate::recipe_schema(),
                    )?;
                    let synced = out.get("synced_content").and_then(|v| v.as_str()).unwrap_or("");
                    // Empty synthesis is a genuine failure → Reject (finding G: NEVER truncate
                    // the target to empty). Mirrors the reconcile `empty_rewrite` reject.
                    if synced.is_empty() {
                        return Ok(MandateAction::Reject {
                            reason: "empty synthesis".into(),
                            sources_hash,
                        });
                    }
                    // Output-size bound (review carry-forward, Minor #3): an over-cap synthesis
                    // Elides BEFORE it is cached — bounding the synthesis cache row (a runaway
                    // model output can never bloat the encrypted cache), and, because the phase
                    // returns Elide, the proposal too. Retryable: the source-state is unchanged,
                    // so a later tick recomputes the same `sources_hash` (and a model that
                    // eventually returns in-bound bytes converges). NEVER a Reject — an oversized
                    // output is a transient model artifact, not a terminal (path,key) failure.
                    if synced.len() > crate::graph::MAX_SYNCED_CONTENT_BYTES {
                        return Ok(MandateAction::Elide);
                    }
                    let bytes = synced.as_bytes().to_vec();
                    let expected_hash = hex::encode(Sha256::digest(&bytes));
                    // Cache the bytes WITH the engine-gathered ids just read (finding B): a
                    // later hit unions exactly these — never a model-named id.
                    self.put_synthesis_cache(
                        &m.mandate_grant_id,
                        &sources_hash,
                        &bytes,
                        &expected_hash,
                        &current_source_ids,
                    )?;
                    (bytes, current_source_ids.clone())
                }
            };

        // ── 4. Compare vs the ON-DISK target (NEVER the stale `files` projection hash:
        //    it is out of date the instant an actuator write lands). Equal → Elide. ──
        let actual = std::fs::read(target).ok();
        if actual.as_deref() == Some(expected_bytes.as_slice()) {
            return Ok(MandateAction::Elide);
        }
        // Op is Create iff the target file does not exist yet, else Edit. M6c NEVER deletes.
        let op = if target.exists() {
            crate::actuator::WriteOp::Edit
        } else {
            crate::actuator::WriteOp::Create
        };

        // ── 5. Lineage (finding B — the anti-laundering union). ──
        // SECURITY INVARIANT (Task-5 carry-forward): every id in `union` is an
        // ENGINE-GATHERED `file_event_id` — `synth_lineage_ids` comes from the cache row
        // or the `FileRecord`s read in step 1, and `current_source_ids` are the step-1
        // `FileRecord.file_event_id`s. NONE is ever derived from the model's
        // `synced_content` or any model output. The model produces only file BYTES; it
        // never names a source id. Deriving any lineage id from model output would launder
        // an attacker-chosen id into the gate's trust set — forbidden.
        let mut union: Vec<String> = synth_lineage_ids;
        union.extend(current_source_ids);
        union.sort();
        union.dedup();
        let lineage = crate::mandate::mandate_lineage(&m.mandate_grant_id, &union)?;

        Ok(MandateAction::Propose { expected: expected_bytes, lineage, op, sources_hash })
    }

    /// M6c — act on ONE mandate `m` this tick (§5.1/§5.5/§5.6, Task 9b). Calls
    /// [`mandate_phase_for`](Self::mandate_phase_for) for the gather + cached-or-synth
    /// DECISION, then turns the [`MandateAction`] into the SAME recorded outcomes the M6b
    /// sibling [`reconcile_confirmed_contradiction`](Self::reconcile_confirmed_contradiction)
    /// produces — a gated `write_proposal` (+ cached bytes) or a `write_rejected`. The
    /// EXACT mirror of that method's propose/reject/record half, with the M6c producer
    /// stamp and the source-state `inducing_key`.
    ///
    /// Action handling:
    /// - [`MandateAction::Elide`] → do nothing, emit NO event (stays retryable next tick).
    /// - [`MandateAction::Reject`] → record a `write_rejected` (a genuine synthesis
    ///   failure, e.g. empty synthesis) stamped [`M6C_PROPOSER_PRODUCER`]; `count`.
    /// - [`MandateAction::Propose`] →
    ///   1. `inducing_key` = `{mandate, target, sources_hash}` — the SHAPE
    ///      [`is_mandate_proposal_suppressed`](Self::is_mandate_proposal_suppressed)
    ///      expects (§5.4); `sources_hash` is the one the phase computed (returned in the
    ///      action so the two sites cannot drift).
    ///   2. **Idempotency** — a suppressed `(target, key)` skips (emits nothing).
    ///   3. **Caps** — the GLOBAL per-tick cap ([`MAX_PROPOSALS_PER_TICK`], SHARED with M6b
    ///      via `report.proposals_emitted`) OR the per-mandate cap
    ///      ([`MAX_PROPOSALS_PER_MANDATE_PER_TICK`]) ⇒ ELIDE: no event,
    ///      `report.proposals_elided_cap += 1` (NEVER a `write_rejected` — a cap is retryable).
    ///   4. **Gate + record** — `propose_write`; a `reject_reason` (e.g. the target left a
    ///      write-grant: the never-widen check) records a `write_rejected`; otherwise an
    ///      `append_write_proposal_with` + `put_proposal_bytes`, `proposals_emitted += 1`.
    ///
    /// `proposals_emitted` is the SHARED global counter (M6b increments it too), so the
    /// global cap bounds M6b + M6c proposals TOGETHER in one tick. Best-effort isolation
    /// is the CALLER's (`evolve_once` wraps each call so an `Err` is logged + the loop
    /// continues); this method returns `Err` only on a genuine append/IO failure.
    #[cfg(unix)]
    fn run_mandate(
        &self,
        m: &crate::graph::Mandate,
        reasoner: &dyn crate::reason::Reasoner,
        report: &mut EvolveReport,
    ) -> Result<(), BossclawError> {
        let (expected, lineage, op, sources_hash) = match self.mandate_phase_for(m, reasoner)? {
            // In sync / no sources / over an input cap → nothing to do, stays retryable.
            MandateAction::Elide => return Ok(()),
            // A genuine synthesis failure (e.g. empty synthesis) → record a terminal
            // `write_rejected` for the target, stamped with the M6c producer. `target` is
            // the mandate's canonical target; the inducing_key carries the source-state so a
            // later DIFFERENT source-state is a fresh (non-suppressed) ask. Best-effort:
            // a Reject NEVER unwinds committed work (the caller isolates an Err).
            MandateAction::Reject { reason, sources_hash } => {
                // The inducing_key carries the REAL source-state digest, so the
                // `write_rejected` is terminal for exactly this source-state (a later
                // DIFFERENT source-state is a fresh ask), mirroring M6b's same-key reject.
                // Lineage cites the mandate id (a genuine, resolvable Tier-B source).
                let inducing_key = serde_json::json!({
                    "mandate": m.mandate_grant_id, "target": m.target, "sources_hash": sources_hash,
                });
                self.append_write_rejected_with(
                    Some(&m.target),
                    &reason,
                    &inducing_key,
                    std::slice::from_ref(&m.mandate_grant_id),
                    crate::graph::M6C_PROPOSER_PRODUCER,
                )?;
                report.proposals_rejected += 1;
                return Ok(());
            }
            MandateAction::Propose { expected, lineage, op, sources_hash } => {
                (expected, lineage, op, sources_hash)
            }
        };

        // ── 1. inducing_key = the SOURCE-STATE key (§5.4): {mandate, target, sources_hash}.
        //    This is the exact shape `is_mandate_proposal_suppressed` matches on; keying on
        //    `sources_hash` makes a NEW source-state a fresh ask (a prior state's decline
        //    cannot suppress it) while a decline is sticky for THIS state. ──
        let inducing_key = serde_json::json!({
            "mandate": m.mandate_grant_id, "target": m.target, "sources_hash": sources_hash,
        });

        // ── 2. Idempotency: a suppressed (target, key) — an OPEN proposal, a prior
        //    write_rejected, or a DECLINED sync for THIS source-state — skips silently. ──
        if self.is_mandate_proposal_suppressed(&m.target, &inducing_key)? {
            return Ok(());
        }

        // ── 3. Caps (ELIDE, never reject — a cap is retryable). Two bounds:
        //    - the GLOBAL per-tick cap, SHARED with M6b via `report.proposals_emitted`, so
        //      M6b + M6c proposals together never exceed MAX_PROPOSALS_PER_TICK in one tick;
        //    - the per-mandate cap. A mandate owns ONE target, so one `run_mandate` emits at
        //      most one proposal — the per-mandate bound is structural here; the explicit
        //      guard encodes it (and is future-proof if a mandate ever fans out). ──
        let already_emitted_for_mandate = 0usize; // one target per run_mandate call
        if report.proposals_emitted >= crate::extract::MAX_PROPOSALS_PER_TICK
            || already_emitted_for_mandate >= crate::graph::MAX_PROPOSALS_PER_MANDATE_PER_TICK
        {
            report.proposals_elided_cap += 1;
            return Ok(());
        }

        // ── 4. Gate + record. Build the proposal with the engine-gathered lineage (mandate
        //    ∪ source ids) and the synthesized whole-file bytes; the op is the phase's
        //    Create/Edit decision (M6c never deletes). The hash is the engine's canonical
        //    content hash (the same `get_proposal_bytes_checked` recomputes). ──
        let expected_hash = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(&expected))
        };
        let gated = self.propose_write(crate::actuator::WriteProposal {
            target: std::path::PathBuf::from(&m.target),
            new_content: expected.clone(),
            op,
            source_event_ids: lineage.clone(),
            rationale: m.recipe.clone(),
        })?;

        // A gate FAILURE → `write_rejected` (terminal for this source-state).
        // `gate_reject_reason()` folds BOTH signals — a `reject_reason` OR `!allowed`
        // (the load-bearing never-widen check; `execute_write_resolving` re-checks at
        // execute). Single-sourced on `WriteVerdict` so M6b/M6c cannot drift.
        if let Some(reason) = gated.verdict.gate_reject_reason() {
            self.append_write_rejected_with(
                Some(&m.target),
                reason,
                &inducing_key,
                &lineage,
                crate::graph::M6C_PROPOSER_PRODUCER,
            )?;
            report.proposals_rejected += 1;
            return Ok(());
        }

        // Record the gated proposal (M6c producer) + its bytes (the side-table cache). The
        // verdict_summary mirrors M6b's shape so the app surfaces the same fields. The
        // append chokepoint stamps `origin:"external"` automatically when the lineage cites
        // an external source — taint is never shed.
        let op_str = match op {
            crate::actuator::WriteOp::Create => "create",
            crate::actuator::WriteOp::Edit => "edit",
            // M6c never deletes; the phase only ever returns Create/Edit. A Delete here
            // would be a contract violation, so name it explicitly rather than silently
            // mislabeling — it cannot occur.
            crate::actuator::WriteOp::Delete => "delete",
        };
        let verdict_summary = serde_json::json!({
            "requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint),
            "allowed": gated.verdict.allowed,
        });
        let pid = self.append_write_proposal_with(
            &m.target,
            op_str,
            &expected_hash,
            expected.len() as u64,
            &m.recipe,
            &inducing_key,
            &verdict_summary,
            &lineage,
            crate::graph::M6C_PROPOSER_PRODUCER,
        )?;
        self.put_proposal_bytes(&pid, &expected, &expected_hash)?;
        report.proposals_emitted += 1;
        Ok(())
    }

    /// M6b reconciliation for ONE confirmed contradiction `r` (resolved entity ids),
    /// called from `evolve_once`'s confirmed-contradiction loop AFTER the `invalidate`
    /// is committed (spec §5.1–5.3). Best-effort: an `Err` here is logged by the
    /// caller and the loop proceeds — the committed invalidate is NEVER unwound.
    ///
    /// Steps (spec §5.7):
    /// 1. **Backward walk** `neighbors(&r.src)` → the ACTIVE edge matching
    ///    `(relation, dst)` → its BARE `edge_id`. (Must run while the edge is still
    ///    active — before the tick's end-of-loop `rebuild_graph` folds it closed.)
    /// 2. `reconciliation_lineage(edge_id, read_set)` — the `Err` is PROPAGATED (never
    ///    `unwrap_or_default`), the fail-closed signal against laundering taint to an
    ///    empty lineage.
    /// 3. For each lineage id that `is_reconcilable_target` reports as a still-current
    ///    tracked file (deduped by canonical path — Q-2: one proposal per distinct
    ///    file, each against the cap): idempotency-skip, cap-elide, read LIVE bytes,
    ///    synthesize a corrected rewrite, run the M6a gate, and record either a
    ///    `write_proposal` (+ proposal bytes) or a `write_rejected`.
    ///
    /// **`write_rejected` is reserved for genuine synthesis/gate failures**
    /// (unrenderable target, empty rewrite, a gate `reject_reason`). The cap-elision
    /// and the off-switch (handled by the caller) MUST NOT emit `write_rejected` — a
    /// rejection PERMANENTLY suppresses that `(path, inducing_key)` (T6), so those
    /// deferrals only `continue`/count to stay retryable on a later tick.
    #[cfg(unix)]
    fn reconcile_confirmed_contradiction(
        &self,
        reasoner: &dyn crate::reason::Reasoner,
        r: &crate::extract::ProposedRetraction,
        read_set: &[String],
        report: &mut EvolveReport,
    ) -> Result<(), BossclawError> {
        use sha2::{Digest, Sha256};

        // ── 1. Backward walk to the retired edge's BARE event id. `neighbors` is the
        //    active-edge lookup; the edge is still active here (pre rebuild_graph). ──
        let edge_id = match self
            .neighbors(&r.src)?
            .into_iter()
            .find(|e| e.relation == r.relation && e.dst == r.dst)
        {
            Some(e) => e.edge_id, // `edge_id == ev.id` — exactly the bare link-event id
            None => return Ok(()), // no active edge (shouldn't happen for a confirmed retraction)
        };

        // ── 2. Engine-gathered lineage (D8). Propagate the Err — never launder to []. ──
        let lineage = self.reconciliation_lineage(&edge_id, read_set)?;

        // ── 3. Find distinct current target files in the lineage, deduped by path. ──
        let mut seen_paths: HashSet<String> = HashSet::new();
        for id in &lineage {
            let Some(rec) = self.is_reconcilable_target(id)? else { continue };
            if !seen_paths.insert(rec.canonical_path.clone()) {
                continue; // already proposed against this file this contradiction
            }

            // SP4 change-(a): be smart — check the write-grant FIRST. An ingested-but-not-
            // -writable target is SKIPPED (no LLM, no propose, no write_rejected) so the
            // folder stays clean and re-enabling it later starts fresh. We use
            // `is_write_allowed` (not `gate_reject_reason`, which folds `!allowed` into the
            // genuine-reject set) so the pure no-grant case never records terminal dead state.
            if !self.is_write_allowed(std::path::Path::new(&rec.canonical_path))? {
                continue;
            }

            // a. inducing_key = the RESOLVED contradiction (entity ids).
            let inducing_key = serde_json::json!({
                "src": r.src, "relation": r.relation, "dst": r.dst,
            });

            // b. idempotency: skip if already pending/rejected for (path, key).
            if self.is_proposal_suppressed(&rec.canonical_path, &inducing_key)? {
                continue;
            }

            // c. cap: elide (count, do NOT reject) once the per-tick max is reached.
            if report.proposals_emitted >= crate::extract::MAX_PROPOSALS_PER_TICK {
                report.proposals_elided_cap += 1;
                continue;
            }

            // d. read LIVE bytes. A read failure, an oversized file, or non-UTF-8
            //    content is a genuine synthesis failure → write_rejected (terminal).
            //    Read once, size-check, then convert in ONE fallible step (no second
            //    UTF-8 pass): `from_utf8` is the validation, its `Err` is the reject.
            let within_cap = std::fs::read(&rec.canonical_path)
                .ok()
                .filter(|bytes| bytes.len() <= crate::extract::MAX_INPUT_TEXT_BYTES);
            let live_text = match within_cap.map(String::from_utf8) {
                Some(Ok(text)) => text,
                _ => {
                    self.append_write_rejected(
                        Some(&rec.canonical_path),
                        "unrenderable_target",
                        &inducing_key,
                        &lineage,
                    )?;
                    report.proposals_rejected += 1;
                    continue;
                }
            };

            // e. The engine-rendered fact (build_rewrite_prompt sanitizes it).
            let engine_fact = format!("{} -{}-> {}", r.src, r.relation, r.dst);

            // f. Synthesize the corrected rewrite. A missing/empty `corrected_content`
            //    is a genuine synthesis failure → write_rejected (terminal).
            let prompt = crate::reconcile::build_rewrite_prompt(&engine_fact, &live_text);
            let out = reasoner.complete_json(
                RECONCILE_SYSTEM,
                &prompt,
                &crate::reconcile::rewrite_schema(),
            )?;
            let corrected = out.get("corrected_content").and_then(|v| v.as_str()).unwrap_or("");
            if corrected.is_empty() {
                self.append_write_rejected(
                    Some(&rec.canonical_path),
                    "empty_rewrite",
                    &inducing_key,
                    &lineage,
                )?;
                report.proposals_rejected += 1;
                continue;
            }

            // g. Hash the proposed bytes EXACTLY as the engine hashes file content.
            let bytes = corrected.as_bytes().to_vec();
            let hash = hex::encode(Sha256::digest(&bytes));

            // h. Run the M6a write gate (taint anchor, eligibility, op×existence).
            let gated = self.propose_write(crate::actuator::WriteProposal {
                target: rec.canonical_path.clone().into(),
                new_content: bytes.clone(),
                op: crate::actuator::WriteOp::Edit,
                source_event_ids: lineage.clone(),
                rationale: engine_fact.clone(),
            })?;

            // i. A GENUINE gate failure (symlink/taint/op×existence) → write_rejected.
            //    `reject_reason` (NOT `gate_reject_reason`) is the genuine-reject signal:
            //    a bare `!allowed` (grant revoked between the hoisted check above and here)
            //    is skipped, never recorded as terminal dead state.
            if let Some(reason) = gated.verdict.reject_reason.as_deref() {
                self.append_write_rejected(
                    Some(&rec.canonical_path),
                    reason,
                    &inducing_key,
                    &lineage,
                )?;
                report.proposals_rejected += 1;
                continue;
            }
            if !gated.verdict.allowed {
                // Grant vanished mid-tick — skip (retryable), do not reject.
                continue;
            }

            // j. Record the gated proposal + its bytes (the worklist side table). The
            //    verdict_summary also carries the base fingerprint (`base_content_hash`) so the
            //    desktop apply can fail closed if the file diverged since this propose
            //    (a fresh re-propose at apply re-bases on LIVE bytes and cannot see the drift).
            let verdict_summary = serde_json::json!({
                "requires_loud_modal": gated.verdict.requires_loud_modal,
                "taint": format!("{:?}", gated.verdict.taint),
                "allowed": gated.verdict.allowed,
                "base_content_hash": gated.verdict.base_content_hash,
            });
            let pid = self.append_write_proposal(
                &rec.canonical_path,
                "edit",
                &hash,
                bytes.len() as u64,
                &engine_fact,
                &inducing_key,
                &verdict_summary,
                &lineage,
            )?;
            self.put_proposal_bytes(&pid, &bytes, &hash)?;
            report.proposals_emitted += 1;
        }
        Ok(())
    }

    /// Resolve one `mention` to an entity id, minting a signed `entity` event +
    /// its resolution vector when resolution says Mint. Returns `(entity_id,
    /// minted)` where `minted` is `true` iff a fresh entity was created.
    ///
    /// `entity_type` labels a freshly minted entity; `read_set` is the provenance
    /// (EVENT ids only) stamped as the mint's `source_event_ids`. The
    /// `Adjudicate` arm is already collapsed to Merge/Mint inside
    /// [`EventLog::resolve_mention`]; the match below is exhaustive defensively.
    ///
    /// `tick_cache` carries mints WITHIN the current tick: because the entity
    /// index is rebuilt only after the batch (Rev 2 F6), a mention this tick
    /// already minted is not yet searchable, so the cache is consulted FIRST to
    /// reuse that id (returning `minted = false` — the mint was already counted).
    /// This keeps one surface mention = one entity per tick and is what lets the
    /// within-tick edge dedup (F5) compare equal keys.
    fn resolve_or_mint(
        &self,
        embedder: &dyn Embedder,
        reasoner: &dyn crate::reason::Reasoner,
        mention: &str,
        entity_type: &str,
        read_set: &[String],
        tick_cache: &mut HashMap<String, String>,
    ) -> Result<(String, bool), BossclawError> {
        if let Some(id) = tick_cache.get(mention) {
            return Ok((id.clone(), false)); // already minted/resolved this tick
        }
        let resolved = match self.resolve_mention(embedder, reasoner, mention)? {
            ResolveDecision::Merge(id) => (id, false),
            // resolve_mention collapses Adjudicate→Merge/Mint; Mint (and the
            // unreachable Adjudicate) mint a fresh signed entity.
            ResolveDecision::Mint | ResolveDecision::Adjudicate(_) => {
                let new_id = self.entity(mention, &[], entity_type, reasoner.model_id(), read_set)?;
                self.derive_entity_vector(embedder, &new_id, mention)?;
                (new_id, true)
            }
        };
        tick_cache.insert(mention.to_string(), resolved.0.clone());
        Ok(resolved)
    }

    /// The current 1-hop neighborhood of the resolved entity ids as human-readable
    /// `src -relation-> dst` lines (spec §6 cheat-sheet, second half), de-duped and
    /// deterministically ordered. Fed to Pass B so the model can confirm
    /// contradictions against KNOWN edges. Best-effort per id: a graph read error
    /// on one id is skipped (degrade, never break — spec §10).
    ///
    /// Endpoints are rendered by their human-readable NAME, not the opaque
    /// `entity:<ulid>` node id: first the surface mention used in THIS memory
    /// (the inverse of `mention_to_id`, so the line aligns with the identifiers in
    /// the Pass-B PROPOSED lists), else the entity's stored `label`, else the raw
    /// id as a last resort. A small local model (the live 7b) cannot reason about
    /// opaque ULIDs — worse, it copies them into its echoed relation/retraction
    /// endpoints, which then fail the `(src, relation, dst)` identity match in
    /// [`crate::extract::intersect_keep_floor`] and silently drop the retraction,
    /// so the F4 contradiction-retirement never fires. Naming the endpoints keeps
    /// the neighborhood usable AND keeps the model's echo aligned with the floor.
    fn neighborhood_lines(
        &self,
        mention_to_id: &HashMap<String, String>,
    ) -> Result<Vec<String>, BossclawError> {
        // id → display name. Prefer the surface mention from THIS memory (so the
        // rendered line uses the exact identifiers the model sees in the PROPOSED
        // lists); fall back to the stored entity label for endpoints this memory
        // did not mention (the OTHER end of a 1-hop edge).
        let mut name_of: HashMap<String, String> = HashMap::new();
        for (mention, id) in mention_to_id {
            // First-mention-wins is irrelevant here (a given id maps from one
            // mention per tick via the resolve cache); insert is fine.
            name_of.entry(id.clone()).or_insert_with(|| mention.clone());
        }
        for ent in self.all_entities()? {
            name_of.entry(ent.entity_id).or_insert(ent.label);
        }
        // Render an endpoint id to its name, falling back to the raw id so an
        // unknown endpoint is never silently blanked.
        let render = |id: &str| name_of.get(id).cloned().unwrap_or_else(|| id.to_string());

        let mut seen: BTreeMap<String, ()> = BTreeMap::new();
        for id in mention_to_id.values() {
            let edges = match self.neighbors(id) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("evolve: neighborhood lookup failed for {id}: {e}");
                    continue;
                }
            };
            for edge in edges {
                // extraction-from-files I2: sanitize file-derived endpoint names and
                // relation so a control-char/overlong label can't escape the line.
                // No-op for well-formed names (no control chars, < 200 bytes).
                let src_name = crate::summarize::sanitize_ident(&render(&edge.src));
                let rel = crate::summarize::sanitize_ident(&edge.relation);
                let dst_name = crate::summarize::sanitize_ident(&render(&edge.dst));
                seen.insert(
                    format!("{src_name} -{rel}-> {dst_name}"),
                    (),
                );
            }
        }
        Ok(seen.into_keys().collect())
    }

    /// A snapshot of evolve-loop health (spec §8). `queue_depth` = unprocessed
    /// extractable (`memory` + `file_ingested`) events behind the cursor (LIVE);
    /// `enabled` reflects the sticky off-switch (LIVE).
    /// `last_tick_ms`/`error_count`/`last_error` are honest M4a stubs
    /// (`None`/`0`/`None`) — the running tick/error counters are owned by M7's
    /// long-lived loop driver, not persisted here, so this method stays a pure
    /// read and is unit-testable.
    pub fn evolve_status(&self) -> Result<EvolveStatus, BossclawError> {
        let cursor = self.evolve_cursor()?;
        let queue_depth = {
            let store = self.inner.lock().expect(POISON);
            let conn = store.conn();
            conn.query_row(
                "SELECT count(*) FROM events WHERE event_type IN (?1, ?2) AND seq > ?3",
                rusqlite::params![MEMORY_EVENT_TYPE, crate::graph::FILE_INGESTED_EVENT_TYPE, cursor],
                |r| r.get::<_, i64>(0),
            )? as usize
        };
        Ok(EvolveStatus {
            queue_depth,
            last_tick_ms: None,
            error_count: 0,
            last_error: None,
            enabled: self.evolve_enabled()?,
        })
    }

    /// Persist the current tip as the signed high-water mark (debounced by the
    /// caller — every K events / on idle / on clean shutdown, NOT per append).
    pub fn checkpoint_highwater(&self) -> Result<(), BossclawError> {
        let hw = match &self.highwater {
            Some(h) => h,
            None => return Ok(()),
        };
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let count: i64 = conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
        let tip_hash: String = conn
            .query_row("SELECT hash FROM events ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
            .unwrap_or_else(|_| GENESIS.to_string());
        hw.save(&Mark { count, tip_hash })
    }

    /// Every applied write attributable to a MANDATE (M6c), newest-LAST in event order
    /// (`events_of_types` returns `seq ASC`; the desktop reverses for newest-first display).
    ///
    /// Attribution requires a JOIN: a `file_written` is stamped `ACTUATOR_PRODUCER`, so the only
    /// link to a mandate is `content.resolves_proposal` → a `write_proposal` whose
    /// `model_meta.model_id == M6C_PROPOSER_PRODUCER`. Two invariants govern the join (SP5 L2 +
    /// security L3):
    ///   • COMPLETENESS — because Option B removed the preventive review, an applied M6c write that
    ///     never surfaced (with no Undo offered) would be an invisible autonomous change. In PRACTICE
    ///     the join is TOTAL: an M6c `write_proposal` is never GC'd while its `file_written` is live
    ///     (a resolved proposal is retained), so every applied M6c write is attributable here.
    ///   • FAIL-CLOSED against FALSE attribution — a row is included ONLY when its resolved proposal's
    ///     producer is PROVABLY `M6C_PROPOSER_PRODUCER`. A `file_written` whose `resolves_proposal`
    ///     cannot be resolved to a known M6c producer is EXCLUDED (not "degraded to target-only"):
    ///     claiming an unprovable write is a mandate write would be worse than omitting it, and the
    ///     completeness invariant means this exclusion is unreachable for a real M6c write anyway.
    /// `#[cfg(unix)]` (mandate surface).
    #[cfg(unix)]
    pub fn mandate_writes(&self) -> Result<Vec<MandateWriteRecord>, BossclawError> {
        use std::collections::{HashMap, HashSet};
        // proposal id → producer (from write_proposal events).
        let mut producer_of: HashMap<String, String> = HashMap::new();
        // file_written_id → (target, written_at) for the FORWARD (non-undo) applied writes that
        // resolve a proposal.
        let mut applied: Vec<(String, String, String, String)> = Vec::new(); // (fw_id, target, ts, resolves)
        // the set of file_written ids that a later undo cites (`undo_of`).
        let mut undone_ids: HashSet<String> = HashSet::new();

        for ev in self.events_of_types(&[
            crate::graph::WRITE_PROPOSAL_EVENT_TYPE,
            crate::graph::FILE_WRITTEN_EVENT_TYPE,
        ])? {
            match ev.event_type.as_str() {
                t if t == crate::graph::WRITE_PROPOSAL_EVENT_TYPE => {
                    let producer = ev.model_meta.as_ref()
                        .map(|m| m.model_id.clone()).unwrap_or_default();
                    producer_of.insert(ev.id.clone(), producer);
                }
                _ => {
                    // file_written. An UNDO carries `undo_of` and NO `resolves_proposal` — record
                    // the undone id and skip it as an attributed row.
                    if let Some(undone) = ev.content.get("undo_of").and_then(|v| v.as_str()) {
                        undone_ids.insert(undone.to_string());
                        continue;
                    }
                    if let Some(resolves) = ev.content.get("resolves_proposal").and_then(|v| v.as_str()) {
                        let target = ev.content.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        applied.push((ev.id.clone(), target, ev.ts.clone(), resolves.to_string()));
                    }
                }
            }
        }

        Ok(applied
            .into_iter()
            .filter(|(_fw, _t, _ts, resolves)| {
                // Keep ONLY writes whose resolved proposal is an M6c mandate proposal. An
                // unresolvable producer (GC'd proposal) cannot be proven M6c ⇒ excluded (in
                // practice unreachable — an M6c proposal outlives its file_written).
                producer_of.get(resolves).map(|p| p == crate::graph::M6C_PROPOSER_PRODUCER).unwrap_or(false)
            })
            .map(|(fw, target, ts, _resolves)| {
                let undone = undone_ids.contains(&fw);
                MandateWriteRecord { file_written_id: fw, target, written_at: ts, undone }
            })
            .collect())
    }
}

/// The text fed to the embedder for an event, or `None` if the event is not
/// embeddable.
///
/// The kinds listed in [`EMBEDDABLE_EVENT_TYPES`] carry embeddable prose at
/// `content["text"]` (single-sourced there so this comment cannot go stale).
/// `config`, `grant`, and other control events return `None`.
fn embeddable_text(event: &Event) -> Option<String> {
    if !EMBEDDABLE_EVENT_TYPES.contains(&event.event_type.as_str()) {
        return None;
    }
    event.content["text"].as_str().map(String::from)
}

/// Embed a single text as a one-item batch and return its vector.
///
/// Centralises the batch-of-one call + the "exactly one vector back" invariant
/// so both [`EventLog::derive_vector`] and [`EventLog::rederive_pending`] agree
/// on the shape contract. A batch that returns the wrong count is surfaced as
/// [`BossclawError::Embed`] rather than panicking.
fn embed_one(embedder: &dyn Embedder, text: &str) -> Result<Vec<f32>, BossclawError> {
    let mut batch = embedder.embed(&[text.to_string()])?;
    if batch.len() != 1 {
        return Err(BossclawError::Embed(format!(
            "embedder returned {} vectors for a 1-item batch",
            batch.len()
        )));
    }
    Ok(batch.remove(0))
}

/// Encode a vector as little-endian `f32` bytes for the `embedding` BLOB.
fn vec_to_blob(vec: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(vec.len() * F32_BYTES);
    for &x in vec {
        blob.extend_from_slice(&x.to_le_bytes());
    }
    blob
}

/// Decode little-endian `f32` bytes from an `embedding` BLOB.
///
/// Returns [`BossclawError::Store`] if the byte length is not a multiple of
/// [`F32_BYTES`] (a corrupt or truncated blob).
fn blob_to_vec(blob: &[u8]) -> Result<Vec<f32>, BossclawError> {
    if !blob.len().is_multiple_of(F32_BYTES) {
        return Err(BossclawError::Store(format!(
            "embedding blob length {} is not a multiple of {F32_BYTES}",
            blob.len()
        )));
    }
    let mut out = Vec::with_capacity(blob.len() / F32_BYTES);
    for chunk in blob.chunks_exact(F32_BYTES) {
        // `chunks_exact(4)` guarantees exactly 4 bytes; index directly rather
        // than `try_into` (which is unreachable and confuses the reader).
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(out)
}

/// Resolve the raw results of the two recall arms into live arm data, applying
/// spec §10 graceful degradation.
///
/// | vector result | keyword result | outcome |
/// |---|---|---|
/// | `Ok(hits)` | `Ok(hits)` | both arms active |
/// | `Err(_)` | `Ok(hits)` | keyword-only (vector failure logged) |
/// | `Ok(hits)` | `Err(_)` | vector-only (keyword failure logged) |
/// | `Err(ve)` | `Err(_)` | `Err(InvalidInput(…ve…))` |
///
/// This is a **pure** function (no I/O, no `self`) so it can be unit-tested
/// directly without a database. `recall` delegates the arm-failure logic here.
pub fn resolve_arms(
    vector: Result<Vec<ArmHit>, BossclawError>,
    keyword: Result<Vec<ArmHit>, BossclawError>,
) -> Result<ArmPair, BossclawError> {
    match (vector, keyword) {
        (Ok(v), Ok(k)) => Ok((v, k)),
        (Err(ve), Ok(k)) => {
            log::warn!("recall: vector arm unavailable, degrading to keyword-only: {ve}");
            Ok((Vec::new(), k))
        }
        (Ok(v), Err(ke)) => {
            log::warn!("recall: keyword arm unavailable, degrading to vector-only: {ke}");
            Ok((v, Vec::new()))
        }
        (Err(ve), Err(ke)) => {
            log::warn!("recall: both arms unavailable (keyword: {ke})");
            Err(BossclawError::InvalidInput(format!(
                "recall failed: both arms unavailable (vector: {ve})"
            )))
        }
    }
}

/// Format a session's `ended_at` (Unix seconds) as a `YYYY-MM-DD` label for the
/// embeddable text. Deterministic and clock-free: it renders the caller-supplied
/// timestamp, never `now`. Out-of-range timestamps fall back to the raw integer.
fn session_date_label(ended_at: i64) -> String {
    chrono::DateTime::from_timestamp(ended_at, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| ended_at.to_string())
}

/// Build the signed content of a `session_captured` event (SP3). `text` is
/// top-level (so `embeddable_text` finds it) and title-derived; `origin` is the
/// external taint stamp; the metadata fields let [`fold_sessions`] reconstruct a
/// [`CurrentSession`]. Mirrors `ingest::file_ingested_content`.
fn session_captured_content(meta: &SessionMeta) -> serde_json::Value {
    serde_json::json!({
        "text": format!("{} — {} ({})", meta.title, meta.project, session_date_label(meta.ended_at)),
        "origin": crate::graph::EXTERNAL_ORIGIN,
        "session_id": meta.session_id,
        "title": meta.title,
        "project": meta.project,
        "tool": meta.tool,
        "started_at": meta.started_at,
        "ended_at": meta.ended_at,
        "path": meta.path,
        "sha256": meta.sha256,
        "approx_bytes": meta.approx_bytes,
    })
}

/// A ground-truth `session_captured` Event (`model_meta: None` → plain
/// append/append_pair), signed by the engine DID.
fn session_captured_event(meta: &SessionMeta, signer_did: String) -> Event {
    Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: crate::graph::SESSION_CAPTURED_EVENT_TYPE.to_string(),
        content: session_captured_content(meta),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: signer_did,
        signature: None,
    }
}

/// A ground-truth `supersede` Event retiring `prior_id` (reuses
/// `SUPERSEDE_EVENT_TYPE` with `model_meta: None`; cross-fold safety holds via
/// disjoint event ids). Shared by [`EventLog::capture_session`] (session
/// supersedes) and [`EventLog::supersede_note`] (note supersedes). Mirrors
/// `ingest::ground_truth_supersede`, kept local because that helper is
/// `#[cfg(unix)]`-private to the ingest module.
fn ground_truth_supersede_event(prior_id: &str, signer_did: String) -> Event {
    Event {
        id: String::new(),
        ts: String::new(),
        valid_time: None,
        event_type: crate::graph::SUPERSEDE_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "supersedes": prior_id }),
        model_meta: None,
        prev_hash: String::new(),
        hash: None,
        signed_by_did: signer_did,
        signature: None,
    }
}

/// Parse a `session_captured` event into a [`CurrentSession`], or `None` if any
/// field is missing/mistyped (malformed → skipped by the fold, not fatal —
/// mirrors `graph::parse_page_content`).
fn parse_session_content(ev: &Event) -> Option<CurrentSession> {
    let c = &ev.content;
    Some(CurrentSession {
        event_id: ev.id.clone(),
        session_id: c.get("session_id")?.as_str()?.to_string(),
        title: c.get("title")?.as_str()?.to_string(),
        project: c.get("project")?.as_str()?.to_string(),
        tool: c.get("tool")?.as_str()?.to_string(),
        started_at: c.get("started_at")?.as_i64()?,
        ended_at: c.get("ended_at")?.as_i64()?,
        path: c.get("path")?.as_str()?.to_string(),
        sha256: c.get("sha256")?.as_str()?.to_string(),
        approx_bytes: c.get("approx_bytes")?.as_u64()?,
    })
}

/// The projection produced by [`fold_sessions`]. A named struct (not a tuple)
/// because `deleted` and `superseded` share a type but NOT a meaning —
/// `deleted` holds `session_id`s, `superseded` holds EVENT ids — so a
/// positional swap would compile silently. House-consistent with the other
/// folds returning named projections.
struct SessionFold {
    /// The CURRENT sessions: the latest `session_captured` per `session_id`
    /// NOT retired by a `supersede` and whose `session_id` has NO
    /// `session_deleted` tombstone. Sorted by `session_id` (deterministic).
    current: Vec<CurrentSession>,
    /// Tombstoned `session_id`s (NOT event ids) — so `capture_session` can
    /// enforce I9 (a deleted session is never recapturable).
    deleted: std::collections::HashSet<String>,
    /// EVENT ids retired by a `supersede`. Because the fold's input carries
    /// EVERY `supersede` event (they are shared across the page/file/session/
    /// note folds), this is the complete retired-id universe — the recall
    /// exclusion (A3) reuses it instead of a second scan.
    superseded: std::collections::HashSet<String>,
    /// Note EVENT ids retired by a DISTINCT `note_retired` marker and NOT yet
    /// reversed by an `unretire` (Rung-3). Kept strictly separate from
    /// `superseded` so an `unretire` reverses a retire and NEVER an edit — a
    /// bare supersede is byte-identical to an edit, so folding retires into
    /// `superseded` would let `unretire` resurrect edited-away content. Read by
    /// recall's memory-arm exclusion, the embed-rebuild gate, and the
    /// retire/unretire validation (Task 2).
    retired_notes: std::collections::HashSet<String>,
    /// `(session_id, passage_id)` pairs retired by a `passage_retired` marker and
    /// NOT yet reversed by an `unretire` (Rung-3) — the passage-granularity twin
    /// of `retired_notes`. Read by `rebuild_conflict_index` to exclude retired
    /// passages from the conflict index at build time.
    retired_passages: std::collections::HashSet<(String, usize)>,
}

/// One OPEN conflict proposal, folded from a `conflict_proposal` event whose BOTH refs are still
/// current. Carries the full persisted shape (idempotency reads only `a_ref`/`b_ref`; the
/// `pending_conflict_proposals` projection reads all seven). Internal; the PUBLIC row is
/// [`ConflictProposalRow`] (Task 7). Private, so no field docs.
#[cfg(unix)]
struct OpenConflictProposal {
    id: String,
    a_ref: crate::index::ConflictRef,
    b_ref: crate::index::ConflictRef,
    winner_hint: String,
    confidence_band: String,
    why: String,
    detected_at: i64,
}

/// Which terminal action resolved a proposal (spec §2.1). Ordered so `conflict_resolved`'s `action`
/// string maps here. Portable data type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionKind {
    /// `conflict_resolved` with `action == "retire_older"` — a_ref retired.
    RetireOlder,
    /// `conflict_resolved` with `action == "retire_newer"` — b_ref retired.
    RetireNewer,
    /// `coexist_allowed` — keep both.
    KeepBoth,
    /// `dismissed` — snoozed.
    Dismiss,
}

/// One proposal's terminal record (spec §2.1). `retired_event_id` is present only for the two retire
/// kinds. Internal to the resolution fold.
#[cfg(unix)]
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields consumed by resolve_conflict's terminal-state guard (Task 6)
struct ResolutionRecord {
    kind: ResolutionKind,
    retired_event_id: Option<String>,
}

/// The two PAIR-key exclusion sets derived from the terminal resolution markers (spec §2.2). Both keyed
/// by [`crate::index::ConflictRef::unordered_pair_key`] — the SAME space as the finder's `open_pairs` and
/// `conflict_pair_key`. ONE reader ([`EventLog::resolution_exclusions`]) produces both, so the finder
/// union and the `pending_conflict_proposals` filter can never drift on `session_heads` liveness.
#[cfg(unix)]
#[derive(Debug, Default, Clone)]
pub struct ResolutionExclusions {
    /// `unordered_pair_key`s the owner chose KEEP-BOTH — never re-proposed, dropped from the read surface.
    pub coexist_pairs: std::collections::HashSet<String>,
    /// `unordered_pair_key`s DISMISSED and still LIVE (every referenced session head unchanged, §3.1).
    pub dismissed_pairs: std::collections::HashSet<String>,
}

/// Pure session fold (SP3 §4b), mirroring [`crate::graph::fold_pages`]. Given
/// `session_captured` + `session_deleted` + `supersede` + the Rung-3 retire
/// markers (`note_retired` / `passage_retired` / `unretire`) in `seq ASC`
/// order, projects them into a [`SessionFold`] (see its field docs) — the retire
/// markers fold into their OWN sets, strictly disjoint from `superseded`.
///
/// Deterministic → byte-identical rebuild. Cross-fold-safe: a page/file
/// `supersede` targets a disjoint event id, so it never retires a session event.
fn fold_sessions(events: &[Event]) -> SessionFold {
    use std::collections::{BTreeMap, HashSet};
    let mut superseded: HashSet<String> = HashSet::new();
    let mut deleted: HashSet<String> = HashSet::new();
    let mut retired_notes: HashSet<String> = HashSet::new();
    let mut retired_passages: HashSet<(String, usize)> = HashSet::new();
    // A retired passage is keyed on (session_id, passage_id); `passage_id` is a
    // chunk ordinal stored as a JSON number.
    let sid = |ev: &Event| {
        ev.content
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    let pid = |ev: &Event| {
        ev.content
            .get("passage_id")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
    };
    for ev in events {
        match ev.event_type.as_str() {
            crate::graph::SUPERSEDE_EVENT_TYPE => {
                if let Some(p) = ev.content.get("supersedes").and_then(|v| v.as_str()) {
                    superseded.insert(p.to_string());
                }
            }
            crate::graph::SESSION_DELETED_EVENT_TYPE => {
                if let Some(s) = ev.content.get("session_id").and_then(|v| v.as_str()) {
                    deleted.insert(s.to_string());
                }
            }
            // Rung-3 retire markers fold into their OWN sets (disjoint from
            // `superseded`). Events arrive `seq ASC`, so a later `unretire`
            // deterministically reverses an earlier retire.
            crate::graph::NOTE_RETIRED_EVENT_TYPE => {
                if let Some(id) = ev.content.get("retires").and_then(|v| v.as_str()) {
                    retired_notes.insert(id.to_string());
                }
            }
            crate::graph::PASSAGE_RETIRED_EVENT_TYPE => {
                if let (Some(s), Some(p)) = (sid(ev), pid(ev)) {
                    retired_passages.insert((s, p));
                }
            }
            crate::graph::UNRETIRE_EVENT_TYPE => {
                if let Some(id) = ev.content.get("unretires").and_then(|v| v.as_str()) {
                    retired_notes.remove(id);
                } else if let (Some(s), Some(p)) = (sid(ev), pid(ev)) {
                    retired_passages.remove(&(s, p));
                }
            }
            _ => {}
        }
    }
    // Latest non-superseded, non-tombstoned session per id (last write wins).
    // Built as a BTreeMap keyed on session_id so the last capture per id wins
    // and the returned Vec is deterministically sorted by session_id.
    let mut current: BTreeMap<String, CurrentSession> = BTreeMap::new();
    for ev in events {
        if ev.event_type != crate::graph::SESSION_CAPTURED_EVENT_TYPE || superseded.contains(&ev.id) {
            continue;
        }
        if let Some(cs) = parse_session_content(ev) {
            if deleted.contains(&cs.session_id) {
                continue;
            }
            current.insert(cs.session_id.clone(), cs);
        }
    }
    SessionFold {
        current: current.into_values().collect(),
        deleted,
        superseded,
        retired_notes,
        retired_passages,
    }
}

/// Pure note fold (SP3 §7/§9): given `memory` + `supersede` events (plus the
/// rung-3 `note_retired` / `unretire` markers) in `seq ASC` order, returns the
/// CURRENT notes — every `memory`-kind event NOT retired by a `supersede` AND NOT
/// retired by a live `note_retired` marker — sorted NEWEST-FIRST (`created_at`
/// desc, then `event_id` desc as a deterministic tie-break; ULIDs are monotonic +
/// lexicographically sortable, so `event_id` desc IS newest-first within a
/// same-second group).
///
/// A returned note's `superseded_by` is ALWAYS `None`: a superseded note is
/// EXCLUDED from the fold (only live heads survive), mirroring [`fold_sessions`]'s
/// current-only projection — so the Library shows an edited note in place (old text
/// gone, new text present) and recall/list stay consistent. Rung-3 retire is
/// reversible: a `note_retired` marker removes a note, a later `unretire` restores
/// it (walked in `seq` order, so the last marker wins) — kept SEPARATE from the
/// `superseded` set so an `unretire` can never resurrect an edited-away note.
/// `created_at` is the event `ts` (RFC 3339) parsed to Unix seconds, or 0 if
/// absent/unparseable (deterministic fallback — no clock read).
fn fold_notes(events: &[Event]) -> Vec<CurrentNote> {
    use std::collections::HashSet;
    let mut superseded: HashSet<&str> = HashSet::new();
    let mut retired: HashSet<&str> = HashSet::new();
    for ev in events {
        if ev.event_type == crate::graph::SUPERSEDE_EVENT_TYPE {
            if let Some(p) = ev.content.get("supersedes").and_then(|v| v.as_str()) {
                superseded.insert(p);
            }
        } else if ev.event_type == crate::graph::NOTE_RETIRED_EVENT_TYPE {
            if let Some(p) = ev.content.get("retires").and_then(|v| v.as_str()) {
                retired.insert(p);
            }
        } else if ev.event_type == crate::graph::UNRETIRE_EVENT_TYPE {
            if let Some(p) = ev.content.get("unretires").and_then(|v| v.as_str()) {
                retired.remove(p);
            }
        }
    }
    let mut notes: Vec<CurrentNote> = events
        .iter()
        .filter(|ev| {
            ev.event_type == crate::graph::MEMORY_EVENT_TYPE
                && !superseded.contains(ev.id.as_str())
                && !retired.contains(ev.id.as_str())
        })
        .map(|ev| CurrentNote {
            event_id: ev.id.clone(),
            text: ev
                .content
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            created_at: DateTime::parse_from_rfc3339(&ev.ts).map(|dt| dt.timestamp()).unwrap_or(0),
            superseded_by: None,
        })
        .collect();
    notes.sort_by(|a, b| {
        b.created_at.cmp(&a.created_at).then_with(|| b.event_id.cmp(&a.event_id))
    });
    notes
}

/// TEST-ONLY seams for the N-deep undo store (M6a, T5). Compiled out of every
/// non-test build (`#[cfg(test)]`), so there is NO production "undo hook" surface.
/// The W8 crash-ordering test installs a `pre_mutate` probe that fires inside
/// `execute_write_inner` at the exact instant AFTER the undo row is durably
/// committed and BEFORE the FS mutate, letting the test prove the ordering.
#[cfg(test)]
pub(crate) mod undo_test_hooks {
    use std::cell::RefCell;

    /// A pre-mutate probe: receives the just-committed `undo_id`. Boxed so a test can
    /// install an arbitrary closure; aliased so the thread-local type stays simple.
    type PreMutateProbe = Box<dyn FnMut(&str)>;

    thread_local! {
        /// The installed probe, if any. A thread-local (not a global) so parallel
        /// tests cannot collide.
        static PRE_MUTATE_PROBE: RefCell<Option<PreMutateProbe>> = const { RefCell::new(None) };
    }

    /// Install a probe fired right after the undo row commit + right before the FS
    /// mutate. Replaces any previous probe on this thread.
    pub(crate) fn install_pre_mutate_probe(f: PreMutateProbe) {
        PRE_MUTATE_PROBE.with(|p| *p.borrow_mut() = Some(f));
    }

    /// Remove the installed probe (so later same-thread tests are unaffected).
    pub(crate) fn clear_pre_mutate_probe() {
        PRE_MUTATE_PROBE.with(|p| *p.borrow_mut() = None);
    }

    /// Fire the installed probe (no-op if none). Called by `execute_write_inner`.
    pub(crate) fn fire_pre_mutate(undo_id: &str) {
        PRE_MUTATE_PROBE.with(|p| {
            if let Some(f) = p.borrow_mut().as_mut() {
                f(undo_id);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockEmbedder;

    const DEK: [u8; 32] = [42u8; 32];
    const KEY_BYTES: [u8; 32] = [7u8; 32];

    fn open_log(dir: &Path) -> EventLog {
        let key = SigningKey::from_bytes(&KEY_BYTES);
        EventLog::open(&dir.join("m.db"), &DEK, key).unwrap()
    }

    /// Rung-3 Phase-2 (§3.5, I5/I7): a conflict proposal is a signed event carrying ONLY typed refs,
    /// an advisory winner hint, a coarse band, a CONTENT-FREE templated `why`, and `detected_at` —
    /// never a memory body. `#[cfg(unix)]` (mirrors the write_proposal family / `build_proposer_event`).
    #[cfg(unix)]
    #[test]
    fn append_conflict_proposal_stores_typed_refs_and_no_body() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let a = ConflictRef::Note { event_id: "n_old".into() };
        let b = ConflictRef::Passage { session_id: "s1".into(), passage_id: 2 };
        let why = crate::conflict::templated_why("newer", "high", "note", "passage");
        let id = log
            .append_conflict_proposal(&a, &b, "newer", "high", &why, 1_720_000_000, &["n_old".into(), "cap_ev".into()])
            .unwrap();
        let ev = log.event_by_id(&id).unwrap().unwrap();
        assert_eq!(ev.event_type, crate::graph::CONFLICT_PROPOSAL_EVENT_TYPE);
        assert_eq!(ConflictRef::from_json(&ev.content["a_ref"]), Some(a));
        assert_eq!(ConflictRef::from_json(&ev.content["b_ref"]), Some(b));
        assert_eq!(ev.content["winner_hint"], "newer");
        assert_eq!(ev.content["confidence_band"], "high");
        assert_eq!(ev.content["why"], why, "stored why is the content-free template");
        assert_eq!(ev.content["detected_at"], 1_720_000_000i64);
        // I7: no memory-body / raw-text / raw-confidence field is persisted (only refs + template why).
        for forbidden in ["text", "a_text", "b_text", "body", "confidence"] {
            assert!(ev.content.get(forbidden).is_none(), "no {forbidden} field on the proposal");
        }
        // Lineage is the referenced memory event ids.
        let sources = ev.model_meta.as_ref().unwrap().source_event_ids.clone();
        assert_eq!(sources, vec!["n_old".to_string(), "cap_ev".to_string()]);
    }

    /// Rung-3 Phase-3 (§2.3, §2.1 MAJOR-1): `conflict_proposal_by_id` recovers `(a_ref, b_ref)` for a
    /// proposal by id REGARDLESS of open-ness. A successful retire withdraws the proposal from the OPEN
    /// set (a non-current ref ⇒ `open_conflict_proposals` drops it), so the idempotency / roll-forward
    /// path in `resolve_conflict` (Task 6) MUST recover the frozen refs from a by-id read that ignores
    /// open-ness — reading over the open set alone would return "unknown id" for a legitimately-retried
    /// retire. `#[cfg(unix)]`.
    #[cfg(unix)]
    #[test]
    fn conflict_proposal_by_id_recovers_refs_even_after_a_ref_is_retired() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);
        let n1 = log.remember(&emb, "branch is main").unwrap();
        let n2 = log.remember(&emb, "branch is master").unwrap();
        let (a, b) = (
            ConflictRef::Note { event_id: n1.clone() },
            ConflictRef::Note { event_id: n2.clone() },
        );
        let prop = log
            .append_conflict_proposal(&a, &b, "newer", "high", "why", 7, &[n1.clone(), n2.clone()])
            .unwrap();

        // Before any retire: recovers both refs, and the proposal IS in the OPEN set.
        let (ra, rb) = log.conflict_proposal_by_id(&prop).unwrap().expect("proposal exists");
        assert_eq!(ra, a);
        assert_eq!(rb, b);
        assert!(
            log.open_conflict_proposals().unwrap().iter().any(|p| p.id == prop),
            "open before retire",
        );

        // Retire a_ref (withdraws the proposal from the OPEN set) — the by-id reader STILL recovers it.
        log.retire_memory(&n1, Some(&prop)).unwrap();
        assert!(
            !log.open_conflict_proposals().unwrap().iter().any(|p| p.id == prop),
            "withdrawn from the OPEN set once a_ref is non-current",
        );
        let (ra2, rb2) = log
            .conflict_proposal_by_id(&prop)
            .unwrap()
            .expect("still readable by id after retire");
        assert_eq!(ra2, a, "a_ref recovered by id regardless of open-ness (MAJOR-1)");
        assert_eq!(rb2, b);

        // Unknown / wrong-type id → None.
        assert!(log.conflict_proposal_by_id("NOPE").unwrap().is_none());
        assert!(
            log.conflict_proposal_by_id(&n2).unwrap().is_none(),
            "a memory id is not a proposal id",
        );
    }

    /// Rung-3 Phase-3 (§2.2 / §3.1): the SINGLE `resolution_exclusions()` reader returns the coexist +
    /// LIVE-dismissed pair keys. A `coexist_allowed` pair is permanent; a `dismissed` pair is live ONLY
    /// while every referenced session's current head is unchanged (a re-capture advances the head → the
    /// dismissal lapses), and a note↔note pair needs no head (an edit mints a new id → a new pair key
    /// the stored key no longer matches). Seeded with LOCAL terminal-marker helpers (the natural producer
    /// `resolve_conflict` lands in Task 6) whose content shape mirrors EXACTLY what it will write.
    /// `#[cfg(unix)]` (the reader + `ResolutionExclusions` are part of the conflict subsystem).
    #[cfg(unix)]
    #[test]
    fn resolution_exclusions_are_live_and_dismiss_lapses_on_session_head_change() {
        use crate::index::ConflictRef;

        /// Append a `coexist_allowed` terminal marker by hand (mirrors resolve_conflict's KeepBoth write).
        fn append_coexist(log: &EventLog, pk: &str, a: &ConflictRef, b: &ConflictRef, prop: &str) {
            log.append(crate::event::Event {
                id: String::new(), ts: String::new(), valid_time: None,
                event_type: crate::graph::COEXIST_ALLOWED_EVENT_TYPE.to_string(),
                content: serde_json::json!({ "proposal_id": prop, "pair_key": pk, "a_ref": a.to_json(), "b_ref": b.to_json() }),
                model_meta: None, prev_hash: String::new(), hash: None,
                signed_by_did: log.signer_did(), signature: None,
            }).unwrap();
        }
        /// Append a `dismissed` marker whose `session_heads` records each passage member's session
        /// CURRENT head event id AT SEED TIME (§3.1). A later re-capture mints a NEW head id, so
        /// `resolution_exclusions` no longer matches → the dismissal lapses.
        fn append_dismissed(
            log: &EventLog, pk: &str, a: &ConflictRef, b: &ConflictRef, prop: &str,
            session_heads: serde_json::Value,
        ) {
            log.append(crate::event::Event {
                id: String::new(), ts: String::new(), valid_time: None,
                event_type: crate::graph::DISMISSED_EVENT_TYPE.to_string(),
                content: serde_json::json!({
                    "proposal_id": prop, "pair_key": pk,
                    "a_ref": a.to_json(), "b_ref": b.to_json(), "session_heads": session_heads,
                }),
                model_meta: None, prev_hash: String::new(), hash: None,
                signed_by_did: log.signer_did(), signature: None,
            }).unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        // Two notes → a note↔note coexist pair (KEEP-BOTH → a permanent exclusion).
        let n1 = log.remember(&emb, "branch is main").unwrap();
        let n2 = log.remember(&emb, "branch is master").unwrap();
        let (a, b) = (ConflictRef::Note { event_id: n1.clone() }, ConflictRef::Note { event_id: n2.clone() });
        let pk_notes = ConflictRef::unordered_pair_key(&a, &b);
        append_coexist(&log, &pk_notes, &a, &b, "P1");

        // A session-passage ↔ note pair → DISMISSED, with the session's CURRENT head recorded.
        let head = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &head, &["deploy on vercel".to_string()]).unwrap();
        let n3 = log.remember(&emb, "deploy on fly.io").unwrap();
        let (pa, pb) = (
            ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 },
            ConflictRef::Note { event_id: n3.clone() },
        );
        let pk_pass = ConflictRef::unordered_pair_key(&pa, &pb);
        append_dismissed(&log, &pk_pass, &pa, &pb, "P2", serde_json::json!({ "s1": head }));

        let excl = log.resolution_exclusions().unwrap();
        assert!(excl.coexist_pairs.contains(&pk_notes), "keep-both pair is a live coexist exclusion");
        assert!(excl.dismissed_pairs.contains(&pk_pass), "dismissed pair is live while the head is unchanged");

        // Re-capture the SAME session with a DIFFERENT body (advances its head) → the dismissal lapses (§3.1).
        let head2 = log.capture_session(&emb, &session_meta("s1", "bb")).unwrap();
        log.store_session_passages(&emb, &head2, &["deploy on vercel".to_string()]).unwrap();
        assert_ne!(head, head2, "a re-capture with a new sha mints a new head event id");
        let excl2 = log.resolution_exclusions().unwrap();
        assert!(excl2.coexist_pairs.contains(&pk_notes), "coexist (note↔note) is unaffected by the re-capture");
        assert!(!excl2.dismissed_pairs.contains(&pk_pass), "dismissal LAPSED after the session head advanced");
    }

    /// Rung-3 Phase-3 (§2.1 idempotency universe): `resolution_markers` folds ALL terminal markers
    /// (`conflict_resolved` / `coexist_allowed` / `dismissed`) into `proposal_id -> ResolutionRecord`, over
    /// the WHOLE log — NOT the open set (a retire withdraws the proposal from open, MAJOR-1, but its terminal
    /// marker persists so the SAME-vs-DIFFERENT-action guard in `resolve_conflict` (Task 6) still fires). A
    /// `conflict_resolved` marker captures the ACTION (`retire_older` → `RetireOlder`) and the retired event
    /// id; a `coexist_allowed` folds to `KeepBoth`; an unresolved proposal is absent. Seeded with LOCAL
    /// hand-built markers (the natural producer `resolve_conflict` lands in Task 6) whose content shape
    /// mirrors EXACTLY what it will write. `#[cfg(unix)]`.
    #[cfg(unix)]
    #[test]
    fn resolution_markers_key_by_proposal_and_first_marker_wins() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        // Append a conflict_resolved (retire_older) for PROP1 and a coexist for PROP2.
        log.append(crate::event::Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::CONFLICT_RESOLVED_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "proposal_id": "PROP1", "action": "retire_older", "retired_event_id": "E1" }),
            model_meta: None, prev_hash: String::new(), hash: None, signed_by_did: log.signer_did(), signature: None,
        }).unwrap();
        log.append(crate::event::Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::COEXIST_ALLOWED_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "proposal_id": "PROP2", "pair_key": "PK", "a_ref": {"kind":"note","event_id":"a"}, "b_ref": {"kind":"note","event_id":"b"} }),
            model_meta: None, prev_hash: String::new(), hash: None, signed_by_did: log.signer_did(), signature: None,
        }).unwrap();

        let m = log.resolution_markers().unwrap();
        let r1 = m.get("PROP1").expect("PROP1 resolved");
        assert_eq!(r1.kind, ResolutionKind::RetireOlder);
        assert_eq!(r1.retired_event_id.as_deref(), Some("E1"));
        assert_eq!(m.get("PROP2").unwrap().kind, ResolutionKind::KeepBoth);
        assert!(!m.contains_key("PROP_NONE")); // unresolved proposal folds to absent
    }

    /// Rung-3 Phase-3 (§2.1, §3.4, MAJOR-1/MAJOR-2): `resolve_conflict` settles a detected conflict.
    /// The retire actions retire the FROZEN loser (`RetireOlder`=a_ref, `RetireNewer`=b_ref — NO ts
    /// recompute) with conflict provenance, then append `conflict_resolved`. Idempotency is owned over the
    /// ALL-proposals terminal fold (a retire withdraws the proposal from the OPEN set, so the guard must NOT
    /// key off open membership): same-action repeat = clean `NoOp`, different action = reject, unknown id =
    /// error. The torn-write / cross-source roll-forward gate is retired-SET membership (§3.4), NOT
    /// tag-equality: if the frozen loser is already retired — by THIS proposal, a DIFFERENT one, or a manual
    /// tagless App retire — `resolve_conflict` appends the missing `conflict_resolved` and returns `NoOp`,
    /// never re-calling the fail-loud primitive. `#[cfg(unix)]`.
    #[cfg(unix)]
    #[test]
    fn resolve_conflict_retires_frozen_loser_and_is_idempotent_and_rolls_forward() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        // ── RetireOlder retires the FROZEN a_ref (no ts recompute) ──
        let older = log.remember(&emb, "branch is main").unwrap();
        let newer = log.remember(&emb, "branch is master").unwrap();
        let (a, b) = (ConflictRef::Note { event_id: older.clone() }, ConflictRef::Note { event_id: newer.clone() });
        let prop = log.append_conflict_proposal(&a, &b, "newer", "high", "why", 0, &[older.clone(), newer.clone()]).unwrap();

        let out = log.resolve_conflict(&prop, ResolveAction::RetireOlder).unwrap();
        assert!(matches!(out, ResolveOutcome::Applied(_)), "first resolution applies");
        assert!(!log.current_notes().unwrap().iter().any(|c| c.event_id == older), "a_ref (older) retired");
        assert!(log.current_notes().unwrap().iter().any(|c| c.event_id == newer), "b_ref (newer) survives");
        // The conflict_resolved marker exists AND the retire marker carries the conflict provenance tag.
        let resolved = log.resolution_markers_for_test(&prop); // helper: reads resolution_markers().get(prop).cloned()
        assert!(resolved.is_some(), "conflict_resolved recorded");

        // Idempotent repeat of the SAME action — EVEN THOUGH the retire withdrew the proposal from the open
        // set — is a clean no-op success (no primitive Err bubbles up).
        let again = log.resolve_conflict(&prop, ResolveAction::RetireOlder).unwrap();
        assert!(matches!(again, ResolveOutcome::NoOp), "same-action repeat = no-op success");

        // A DIFFERENT action on a resolved proposal is rejected (first resolution wins).
        let diff = log.resolve_conflict(&prop, ResolveAction::KeepBoth);
        assert!(matches!(diff, Err(BossclawError::InvalidInput(_))), "different action on resolved = reject");

        // Unknown proposal id → error.
        assert!(matches!(log.resolve_conflict("NOPE", ResolveAction::Dismiss), Err(BossclawError::InvalidInput(_))));

        // ── KeepBoth + Dismiss append their markers ──
        let k1 = log.remember(&emb, "x=1").unwrap();
        let k2 = log.remember(&emb, "x=2").unwrap();
        let kp = log.append_conflict_proposal(
            &ConflictRef::Note { event_id: k1.clone() }, &ConflictRef::Note { event_id: k2.clone() },
            "unclear", "med", "why", 0, &[k1.clone(), k2.clone()]).unwrap();
        assert!(matches!(log.resolve_conflict(&kp, ResolveAction::KeepBoth).unwrap(), ResolveOutcome::Applied(_)));
        assert_eq!(log.resolution_markers_for_test(&kp).unwrap(), ResolutionKind::KeepBoth);

        // ── Torn-write roll-forward: loser retired, conflict_resolved MISSING → append it, no-op success ──
        let o2 = log.remember(&emb, "y=old").unwrap();
        let n2b = log.remember(&emb, "y=new").unwrap();
        let (ra, rb) = (ConflictRef::Note { event_id: o2.clone() }, ConflictRef::Note { event_id: n2b.clone() });
        let torn = log.append_conflict_proposal(&ra, &rb, "newer", "high", "why", 0, &[o2.clone(), n2b.clone()]).unwrap();
        // Simulate the crash window: the tagged retire marker landed, the conflict_resolved did NOT.
        log.retire_memory(&o2, Some(&torn)).unwrap();
        assert!(log.resolution_markers_for_test(&torn).is_none(), "precondition: no conflict_resolved yet");
        let rolled = log.resolve_conflict(&torn, ResolveAction::RetireOlder).unwrap();
        assert!(matches!(rolled, ResolveOutcome::NoOp), "roll-forward returns no-op success (no primitive Err)");
        assert_eq!(log.resolution_markers_for_test(&torn).unwrap(), ResolutionKind::RetireOlder, "missing marker appended");

        // ── DISCRIMINATING roll-forward: the frozen loser is already retired by a DIFFERENT source (a MANUAL
        // App retire, via=None), NOT by this proposal. The gate is retired-SET MEMBERSHIP (§3.4), NOT
        // tag-equality — so a regression to a "was-this-proposal's-tag" gate would wrongly re-call the
        // fail-loud primitive here and bubble an Err. resolve_conflict must still roll forward to a clean NoOp.
        let o3 = log.remember(&emb, "z=old").unwrap();
        let n3c = log.remember(&emb, "z=new").unwrap();
        let (za, zb) = (ConflictRef::Note { event_id: o3.clone() }, ConflictRef::Note { event_id: n3c.clone() });
        let cross = log.append_conflict_proposal(&za, &zb, "newer", "high", "why", 0, &[o3.clone(), n3c.clone()]).unwrap();
        log.retire_memory(&o3, None).unwrap(); // MANUAL retire of the frozen loser — a DIFFERENT source, no tag
        assert!(log.resolution_markers_for_test(&cross).is_none(), "precondition: proposal not yet resolved");
        let crossed = log.resolve_conflict(&cross, ResolveAction::RetireOlder)
            .expect("must NOT bubble a fail-loud `already retired` Err — the gate is retired-set membership");
        assert!(matches!(crossed, ResolveOutcome::NoOp), "roll-forward on a differently-retired loser = no-op");
        assert_eq!(log.resolution_markers_for_test(&cross).unwrap(), ResolutionKind::RetireOlder, "missing marker appended");
    }

    #[cfg(unix)]
    impl EventLog {
        fn resolution_markers_for_test(&self, prop: &str) -> Option<ResolutionKind> {
            self.resolution_markers().unwrap().get(prop).map(|r| r.kind)
        }
    }

    /// Rung-3 Phase-3 (§2.1, point-3 "frozen loser"): the loser is chosen by the ACTION→FROZEN-REF mapping
    /// (`RetireNewer`=b_ref), NOT by any ts recompute at resolve time. Detection fixes older→a_ref /
    /// newer→b_ref once; resolve time must retire exactly that frozen side. This deterministically pins the
    /// `RetireNewer` retire path (the sibling of the main test's `RetireOlder`) with NO wall-clock/ts
    /// dependence — a ts-flip discriminator would hinge on events landing in distinct seconds (flaky), which
    /// this crate intentionally avoids; the impl is structurally ts-free, so the mapping is what matters.
    /// `#[cfg(unix)]`.
    #[cfg(unix)]
    #[test]
    fn resolve_conflict_retire_newer_retires_frozen_b_ref() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        let older = log.remember(&emb, "port is 8080").unwrap();
        let newer = log.remember(&emb, "port is 9090").unwrap();
        let (a, b) = (ConflictRef::Note { event_id: older.clone() }, ConflictRef::Note { event_id: newer.clone() });
        let prop = log.append_conflict_proposal(&a, &b, "older", "high", "why", 0, &[older.clone(), newer.clone()]).unwrap();

        let out = log.resolve_conflict(&prop, ResolveAction::RetireNewer).unwrap();
        assert!(matches!(out, ResolveOutcome::Applied(_)), "first resolution applies");
        // RetireNewer retires the FROZEN b_ref (newer); the frozen a_ref (older) survives.
        assert!(!log.current_notes().unwrap().iter().any(|c| c.event_id == newer), "b_ref (newer) retired");
        assert!(log.current_notes().unwrap().iter().any(|c| c.event_id == older), "a_ref (older) survives");
        assert_eq!(log.resolution_markers_for_test(&prop).unwrap(), ResolutionKind::RetireNewer);
    }

    /// Rung-3 Phase-2 (§3.5, I9): a proposal for an unordered typed pair suppresses a duplicate for
    /// the SAME pair (either order) but not a different pair — covering both a note↔note pair and a
    /// cross-kind note↔passage pair (whose `pair_key`s have different shapes, so the sort bites).
    /// `#[cfg(unix)]` (uses the append family).
    #[cfg(unix)]
    #[test]
    fn conflict_proposal_idempotency_is_unordered_by_typed_pair() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(8);
        let log = open_log(dir.path());
        // Two CURRENT notes so both refs resolve (open).
        let n1 = log.remember(&emb, "branch is master").unwrap();
        let n2 = log.remember(&emb, "renamed default branch to main").unwrap();
        let a = ConflictRef::Note { event_id: n1.clone() };
        let b = ConflictRef::Note { event_id: n2.clone() };
        let why = crate::conflict::templated_why("newer", "high", "note", "note");
        assert!(!log.is_conflict_proposal_suppressed(&a, &b).unwrap(), "no proposal yet");
        log.append_conflict_proposal(&a, &b, "newer", "high", &why, 1, &[n1.clone(), n2.clone()]).unwrap();
        assert!(log.is_conflict_proposal_suppressed(&a, &b).unwrap(), "same pair suppressed");
        assert!(log.is_conflict_proposal_suppressed(&b, &a).unwrap(), "reversed order also suppressed");
        // A different pair is not suppressed.
        let n3 = log.remember(&emb, "unrelated note").unwrap();
        let c = ConflictRef::Note { event_id: n3 };
        assert!(!log.is_conflict_proposal_suppressed(&a, &c).unwrap(), "different pair not suppressed");

        // Cross-kind: a Note pair_key vs a Passage pair_key differ in shape, so `conflict_pair_key`
        // must still map either order to the same unordered identity. Capture a CURRENT session with
        // one passage so the passage ref resolves (open).
        let cap = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &cap, &["we deploy on vercel".to_string()]).unwrap();
        let p = ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 };
        let why_np = crate::conflict::templated_why("newer", "high", "note", "passage");
        assert!(!log.is_conflict_proposal_suppressed(&a, &p).unwrap(), "no cross-kind proposal yet");
        log.append_conflict_proposal(&a, &p, "newer", "high", &why_np, 2, &[n1.clone(), cap.clone()]).unwrap();
        assert!(log.is_conflict_proposal_suppressed(&a, &p).unwrap(), "cross-kind pair suppressed");
        assert!(log.is_conflict_proposal_suppressed(&p, &a).unwrap(), "cross-kind reversed order also suppressed");
        // A note↔different-passage pair is not suppressed.
        let p_other = ConflictRef::Passage { session_id: "s1".into(), passage_id: 1 };
        assert!(!log.is_conflict_proposal_suppressed(&a, &p_other).unwrap(), "note↔different-passage not suppressed");
    }

    /// Rung-3 Phase-2 (§6.4, I-gc): pending lists an open proposal; retiring / deleting / editing a
    /// referenced memory withdraws it (fold-derived → restart-safe) and frees the pair to re-propose.
    /// `#[cfg(unix)]` (uses the append/projection family).
    #[cfg(unix)]
    #[test]
    fn pending_conflict_proposals_project_and_gc_withdraw() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(8);
        let log = open_log(dir.path());
        let n1 = log.remember(&emb, "branch is master").unwrap();
        let n2 = log.remember(&emb, "renamed default branch to main").unwrap();
        let a = ConflictRef::Note { event_id: n1.clone() };
        let b = ConflictRef::Note { event_id: n2.clone() };
        let why = crate::conflict::templated_why("newer", "high", "note", "note");
        log.append_conflict_proposal(&a, &b, "newer", "high", &why, 1, &[n1.clone(), n2.clone()]).unwrap();
        assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1, "one open proposal");

        // Retire one referenced note → the proposal is GC-withdrawn and the pair is re-proposable.
        log.retire_memory(&n1, None).unwrap();
        assert!(log.pending_conflict_proposals().unwrap().is_empty(), "withdrawn on retire");
        assert!(!log.is_conflict_proposal_suppressed(&a, &b).unwrap(), "pair freed to re-propose");

        // Unretire restores currency → the SAME signed proposal is open again (fold-derived).
        log.unretire(&n1).unwrap();
        assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1, "restored on unretire");

        // Editing (supersede) a referenced note mints a NEW id → old ref no longer current → withdrawn.
        log.supersede_note(&emb, &n2, "default branch is main now").unwrap();
        assert!(log.pending_conflict_proposals().unwrap().is_empty(), "withdrawn on edit (supersede)");
    }

    /// Rung-3 Phase-2 (§6.4, I-gc): a session BODY edit (a same-`session_id` supersede that emits no
    /// `passage_retired`) that re-chunks to FEWER passages withdraws any open proposal referencing an
    /// ordinal the NEW head no longer has — the ordinal-existence half of the passage GC story, proven
    /// at the shared open set so suppression and the projection agree. `#[cfg(unix)]`.
    #[cfg(unix)]
    #[test]
    fn pending_conflict_proposals_withdraws_passage_when_ordinal_vanishes_on_recapture() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(8);
        let log = open_log(dir.path());

        // A current note + a capture of s1 with THREE passages (ordinals 0,1,2).
        let n = log.remember(&emb, "branch is master").unwrap();
        let cap_old = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(
            &emb,
            &cap_old,
            &["p0".to_string(), "p1".to_string(), "p2".to_string()],
        )
        .unwrap();
        let note_ref = ConflictRef::Note { event_id: n.clone() };
        let p0 = ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 };
        let p2 = ConflictRef::Passage { session_id: "s1".into(), passage_id: 2 };
        let why = crate::conflict::templated_why("newer", "high", "note", "passage");
        // TWO proposals: one on the SOON-TO-VANISH ordinal 2, one on the SURVIVING ordinal 0. Both
        // refs currently resolve (ordinals 0 and 2 both exist in the 3-passage head) → both are open.
        log.append_conflict_proposal(&note_ref, &p2, "newer", "high", &why, 1, &[n.clone(), cap_old.clone()])
            .unwrap();
        log.append_conflict_proposal(&note_ref, &p0, "newer", "high", &why, 2, &[n.clone(), cap_old.clone()])
            .unwrap();
        assert_eq!(log.pending_conflict_proposals().unwrap().len(), 2, "ordinals 0 and 2 both exist → both open");

        // Re-capture s1 (different sha ⇒ supersedes the old capture) re-chunked to only TWO passages
        // (ordinals 0,1). Ordinal 2 no longer exists in the current head, yet no `passage_retired`
        // marker was emitted — only the ordinal-existence check can withdraw it.
        let cap_new = log.capture_session(&emb, &session_meta("s1", "bb")).unwrap();
        assert_ne!(cap_old, cap_new, "different-sha recapture supersedes the old capture");
        log.store_session_passages(&emb, &cap_new, &["q0".to_string(), "q1".to_string()]).unwrap();

        // SELECTIVE withdrawal: ONLY the vanished-ordinal proposal drops; the surviving-ordinal one
        // stays open. A whole-session over-withdrawal (e.g. a mis-wired 0 count) would empty BOTH, so
        // asserting the survivor remains is what distinguishes correct from over-aggressive GC.
        assert_eq!(
            log.pending_conflict_proposals().unwrap().len(),
            1,
            "only the ordinal-2 proposal withdrew; the ordinal-0 proposal remains open"
        );
        assert!(
            !log.is_conflict_proposal_suppressed(&note_ref, &p2).unwrap(),
            "the vanished-ordinal pair (ordinal 2) is freed to re-propose"
        );
        assert!(
            log.is_conflict_proposal_suppressed(&note_ref, &p0).unwrap(),
            "the surviving-ordinal pair (ordinal 0) is STILL suppressed — not over-withdrawn"
        );
    }

    /// Rung-3 Phase-3 (§2.2 item 2, I9): the READER complement to Task 7's finder suppression.
    /// KeepBoth/Dismiss retire NOTHING, so both refs stay current and the proposal stays in the OPEN
    /// set — without a filter the pending count / `ListConflicts` would nag every session forever.
    /// `pending_conflict_proposals` drops any open proposal whose `unordered_pair_key` is in the SAME
    /// live `resolution_exclusions()` set the finder consumes (single source → no drift): a KeepBoth'd
    /// note↔note pair AND a Dismissed passage↔note pair both disappear from the read surface, and the
    /// pending count drops with them. `#[cfg(unix)]`.
    #[cfg(unix)]
    #[test]
    fn keep_both_and_dismiss_drop_the_proposal_from_the_read_surface() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);
        let n1 = log.remember(&emb, "x=1").unwrap();
        let n2 = log.remember(&emb, "x=2").unwrap();
        let (a, b) = (ConflictRef::Note { event_id: n1.clone() }, ConflictRef::Note { event_id: n2.clone() });
        let prop = log.append_conflict_proposal(&a, &b, "unclear", "med", "why", 0, &[n1.clone(), n2.clone()]).unwrap();
        assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1, "open before resolution");

        // KeepBoth retires nothing — both refs stay current — but the reader must drop it (I9).
        log.resolve_conflict(&prop, ResolveAction::KeepBoth).unwrap();
        assert!(log.pending_conflict_proposals().unwrap().is_empty(), "kept-both drops from the read surface");

        // A dismissed passage pair drops while live; re-appears if the head advances (covered in Task 3).
        let cev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &cev, &["deploy vercel".to_string()]).unwrap();
        let n3 = log.remember(&emb, "deploy fly").unwrap();
        let pa = ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 };
        let pb = ConflictRef::Note { event_id: n3.clone() };
        let prop2 = log.append_conflict_proposal(&pa, &pb, "unclear", "med", "why", 0, &[cev.clone(), n3.clone()]).unwrap();
        assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1);
        log.resolve_conflict(&prop2, ResolveAction::Dismiss).unwrap();
        assert!(log.pending_conflict_proposals().unwrap().is_empty(), "dismissed drops while the head is unchanged");
    }

    #[test]
    fn remember_appends_external_tainted_recallable_memory() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        // A remembered note is a signed `memory` event stamped external-taint.
        let id = log.remember(&embedder, "ferris the crab loves rust").unwrap();
        let ev = log.event_by_id(&id).unwrap().expect("event present");
        assert_eq!(ev.event_type, "memory", "remember writes a memory-type event");
        assert_eq!(
            ev.content.get("origin").and_then(|v| v.as_str()),
            Some("external"),
            "remembered memories are external-tainted (I2): recallable, never auto-trusted"
        );
        assert_eq!(
            ev.content.get("text").and_then(|v| v.as_str()),
            Some("ferris the crab loves rust"),
            "the note text is stored top-level so the embedder finds it"
        );

        // Recallable immediately: rebuild the indexes, then a recall surfaces it.
        log.rebuild_indexes(&embedder).unwrap();
        let hits = log
            .recall(&embedder, "ferris rust", 5, &RecallOptions::default())
            .unwrap();
        assert!(hits.iter().any(|h| h.event_id == id), "remembered note is recallable");

        // Empty / blank text is rejected (no empty memory events).
        assert!(matches!(
            log.remember(&embedder, "   "),
            Err(BossclawError::InvalidInput(_))
        ));
    }

    #[test]
    fn vector_index_len_is_zero_until_rebuilt_then_reflects_the_recall_index() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        // `open` does not build the ANN index (it is None), so the count is 0.
        assert_eq!(log.vector_index_len(), 0, "no index built yet ⇒ 0");

        // A remembered note plus a rebuild populates the recall index.
        log.remember(&embedder, "ferris the crab loves rust").unwrap();
        log.rebuild_indexes(&embedder).unwrap();
        assert!(
            log.vector_index_len() > 0,
            "after remember + rebuild the recall index holds ≥1 vector"
        );
    }

    /// Rung-3 Phase-1 (§7.1): a capture's body passages embed + persist into the
    /// dedicated `session_passage_vectors` table (the conflict index's restart-safe
    /// source) and survive a reopen, keyed by the capture's event id + passage ix.
    #[test]
    fn store_session_passages_persists_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(8);
        let chunks = vec!["we deploy on Vercel".to_string(), "db is Postgres".to_string()];
        {
            let log = open_log(dir.path());
            log.store_session_passages(&emb, "cap1", &chunks).unwrap();
        }
        let log = open_log(dir.path()); // reopen — the table is restart-durable
        let rows = log.session_passages_for_model(emb.model_id()).unwrap();
        assert_eq!(
            rows.iter().filter(|(e, _, _)| e == "cap1").count(),
            2,
            "both passages persisted under cap1 and survive a reopen"
        );
    }

    /// Rung-3 Phase-2 (§3.2/§3.3): the `(seq, subject_offset)` cursor round-trips + survives reopen; the
    /// enumeration returns each NEW note (within-seq id 0) and each NEW capture's live passages (within-
    /// seq ids = passage_id); and RESUMING at a within-capture offset skips already-judged passages.
    #[test]
    fn conflict_cursor_and_subject_enumeration_are_incremental() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(8);

        let (persisted_seq, persisted_off) = {
            let log = open_log(dir.path());
            assert_eq!(log.conflict_cursor().unwrap(), (0, 0), "unset cursor defaults to (0, 0)");
            let note_id = log.remember(&emb, "branch is main").unwrap();
            let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
            log.store_session_passages(&emb, &ev, &["p0".to_string(), "p1".to_string()]).unwrap();

            // From (0, 0): the note (within-seq id 0) + the capture's two passages (within-seq ids 0, 1).
            let subjects = log.unprocessed_conflict_subjects_since(0, 0, 64).unwrap();
            let refs: Vec<ConflictRef> = subjects.iter().map(|s| s.subject.clone()).collect();
            assert!(refs.contains(&ConflictRef::Note { event_id: note_id.clone() }));
            assert!(refs.contains(&ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 }));
            assert!(refs.contains(&ConflictRef::Passage { session_id: "s1".into(), passage_id: 1 }));

            // Within-capture resume: from the capture's seq at offset 1, passage 0 is skipped (judged),
            // passage 1 still pends — this is the anti-stall resume.
            let cap_seq = subjects
                .iter()
                .find(|s| matches!(s.subject, ConflictRef::Passage { .. }))
                .unwrap()
                .seq;
            let resumed = log.unprocessed_conflict_subjects_since(cap_seq, 1, 64).unwrap();
            assert!(
                !resumed.iter().any(|s| matches!(&s.subject, ConflictRef::Passage { passage_id: 0, .. })),
                "passage 0 of the in-progress capture is skipped at offset 1"
            );
            assert!(
                resumed.iter().any(|s| matches!(&s.subject, ConflictRef::Passage { passage_id: 1, .. })),
                "passage 1 still pending at offset 1"
            );

            // Advancing past the last subject empties the queue.
            let max_seq = subjects.iter().map(|s| s.seq).max().unwrap();
            let last_off =
                subjects.iter().filter(|s| s.seq == max_seq).map(|s| s.within_seq_id).max().unwrap();
            log.set_conflict_cursor(max_seq, last_off + 1).unwrap();
            assert!(log.unprocessed_conflict_subjects_since(max_seq, last_off + 1, 64).unwrap().is_empty());
            (max_seq, last_off + 1)
        };
        // Restart: the cursor is persistent progress state (survives reopen) — assert the EXACT
        // round-trip, so a column swap or wrong-offset persist can't slip through.
        let log = open_log(dir.path());
        let (cseq, coff) = log.conflict_cursor().unwrap();
        assert_eq!(
            (cseq, coff),
            (persisted_seq, persisted_off),
            "cursor round-trips exactly across reopen"
        );
        assert!(
            log.unprocessed_conflict_subjects_since(cseq, coff, 64).unwrap().is_empty(),
            "nothing new after restart"
        );
    }

    /// Rung-3 Phase-3 (§3.2, Open-Q5): un-retiring makes a memory current again, but the conflict
    /// cursor already swept past it. Both `unretire` and `unretire_passage` rewind the 2-D
    /// `(last_seq, subject_offset)` cursor to the lexicographic MIN of its position and the memory's
    /// coordinate — re-scheduling a re-examination — and the rewind is MONOTONE (never advances).
    #[test]
    fn unretire_rewinds_the_conflict_cursor_to_re_examine_the_memory() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        // A note, retired, then the cursor advanced well past it (as detection would).
        let note = log.remember(&emb, "branch is main").unwrap();
        let note_seq = log.seq_of_event(&note).unwrap().expect("note has a seq");
        let m = log.retire_memory(&note, None).unwrap();
        log.set_conflict_cursor(note_seq + 100, 5).unwrap();

        // Unretire rewinds to (note_seq, 0) — the lexicographic min — never advances.
        log.unretire(&note).unwrap();
        assert_eq!(log.conflict_cursor().unwrap(), (note_seq, 0), "cursor rewound to the un-retired note");

        // A rewind never ADVANCES: unretire when the cursor is already behind the note is a no-op on the cursor.
        let _ = m; // (marker id unused)
        log.set_conflict_cursor(0, 0).unwrap();
        // re-retire + unretire again; cursor stays at/below the note position (min semantics).
        log.retire_memory(&note, None).unwrap();
        log.unretire(&note).unwrap();
        assert_eq!(log.conflict_cursor().unwrap(), (0, 0), "rewind is monotone: never advances past current");

        // Passage rewind: unretire_passage rewinds to (capture_seq, passage_id).
        let cev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &cev, &["p0".to_string(), "p1".to_string()]).unwrap();
        let cap_seq = log.seq_of_event(&cev).unwrap().unwrap();
        log.retire_passage("s1", 1, None).unwrap();
        log.set_conflict_cursor(cap_seq + 50, 0).unwrap();
        log.unretire_passage("s1", 1).unwrap();
        assert_eq!(log.conflict_cursor().unwrap(), (cap_seq, 1), "passage unretire rewinds to (capture_seq, passage_id)");
    }

    /// Rung-3 Phase-2 (§3.3): the enumeration's exclusion GATES — the whole point of the fold — are
    /// each a live subject that must NOT appear. Pins all three negative paths (a positive-only test
    /// would still pass if a gate were deleted): a superseded note's old head is dropped (its live
    /// replacement kept); a `note_retired` note is dropped; a retired passage is dropped (its sibling
    /// kept); and a supersede-replaced capture contributes ZERO passage subjects (only the current
    /// head for the `session_id` does).
    #[test]
    fn conflict_subject_enumeration_respects_current_head_and_retire_gates() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        // (1) A note that gets SUPERSEDED (edited) — old head must drop, replacement must stay.
        let note_old = log.remember(&emb, "the branch is main").unwrap();
        let note_new = log.supersede_note(&emb, &note_old, "the branch is trunk").unwrap();
        // (2) A note that gets RETIRED via the distinct note_retired marker — must drop.
        let note_retired = log.remember(&emb, "db is postgres").unwrap();
        log.retire_memory(&note_retired, None).unwrap();
        // (3) A capture whose passage 0 is RETIRED — passage 0 drops, passage 1 stays.
        let cap_s2 = log.capture_session(&emb, &session_meta("s2", "cc")).unwrap();
        log.store_session_passages(&emb, &cap_s2, &["p0".to_string(), "p1".to_string()]).unwrap();
        log.retire_passage("s2", 0, None).unwrap();
        // (4) A capture SUPERSEDED by a newer capture of the SAME session_id (different sha) — the
        // OLD capture event must contribute zero passage subjects; only the new head's do.
        let cap_s1_old = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &cap_s1_old, &["a0".to_string(), "a1".to_string()]).unwrap();
        let cap_s1_new = log.capture_session(&emb, &session_meta("s1", "bb")).unwrap();
        log.store_session_passages(&emb, &cap_s1_new, &["b0".to_string(), "b1".to_string()]).unwrap();
        assert_ne!(cap_s1_old, cap_s1_new, "the different-sha recapture supersedes the old capture");

        let subjects = log.unprocessed_conflict_subjects_since(0, 0, 64).unwrap();
        let has = |r: ConflictRef| subjects.iter().any(|s| s.subject == r);

        // (1) supersede: old head excluded, replacement present.
        assert!(!has(ConflictRef::Note { event_id: note_old.clone() }), "superseded note's old head is excluded");
        assert!(has(ConflictRef::Note { event_id: note_new.clone() }), "the live replacement note IS enumerated");
        // (2) note_retired: excluded.
        assert!(!has(ConflictRef::Note { event_id: note_retired.clone() }), "a note_retired note is excluded");
        // (3) passage retire: passage 0 excluded, sibling present.
        assert!(!has(ConflictRef::Passage { session_id: "s2".into(), passage_id: 0 }), "retired passage 0 of s2 is excluded");
        assert!(has(ConflictRef::Passage { session_id: "s2".into(), passage_id: 1 }), "non-retired sibling passage 1 of s2 is enumerated");
        // (4) capture supersede: only the current head contributes — exactly 2 s1 passage subjects,
        // not 4 (a broken current-head gate would double them from the superseded old capture too).
        let s1_passages = subjects
            .iter()
            .filter(|s| matches!(&s.subject, ConflictRef::Passage { session_id, .. } if session_id == "s1"))
            .count();
        assert_eq!(s1_passages, 2, "only the current-head capture of s1 contributes its 2 passages (the superseded old capture contributes none)");
        assert!(has(ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 }));
        assert!(has(ConflictRef::Passage { session_id: "s1".into(), passage_id: 1 }));
    }

    /// Rung-3 Phase-1 (§7.1): the SEPARATE conflict index retrieves a captured
    /// session's passages keyed by the fold-resolved `session_id` (not the raw
    /// capture event id), and building it leaves the recall `vector_index`
    /// byte-untouched (its `len` is the guard). `k ≥ #passages` ⇒ set-membership is
    /// stable across HNSW rank non-determinism (spec §13).
    #[test]
    fn conflict_index_retrieves_by_session_and_leaves_recall_len_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);
        let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap(); // real capture → fold head "s1"
        let chunks = vec!["we deploy on Vercel".to_string(), "db is Postgres".to_string()];
        log.store_session_passages(&emb, &ev, &chunks).unwrap();
        log.rebuild_indexes(&emb).unwrap();
        let recall_len = log.vector_index_len();
        assert_eq!(recall_len, 1, "one capture event is in the recall index");
        log.rebuild_conflict_index(&emb).unwrap();
        let hits = log.conflict_search(&emb.embed(&["Vercel".into()]).unwrap()[0], 8); // k ≥ #chunks → membership stable
        assert!(hits.iter().any(|(sid, pid, _)| sid == "s1" && *pid == 0)); // session id resolved via fold head
        assert_eq!(log.vector_index_len(), recall_len, "recall vector_index byte-untouched");
    }

    /// Rung-3 Phase-2 (§2): the conflict index holds BOTH a note body and a session passage; the
    /// typed search returns each as its `ConflictRef` kind; the recall `vector_index` stays untouched.
    #[test]
    fn conflict_index_note_arm_and_passage_arm_are_both_typed_searchable() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        // A note whose body is embeddable.
        let note_id = log.remember(&emb, "the default git branch is main").unwrap();
        // A captured session with one passage.
        let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &ev, &["we deploy on vercel".to_string()]).unwrap();

        log.rebuild_indexes(&emb).unwrap();
        let recall_len = log.vector_index_len();
        log.rebuild_conflict_index(&emb).unwrap();

        // The note is retrievable as a Note ref.
        let note_hits =
            log.conflict_search_refs(&emb.embed(&["default git branch".into()]).unwrap()[0], 8);
        assert!(
            note_hits.iter().any(|(r, _)| *r == ConflictRef::Note { event_id: note_id.clone() }),
            "note body is a typed Note hit"
        );
        // The passage is retrievable as a Passage ref.
        let pass_hits = log.conflict_search_refs(&emb.embed(&["vercel".into()]).unwrap()[0], 8);
        assert!(
            pass_hits
                .iter()
                .any(|(r, _)| *r == ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 }),
            "passage is a typed Passage hit"
        );
        // The recall index was not perturbed by adding the note arm.
        assert_eq!(log.vector_index_len(), recall_len, "recall vector_index byte-untouched");

        // The legacy passage-tuple search still works (memharness contract).
        let legacy = log.conflict_search(&emb.embed(&["vercel".into()]).unwrap()[0], 8);
        assert!(legacy.iter().any(|(sid, pid, _)| sid == "s1" && *pid == 0));
    }

    /// Rung-3 Phase-2 (§2): the note arm inherits `current_notes`' supersede exclusion — a
    /// SUPERSEDED note's old head must NOT enter the fights index (a phantom-conflict false
    /// positive is the worst detector failure), while its live replacement MUST. Pins the
    /// exclusion AT the conflict-index level, not just at recall.
    #[test]
    fn superseded_note_is_excluded_from_conflict_index_but_replacement_is_present() {
        use crate::index::ConflictRef;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        let old = log.remember(&emb, "the default git branch is master").unwrap();
        let replacement = log.supersede_note(&emb, &old, "the default git branch is main").unwrap();
        log.rebuild_conflict_index(&emb).unwrap();

        let hits = log.conflict_search_refs(&emb.embed(&["default git branch".into()]).unwrap()[0], 8);
        assert!(
            !hits.iter().any(|(r, _)| *r == ConflictRef::Note { event_id: old.clone() }),
            "superseded note head is NOT in the fights index"
        );
        assert!(
            hits.iter().any(|(r, _)| *r == ConflictRef::Note { event_id: replacement.clone() }),
            "the live replacement note IS in the fights index"
        );
    }

    /// Rung-3 Phase-1 (§7.1): a BUILT-but-EMPTY conflict index yields no hits. This
    /// pins the empty-result contract via the legitimate "rebuild first" path (a
    /// fresh log has no passages, so the rebuild produces an empty index) — so it
    /// respects `conflict_search`'s dev/test `debug_assert` that the index was built,
    /// rather than probing the unbuilt-`None` path (which is a caller bug the assert
    /// deliberately trips).
    #[test]
    fn conflict_search_on_empty_built_index_yields_no_hits() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);
        log.rebuild_conflict_index(&emb).unwrap(); // no passages ⇒ empty index (but built)
        let hits = log.conflict_search(&emb.embed(&["x".into()]).unwrap()[0], 8);
        assert!(hits.is_empty(), "an empty (built) conflict index yields no hits");
    }

    /// Rung-3 §7.2/§7.3: the passage-retire ACTION — retiring one passage hides
    /// exactly that passage from `conflict_search`, siblings stay, the retire
    /// survives a sweeper cycle (same-sha re-capture is an A2 dedup no-op that
    /// keeps the marker), and `unretire_passage` reverses it.
    #[test]
    fn passage_retire_hides_one_survives_sweep_and_reverses() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);
        let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        let chunks = vec!["Vercel".to_string(), "Postgres".to_string()];
        log.store_session_passages(&emb, &ev, &chunks).unwrap();
        log.rebuild_conflict_index(&emb).unwrap();
        log.retire_passage("s1", 0, None).unwrap();
        log.rebuild_conflict_index(&emb).unwrap();
        let hit = |q: &str| {
            log.conflict_search(&emb.embed(&[q.into()]).unwrap()[0], 8)
                .iter()
                .any(|(s, p, _)| s == "s1" && *p == 0)
        };
        assert!(!hit("Vercel"), "retired passage 0 hidden");
        assert!(
            log.conflict_search(&emb.embed(&["Postgres".into()]).unwrap()[0], 8)
                .iter()
                .any(|(s, p, _)| s == "s1" && *p == 1),
            "sibling kept"
        );
        // Reject branches, asserted WHILE passage 0 is still retired (all three are
        // read-only rejects that append nothing, so they never disturb the flow below).
        assert!(
            matches!(log.retire_passage("s1", 0, None), Err(BossclawError::InvalidInput(_))),
            "double-retire of a still-retired passage is rejected (I6)"
        );
        assert!(
            matches!(log.retire_passage("s1", 9, None), Err(BossclawError::InvalidInput(_))),
            "out-of-range passage_id (≥ N=2) is rejected"
        );
        assert!(
            matches!(log.retire_passage("ghost", 0, None), Err(BossclawError::InvalidInput(_))),
            "retire against a non-current session is rejected"
        );
        // SWEEP durability: same-sha re-capture is a no-op (returns the same id); marker persists across rebuild.
        log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.rebuild_conflict_index(&emb).unwrap();
        assert!(!hit("Vercel"), "retire survives a sweeper cycle");
        log.unretire_passage("s1", 0).unwrap();
        log.rebuild_conflict_index(&emb).unwrap();
        assert!(hit("Vercel"), "unretire restores the passage");
        assert!(
            matches!(log.unretire_passage("s1", 0), Err(BossclawError::InvalidInput(_))),
            "unretiring a no-longer-retired passage is rejected (I6)"
        );
    }

    /// Rung-3 §2.1 (MAJOR-2) / §3.4: the optional `source_proposal_id` provenance stamp. When
    /// `Some`, the SAME `note_retired`/`passage_retired` marker gains `{"via":"conflict",
    /// "proposal_id":id}` (additive, so the retire fold — keyed on `retires` / `session_id`+
    /// `passage_id`, no `deny_unknown_fields` — is UNTOUCHED and still retires). When `None`
    /// (the manual App path) the marker is byte-identical to today, carrying NO provenance tag.
    #[test]
    fn retire_stamps_conflict_provenance_but_fold_and_app_path_are_untouched() {
        use crate::graph::{NOTE_RETIRED_EVENT_TYPE, PASSAGE_RETIRED_EVENT_TYPE};
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        // (a) App path (None) — the marker is byte-identical to today: {"retires": id}, no `via`.
        let n_app = log.remember(&emb, "app-retired note").unwrap();
        let m_app = log.retire_memory(&n_app, None).unwrap();
        let ev = log.event_by_id(&m_app).unwrap().unwrap();
        assert_eq!(ev.event_type, NOTE_RETIRED_EVENT_TYPE);
        assert_eq!(ev.content.get("retires").and_then(|v| v.as_str()), Some(n_app.as_str()));
        assert!(ev.content.get("via").is_none(), "App retire carries NO provenance tag");

        // (b) Conflict path (Some) — same marker TYPE, plus the provenance tag; the fold still retires it.
        let n_conf = log.remember(&emb, "conflict-retired note").unwrap();
        let m_conf = log.retire_memory(&n_conf, Some("PROP1")).unwrap();
        let ev = log.event_by_id(&m_conf).unwrap().unwrap();
        assert_eq!(ev.event_type, NOTE_RETIRED_EVENT_TYPE, "SAME marker type as the App path");
        assert_eq!(ev.content.get("retires").and_then(|v| v.as_str()), Some(n_conf.as_str()));
        assert_eq!(ev.content.get("via").and_then(|v| v.as_str()), Some("conflict"));
        assert_eq!(ev.content.get("proposal_id").and_then(|v| v.as_str()), Some("PROP1"));
        // The retire fold is untouched: the note is no longer current (recall/list drop it).
        assert!(!log.current_notes().unwrap().iter().any(|c| c.event_id == n_conf), "tagged retire still retires");

        // (c) Passage retire carries the tag too, same shape.
        let cev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &cev, &["p0".to_string(), "p1".to_string()]).unwrap();
        let pm = log.retire_passage("s1", 0, Some("PROP2")).unwrap();
        let ev = log.event_by_id(&pm).unwrap().unwrap();
        assert_eq!(ev.event_type, PASSAGE_RETIRED_EVENT_TYPE);
        assert_eq!(ev.content.get("session_id").and_then(|v| v.as_str()), Some("s1"));
        assert_eq!(ev.content.get("passage_id").and_then(|v| v.as_u64()), Some(0));
        assert_eq!(ev.content.get("via").and_then(|v| v.as_str()), Some("conflict"));
        assert_eq!(ev.content.get("proposal_id").and_then(|v| v.as_str()), Some("PROP2"));
    }

    /// Rung-3 §9/§13 recall-NEUTRALITY (by construction + measured): building the SEPARATE
    /// conflict index and retiring a body passage write ONLY `conflict_index` / append a
    /// `passage_retired` marker — they NEVER touch the recall `vector_index`. So a note recall
    /// over a fixed corpus is BYTE-IDENTICAL before and after the whole conflict-side sequence,
    /// and the recall index's element count is unchanged. This is the core (by-construction) half
    /// of the harness recall-neutrality proof; the `memharness` `compare`/`recall_regressed` guard
    /// is the frozen-corpus statistical half. The recall index is built ONCE and never rebuilt
    /// between the two recalls, so it is the SAME instance — the assertion is a true byte-identity,
    /// not a re-embedding round-trip (no HNSW rank non-determinism is in play).
    #[test]
    fn conflict_index_and_passage_retire_leave_note_recall_identical() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        // A small fixed note corpus + one built recall index.
        for text in [
            "ferris the crab loves rust",
            "postgres is the datastore",
            "we deploy on vercel",
        ] {
            log.remember(&emb, text).unwrap();
        }
        log.rebuild_indexes(&emb).unwrap();

        // Baseline: the ORDERED note-recall hit-id sequence for a fixed query + the recall index's
        // element count. The query overlaps only the notes, never the session title captured below.
        let recall_ids = |q: &str| -> Vec<String> {
            log.recall(&emb, q, 5, &RecallOptions::default())
                .unwrap()
                .into_iter()
                .map(|h| h.event_id)
                .collect()
        };
        let baseline_ids = recall_ids("datastore postgres");
        let baseline_len = log.vector_index_len();
        assert!(!baseline_ids.is_empty(), "the query recalls ≥1 note (non-vacuous baseline)");

        // The ENTIRE conflict-side sequence: capture a session, persist its body passages, build
        // the conflict index, retire a passage, rebuild the conflict index to reflect the retire.
        // NONE of these calls rebuild the recall index, so the recall `vector_index` box is the
        // SAME instance the baseline searched.
        let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        let chunks = vec!["we deploy on vercel".to_string(), "db is postgres".to_string()];
        log.store_session_passages(&emb, &ev, &chunks).unwrap();
        log.rebuild_conflict_index(&emb).unwrap();
        log.retire_passage("s1", 0, None).unwrap();
        log.rebuild_conflict_index(&emb).unwrap();

        // Byte-identity: same ordered hit ids, same recall-index element count. The conflict index
        // build + passage retire provably could not perturb note recall.
        assert_eq!(
            recall_ids("datastore postgres"),
            baseline_ids,
            "note recall hit sequence is byte-identical after the conflict-side sequence"
        );
        assert_eq!(
            log.vector_index_len(),
            baseline_len,
            "recall vector_index element count unchanged"
        );
    }

    /// A `SessionMeta` for `session_id` at content-hash `sha`; all other fields
    /// fixed so a test varies only the two axes the fold decisions turn on.
    fn session_meta(session_id: &str, sha: &str) -> SessionMeta {
        SessionMeta {
            session_id: session_id.into(),
            title: "fix the parser".into(),
            project: "/repo".into(),
            tool: "claude-code".into(),
            started_at: 1,
            ended_at: 2,
            path: format!("/data/sessions/{session_id}.md"),
            sha256: sha.into(),
            approx_bytes: 10,
        }
    }

    /// Like [`session_meta`] but with a caller-chosen `title`, so a recall test
    /// can key on a distinctive title keyword (the title is what becomes the
    /// event's embeddable + FTS-indexed text).
    fn session_meta_titled(session_id: &str, sha: &str, title: &str) -> SessionMeta {
        SessionMeta { title: title.into(), ..session_meta(session_id, sha) }
    }

    #[test]
    fn capture_session_appends_embeddable_external_event_and_fold_sees_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);
        let id = log
            .capture_session(
                &embedder,
                &SessionMeta {
                    session_id: "abc-123".into(),
                    title: "fix the parser".into(),
                    project: "/repo".into(),
                    tool: "claude-code".into(),
                    started_at: 1,
                    ended_at: 2,
                    path: "/data/sessions/abc-123.md".into(),
                    sha256: "aa".repeat(32),
                    approx_bytes: 10,
                },
            )
            .unwrap();
        let cur = log.current_sessions().unwrap();
        assert_eq!(cur.len(), 1);
        assert_eq!(cur[0].session_id, "abc-123");
        assert_eq!(cur[0].event_id, id);
    }

    #[test]
    fn delete_session_tombstones_in_fold() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        log.capture_session(&embedder, &session_meta("abc", "aa")).unwrap();
        assert_eq!(log.current_sessions().unwrap().len(), 1);

        log.delete_session("abc").unwrap();
        assert!(log.current_sessions().unwrap().is_empty(), "deleted session is gone from the fold");

        // Deleting an id with no current session is rejected.
        assert!(matches!(
            log.delete_session("never-existed"),
            Err(BossclawError::InvalidInput(_))
        ));
    }

    #[test]
    fn note_retire_folds_reversibly_and_leaves_edit_supersedes_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        // A real note + a real edit-supersede of it (existing public path).
        let n = log.remember(&emb, "uses Vercel").unwrap();
        let edited = log.supersede_note(&emb, &n, "left Vercel").unwrap(); // n now in fold.superseded

        // Retire the (edited) head via a DISTINCT note_retired marker, appended
        // inline exactly like `delete_session` builds its tombstone.
        log.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::NOTE_RETIRED_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "retires": edited }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: log.signer_did(),
            signature: None,
        })
        .unwrap();

        let fold = fold_sessions(&log.session_events_ordered().unwrap());
        assert!(fold.retired_notes.contains(&edited));
        assert!(fold.superseded.contains(&n), "the ORIGINAL edit-supersede is untouched");

        // Unretire removes ONLY from retired_notes, never from superseded.
        log.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::UNRETIRE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "unretires": edited }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: log.signer_did(),
            signature: None,
        })
        .unwrap();

        let fold = fold_sessions(&log.session_events_ordered().unwrap());
        assert!(!fold.retired_notes.contains(&edited));
        assert!(fold.superseded.contains(&n), "unretire did NOT reverse the edit");
    }

    #[test]
    fn passage_retire_folds_reversibly_by_session_and_passage_id_leaving_supersede_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);

        // A live edit-supersede so `superseded` is non-empty — the passage cycle
        // below must never disturb it (the disjointness invariant holds for
        // passages too, not just notes).
        let n = log.remember(&emb, "uses Vercel").unwrap();
        log.supersede_note(&emb, &n, "left Vercel").unwrap(); // n now in fold.superseded

        // A passage is keyed on (session_id, passage_id) — passage_id is a chunk
        // ordinal stored as a JSON number, so the fold must read it back as usize.
        let sess = "sess-abc";
        let passage: usize = 3;
        let key = (sess.to_string(), passage);

        // Retire the passage via a DISTINCT passage_retired marker, appended
        // inline exactly like `delete_session` builds its tombstone.
        log.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::PASSAGE_RETIRED_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "session_id": sess, "passage_id": passage }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: log.signer_did(),
            signature: None,
        })
        .unwrap();

        let fold = fold_sessions(&log.session_events_ordered().unwrap());
        assert!(fold.retired_passages.contains(&key));
        assert!(fold.superseded.contains(&n), "the edit-supersede is untouched by a passage retire");

        // Unretire (passage form) removes ONLY from retired_passages.
        log.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::UNRETIRE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "session_id": sess, "passage_id": passage }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: log.signer_did(),
            signature: None,
        })
        .unwrap();

        let fold = fold_sessions(&log.session_events_ordered().unwrap());
        assert!(!fold.retired_passages.contains(&key));
        assert!(fold.superseded.contains(&n), "unretire of a passage did NOT reverse the edit");
    }

    #[test]
    fn recapture_same_sha_dedups_and_new_sha_supersedes() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        let id1 = log.capture_session(&embedder, &session_meta("abc", "aa")).unwrap();
        assert_eq!(log.current_sessions().unwrap().len(), 1);

        // Same id + same sha → dedup no-op, still one current, same event id.
        let id_again = log.capture_session(&embedder, &session_meta("abc", "aa")).unwrap();
        let cur = log.current_sessions().unwrap();
        assert_eq!(cur.len(), 1);
        assert_eq!(id_again, id1, "same-sha recapture returns the existing event id (no-op)");
        assert_eq!(cur[0].event_id, id1);

        // Same id + different sha → supersede: one current, but the event id changed.
        let id2 = log.capture_session(&embedder, &session_meta("abc", "bb")).unwrap();
        let cur = log.current_sessions().unwrap();
        assert_eq!(cur.len(), 1);
        assert_ne!(id2, id1, "changed-sha recapture appends a supersede pair (new event id)");
        assert_eq!(cur[0].event_id, id2);
        assert_eq!(cur[0].sha256, "bb");
    }

    #[test]
    fn deleted_session_is_not_recapturable() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        log.capture_session(&embedder, &session_meta("abc", "aa")).unwrap();
        log.delete_session("abc").unwrap();

        // I9: a tombstoned session can never be recaptured, whatever the sha.
        assert!(matches!(
            log.capture_session(&embedder, &session_meta("abc", "cc")),
            Err(BossclawError::InvalidInput(_))
        ));
        assert!(log.current_sessions().unwrap().is_empty());
    }

    #[test]
    fn capture_session_content_is_external_tainted_and_embeddable() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        let id = log.capture_session(&embedder, &session_meta("abc", "aa")).unwrap();
        let ev = log.event_by_id(&id).unwrap().expect("event present");

        // External-tainted: the taint model keys on content["origin"] exactly.
        assert_eq!(
            ev.content.get("origin").and_then(|v| v.as_str()),
            Some("external"),
            "captured sessions are external-tainted (recallable, never auto-trusted)"
        );
        let text = ev.content.get("text").and_then(|v| v.as_str()).unwrap_or_default();
        assert!(!text.is_empty(), "embeddable text is non-empty");
        assert!(text.contains("fix the parser"), "embeddable text carries the session title");

        // Embeddable: a vector was derived under the embedder's model.
        let vecs = log.vectors_for_model(embedder.model_id()).unwrap();
        assert!(vecs.iter().any(|(vid, _)| vid == &id), "a vector exists for the captured session");
    }

    #[test]
    fn supersede_note_rejects_non_note_targets_and_blank_text() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        let note = log.remember(&embedder, "original").unwrap();

        // Blank replacement text is rejected (same check as `remember`).
        assert!(matches!(
            log.supersede_note(&embedder, &note, "  "),
            Err(BossclawError::InvalidInput(_))
        ));

        // Only memory-kind events can be superseded: a captured session is rejected.
        log.capture_session(&embedder, &session_meta("abc", "aa")).unwrap();
        let sess = log.current_sessions().unwrap()[0].event_id.clone();
        assert!(matches!(
            log.supersede_note(&embedder, &sess, "nope"),
            Err(BossclawError::InvalidInput(_))
        ));

        // Unknown target id is rejected.
        assert!(matches!(
            log.supersede_note(&embedder, "no-such-id", "x"),
            Err(BossclawError::InvalidInput(_))
        ));

        // Superseding a current note succeeds and yields a NEW event id.
        let newer = log.supersede_note(&embedder, &note, "corrected").unwrap();
        assert_ne!(newer, note);

        // Superseding an ALREADY-superseded note is rejected (chain heads only).
        assert!(matches!(
            log.supersede_note(&embedder, &note, "again"),
            Err(BossclawError::InvalidInput(_))
        ));
    }

    // ── SP3 A3: durable recall exclusion for deleted sessions + superseded notes ──

    #[test]
    fn deleted_session_absent_from_recall_even_by_keyword() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        // A distinctive title so the FTS/keyword arm is what surfaces the session.
        log.capture_session(&embedder, &session_meta_titled("abc", "aa", "quixotic zanzibar refactor"))
            .unwrap();
        log.rebuild_indexes(&embedder).unwrap();

        let hits = log
            .recall(&embedder, "quixotic zanzibar", 10, &RecallOptions::default())
            .unwrap();
        assert!(!hits.is_empty(), "sanity: title recallable before delete");

        log.delete_session("abc").unwrap();
        log.rebuild_indexes(&embedder).unwrap();

        let hits = log
            .recall(&embedder, "quixotic zanzibar", 10, &RecallOptions::default())
            .unwrap();
        assert!(hits.is_empty(), "keyword arm must also exclude a deleted session (critic M1)");
    }

    #[test]
    fn superseded_note_excluded_but_replacement_recallable() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        let old = log.remember(&embedder, "the API key lives in vault slot seven").unwrap();
        log.supersede_note(&embedder, &old, "the API key lives in vault slot nine").unwrap();
        log.rebuild_indexes(&embedder).unwrap();

        let hits = log
            .recall(&embedder, "vault slot", 10, &RecallOptions::default())
            .unwrap();
        assert!(hits.iter().all(|h| h.event_id != old), "old (superseded) head excluded");
        assert!(!hits.is_empty(), "the replacement note surfaces");
    }

    #[test]
    fn retire_memory_note_excludes_from_recall_and_list_and_unretire_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(8);
        let ev = log.remember(&emb, "we deploy on Vercel").unwrap();
        log.rebuild_indexes(&emb).unwrap();
        assert!(log.recall(&emb, "Vercel", 10, &RecallOptions::default()).unwrap().iter().any(|h| h.event_id == ev));
        log.retire_memory(&ev, None).unwrap();
        assert!(!log.recall(&emb, "Vercel", 10, &RecallOptions::default()).unwrap().iter().any(|h| h.event_id == ev), "retired note excluded from recall");
        assert!(!log.current_notes().unwrap().iter().any(|n| n.event_id == ev), "retired note excluded from the Library list");
        // (a) retiring an already-retired note rejects (guard path).
        assert!(matches!(log.retire_memory(&ev, None), Err(BossclawError::InvalidInput(_))), "cannot retire an already-retired note");
        assert!(matches!(log.unretire("not-a-retired-id"), Err(BossclawError::InvalidInput(_))), "unretire refuses a non-retired id");
        log.unretire(&ev).unwrap();
        assert!(log.recall(&emb, "Vercel", 10, &RecallOptions::default()).unwrap().iter().any(|h| h.event_id == ev), "unretire restores recall");
        // (c) the Library list restores too — a SEPARATE path (`fold_notes` builds its own retired set).
        assert!(log.current_notes().unwrap().iter().any(|n| n.event_id == ev), "unretire restores the Library list");
        // (b) double unretire rejects: it is no longer in `retired_notes`.
        assert!(matches!(log.unretire(&ev), Err(BossclawError::InvalidInput(_))), "cannot unretire a note that is not retired");
        assert!(matches!(log.retire_memory("nope", None), Err(BossclawError::InvalidInput(_))));
    }

    #[test]
    fn deleted_session_stays_deleted_after_reopen() {
        // The resurrection test (architect Critical #1): rebuild_indexes runs on
        // reopen and re-adds the deleted session's PERSISTED vector from the
        // `vectors` table (VectorIndex::remove is in-memory only), so the ONLY
        // durable exclusion is the fold-derived filter recomputed inside recall.
        let dir = tempfile::tempdir().unwrap();
        {
            let log = open_log(dir.path());
            let embedder = MockEmbedder::new(8);
            log.capture_session(
                &embedder,
                &session_meta_titled("abc", "aa", "quixotic zanzibar refactor"),
            )
            .unwrap();
            log.rebuild_indexes(&embedder).unwrap();
            let hits = log
                .recall(&embedder, "quixotic zanzibar", 10, &RecallOptions::default())
                .unwrap();
            assert!(!hits.is_empty(), "sanity: recallable before delete");
            log.delete_session("abc").unwrap();
        } // drop the log — nothing in-memory survives

        // Reopen from the same path and rebuild indexes (as a normal open does).
        let log2 = open_log(dir.path());
        let embedder = MockEmbedder::new(8);
        log2.rebuild_indexes(&embedder).unwrap();

        let hits = log2
            .recall(&embedder, "quixotic zanzibar", 10, &RecallOptions::default())
            .unwrap();
        assert!(hits.is_empty(), "deleted session stays deleted after reopen (rebuild-proof)");
        assert!(log2.current_sessions().unwrap().is_empty(), "and is gone from the session fold");
    }

    #[test]
    fn session_events_never_enter_extraction_queue() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        let sess_id = log.capture_session(&embedder, &session_meta("abc", "aa")).unwrap();
        let note_id = log.remember(&embedder, "a genuinely extractable note").unwrap();

        // The evolve loop's extraction batch: only user notes + ingested files.
        let batch = log.unprocessed_extractable_since(0, 100).unwrap();
        assert!(
            batch.iter().any(|(_, id, _)| id == &note_id),
            "notes ARE extraction subjects"
        );
        assert!(
            !batch.iter().any(|(_, id, _)| id == &sess_id),
            "a captured session must never enter the extraction queue"
        );
    }

    #[test]
    fn deleted_session_not_re_embedded_by_collect_pending() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let embedder = MockEmbedder::new(8);

        let sess_id = log.capture_session(&embedder, &session_meta("abc", "aa")).unwrap();
        log.delete_session("abc").unwrap();

        // Force the pending path exactly as a language migration does: a NEW model
        // id has no vectors, so every embeddable event is "pending" to re-embed.
        let pending = log.collect_pending("other-model-v1").unwrap();
        assert!(
            pending.iter().all(|e| e.id != sess_id),
            "a deleted session must never re-vectorize (rebuild/migration embed gate)"
        );
    }

    #[test]
    fn set_active_model_writes_discoverable_config() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path()); // the existing test helper in this module (log.rs:6542)
        assert!(log.active_model().unwrap().is_none());

        log.set_active_model("minishlab/potion-base-8M", 256).unwrap();

        let m = log.active_model().unwrap().expect("config now present");
        assert_eq!(m.active_model_id, "minishlab/potion-base-8M");
        assert_eq!(m.dim, 256);
        assert_eq!(m.schema_version, SCHEMA_VERSION);

        // Idempotent re-set with the same model keeps it discoverable.
        log.set_active_model("minishlab/potion-base-8M", 256).unwrap();
        assert_eq!(log.active_model().unwrap().unwrap().active_model_id, "minishlab/potion-base-8M");
    }

    /// Task 7 (spec R1): the cloud-reasoner CONFIG and the cloud-enable CONSENT
    /// are SIGNED `config` events in the tamper-evident log (cloning the evolve
    /// off-switch mechanism), NOT a webview-writable file. Both readers
    /// default-CLOSED (`None`) when never set, the newest write is sticky, and
    /// the whole chain still verifies (every event is Ed25519-signed).
    #[test]
    fn reasoner_config_and_consent_roundtrip_signed() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path()); // same signed-EventLog helper the config tests use

        // Defaults: absent -> None (fail-closed).
        assert!(log.reasoner_config_json().unwrap().is_none());
        assert!(log.cloud_reasoner_consent_json().unwrap().is_none());

        // Write non-security config.
        let cfg = serde_json::json!({
            "mode": "cloud", "provider": "anthropic",
            "model": "claude-sonnet-4-6", "base_url": null
        });
        log.set_reasoner_config(cfg.clone()).unwrap();
        assert_eq!(log.reasoner_config_json().unwrap().unwrap(), cfg);

        // Write the signed consent record.
        let consent = serde_json::json!({
            "provider": "anthropic", "base_url_host": "api.anthropic.com",
            "key_fingerprint": "deadbeef", "consented_at": "2026-06-30T00:00:00Z"
        });
        log.set_cloud_reasoner_consent(consent.clone()).unwrap();
        assert_eq!(log.cloud_reasoner_consent_json().unwrap().unwrap(), consent);

        // Newest write wins (sticky) and the whole chain still verifies (signed).
        log.verify_chain().unwrap();
    }

    /// SP3 §6a (critic Critical C1): both capture flags are default-CLOSED at the engine — a fresh
    /// log has capture OFF, backfill un-consented, and `CaptureEnabled` never explicitly set, so a
    /// user who never connected has capture no-op with zero files (I10).
    #[test]
    fn capture_flags_default_off() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        assert!(!log.capture_enabled().unwrap(), "capture default OFF");
        assert!(!log.backfill_consented().unwrap(), "backfill default OFF");
        assert!(
            !log.explicitly_set(ConfigFlag::CaptureEnabled).unwrap(),
            "never-set means explicitly_set is false (the boot cascade keys off this)"
        );
        assert_eq!(log.capture_enabled_at().unwrap(), None, "no ON transition yet ⇒ no timestamp");
    }

    /// Rung-3 Phase-2 (§3.6, I3): conflict-detect is DEFAULT-CLOSED, is sticky once set, and
    /// registers as explicitly-set (what the boot force-off keys off).
    #[test]
    fn conflict_detect_flag_is_default_closed_and_sticky() {
        use crate::ConfigFlag;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        assert!(!log.conflict_detect_enabled().unwrap(), "default CLOSED");
        assert!(!log.explicitly_set(ConfigFlag::ConflictDetect).unwrap(), "never set yet");
        log.set_conflict_detect_enabled(true).unwrap();
        assert!(log.conflict_detect_enabled().unwrap(), "sticky ON after set");
        assert!(log.explicitly_set(ConfigFlag::ConflictDetect).unwrap(), "now explicit");
        log.set_conflict_detect_enabled(false).unwrap();
        assert!(!log.conflict_detect_enabled().unwrap(), "sticky OFF");
    }

    /// SP3 §6a: the Integrations-toggle path — enable ongoing capture WITHOUT backfill. Records the
    /// `capture_enabled_at` timestamp, marks the flag explicitly set, and leaves backfill un-granted
    /// (a later plain toggle is NOT history consent — critic M4).
    #[test]
    fn set_capture_enabled_records_timestamp_and_is_forward_only_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        log.set_capture_enabled(true, /*backfill=*/ false, /*at=*/ 1_000).unwrap();
        assert!(log.capture_enabled().unwrap());
        assert_eq!(log.capture_enabled_at().unwrap(), Some(1_000));
        assert!(!log.backfill_consented().unwrap(), "a later toggle is NOT history consent (M4)");
        assert!(log.explicitly_set(ConfigFlag::CaptureEnabled).unwrap());
        log.verify_chain().unwrap(); // the flag event is signed + hash-chained
    }

    /// SP3 §6a: the Connect-checkbox path — enable + backfill in one atomic call sets BOTH flags and
    /// the timestamp.
    #[test]
    fn connect_path_sets_both_flags() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        log.set_capture_enabled(true, /*backfill=*/ true, /*at=*/ 2_000).unwrap();
        assert!(log.capture_enabled().unwrap());
        assert!(log.backfill_consented().unwrap());
        assert_eq!(log.capture_enabled_at().unwrap(), Some(2_000));
    }

    /// SP3 §6a (critic M4 — the disable/re-enable invariant). CHOSEN semantics: disabling clears
    /// the one-time backfill consent (it is spent), so a later forward-only re-enable gets a FRESH
    /// timestamp and does NOT silently re-import the declined backlog. Proven purely at the flag
    /// layer (no dependence on the sweeper's window logic).
    #[test]
    fn disabling_capture_clears_enabled_but_records_the_state() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());

        // Connect grants ongoing capture + historical backfill.
        log.set_capture_enabled(true, /*backfill=*/ true, /*at=*/ 1_000).unwrap();
        assert!(log.capture_enabled().unwrap());
        assert!(log.backfill_consented().unwrap());

        // Disable: capture off, and the spent backfill consent is cleared.
        log.set_capture_enabled(false, /*backfill=*/ false, /*at=*/ 2_000).unwrap();
        assert!(!log.capture_enabled().unwrap(), "disable turns capture off");
        assert!(!log.backfill_consented().unwrap(), "disable spends/clears the one-time backfill");
        // The flag stays explicitly set (disable is a real choice, not a return to never-set).
        assert!(log.explicitly_set(ConfigFlag::CaptureEnabled).unwrap());

        // Re-enable forward-only: fresh timestamp, backfill STAYS cleared → no silent re-import (M4).
        log.set_capture_enabled(true, /*backfill=*/ false, /*at=*/ 3_000).unwrap();
        assert!(log.capture_enabled().unwrap());
        assert_eq!(log.capture_enabled_at().unwrap(), Some(3_000), "forward-only window moves to re-enable");
        assert!(
            !log.backfill_consented().unwrap(),
            "a forward-only re-enable must NOT resurrect the spent backfill consent (M4)"
        );
        log.verify_chain().unwrap();
    }

    /// F2 security gate (parent §5.11): the private `append_graph_event` defaults
    /// `source_event_ids` to `[src, dst]` ONLY for the manual producer; a
    /// non-manual producer with an empty source set is REJECTED so taint cannot be
    /// laundered past the lineage walk. This unit test reaches the private helper
    /// directly (the public `link`/`invalidate` always pass `MANUAL_LINK_PRODUCER`,
    /// so they can never trigger the reject arm).
    #[test]
    fn append_graph_event_rejects_non_manual_producer_with_empty_sources() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());

        // Non-manual producer + empty sources → the F2 reject arm fires.
        let err = log
            .append_graph_event("link", "m4-reasoner", "a", "works_at", "b", None, &[])
            .expect_err("non-manual producer with empty sources must be rejected");
        match err {
            BossclawError::InvalidInput(msg) => assert!(
                msg.contains("non-manual"),
                "reject message should name the non-manual gate, got: {msg}"
            ),
            other => panic!("expected BossclawError::InvalidInput, got {other:?}"),
        }

        // Manual producer + empty sources → succeeds, defaulting to [src, dst].
        let id = log
            .append_graph_event("link", MANUAL_LINK_PRODUCER, "a", "works_at", "b", None, &[])
            .expect("manual producer with empty sources must succeed");
        let ev = log.stream_all().unwrap().into_iter().find(|e| e.id == id).unwrap();
        let meta = ev.model_meta.expect("link is Tier-B");
        assert_eq!(meta.model_id, MANUAL_LINK_PRODUCER);
        assert_eq!(
            meta.source_event_ids,
            vec!["a".to_string(), "b".to_string()],
            "manual empty-source link defaults to [src, dst]"
        );
    }

    #[test]
    fn grants_persist_revoke_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        // A real folder to canonicalize.
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        {
            let log = open_log(dir.path());
            log.add_grant(&folder).unwrap();
            let g = log.grants().unwrap();
            assert_eq!(g.len(), 1);
            assert!(!g[0].revoked);
            log.revoke_grant(&folder).unwrap();
            assert!(log.grants().unwrap()[0].revoked, "revoke marks the row");
        }
        // Reopen: grants are a fold over events, so they rebuild from the log.
        let log2 = open_log(dir.path());
        log2.rebuild_graph().unwrap();
        let g = log2.grants().unwrap();
        assert_eq!(g.len(), 1, "grant survives reopen via replay");
        assert!(g[0].revoked, "revoked state survives reopen");
    }

    #[test]
    fn files_projection_rebuilds_and_path_lookup_works() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        // Append a file_ingested event by hand (ingest_grant lands in a later task).
        let v1 = Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::FILE_INGESTED_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "text": "hello", "origin": crate::graph::EXTERNAL_ORIGIN,
                "provenance": { "canonical_path": "/x/a.md", "content_hash": "h1", "grant_root": "/x" }
            }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: ENGINE_SIGNER_DID.to_string(), signature: None,
        };
        let id1 = log.append(v1).unwrap();
        log.rebuild_graph().unwrap();
        let rec = log.current_file_for_path("/x/a.md").unwrap().expect("present");
        assert_eq!(rec.file_event_id, id1);
        assert_eq!(rec.content_hash, "h1");
        assert!(log.current_file_for_path("/x/missing.md").unwrap().is_none());
    }

    /// Minimal `memory` event for the graph-BFS unit test (mirrors the helper in
    /// `tests/graph.rs`; kept local so this unit module stays self-contained).
    fn mk_memory(text: &str) -> Event {
        Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: "memory".to_string(),
            content: serde_json::json!({ "text": text }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: "did:wba:AIR-TEST".to_string(),
            signature: None,
        }
    }

    /// The multi-hop BFS expands to `max_hops` and records the SHORTEST hop
    /// distance per node. Exercises the hop≥2 branch that the shipped
    /// `GRAPH_MAX_HOPS = 1` never reaches, so the `GRAPH_HOP_DECAY^(hop-1)` decay
    /// term and the frontier expansion are proven rather than merely asserted.
    /// Chain a→b→c: from seed `a`, `b` is a direct neighbor (hop 1) and `c` is
    /// reachable only through `b` (hop 2); the seed itself is excluded.
    #[test]
    fn current_neighbors_with_hops_expands_to_max_hops_shortest_distance() {
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let a = log.append(mk_memory("a")).unwrap();
        let b = log.append(mk_memory("b")).unwrap();
        let c = log.append(mk_memory("c")).unwrap();
        log.link(&a, "x", &b, None, &[]).unwrap();
        log.link(&b, "x", &c, None, &[]).unwrap();
        log.rebuild_graph().unwrap();

        let hops = log.current_neighbors_with_hops(std::slice::from_ref(&a), 2).unwrap();
        let expected: HashMap<String, u32> = [(b, 1), (c, 2)].into_iter().collect();
        assert_eq!(
            hops, expected,
            "BFS must reach b at hop 1 and c at hop 2, excluding the seed a"
        );
    }

    // ── T5 in-crate undo tests (need the private store / the test hook) ──────────

    /// Seed a clean memory id for the actuator unit tests (mirrors the integration
    /// harness, kept local). No embedder rebuild needed — `propose_write`'s taint
    /// gate only reads the event by id, not the vector index.
    #[cfg(unix)]
    fn seed_clean(log: &EventLog) -> String {
        log.append(mk_memory("clean inducing memory")).unwrap()
    }

    /// W8 CRASH-ORDERING: the `undo_state` row for a write MUST be durably COMMITTED
    /// before the FS mutation is observable, so a crash after the mutate always
    /// leaves recoverable pre-bytes (spec §7.3/§9 step 4).
    ///
    /// We install a `pre_mutate` probe that fires at the exact instant between the
    /// undo-row commit and the FS mutate. The probe opens an INDEPENDENT SQLCipher
    /// connection to the same DB and asserts the row is already present — proving it
    /// is COMMITTED (a still-open/uncommitted tx would not be visible to a separate
    /// connection under WAL). Without the durable-before-mutate ordering, the probe
    /// would not find the row.
    #[cfg(unix)]
    #[test]
    fn undo_row_is_committed_before_fs_mutate() {
        use crate::actuator::{WriteOp, WriteProposal};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let target = dir.path().join("p.txt");
        std::fs::write(&target, b"base bytes").unwrap();

        let log = open_log(dir.path());
        log.add_write_grant(dir.path()).unwrap();
        let clean = seed_clean(&log);

        // Shared flags the probe writes and the test reads after execute returns.
        let row_seen = Arc::new(AtomicBool::new(false));
        let probe_fired = Arc::new(AtomicBool::new(false));
        let target_str = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
        {
            let row_seen = Arc::clone(&row_seen);
            let probe_fired = Arc::clone(&probe_fired);
            let db = db.clone();
            let target_for_probe = target.clone();
            undo_test_hooks::install_pre_mutate_probe(Box::new(move |undo_id: &str| {
                probe_fired.store(true, Ordering::SeqCst);
                // The file must NOT yet be mutated at this instant: the base bytes
                // are still on disk (the mutate happens AFTER this probe returns).
                assert_eq!(
                    std::fs::read(&target_for_probe).unwrap(),
                    b"base bytes",
                    "the FS must be unmutated when the undo row is committed"
                );
                // Independent connection → only sees COMMITTED rows.
                let store = crate::store::Store::open(&db, &DEK).unwrap();
                let present: Option<String> = store
                    .conn()
                    .query_row(
                        "SELECT canonical_target FROM undo_state WHERE undo_id = ?1",
                        rusqlite::params![undo_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .unwrap();
                if present.as_deref() == Some(target_str.as_str()) {
                    row_seen.store(true, Ordering::SeqCst);
                }
            }));
        }

        let proposal = WriteProposal {
            target: target.clone(),
            new_content: b"mutated bytes".to_vec(),
            op: WriteOp::Edit,
            source_event_ids: vec![clean],
            rationale: "w8 ordering".to_string(),
        };
        let gated = log.propose_write(proposal).unwrap();
        let id = log.execute_write(gated, false).unwrap();
        undo_test_hooks::clear_pre_mutate_probe();

        assert!(probe_fired.load(Ordering::SeqCst), "the pre-mutate probe must have fired");
        assert!(
            row_seen.load(Ordering::SeqCst),
            "the undo_state row must be durably committed BEFORE the FS mutate (W8)"
        );
        // Sanity: the write did complete (the row is now bound to the event id).
        assert_eq!(std::fs::read(&target).unwrap(), b"mutated bytes");
        let ev = log.event_by_id(&id).unwrap().unwrap();
        assert_eq!(ev.event_type, crate::graph::FILE_WRITTEN_EVENT_TYPE);
    }

    /// W9 TAMPER (REVERT-SENSITIVE, in-crate because it needs the private store):
    /// `undo_write` verifies the captured `pre_bytes` hash equals the recorded
    /// `base_content_hash` before restoring. Corrupting the stored `pre_bytes` so its
    /// hash no longer matches MUST make the undo fail closed — the recovery store can
    /// never be turned into an injection vector.
    ///
    /// REVERT-SENSITIVITY: delete the hash-check block in `undo_write` (the
    /// `actual != base_content_hash` guard) → the tampered bytes would be written and
    /// this test's `is_err()` assertion + the "file untouched" assertion both fail.
    #[cfg(unix)]
    #[test]
    fn undo_tamper_pre_bytes_fails_closed() {
        use crate::actuator::{WriteOp, WriteProposal};

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("p.txt");
        std::fs::write(&target, b"v0 original").unwrap();

        let log = open_log(dir.path());
        log.add_write_grant(dir.path()).unwrap();
        let clean = seed_clean(&log);

        // Edit v0 → v1; this captures "v0 original" as the undo pre_bytes.
        let proposal = WriteProposal {
            target: target.clone(),
            new_content: b"v1 edited bytes".to_vec(),
            op: WriteOp::Edit,
            source_event_ids: vec![clean],
            rationale: "tamper".to_string(),
        };
        let gated = log.propose_write(proposal).unwrap();
        let write_id = log.execute_write(gated, false).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"v1 edited bytes");

        // Tamper the stored pre_bytes (so its hash != recorded base_content_hash),
        // reaching the private store directly (no public corruption surface).
        {
            let store = log.inner.lock().expect(POISON);
            let n = store
                .conn()
                .execute(
                    "UPDATE undo_state SET pre_bytes = ?1 WHERE file_written_id = ?2",
                    rusqlite::params![b"EVIL injected bytes".to_vec(), write_id],
                )
                .unwrap();
            assert_eq!(n, 1, "exactly one undo row must be tampered");
        }

        // The undo must fail closed (hash mismatch), and the file must NOT receive
        // the injected bytes — it stays at v1.
        assert!(
            log.undo_write(&write_id).is_err(),
            "a tampered pre_bytes (hash mismatch) must make undo fail closed"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"v1 edited bytes",
            "the tampered undo must not write the injected bytes"
        );
    }

    /// Rung-3 Phase-2 (§3.3–§3.5, I3/I4/I6/I7): one cycle over two contradicting notes emits exactly
    /// one proposal with a CONTENT-FREE `why` (the model's raw rationale never persists); a second
    /// cycle with nothing new does ZERO judge calls (proven with a PanicReasoner); gate-off is a no-op.
    /// `#[cfg(unix)]` (drives the append family).
    #[cfg(unix)]
    #[test]
    fn detect_conflicts_once_proposes_then_is_incremental_and_gated() {
        use crate::conflict::build_conflict_prompt;
        use crate::reason::{Reasoner, ScriptedReasoner};
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(64); // dim=64: the dim=8 marquee pair falls below CANDIDATE_SIM_MIN
        let log = open_log(dir.path());

        // Two near-duplicate notes (one token apart) so the finder clears the similarity floor.
        let older_text = "the default deploy target is vercel";
        let newer_text = "the default deploy target is fly";
        let _older = log.remember(&emb, older_text).unwrap();
        let _newer = log.remember(&emb, newer_text).unwrap();

        // Script the pair as a contradiction whose model `why` embeds a memory fragment SENTINEL — the
        // stored `why` must NOT contain it (I7: persisted why is a content-free template).
        let reasoner = ScriptedReasoner::new("test").with_response(
            crate::conflict::CONFLICT_SYSTEM,
            &build_conflict_prompt(older_text, newer_text),
            serde_json::json!({ "contradicts": true, "winner": "newer", "confidence": 92, "why": "SENTINEL_LEAK vercel vs fly verbatim" }),
        );
        let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
        let empty = std::collections::HashSet::new();

        // Gate OFF → skipped, no proposal (I3). PanicReasoner proves zero model calls.
        struct PanicReasoner;
        impl Reasoner for PanicReasoner {
            fn complete_json(&self, _s: &str, _p: &str, _sc: &serde_json::Value) -> Result<serde_json::Value, BossclawError> {
                panic!("reasoner must not be called");
            }
            fn model_id(&self) -> &str { "panic" }
        }
        let off = log.detect_conflicts_once(&emb, &PanicReasoner, &no_passages, &empty, 100).unwrap();
        assert!(off.skipped_disabled && off.proposed == 0, "gate off is a no-op with no model call");

        // Enable + run: exactly one proposal, with a content-free `why`.
        log.set_conflict_detect_enabled(true).unwrap();
        let r1 = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 100).unwrap();
        assert_eq!(r1.proposed, 1, "one contradiction proposed");
        let pending = log.pending_conflict_proposals().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(!pending[0].why.contains("SENTINEL_LEAK"), "I7: model's raw why never persisted");
        assert!(pending[0].why.contains("confidence"), "why is the content-free template");

        // Second cycle, nothing new since the cursor → ZERO judge calls (PanicReasoner must not fire).
        let r2 = log.detect_conflicts_once(&emb, &PanicReasoner, &no_passages, &empty, 100).unwrap();
        assert_eq!(r2.judged, 0, "no new subjects → no judging (cursor incrementality, I4)");
        assert_eq!(r2.proposed, 0);
        assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1, "still exactly one (idempotent)");
    }

    /// Rung-3 Phase-3 (§2.2 item 1, I9 — the FINDER half of stop-nagging): a `coexist_allowed` marker
    /// for a pair with NO open `conflict_proposal` must still suppress re-proposal. The finder unions
    /// `resolution_exclusions().{coexist_pairs ∪ dismissed_pairs}` into `open_pairs` (the SAME
    /// `unordered_pair_key` space it already screens against). The coexist-marker-WITHOUT-open-proposal
    /// construction isolates this union from open-set membership: a still-open proposal would suppress
    /// on its own, so without the union the judge fires and mints a proposal (`proposed == 1`) — the RED
    /// failure; with it, the pair is screened out BEFORE judging (`proposed == 0`).
    #[cfg(unix)]
    #[test]
    fn finder_union_suppresses_a_coexist_pair_with_no_open_proposal() {
        // REAL test-double API (verified `reason.rs:56-112`, Phase-2 ref `log.rs:10620-10668`).
        use crate::index::ConflictRef;
        use crate::conflict::{build_conflict_prompt, CONFLICT_SYSTEM};
        use crate::reason::ScriptedReasoner;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        // MUST be 64: at dim=8 these near-dups fall below CANDIDATE_SIM_MIN → zero candidate pairs → the test
        // would trivially pass for the wrong reason.
        let emb = MockEmbedder::new(64);

        // Proven near-duplicate texts (one token apart) that clear the similarity floor. n1 is remembered
        // first, so ref_ts makes it the OLDER side; register BOTH orderings so the pair surfaces from either
        // endpoint the finder judges first.
        let t1 = "the default deploy target is vercel";
        let t2 = "the default deploy target is fly";
        let n1 = log.remember(&emb, t1).unwrap();
        let n2 = log.remember(&emb, t2).unwrap();
        log.set_conflict_detect_enabled(true).unwrap();

        // The judge WOULD rule this pair a conflict (verdict keyed on SHA of (CONFLICT_SYSTEM, prompt)).
        let verdict = serde_json::json!({
            "contradicts": true, "winner": "newer", "confidence": 92, "why": "conflicting deploy targets"
        });
        let reasoner = ScriptedReasoner::new("test")
            .with_response(CONFLICT_SYSTEM, &build_conflict_prompt(t1, t2), verdict.clone())
            .with_response(CONFLICT_SYSTEM, &build_conflict_prompt(t2, t1), verdict);
        let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
        let empty = std::collections::HashSet::new();

        // A `coexist_allowed` marker exists for this exact pair, but NO open `conflict_proposal` does. This
        // ISOLATES the finder union from open-set membership: without the Task-7 union, `open_pairs` is empty,
        // the judge returns a conflict, and a proposal is minted (`proposed == 1`) — the RED failure. With the
        // union, `coexist_pairs` holds this pk → the finder screens it out BEFORE judging → `proposed == 0`.
        let (a, b) = (ConflictRef::Note { event_id: n1.clone() }, ConflictRef::Note { event_id: n2.clone() });
        let pk = ConflictRef::unordered_pair_key(&a, &b);
        log.append(crate::event::Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::COEXIST_ALLOWED_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "proposal_id": "P", "pair_key": pk, "a_ref": a.to_json(), "b_ref": b.to_json() }),
            model_meta: None, prev_hash: String::new(), hash: None, signed_by_did: log.signer_did(), signature: None,
        }).unwrap();

        let r = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 100).unwrap();
        assert_eq!(r.proposed, 0, "the coexist pair is never (re-)proposed by the finder (open_pairs union, I9)");
        assert!(log.pending_conflict_proposals().unwrap().is_empty(), "no proposal materializes for a coexist pair");
    }

    /// Rung-3 Phase-2 (§3.3, I4 — the multi-passage NO-STALL fix): a single capture whose near-
    /// duplicate passages produce MORE candidate pairs (up to C(5,2)=10) than one cycle's budget (8)
    /// must NOT defer the whole seq-group. Detection advances SUBJECT-BY-SUBJECT across cycles, emits
    /// passage-pair proposals, judges every subject with no mid-pipeline drop, fully drains (no
    /// permanent stall), and is restart-safe. The OLD whole-seq-group-deferral bug would leave `r1`
    /// with ZERO proposals and never advance the cursor within the capture — both asserted below.
    ///
    /// The fights index is an APPROXIMATE ANN rebuilt each cycle, so over a tight cluster of near-
    /// identical passages (tied distances) the EXACT number of the ≤10 pairs surfaced is not
    /// deterministic (correlated misses within one build can leave a few unsurfaced). This test
    /// therefore asserts the ENGINE invariants (no-stall / drip / no-drop / restart), which ARE
    /// deterministic — not exhaustive ANN recall, which is not an engine guarantee.
    #[cfg(unix)]
    #[test]
    fn detect_conflicts_once_advances_multi_passage_capture_without_stall() {
        use crate::conflict::{build_conflict_prompt, CONFLICT_SYSTEM};
        use crate::reason::ScriptedReasoner;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(64);
        let log = open_log(dir.path());
        log.set_conflict_detect_enabled(true).unwrap();

        let chunks: Vec<String> = ["alpha", "bravo", "charlie", "delta", "echo"]
            .iter()
            .map(|w| format!("config {w} sets the deploy target to vercel"))
            .collect();
        let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &ev, &chunks).unwrap();

        let chunks2 = chunks.clone();
        let passage_text = move |sid: &str, pid: usize| -> Option<String> {
            if sid == "s1" { chunks2.get(pid).cloned() } else { None }
        };
        // Script BOTH orderings of every pair. The passages are near-identical (tied cosine distances),
        // so the ANN fights index may surface a pair from either endpoint across runs; a real reasoner
        // answers either order, so the double MUST too (else a reversed pair would spuriously error and
        // stall — an artifact of the mock, not of the engine).
        let mut reasoner = ScriptedReasoner::new("test");
        for i in 0..chunks.len() {
            for j in 0..chunks.len() {
                if i == j {
                    continue;
                }
                reasoner = reasoner.with_response(
                    CONFLICT_SYSTEM,
                    &build_conflict_prompt(&chunks[i], &chunks[j]),
                    serde_json::json!({ "contradicts": true, "winner": "unclear", "confidence": 90, "why": "same target" }),
                );
            }
        }
        let empty = std::collections::HashSet::new();

        let before = log.conflict_cursor().unwrap();
        let r1 = log.detect_conflicts_once(&emb, &reasoner, &passage_text, &empty, 1).unwrap();
        assert!(r1.judged <= 8, "cycle judging is budget-bounded ({})", r1.judged);
        assert!(r1.proposed >= 1, "passage-pair proposals ARE emitted (0 under the old whole-group stall)");
        assert_ne!(log.conflict_cursor().unwrap(), before, "cursor advanced subject-by-subject");

        // Drive to steady state. Each cycle drips a budget-bounded slice; the cursor advances
        // subject-by-subject and MUST fully drain (a whole-group deferral or a subject stall would
        // spin `scanned_subjects > 0` forever). `dropped` must stay 0 across every cycle — every
        // JUDGED pair became a proposal, nothing was judged-then-discarded mid-pipeline.
        let mut total_dropped = r1.dropped;
        let mut drained = false;
        for _ in 0..12 {
            let rr = log.detect_conflicts_once(&emb, &reasoner, &passage_text, &empty, 1).unwrap();
            total_dropped += rr.dropped;
            if rr.scanned_subjects == 0 {
                drained = true; // no more subjects to enumerate → the cursor consumed all passages
                break;
            }
        }
        assert!(drained, "the cursor fully drains within a bounded number of cycles (no permanent stall)");
        assert_eq!(total_dropped, 0, "every judged pair became a proposal — nothing dropped mid-pipeline");
        let pending = log.pending_conflict_proposals().unwrap().len();
        assert!(pending >= 1, "passage-pair proposals accumulated across cycles ({pending})");

        drop(log);
        let log = open_log(dir.path());
        let r = log.detect_conflicts_once(&emb, &reasoner, &passage_text, &empty, 1).unwrap();
        assert_eq!(r.judged, 0, "restart: cursor persisted, nothing re-judged");
        assert_eq!(
            log.pending_conflict_proposals().unwrap().len(),
            pending,
            "no duplicates + no loss after restart (fold-derived open set is stable)"
        );
    }

    /// Rung-3 Phase-2 (§3.3, I4): one cycle NEVER exceeds the per-cycle judge budget even when many
    /// subjects each carry candidate pairs — it stops at `CONFLICT_JUDGE_PER_SWEEP`, flags
    /// `budget_hit`, and the backlog drips to the next cycle (the cursor keeps advancing).
    #[cfg(unix)]
    #[test]
    fn detect_conflicts_once_caps_judges_at_budget() {
        use crate::conflict::{build_conflict_prompt, CONFLICT_JUDGE_PER_SWEEP, CONFLICT_SYSTEM};
        use crate::reason::ScriptedReasoner;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(64);
        let log = open_log(dir.path());
        log.set_conflict_detect_enabled(true).unwrap();

        // 9 near-duplicate NOTES (8 shared tokens, 1 distinct) — each a distinct subject/seq, all
        // pairwise clearing the similarity floor (≈8/9 cosine, well above 0.82).
        let words = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];
        let notes: Vec<String> = words
            .iter()
            .map(|w| format!("the shared default config parameter setting value is {w}"))
            .collect();
        for n in &notes {
            log.remember(&emb, n).unwrap();
        }
        // Script EVERY unordered pair as a contradiction (either prompt order the sweep might build).
        let mut reasoner = ScriptedReasoner::new("test");
        for i in 0..notes.len() {
            for j in 0..notes.len() {
                if i == j {
                    continue;
                }
                reasoner = reasoner.with_response(
                    CONFLICT_SYSTEM,
                    &build_conflict_prompt(&notes[i], &notes[j]),
                    serde_json::json!({ "contradicts": true, "winner": "unclear", "confidence": 88, "why": "x" }),
                );
            }
        }
        let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
        let empty = std::collections::HashSet::new();

        let r1 = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 1).unwrap();
        assert!(r1.judged <= CONFLICT_JUDGE_PER_SWEEP, "judging capped at the budget ({})", r1.judged);
        assert!(r1.budget_hit, "the per-cycle budget was hit (more pairs than one cycle can judge)");
        assert!(r1.proposed >= 1, "at least one proposal emitted before the budget bit");

        // A follow-up cycle keeps making progress: the cursor advances past ≥1 more subject.
        let c1 = log.conflict_cursor().unwrap();
        let _ = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 1).unwrap();
        assert_ne!(log.conflict_cursor().unwrap(), c1, "backlog drips: the cursor advanced next cycle");
    }

    /// Rung-3 Phase-2 (§3.3, I6): a reasoner transport/decode failure is a NO-OP for that subject —
    /// the cycle stops, counts the error, proposes nothing, and does NOT advance the cursor past the
    /// failed subject, so a LATER cycle with a working reasoner still proposes it (fail-safe resume).
    #[cfg(unix)]
    #[test]
    fn detect_conflicts_once_reasoner_error_is_noop_and_resumable() {
        use crate::conflict::{build_conflict_prompt, CONFLICT_SYSTEM};
        use crate::reason::ScriptedReasoner;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(64);
        let log = open_log(dir.path());
        log.set_conflict_detect_enabled(true).unwrap();

        let older_text = "the default deploy target is vercel";
        let newer_text = "the default deploy target is fly";
        log.remember(&emb, older_text).unwrap();
        log.remember(&emb, newer_text).unwrap();
        let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
        let empty = std::collections::HashSet::new();

        // A reasoner with NO canned response for this pair → `complete_json` errors → `judge_pair` Err.
        let broken = ScriptedReasoner::new("broken");
        let before = log.conflict_cursor().unwrap();
        let r_err = log.detect_conflicts_once(&emb, &broken, &no_passages, &empty, 1).unwrap();
        assert!(r_err.reasoner_errors >= 1, "the reasoner failure is counted");
        assert_eq!(r_err.proposed, 0, "a failed judge proposes nothing (I6)");
        assert_eq!(log.pending_conflict_proposals().unwrap().len(), 0);
        assert_eq!(log.conflict_cursor().unwrap(), before, "cursor did NOT skip the failed subject");

        // A later cycle with a correctly-scripted reasoner proposes the SAME pair (nothing was lost).
        let good = ScriptedReasoner::new("good").with_response(
            CONFLICT_SYSTEM,
            &build_conflict_prompt(older_text, newer_text),
            serde_json::json!({ "contradicts": true, "winner": "newer", "confidence": 91, "why": "y" }),
        );
        let r_ok = log.detect_conflicts_once(&emb, &good, &no_passages, &empty, 2).unwrap();
        assert_eq!(r_ok.proposed, 1, "the once-failed pair proposes on a later working cycle");
        assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1);
    }

    /// Rung-3 Phase-3 (§3.3, Open-Q3): a DETERMINISTICALLY-erroring pair must not stall the sweep
    /// forever. A per-pair persistent consecutive-error counter retries below the budget (cursor held,
    /// I6) and, at `CONFLICT_PAIR_ERROR_BUDGET`, marks the pair `poison_skipped` — it stops holding the
    /// cursor AND stops being judged. `#[cfg(unix)]`.
    #[cfg(unix)]
    #[test]
    fn poison_pair_is_skipped_after_budget_and_frees_the_cursor() {
        // REAL test-double API. An ERRORING pair is created by simply NOT registering its
        // (CONFLICT_SYSTEM, build_conflict_prompt(a, b)) response — an unregistered prompt returns
        // `Err(Reasoner(..))` naturally (reason.rs:106-111). NO `err_on` builder exists.
        use crate::conflict::CONFLICT_PAIR_ERROR_BUDGET;
        use crate::reason::ScriptedReasoner;
        let dir = tempfile::tempdir().unwrap();
        let log = open_log(dir.path());
        let emb = MockEmbedder::new(64); // 64 — a lower dim drops the pair below CANDIDATE_SIM_MIN

        // EXACTLY ONE candidate pair: two near-duplicate notes (one token apart). A 2-note graph guarantees a
        // single pair — the architect traced that a 3-note {anchor, good, bad} graph produces STAGGERED poison
        // pairs (`bad` neighbours both), so `poison_skipped` on a fixed cycle count can be 0. Keep it to one pair.
        log.remember(&emb, "the default deploy target is vercel").unwrap();
        log.remember(&emb, "the default deploy target is fly").unwrap();
        log.set_conflict_detect_enabled(true).unwrap();

        // NO responses registered → every judge of this pair returns Err → a DETERMINISTIC poison pair.
        let reasoner = ScriptedReasoner::new("test");
        let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
        let empty = std::collections::HashSet::new();

        // Sub-budget cycles: the pair keeps erroring and the cursor does NOT advance past the subject (I6 — a
        // transient reasoner outage must retry next cycle, not be dropped). Assert the INVARIANTS each cycle,
        // not "on the Nth call".
        let mut r = None;
        for _ in 0..CONFLICT_PAIR_ERROR_BUDGET {
            r = Some(log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 100).unwrap());
            assert!(r.as_ref().unwrap().reasoner_errors >= 1, "the poison pair errored this cycle");
        }
        // On the budget-th consecutive error the pair is poison_skipped, stops holding the cursor, and the
        // sweep advances — a permanent stall becomes a bounded dropped-counter on ONE pair.
        assert!(r.unwrap().poison_skipped >= 1, "poison pair skipped once it reaches CONFLICT_PAIR_ERROR_BUDGET");
        let (cseq, _off) = log.conflict_cursor().unwrap();
        assert!(cseq > 0, "cursor advanced past the poisoned subject (sweep no longer stalls)");

        // It is truly STOPPED being judged (the top-of-loop poison check, not merely re-erroring): rewind the
        // cursor to re-scan the same subject and confirm NO fresh reasoner error is attributed to the pair.
        log.set_conflict_cursor(0, 0).unwrap();
        let after = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 100).unwrap();
        assert_eq!(after.reasoner_errors, 0, "a fully-poisoned pair is skipped BEFORE the judge, not re-judged");
    }

    /// Rung-3 Phase-2 (§3.5, I9): the open-proposal ceiling caps the pending set. A note cluster whose
    /// pairwise contradictions EXCEED `CONFLICT_OPEN_CEILING` drains to EXACTLY the ceiling, and at
    /// least one cycle reports `ceiling_hit` (the quiet "many pending" signal). `#[cfg(unix)]`.
    #[cfg(unix)]
    #[test]
    fn detect_conflicts_once_stops_proposing_at_open_ceiling() {
        use crate::conflict::{build_conflict_prompt, CONFLICT_OPEN_CEILING, CONFLICT_SYSTEM};
        use crate::reason::ScriptedReasoner;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(64);
        let log = open_log(dir.path());
        log.set_conflict_detect_enabled(true).unwrap();

        // 8 near-duplicate notes → C(8,2) = 28 pairwise contradictions > CONFLICT_OPEN_CEILING (20).
        let words = ["one", "two", "three", "four", "five", "six", "seven", "eight"];
        let notes: Vec<String> = words
            .iter()
            .map(|w| format!("the shared default config parameter setting value is {w}"))
            .collect();
        for n in &notes {
            log.remember(&emb, n).unwrap();
        }
        let mut reasoner = ScriptedReasoner::new("test");
        for i in 0..notes.len() {
            for j in 0..notes.len() {
                if i == j {
                    continue;
                }
                reasoner = reasoner.with_response(
                    CONFLICT_SYSTEM,
                    &build_conflict_prompt(&notes[i], &notes[j]),
                    serde_json::json!({ "contradicts": true, "winner": "unclear", "confidence": 88, "why": "x" }),
                );
            }
        }
        let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
        let empty = std::collections::HashSet::new();

        // Drive to steady state (cursor fully drained → a cycle enumerates no new subjects).
        let mut saw_ceiling = false;
        for _ in 0..40 {
            let r = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 1).unwrap();
            saw_ceiling |= r.ceiling_hit;
            if r.scanned_subjects == 0 {
                break; // nothing left to enumerate — drained
            }
        }
        assert_eq!(
            log.pending_conflict_proposals().unwrap().len(),
            CONFLICT_OPEN_CEILING,
            "the open set is capped at exactly the ceiling"
        );
        assert!(saw_ceiling, "some cycle reported ceiling_hit (the quiet 'many pending' signal)");
    }

    /// Rung-3 Phase-2 (§3.3, the unified fights index headline): a note↔passage cross-kind conflict is
    /// detected end-to-end through one full cycle — the emitted proposal binds ONE `Note` ref and ONE
    /// `Passage` ref. `#[cfg(unix)]`, `MockEmbedder::new(64)`.
    #[cfg(unix)]
    #[test]
    fn detect_conflicts_once_detects_note_vs_passage_cross_kind() {
        use crate::conflict::{build_conflict_prompt, CONFLICT_SYSTEM};
        use crate::index::ConflictRef;
        use crate::reason::ScriptedReasoner;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(64);
        let log = open_log(dir.path());
        log.set_conflict_detect_enabled(true).unwrap();

        // A note and a session passage that are near-duplicates (5/6 shared tokens ≈ 0.833 > 0.82).
        let note_text = "the default deploy target is vercel";
        let passage = "the default deploy target is fly".to_string();
        log.remember(&emb, note_text).unwrap();
        let ev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
        log.store_session_passages(&emb, &ev, std::slice::from_ref(&passage)).unwrap();

        let passage_owned = passage.clone();
        let passage_text = move |sid: &str, pid: usize| -> Option<String> {
            if sid == "s1" && pid == 0 { Some(passage_owned.clone()) } else { None }
        };
        // The note is older (remembered first), the passage newer → prompt order (note, passage).
        let reasoner = ScriptedReasoner::new("test").with_response(
            CONFLICT_SYSTEM,
            &build_conflict_prompt(note_text, &passage),
            serde_json::json!({ "contradicts": true, "winner": "newer", "confidence": 90, "why": "z" }),
        );
        let empty = std::collections::HashSet::new();

        let r = log.detect_conflicts_once(&emb, &reasoner, &passage_text, &empty, 5).unwrap();
        assert_eq!(r.proposed, 1, "the note↔passage contradiction is proposed once");
        let pending = log.pending_conflict_proposals().unwrap();
        assert_eq!(pending.len(), 1);
        let a_is_note = matches!(pending[0].a_ref, ConflictRef::Note { .. });
        let b_is_note = matches!(pending[0].b_ref, ConflictRef::Note { .. });
        let a_is_passage = matches!(pending[0].a_ref, ConflictRef::Passage { .. });
        let b_is_passage = matches!(pending[0].b_ref, ConflictRef::Passage { .. });
        assert!(
            (a_is_note && b_is_passage) || (a_is_passage && b_is_note),
            "the proposal binds exactly one Note ref and one Passage ref (cross-kind)"
        );
    }

    /// Rung-3 Phase-2 (§3.3): the judge-declines path — a non-contradiction verdict makes `judge_pair`
    /// return `Ok(None)`, which is COUNTED as `dropped` and NEVER proposed. `#[cfg(unix)]`.
    #[cfg(unix)]
    #[test]
    fn detect_conflicts_once_counts_judge_declines_as_dropped() {
        use crate::conflict::{build_conflict_prompt, CONFLICT_SYSTEM};
        use crate::reason::ScriptedReasoner;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(64);
        let log = open_log(dir.path());
        log.set_conflict_detect_enabled(true).unwrap();

        let older_text = "the default deploy target is vercel";
        let newer_text = "the default deploy target is fly";
        log.remember(&emb, older_text).unwrap();
        log.remember(&emb, newer_text).unwrap();

        // `contradicts: false` → `judge_pair` returns `Ok(None)` (a decline), even at high confidence.
        // Script BOTH orderings: a declined pair is not opened, so the second note re-judges the pair
        // from its own endpoint (reversed order) — a real reasoner answers either order.
        let decline = serde_json::json!({ "contradicts": false, "winner": "unclear", "confidence": 95, "why": "different scope" });
        let reasoner = ScriptedReasoner::new("test")
            .with_response(CONFLICT_SYSTEM, &build_conflict_prompt(older_text, newer_text), decline.clone())
            .with_response(CONFLICT_SYSTEM, &build_conflict_prompt(newer_text, older_text), decline);
        let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
        let empty = std::collections::HashSet::new();

        let r = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 3).unwrap();
        assert!(r.judged >= 1, "the pair WAS judged");
        assert!(r.dropped >= 1, "a non-contradiction verdict is counted as dropped");
        assert_eq!(r.proposed, 0, "a declined pair is never proposed");
        assert!(log.pending_conflict_proposals().unwrap().is_empty(), "no proposal persisted");
    }

    /// Rung-3 Phase-2 (§7, I9): on the FIRST enable over a big existing memory set, day one is a
    /// budget-bounded TRICKLE, not a wall — one cycle judges/proposes at most the per-cycle budget.
    /// `#[cfg(unix)]` (drives the append family).
    #[cfg(unix)]
    #[test]
    fn first_enable_is_a_trickle_not_a_wall() {
        use crate::conflict::{build_conflict_prompt, CONFLICT_JUDGE_PER_SWEEP, CONFLICT_SYSTEM};
        use crate::reason::ScriptedReasoner;
        let dir = tempfile::tempdir().unwrap();
        let emb = MockEmbedder::new(64); // dim=64 so the near-identical notes clear CANDIDATE_SIM_MIN
        let log = open_log(dir.path());
        log.set_conflict_detect_enabled(true).unwrap();

        // Seed many near-identical "the flag is X" notes so every pair is a candidate.
        let mut reasoner = ScriptedReasoner::new("test");
        let mut texts = Vec::new();
        for i in 0..12 {
            let t = format!("the feature flag is value {i} in the shared config");
            log.remember(&emb, &t).unwrap();
            texts.push(t);
        }
        // Script EVERY ordered pair as a contradiction so nothing is dropped for lack of a response.
        for i in 0..texts.len() {
            for j in 0..texts.len() {
                if i != j {
                    reasoner = reasoner.with_response(
                        CONFLICT_SYSTEM,
                        &build_conflict_prompt(&texts[i], &texts[j]),
                        serde_json::json!({ "contradicts": true, "winner": "newer", "confidence": 90, "why": "same flag" }),
                    );
                }
            }
        }
        let no_passages = |_: &str, _: usize| None;
        let empty = std::collections::HashSet::new();
        let r = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 1).unwrap();
        assert!(r.judged <= CONFLICT_JUDGE_PER_SWEEP, "day-one judging is budget-bounded ({})", r.judged);
        assert!(r.proposed <= CONFLICT_JUDGE_PER_SWEEP, "day-one proposals are a trickle ({})", r.proposed);
    }
}
