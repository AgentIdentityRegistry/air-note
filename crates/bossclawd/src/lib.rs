//! `bossclawd` — the local Unix-socket memory daemon for AIR Agent (M1a).
//!
//! This crate is the single owner of the encrypted memory engine: it holds the
//! DEK, opens the SQLCipher store, and hosts the embedder + reasoner + evolve
//! scheduler that live in the desktop app today. It serves the `bossclawd-proto`
//! Request/Response protocol over a `0600` Unix domain socket; the desktop app
//! (and, from M1b, Claude Code) become clients that reach the engine through it.
//!
//! Module map:
//! - [`lock`] — single-owner arbitration (PID lockfile + socket-liveness probe).
//! - [`server`] — per-connection dispatch: handshake → `Request` → `EngineHandle` → `Response`.
//! - [`engine`] — the copied engine host (`EngineHandle`, keystore, embedder, reasoner,
//!   cloud reasoner, Ollama probe, evolve scheduler). Copied from the desktop crate (Task 4);
//!   the in-app originals are removed in Task 6.
//! - [`secrets`] / [`vault`] — keychain access over the SHARED service name so the daemon and
//!   the app open the identical engine keystore + provider-key blob (the vault-key seam).
//! - [`identity`] — the daemon-local onboarding check (mirrors the app's `is_onboarded`).
//! - [`net_guard`] — the SSRF address screen the cloud reasoner's connect-time DNS pin uses.
//!
//! # Safety
//! The crate forbids `unsafe`. The PID-liveness probe (signal 0) is done via the
//! `nix` crate's safe syscall bindings rather than a hand-rolled `libc::kill`.
#![forbid(unsafe_code)]

pub mod lock;

// The engine host + its keychain/onboarding/SSRF support are Unix-only (bossclaw-core is
// Unix-only: bundled SQLCipher + rustix), exactly like the desktop crate's `#[cfg(unix)] mod engine`.
#[cfg(unix)]
pub mod engine;
#[cfg(unix)]
pub mod identity;
#[cfg(unix)]
pub mod net_guard;
#[cfg(unix)]
pub mod secrets;
#[cfg(unix)]
pub mod server;
#[cfg(unix)]
pub mod vault;
