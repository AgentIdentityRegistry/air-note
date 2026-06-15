//! # bossclaw-core
//!
//! Local-first, signed, encrypted memory engine for BossClaw.
//! Milestone 1 (Bedrock): the encrypted, append-only, Ed25519-signed event log.
//!
//! The event log is the single source of truth; every other structure (M2+) is
//! derived and rebuildable from it. See
//! `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod error;
pub mod event;
pub mod highwater;
pub mod log;
pub mod sign;
pub mod store;

pub use error::BossclawError;
pub use event::{Event, ModelMeta};
pub use log::EventLog;
