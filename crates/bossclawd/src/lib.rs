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
//! - [`telemetry`] — best-effort recall-miss telemetry backing the App-only `RecallStats` op (SP3).
//!
//! # Safety
//! The crate forbids `unsafe`. The PID-liveness probe (signal 0) is done via the
//! `nix` crate's safe syscall bindings rather than a hand-rolled `libc::kill`.
#![forbid(unsafe_code)]

pub mod lock;

// Session-capture path discipline (SP3). Cross-platform: the session-id allowlist
// + projects-root resolver compile everywhere; only the confined careful-open's
// fd-chain body is Unix-gated (a non-unix "unsupported" stub mirrors how `ingest`
// splits its per-OS open), so the module itself is NOT `#[cfg(unix)]`.
pub mod capture;

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
// Rung-3 Phase-2 conflict DETECTION — the off-by-default background sweep that records signed
// `conflict_proposal`s for contradictions between the brain's own memories (no UI, no mutation).
// Unix-gated like `server`/`telemetry`: it calls the cfg(unix) `engine`/`identity`.
#[cfg(unix)]
pub mod conflict;
// Recall-miss telemetry (SP3 A12): the rung-1/2 retrieval-floor tuning signal. Records every
// recall's hit/miss outcome under `<data_dir>/telemetry/` (best-effort, rotation-proof counters,
// queries-only recent-miss ring) and backs the App-only `RecallStats` op. Unix-only to reuse
// `bossclawd-paths`' 0600/0700 primitives, matching the capture siblings.
#[cfg(unix)]
pub mod telemetry;
#[cfg(unix)]
pub mod vault;
