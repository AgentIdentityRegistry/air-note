# Desktop Engine Spine (SP1) — Design

**Status:** **Rev 2** (2026-06-22) — folds the critic + security second-opinion findings (both verdicts: SHIP-WITH-FIXES; core design verified sound against the code). Sub-project **1 of 5** in the "engine-in-the-desktop" milestone. Under user review.

## Context — the parent milestone

The AIR Agent desktop app currently knows only about identity + messaging (it depends on `air-rs` only). It has **no connection to the memory/actuator engine** (`bossclaw-core`, milestones M1–M6c). The parent milestone brings the full engine into the desktop: ingest, the evolve loop, mandate management, and the human confirm/preview surface for M6 write-proposals.

Too large for one spec, so it decomposes into 5 sub-projects, each its own spec → plan → build:

1. **Engine spine (this doc)** — a live, encrypted `EventLog` in the app, unlocked from the keychain. Foundation for all others.
2. **Ingest management** — folder read-grants + run ingest (native + markitdown jail) + list ingested files.
3. **Evolve loop runtime** — background `evolve_once` on a schedule (off-switch + per-tick caps); produces graph knowledge + write-proposals. (Overlaps the M7 "running scheduler.")
4. **Confirm / preview UI** — the proposal queue + before/after diff + Confirm/Decline + loud modal for risky changes.
5. **Mandate management** — add/revoke/list mandates + the global mandates on/off switch.

**Build order:** 1 → 2 → 3 → 4 (+5 alongside). This spec covers **only #1**.

## Goal

On first engine use after onboarding, open a single live `bossclaw_core::EventLog` — encrypted at rest, its hash chain verified — held in the app's shared state, and prove it is alive via a status probe. Establish the keys, storage, lifecycle, teardown, and access pattern that sub-projects 2–5 build on.

## Non-goals (explicitly deferred)

- File ingest / read-grants → SP2. The evolve loop / scheduler → SP3. Recall (semantic search index) → opened by SP3; the spine opens the **bare** log. Write-proposal listing/diff/confirm UI → SP4. Mandate add/revoke/list → SP5. Multi-device / portable-brain export (incl. key backup) → out of scope for the whole milestone.

## Architecture

### Dependency wiring (the literal first step)

Add `bossclaw-core = { path = "../../../crates/bossclaw-core" }` to `apps/desktop/src-tauri/Cargo.toml` (today it depends on `air-rs` only). **Enable NO optional features** — not `fastembed`, `ollama`, or `markitdown`. Dependency-surface note (honest): the default build pulls `model2vec-rs` (pure-Rust, lightweight) + bundled **SQLCipher** (via the engine), which ship in the binary from SP1 regardless; the heavy ONNX embedder (`fastembed`/`ort`) is feature-gated OFF and is not compiled. So "minimal surface" means *no ONNX/Ollama*, not zero cost — SQLCipher is the floor.

### Keys & unlock

The engine opens with `EventLog::open(path: &Path, dek: &[u8; 32], key: SigningKey)`. Three inputs, all local:

- **Brain signing key** — a fresh Ed25519 key, SEPARATE from the AIR identity key (decision 2026-06-22: decouples the memory log from network-identity rotation; a compromise of one key cannot forge the other). Minted with `SigningKey::generate(OsRng)` on first run.
- **DEK** — a fresh random 32-byte SQLCipher key, minted from `OsRng` on first run.
- **DB path** — `app_data_dir()/brain.db` (Tauri `app.path().app_data_dir()`).

**Storage = the per-key `SecretsVault`, NOT the API-key blob-cache.** Store both secrets via the same `Arc<dyn SecretsVault>` (`set/get/delete`, hex-encoded) that `IdentityStore` already uses for `air-agent.agent.signing_key` — one keychain item per secret, under new slots:
- `air-agent.engine.signing_key` — hex(32-byte Ed25519 brain key)
- `air-agent.engine.dek` — hex(32-byte DEK)

(Rationale, from the security review: `secret_get_cached`'s blob-cache keeps un-zeroized `String` copies for the process lifetime; `SecretsVault` is per-key and avoids that cache. On load, decode straight into `Zeroizing<[u8; 32]>` so the in-memory copy is wiped on drop — matching the engine's own `Zeroizing` of the DEK in `Store::open`.) These slots are **minted, never migrated** — do NOT add them to `vault.rs::LEGACY_KEYS` or `identity.rs` migration `PAIRS` (verified: neither migration namespace touches `engine.*`).

