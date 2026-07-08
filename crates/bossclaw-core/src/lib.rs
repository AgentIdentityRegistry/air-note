//! # bossclaw-core
//!
//! Local-first, signed, encrypted memory engine for BossClaw.
//! Milestone 1 (Bedrock): the encrypted, append-only, Ed25519-signed event log.
//! Milestone 2 (Recall): config-event convention, active-model lookup, and the
//! `Embedder` trait with a deterministic `MockEmbedder` for tests.
//! Milestone 3 (Graph): bi-temporal link/invalidate fold + graph-proximity recall boost.
//! Milestone 4a (Clever Linker): the Reasoner seam + LLM auto-linker (entity/link/invalidate
//! from memories), the evolve-loop runtime, the edge-trust gate.
//! Milestone 4b (Summarizer): per-entity dossier pages + supersede + the citation floor.
//!
//! The event log is the single source of truth; every other structure (M2+) is
//! derived and rebuildable from it. See
//! `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod actuator;
pub mod embed;
pub mod error;
pub mod event;
pub mod evolve;
pub mod extract;
#[cfg(feature = "fastembed")]
pub mod fastembed;
pub mod graph;
pub mod highwater;
pub mod index;
pub mod ingest;
pub mod keyword;
pub mod log;
pub mod mandate;
pub mod model2vec;
#[cfg(feature = "ollama")]
pub mod ollama;
pub mod reason;
pub mod recall;
pub mod reconcile;
pub mod sign;
pub mod summarize;
pub mod store;
/// M6c Task 10: the live filesystem watcher + debounced self-driver. unix-only —
/// it depends on the `notify` crate, which is itself `cfg(unix)`-gated in
/// `Cargo.toml`, so a non-unix target never pulls it.
#[cfg(unix)]
pub mod watch;

pub use actuator::{
    classify_op_existence, diff_guard, DiffFlags, FileId, GatedProposal, OpExistence, Provenance,
    Taint, WriteOp, WriteProposal, WriteVerdict,
};
pub use embed::{Embedder, MockEmbedder};
pub use error::BossclawError;
pub use event::{Event, ModelMeta};
pub use evolve::{EvolveReport, EvolveStatus};
#[cfg(feature = "fastembed")]
pub use fastembed::FastEmbed;
pub use graph::{AsOf, Edge, Entity, Grant, Mandate, Node, Page, WriteGrant, MAX_RECIPE_LEN};
pub use index::{HnswIndex, VectorIndex};
pub use log::{
    ActiveModel, ConfigFlag, EventLog, LanguagePackRecord, MigrationState, ReembedStats,
    SynthCacheRow, LOUD_ACK_REQUIRED_MSG, SCHEMA_VERSION,
};
#[cfg(unix)]
pub use log::MandateAction;
#[cfg(unix)]
pub use log::PendingProposal;
#[cfg(unix)]
pub use log::MandateWriteRecord;
#[cfg(unix)]
pub use watch::MandateWatcher;
pub use model2vec::Model2Vec;
#[cfg(feature = "ollama")]
pub use ollama::OllamaReasoner;
pub use reason::{Reasoner, ScriptedReasoner};
#[cfg(all(any(target_os = "macos", target_os = "linux"), any(feature = "markitdown", feature = "sandbox-test-hooks")))]
mod sandbox;
#[cfg(all(any(target_os = "macos", target_os = "linux"), feature = "markitdown"))]
pub use sandbox::SandboxedMarkitdownParser;
#[cfg(all(any(target_os = "macos", target_os = "linux"), feature = "sandbox-test-hooks"))]
pub use sandbox::sandbox_test_hooks;

pub use ingest::{is_external, IngestReport, NativeTextParser, Parser, PathHint};
pub use recall::{Hit, NoopReranker, RecallOptions, RecallSource, Reranker};
pub use summarize::{FactSet, RenderedPage};
