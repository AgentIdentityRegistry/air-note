//! # bossclaw-core
//!
//! Local-first, signed, encrypted memory engine for BossClaw.
//! Milestone 1 (Bedrock): the encrypted, append-only, Ed25519-signed event log.
//! Milestone 2 (Recall): config-event convention, active-model lookup, and the
//! `Embedder` trait with a deterministic `MockEmbedder` for tests.
//!
//! The event log is the single source of truth; every other structure (M2+) is
//! derived and rebuildable from it. See
//! `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod embed;
pub mod error;
pub mod event;
#[cfg(feature = "fastembed")]
pub mod fastembed;
pub mod highwater;
pub mod index;
pub mod log;
pub mod model2vec;
pub mod sign;
pub mod store;

pub use embed::{Embedder, MockEmbedder};
pub use error::BossclawError;
pub use event::{Event, ModelMeta};
pub use index::{HnswIndex, VectorIndex};
#[cfg(feature = "fastembed")]
pub use fastembed::FastEmbed;
pub use log::{ActiveModel, EventLog};
pub use model2vec::Model2Vec;
