//! `bossclawd` — the local Unix-socket memory daemon for AIR Agent (M1a).
//!
//! This crate is a library target so the daemon binary (Task 4: accept-loop +
//! engine wiring in `src/main.rs`) can depend on the modules here without a
//! later restructure. For now it exposes exactly one module: [`lock`], the
//! single-owner arbitration that guarantees only one live daemon advances the
//! shared memory cursor per home directory.
//!
//! # Safety
//! The crate forbids `unsafe`. The PID-liveness probe (signal 0) is done via the
//! `nix` crate's safe syscall bindings rather than a hand-rolled `libc::kill`.
#![forbid(unsafe_code)]

pub mod lock;
