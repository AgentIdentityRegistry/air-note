# Desktop Engine Spine (SP1) — Design

**Status:** Design approved 2026-06-22; spec under review. Sub-project **1 of 5** in the "engine-in-the-desktop" milestone.

## Context — the parent milestone

The AIR Agent desktop app currently knows only about identity + messaging (it depends on `air-rs` only). It has **no connection to the memory/actuator engine** (`bossclaw-core`, milestones M1–M6c). The parent milestone brings the full engine into the desktop: ingest, the evolve loop, mandate management, and the human confirm/preview surface for M6 write-proposals.

That milestone is too large for one spec, so it decomposes into 5 sub-projects, each its own spec → plan → build:

1. **Engine spine (this doc)** — a live, encrypted `EventLog` in the app, unlocked from the keychain. Foundation for all others.
2. **Ingest management** — folder read-grants + run ingest (native + markitdown jail) + list ingested files.
3. **Evolve loop runtime** — background `evolve_once` on a schedule (off-switch + per-tick caps); produces graph knowledge + write-proposals. (Overlaps the M7 "running scheduler.")
4. **Confirm / preview UI** — the proposal queue + before/after diff + Confirm/Decline + loud modal for risky changes.
5. **Mandate management** — add/revoke/list mandates + the global mandates on/off switch.

**Build order:** 1 → 2 → 3 → 4 (+5 alongside). This spec covers **only #1**.

## Goal

On first engine use after onboarding, open a single live `bossclaw_core::EventLog` — encrypted at rest, its hash chain verified — held in the app's shared state, and prove it is alive via a status probe. Establish the keys, storage location, lifecycle, and access pattern that sub-projects 2–5 build on.

## Non-goals (explicitly deferred)

- File ingest / read-grants → SP2.
- The evolve loop / scheduler → SP3.
- Recall (the in-memory semantic search index) → opened by SP3 when it needs it; the spine opens the **bare** log.
- Any write-proposal listing, diff, or confirm/decline UI → SP4.
- Mandate add/revoke/list → SP5.
- Multi-device / portable-brain export → out of scope for the whole milestone.

## Architecture

### Keys & unlock

The engine opens with `EventLog::open(path: &Path, dek: &[u8; 32], key: SigningKey)`. Three inputs, all sourced locally:

- **Brain signing key** — a fresh Ed25519 key, SEPARATE from the AIR identity key (decision 2026-06-22: decouples the memory log from network-identity rotation, and a compromise of one key cannot forge the other). Stored in the existing keychain blob-vault (`secret_set_cached`/`secret_get_cached`) under a new slot **`air-agent.engine.signing_key`**, encoded the same way the identity key is. Minted with `SigningKey::generate(OsRng)` on first run.
- **DEK** — a fresh random 32-byte database-encryption key (SQLCipher), stored under **`air-agent.engine.dek`**. Minted from `OsRng` on first run. Lives ONLY in the keychain + process memory — never written to disk plaintext, never logged.
- **DB path** — `app_data_dir()/brain.db` (Tauri `app.path().app_data_dir()`).

First-run sequence: generate both secrets → persist BOTH to the keychain → open the DB. On later runs: load both → open. (A keychain holding one secret but not the other is a corrupt state → hard error; see Error handling — never silently re-mint, which would orphan an existing encrypted DB.)

### Lifecycle & access

