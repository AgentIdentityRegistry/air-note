//! The engine spine (SP1): a single live, encrypted `EventLog` wired into the desktop.
//! See docs/superpowers/specs/2026-06-22-desktop-engine-spine-design.md.

// The keystore is fully implemented here (SP1 Task 2) but its constructor and key
// material are first consumed by `EngineHandle` in Task 3 and the `engine_status`
// command in Task 4. Until those land, the symbols are unreferenced from non-test
// code, so scope a dead-code allow to the engine module (same pattern as
// `secrets::trait_def`). Remove once Task 3 wires `EngineHandle`.
#![allow(dead_code)]

pub mod keystore;

use std::fmt;

/// Errors from opening / accessing the engine. Mapped to `EngineState` for the UI.
#[derive(Debug)]
pub enum EngineError {
    /// No identity yet — the brain is not created before onboarding.
    NotOnboarded,
    /// Exactly one of (brain key, DEK) is present — never re-mint (would orphan the DB).
    KeystoreInconsistent,
    /// The DB could not be opened with the stored DEK (wrong key or unopenable).
    KeystoreDbMismatch(String),
    /// The DB opened but its hash chain failed verification (tamper/truncation).
    ChainFailed,
    /// A keychain or other I/O error.
    Vault(String),
    /// A background task failed to join.
    Join(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NotOnboarded => write!(f, "not onboarded"),
            EngineError::KeystoreInconsistent => write!(f, "engine keystore inconsistent"),
            EngineError::KeystoreDbMismatch(e) => write!(f, "engine keystore/DB mismatch: {e}"),
            EngineError::ChainFailed => write!(f, "engine chain verification failed"),
            EngineError::Vault(e) => write!(f, "engine keychain error: {e}"),
            EngineError::Join(e) => write!(f, "engine task error: {e}"),
        }
    }
}