First-run sequence: generate both → persist BOTH via `SecretsVault::set` → open the DB. Later runs: load both → open.

### Engine keystore seam (testability)

A small `EngineKeystore { vault: Arc<dyn SecretsVault> }` owns mint/load/decode → `Zeroizing` key material. Production wires the real keychain vault; the existing `secrets::tests::MockVault` injects deterministic keys in tests — the engine's own tests use fixed `DEK`/`KEY_BYTES` the same way. This is the single seam the test substitutes; no other test hook needed.

### Lifecycle, access & concurrency

- **Single instance, async-safe lazy init.** Hold the engine in `AppState` behind an **async-safe lazy cell** — `tokio::sync::OnceCell<Arc<EventLog>>` via `get_or_try_init` (or a `tokio::sync::Mutex<Option<Arc<EventLog>>>`). This SERIALIZES concurrent first-opens so mint-once is guaranteed and exactly one `EventLog` is ever constructed (the single-writer invariant SP3/SP4 depend on). A plain check-then-store cell is INSUFFICIENT — two concurrent `engine_*` calls could double-mint or open two `EventLog`s over the same SQLCipher file. The blocking `EventLog::open` runs inside `spawn_blocking`; the async cell may be awaited across it.
- **Onboarding gate lives INSIDE the chokepoint.** `get_or_open()` itself checks "identity exists" — callers do not pre-check (so SP3/SP4 commands can't forget the gate). No identity → returns the `not_onboarded` state, no mint, no DB.
- **Access.** `EventLog` is `Send + Sync` and shared as `Arc<EventLog>` (it guards its `Store`/connection behind an internal `Mutex`); every op takes `&self`. Tauri handlers call it via `spawn_blocking` (matching the inbox's "spawn_blocking on all writes"), so the UI never stalls.
- **Process-lifetime handle.** The `EventLog` holds an open SQLCipher WAL connection for the app's lifetime; there is no explicit `close` (SP3/SP4 must not assume one). It is dropped on process exit or on reset (below).

### Teardown on identity reset (CRITICAL — from the critic)

`reset_identity` today calls only `IdentityStore::clear()`, which deletes only the `*.agent.*` identity slots + `identity.json` — it leaves the engine keys + `brain.db` intact. Without a fix, reset + re-onboard would silently re-attach the PREVIOUS identity's brain to the new identity (a silent privacy/correctness breach). **SP1 extends reset to a true clean slate:** on `reset_identity`, also `SecretsVault::delete` both engine slots, reset the lazy cell to empty (drop the memoized `Arc<EventLog>`), and delete `app_data_dir()/brain.db`. Reset is best-effort-complete: a failure to delete one part is surfaced, and `get_or_open` additionally fail-closes if it ever finds engine keys present with no identity (defense in depth).

### Demonstrable surface

`#[tauri::command] engine_status() -> EngineStatus` where:
```
enum EngineState { NotOnboarded, Ready, KeystoreInconsistent, KeystoreDbMismatch, ChainFailed }
struct EngineStatus { state: EngineState, event_count: i64, chain_ok: bool }
```
(`event_count` is `i64` to match `EventLog::count() -> Result<i64, _>`.) It get-or-opens the engine, runs `verify_chain()`, counts events, and maps failures to a distinct `state` so the UI can tell "not set up yet" from "something is wrong." (Optional, approved) a small "🧠 Brain ready · N memories" line in Settings that renders `state`/`event_count`.

### Recall deferred

The spine calls the bare `EventLog::open` (no recall index). SP3 switches to `open_with_recall` when the evolve loop needs semantic recall. Keeping the spine bare keeps SP1's dependency + startup surface minimal.

## Data flow

1. The engine is never opened at app startup — it opens lazily on the first `engine_*` command via `get_or_open()`, gated on onboarding inside the chokepoint.
2. `get_or_open()` (serialized via the async cell): identity check → load brain key + DEK from `SecretsVault` (mint both on first run) → `spawn_blocking(EventLog::open(app_data_dir()/brain.db, &dek, brain_key))` → memoize the `Arc<EventLog>`.
3. → `verify_chain()` + `count()` → map to `EngineStatus`.

## Failure / partial-state matrix (from the security review)

| brain-key | DEK | brain.db | Result |
|---|---|---|---|
| absent | absent | — | mint both → open-create → `Ready`, `event_count == 0` |
| present | absent (or vice-versa) | any | **HARD ERROR** `KeystoreInconsistent` — never re-mint (would orphan the DB) |
| present | present | absent | open-create → `Ready` (fresh DB) |
| present | present | present, DEK decrypts, chain ok | `Ready` |
| present | present | present, DEK rejects | `KeystoreDbMismatch` — distinct from tamper; NOT reported as `chain_ok=false` |
| present | present | present, decrypts but chain fails | `ChainFailed` (`chain_ok=false`) + fail-closed for future writers |

Other named states: keys minted but the first DB open fails (disk/permission) → error, no `Ready`; the next run takes "load both → open-create" (intended recovery). First-run **keychain write** failure → error, DB NOT created (clean retry).

## Security invariants

- DEK + brain key live ONLY in the OS keychain (per-key `SecretsVault`) + process memory, held in `Zeroizing` (not the clone-and-keep blob-cache); never on disk plaintext, never logged. (`EventLog`/`Store` derive no `Debug`; `SigningKey`'s `Debug` is redacted; error strings carry no key material — verified.)
- The brain key is distinct from the identity key (separate slot). Slots are minted, never migrated.
- `verify_chain()` runs on open; surfaced via `chain_ok`/`state`.
- **Fail-closed-on-tamper (forward invariant for SP3/SP4):** an `EngineHandle` whose `verify_chain()` failed serves reads for diagnostics but MUST refuse writes. SP1 has no writer, so this is inert today — stated now so later sub-projects inherit it rather than bolt it on.
- **Integrity SPOF note:** losing the brain key permanently fails `verify_chain` over the existing log even with a valid DEK; fold the brain key (not just the DEK) into any future backup/export design.
- No new `unsafe` (the desktop crate is not `forbid(unsafe)`, but this work adds none).

## Known limitations (named, accepted for SP1)

- **Poison-on-panic:** `EventLog`'s internal `Mutex` uses `.expect(POISON)`. If an engine op ever panics mid-lock, subsequent calls panic too — so "the command never panics" holds for clean operation but not across a prior engine panic. Accepted + documented; not mitigated in SP1.
- **Directory ordering:** `Store::open` does not create the parent dir; onboarding's `IdentityStore::save_metadata` already `create_dir_all`s `app_data_dir()`. The onboarding gate (inside `get_or_open`) guarantees the dir exists before open — do not open the engine on a path that bypasses onboarding.

## Testing

- **Rust integration test** (desktop crate, `#[cfg(unix)]`): inject a `MockVault` (existing, `secrets::tests`) + a temp `app_data_dir` → first call mints + opens → `verify_chain()` passes → `event_count == 0`, `state == Ready`. Re-open with the same mock vault → same DB, still verifies. Wrong DEK in the vault → `KeystoreDbMismatch`. One key present, one absent → `KeystoreInconsistent`. Reset path → engine slots + brain.db gone, cell empty.
- Gates: `cargo build -p air_agent_desktop` + `cargo test -p air_agent_desktop` green; `cargo clippy` clean; `cargo audit` in the build gate (new `bossclaw-core` dep tree); `npm run typecheck --workspace @air-agent/desktop` if the optional Settings line is added.

## New constants

- `air-agent.engine.signing_key` — `SecretsVault` slot; hex(32-byte Ed25519 brain key).
- `air-agent.engine.dek` — `SecretsVault` slot; hex(32-byte DEK).
- DB file: `app_data_dir()/brain.db`.

## Future hooks (NOT built here)

- SP3 replaces the bare open with `open_with_recall`.
- SP4 adds an `open_write_proposals()` engine query + the proposal Tauri commands, all routed through this same handle.
- All engine access goes through the one lazy-cell chokepoint — the single-writer invariant holds as SP3 adds the evolve-loop writer and SP4 adds confirm-UI readers.