- **Single instance.** The `EventLog` is a single serialized writer (M1). The app holds exactly one, in `AppState` (alongside `inbox: InboxManager`), as an `EngineHandle` — an `Arc`-wrapped, lazily-opened holder that is the single chokepoint for all engine access (preserving the single-writer invariant for SP3/SP4).
- **Open timing.** Lazy + gated: opened on first engine use via `get_or_open()`, and only if an identity exists (post-onboarding). Before onboarding there is no brain. The open is memoized — subsequent calls reuse the instance.
- **Thread-safety.** `EventLog` ops are blocking SQLCipher calls; Tauri command handlers invoke them via `tauri::async_runtime::spawn_blocking` (matching the inbox's "spawn_blocking on all writes" pattern) so the UI never stalls. The handle is `Clone + Send + Sync`.

### Demonstrable surface

- `#[tauri::command] engine_status() -> EngineStatus` where `EngineStatus = { opened: bool, event_count: u64, chain_ok: bool }`. It get-or-opens the engine, runs `verify_chain()`, and counts events.
- (Optional, approved) a small "🧠 Brain ready · N memories" line in the Settings panel that calls `engine_status`.

### Recall deferred

The spine calls the bare `EventLog::open` (no recall index). SP3 switches to `open_with_recall` when the evolve loop needs semantic recall. Keeping the spine bare avoids pulling the embedding model into the app before it is needed and keeps SP1's dependency surface minimal.

## Data flow

1. The engine is never opened at app startup — it opens lazily on the first `engine_*` command, gated on onboarding (no identity → returns `{ opened: false, .. }`, no mint, no DB).
2. `engine_status` (or any future `engine_*` command) → `EngineHandle::get_or_open(app)`:
   - load brain key + DEK from the keychain (mint both on first run),
   - `EventLog::open(app_data_dir()/brain.db, &dek, brain_key)`,
   - memoize the instance in `AppState`.
3. → `verify_chain()` + event count → return `EngineStatus`.

## Error handling

- **Not onboarded** (no identity) → `engine_status` returns `{ opened: false, .. }`. No mint, no DB.
- **Keychain partial state** (exactly one of brain-key / DEK present) → hard error ("engine keystore inconsistent"). Do NOT silently re-mint — that would orphan the existing encrypted DB. Surfaced to the user; deeper recovery is a later concern.
- **DB open / chain-verify failure** (corrupt or tampered) → `engine_status.chain_ok = false` + a logged error; the command never panics. The rest of the app stays usable.
- **First-run mint failure** (keychain write fails) → error, and the DB is NOT created, so a retry starts clean.

## Security invariants

- The DEK and brain key live ONLY in the keychain + process memory — never on disk in plaintext, never logged.
- The brain key is distinct from the identity key (separate keychain slot).
- `verify_chain()` runs on open (tamper/truncation detection) and its result is surfaced via `chain_ok`.
- No new `unsafe` (the desktop crate is not `forbid(unsafe)`, but this work adds none).

## Testing

- **Rust integration test** in the desktop crate (`#[cfg(unix)]`, mirroring the engine's gating): with a temp `app_data_dir` and an injected deterministic key source, open a fresh brain → `verify_chain()` passes → `event_count == 0`. Re-open with the same keys → same DB, still verifies. Open with a wrong DEK → fails.
- The keychain is awkward in tests, so the engine-open path takes its **key source** via a small trait/param; the test injects deterministic keys (the engine's own tests use fixed `DEK`/`KEY_BYTES` this way). Production wiring supplies the keychain-backed source.
- Gates: `cargo build -p air_agent_desktop` + `cargo test -p air_agent_desktop` green; `cargo clippy` clean; `npm run typecheck --workspace @air-agent/desktop` if the optional Settings line is added.

## New constants

- `air-agent.engine.signing_key` — keychain slot; the 32-byte Ed25519 brain key, string-encoded the same way the identity key already is (plan confirms the exact scheme).
- `air-agent.engine.dek` — keychain slot; the 32-byte SQLCipher DEK, string-encoded the same way.
- DB file: `app_data_dir()/brain.db`.

## Future hooks (NOT built here)

- SP3 replaces the bare open with `open_with_recall`.
- SP4 adds an `open_write_proposals()` engine query + the proposal Tauri commands, all routed through this same `EngineHandle`.
- All engine access goes through the one `EngineHandle` chokepoint — the single-writer invariant holds as later sub-projects add writers (the evolve loop) and readers (the confirm UI).
