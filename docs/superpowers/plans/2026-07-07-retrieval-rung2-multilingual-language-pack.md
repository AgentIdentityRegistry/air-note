# Retrieval Rung 2 — Multilingual Embedder as an Opt-In Language Pack — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user opt into multilingual memory search (notably Korean) by downloading the larger `minishlab/potion-multilingual-128M` embedder as a **language pack** from the desktop Settings, re-embedding their existing memories under the new model, while the default English-only user sees byte-identical behaviour.

**Architecture:** The daemon (`bossclawd`) becomes the sole authority for *which* embedder to load: it resolves the active model from a **signed `config` event** (`language_pack`) in the encrypted log — env override (dev/harness) → signed record → bundled English default — and holds a **swappable in-process embedder** so a consent-gated, crash-safe, all-or-nothing re-embed migration can flip the served model atomically at the end without a daemon restart. The app's only jobs are (a) download + sha-verify the 3 model files into a writable data dir and (b) send one `SetActiveModel` RPC; everything that mutates state happens inside the daemon (which alone holds the signing keystore).

**Tech Stack:** Rust (workspace: `bossclaw-core`, `bossclawd`, `bossclawd-proto`, `memharness`), Tauri 2 (`apps/desktop/src-tauri`), React + TypeScript + Vitest (`apps/desktop/src`), SQLCipher via `rusqlite`, `model2vec-rs`, `reqwest`, `sha2`/`hex`, `serde_json`.

---

## Verified current anchors (re-read on this tree 2026-07-07)

All line numbers below were re-confirmed by reading the files on branch `feat-retrieval-rung2-multilingual` (working tree == `origin/main` `f6c4cbc`). **Trust these, not the spec's numbers** — where the spec drifted it is noted in the Spec-drift section at the end.

**`crates/bossclawd/src/engine/embed.rs`** (68 lines total)
- `:16` — `pub const MODEL_ID: &str = "minishlab/potion-base-8M";` (the seam U1/U2 replace)
- `:19-21` — `pub trait EmbedderProvider { fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError>; }`
- `:25-48` — `struct ResourceModel2Vec { model_dir: PathBuf, cell: Mutex<Option<Arc<dyn Embedder>>> }` + lazy build calling `Model2Vec::from_pretrained(&self.model_dir, MODEL_ID)` at `:42`
- `:50-67` — `#[cfg(test)] MockEmbedderProvider` (dim-parameterised)

**`crates/bossclawd/src/main.rs`** (231 lines)
- `:49-51` — `const ENV_MODEL_DIR: &str = "BOSSCLAWD_MODEL_DIR";`
- `:76-84` — `async_main` resolves `data_dir`, `sock_path`, `lock_path`, `model_dir`
- `:88-113` — advisory lock (`acquire_or_refuse`, LiveOwner → exit 0) + `bind_socket_0600` (AddrInUse → exit 0)
- `:121-133` — builds the `ResourceModel2Vec` provider (`:123`) + `EngineHandle::new(...).with_reasoner_cell(...)`
- `:135-142` — boot reseed of reasoner cell + scheduler spawn (the pattern U4's boot-resume mirrors)
- `:155` — `server::run_accept_loop(engine, listener).await`
- `:202-206` — `fn resolve_model_dir(data_dir) -> PathBuf` = env `BOSSCLAWD_MODEL_DIR` else `<data_dir>/models/potion-base-8M`

**`crates/bossclawd/src/engine/mod.rs`** (2799 lines)
- `:256-289` — `struct EngineHandle` fields (`cell`, `embedder_provider: Arc<dyn EmbedderProvider>`, `indexed: Mutex<bool>`, `reasoner_cell`, …)
- `:291-311` — `EngineHandle::new(vault, data_dir, embedder_provider, reasoner_provider)`
- `:337-366` — `get_or_open(onboarded)` first-open boot path (lazy `EventLog::open`)
- `:371-387` — `status(onboarded) -> EngineStatus` (never errors)
- `:447-472` — `run_ingest`: `provider.embedder()?` at `:452`, `set_active_model`-if-changed at `:456-463`
- `:501-516` — `ensure_indexed(&self, log)`: `self.embedder_provider.embedder()?` at `:502`, `rebuild_indexes`+`rebuild_graph`
- `:521-548` — `recall`: `ensure_indexed` → `spawn_blocking(log.recall(...))`
- `:556-614` — `evolve_once`: `ensure_indexed` at `:577`, `rebuild_entity_index` at `:592`

**`crates/bossclaw-core/src/log.rs`** (7380 lines)
- `:45` — `pub const SCHEMA_VERSION: u32 = 1;`
- `:60-75` — `pub struct ReembedStats { reembedded, gc_removed, elapsed_ms }`
- `:87-97` — `pub struct ActiveModel { active_model_id, dim, schema_version }`
- `:209` — `const CLOUD_REASONER_CONSENT_KEY: &str = "cloud_reasoner_consent";`
- `:216-240` — `pub enum ConfigFlag { Evolve, Proposals, Mandates, ReasonerConfig, CloudReasonerConsent }` + `fn key(self)`
- `:473-481` — `CREATE TABLE ... vectors (event_id, model_id, dim, embedding, PRIMARY KEY(event_id, model_id))`
- `:687-694` — `CREATE TABLE ... entity_vectors (entity_id, model_id, dim, embedding, PRIMARY KEY(entity_id, model_id))`
- `:747` — `pub fn append(&self, event: Event) -> Result<String, BossclawError>`
- `:864-868` — `pub fn count(&self) -> Result<i64, BossclawError>`
- `:1057-1073` — `pub fn active_model(&self) -> Result<Option<ActiveModel>, BossclawError>`
- `:1101-1119` — `pub fn derive_vector(embedder, event) -> Result<bool>` (INSERT OR REPLACE INTO vectors)
- `:1133-1174` — `pub fn rederive_pending(embedder) -> Result<usize>` (skips embed failures with `log::warn!`, propagates DB errors)
- `:1183-1201` — `pub fn vectors_for_model(model_id) -> Result<Vec<(String, Vec<f32>)>>` (ORDER BY event_id ASC)
- `:1226-1249+` — `pub fn rebuild_indexes(embedder)` (reads `vectors_for_model(embedder.model_id())`, rebuilds HNSW + FTS)
- `:1625-1659` — `fn collect_pending(model_id) -> Result<Vec<Event>>` (embeddable events lacking a vectors row for `model_id`)
- `:1668-1690+` — `fn collect_embeddable_events_ordered() -> Result<Vec<(String, String)>>` (embeddable events **with non-empty text**, seq ASC)
- `:1834-1908` — `pub fn reembed_migration(embedder) -> Result<ReembedStats>` — appends config FIRST, `rederive_pending`, `DELETE FROM vectors WHERE model_id != ?1` (`:1873`), `rebuild_indexes`, returns Ok. **Zero production callers. GCs `vectors` only, NOT `entity_vectors`.**
- `:1917-1938` — `pub fn set_active_model(model_id, dim) -> Result<String>` (signed config, no re-embed)
- `:4308-4310` — `pub(crate) fn signer_did(&self) -> String` (returns `ENGINE_SIGNER_DID`)
- `:4871-4886` — `pub fn derive_entity_vector(embedder, entity_id, text)` (INSERT OR REPLACE INTO entity_vectors)
- `:4892-4901` — `pub fn rebuild_entity_index(embedder)` (reads `entity_vectors_for_model`)
- `:4906-4925` — `fn entity_vectors_for_model(model_id) -> Result<Vec<(String, Vec<f32>)>>`
- `:5044-5064` — `pub fn set_evolve_enabled(enabled)` (signed control `config`, template for a new signed setter)
- `:5104-5124` — `pub fn set_cloud_reasoner_consent(record: serde_json::Value)` (**the exact template for the new signed record writer**)
- `:5137-5151` — `pub fn evolve_enabled() -> Result<bool>` (newest-explicit-wins sticky scan)
- `:5164-5187` — `fn latest_config_value(key) -> Result<Option<serde_json::Value>>` (**the exact template for the new record reader**)
- `:6975-6996` — `pub fn resolve_arms(vector, keyword)` (silent keyword-only degrade on a missing vector arm — the reason wrong-model load is silent today)
- `crates/bossclaw-core/src/model2vec.rs:62-79` — `Model2Vec::from_pretrained(dir, model_id)` (dim runtime-probed at `:68`, no dim constant)
- `crates/bossclaw-core/src/embed.rs:41-73` — `pub struct MockEmbedder { dim }` (model_id() == `"mock-v1"`)
- `crates/bossclaw-core/tests/recall.rs:10-16,190-192,1294-1463` — test consts (`DEK`, `KEY_BYTES`, `MID_DIM=64`, `MOCK_MODEL_ID="mock-v1"`), `mk_memory_event`, `MockEmbedderV2` (id `"mock-v2"`), and the four existing `reembed_migration_*` tests

**`crates/bossclawd-proto/src/lib.rs`** (601 lines)
- `:80-143` — `pub enum Request { ... 29 variants ... EnableCloudReasoner { onboarded, config } }`
- `:163-218` — `pub enum Response { Ok, Status(EngineStatusWire), ... Err { kind, message } }`
- `:231-256` — `pub enum OpErrorKindWire { Core, Embedder, ... }`
- `crates/bossclawd-proto/src/types.rs:550-567` — `EngineStateWire` + `EngineStatusWire { state, event_count, chain_ok }`

**`crates/bossclawd/src/server.rs`** (577 lines)
- `:133-259` — `async fn dispatch(engine, req)` 1:1 arm table (`SetEvolveEnabled` → `unit_result`, `Status` → `status_wire`)
- `:293-307` — `pub fn op_error_response(e)` exhaustive typed-error mapping (no `_` wildcard — a new arm forces a decision)
- `:330-342` — `fn status_wire(s: EngineStatus) -> EngineStatusWire`
- `:443-480` — `run_accept_loop` (shared prod + test)
- `:517-533` — `pub fn test_engine` / `test_engine_with_embedder(home, provider)` (test-helpers seam memharness uses)

**`crates/bossclawd/src/engine/client.rs`** (1164 lines)
- `:86-99` — `status(onboarded)` (transport failure → `keystore_mismatch_status`)
- `:405-421` — `async fn request(req)` / `async fn unit(req)` helpers
- `:451-469` — `fn op_error_from_wire(kind, message)` (inverse of `op_error_response`)
- `crates/bossclawd/src/engine/transport.rs:60-63` — `pub trait Transport { async fn request(&self, req: Request) -> Result<Response, EngineOpError>; }`

**`apps/desktop/src-tauri/src/engine/daemon.rs`** (304 lines)
- `:107-111` — `fn build_daemon_command(bin_path, model_dir)` sets `BOSSCLAWD_MODEL_DIR` (the push U1/I1 removes)
- `:119-168` — `ensure_started(sock_path, bin_path, model_dir)` (probe-then-spawn, never kills/supervises)

**`apps/desktop/src-tauri/src/main.rs`** (239 lines)
- `:73-104` — `#[cfg(unix)]` engine setup: resolves `model_dir` from `resource_dir().join("resources/models/potion-base-8M")` (`:86-90`) and passes it to `ensure_started` (the push U1/I1 removes); builds `SocketTransport` + `Engine`
- `:124-236` — `invoke_handler![ ... ]` (where new commands register)

**`apps/desktop/src-tauri/src/engine/mod.rs`** — `:224-247` `pub struct Engine { client }` facade + `Engine::new`; `:251` `status`, `:289` `recall` (delegation pattern the new facade methods mirror). `EngineOpError::Unavailable`/`EngineError::Unavailable` exist (transport-down mapping).

**`apps/desktop/src-tauri/src/commands/engine.rs`** (1085 lines)
- `:105-109` — `#[tauri::command] engine_status(state) -> Result<EngineStatus, String>`
- `:472-481` — `engine_ollama_status()` (payload-encoded, never-throws — the poll template)
- `:531-560` — `ReasonerConfigDto` + `engine_get_reasoner_config` (DTO + command template)
- `:582-592` — `engine_enable_cloud_reasoner(config, state)` (the enable-command template)

**`apps/desktop/src/api/engine.ts`** (135 lines) — `invoke<T>()` wrappers; `:43-46` `evolveStatus`/`setEvolveEnabled`/`evolveNow`; `:66-73` reasoner wrappers (the wrapper template)
**`apps/desktop/src/memory/MemoryPanel.tsx`** (222 lines) — `:38-57` `refreshStatus` + poll loop; `:83-95` `onToggleEvolve` + `toggling`; `:190-207` the "Local model: ready / pull" hint block (the language-pack card template)
**`apps/desktop/src/components/ui/SettingsSectionCard.tsx`** — `SettingsSectionCard({ title, description?, actions?, children, className? })`
**`apps/desktop/src/components/ui/ToggleSwitch.tsx`** — `ToggleSwitch({ checked, onChange, label?, onLabel?, offLabel?, disabled? })`
**`scripts/fetch-model.sh`** (40 lines) — `:18-25` `pinned_sha()` case; `:28-39` curl + `shasum -a 256` verify + `rm` on mismatch (the downloader's verification template)
**`apps/desktop/src-tauri/tauri.conf.json:29`** — `"resources": ["resources/models/potion-base-8M/*"]` (English bundle; multilingual is NOT added here)
**`crates/memharness/src/daemon.rs:29-64`** — env-override model resolution (`BOSSCLAWD_MODEL_DIR` → repo fallback) + `spawn_real` → `ResourceModel2Vec::new(model_dir)` (**must keep working: memharness sets env itself and passes the dir; env override stays highest priority**)
**`scripts/install-bossclawd.sh:42-44,85,179-191`** — launchd/systemd installer pins `BOSSCLAWD_MODEL_DIR` to the English bundle (the pin Ops task O2 removes so the signed record can win on the installed path)

**Deps confirmed present:** `apps/desktop/src-tauri/Cargo.toml` has `reqwest` (`:20`), `sha2` (`:26`), `hex` (`:27`), `tokio` full (`:29`), `tempfile` (`:57`). **No `nix`/`fs2`** in the desktop crate — Task B1 adds `fs2` for the disk preflight. `crates/bossclaw-core/Cargo.toml` has `sha2`/`hex`.

---

## File structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/bossclaw-core/src/log.rs` | New signed `language_pack` record (writer/reader/`ConfigFlag`); hardened all-or-nothing migration primitives (`reembed_prepare` + `reembed_finalize_gc`); entity-vector re-derive; shared `embed_and_upsert` helper; boot GC sweep | A1, A2 |
| `crates/bossclaw-core/tests/recall.rs` | RED tests for A1/A2 (record roundtrip, all-or-nothing shortfall, entity migration, invariance) | A1, A2, A11 |
| `crates/bossclawd-proto/src/lib.rs` | New `Request::SetActiveModel` + `Request::ModelStatus`; `Response::ModelStatus`; serde roundtrip test | A3 |
| `crates/bossclawd-proto/src/types.rs` | `ModelStateWire`, `ReindexProgressWire`, `ModelStatusWire` | A3 |
| `crates/bossclawd/src/engine/embed.rs` | Pull-based, sha-verified, **swappable** production provider (`ResourceModel2Vec` rewrite) with injectable loader test seam; `ModelState`/reindex cells | A4 |
| `crates/bossclawd/src/engine/mod.rs` | Wire resolution into `run_ingest`/`ensure_indexed`/`evolve_once`; `set_active_model` orchestration + background migration; `model_status`; boot `resume_migration_if_pending`; `EngineStatus` gains model-state | A5, A6, A7 |
| `crates/bossclawd/src/server.rs` | Dispatch arms for `SetActiveModel`/`ModelStatus`; `model_status_wire` | A8 |
| `crates/bossclawd/src/main.rs` | Build the resolution-aware provider; call boot resume | A5, A7 |
| `crates/bossclawd/tests/roundtrip.rs` | End-to-end integration gate (swap machinery, resume, fail-loud) | A9 |
| `crates/bossclawd/src/engine/client.rs` | `set_active_model` + `model_status` client methods | A8 |
| `apps/desktop/src-tauri/src/engine/mod.rs` | `Engine` facade delegation for the two new ops | A8 |
| `apps/desktop/src-tauri/src/engine/language_pack.rs` (new) | Downloader: preflight → fetch → sha-verify (fail-closed) → atomic rename → `air-model.json` binding | B1 |
| `apps/desktop/src-tauri/src/commands/engine.rs` | `engine_download_language_pack`, `engine_set_active_model`, `engine_model_status` commands + DTOs; English staging on boot | B2, B3 |
| `apps/desktop/src-tauri/src/main.rs` | Stage English into data dir + stop pushing `BOSSCLAWD_MODEL_DIR`; register new commands | B3 |
| `apps/desktop/src-tauri/src/engine/daemon.rs` | Drop the `BOSSCLAWD_MODEL_DIR` push from `build_daemon_command`/`ensure_started` | B3 |
| `apps/desktop/src/api/engine.ts` | `downloadLanguagePack()`, `setActiveModel()`, `modelStatus()` + DTOs | B4 |
| `apps/desktop/src/memory/LanguagePackCard.tsx` (new) + `MemoryPanel.tsx` | Settings card (states + loud missing/mismatch) | B5 |
| `apps/desktop/src/memory/LanguagePackCard.test.tsx` (new) | Vitest for the card | B5 |
| `scripts/install-bossclawd.sh`, `*.plist.in`, `*.service.in` | Remove `BOSSCLAWD_MODEL_DIR` pin + stage English into the daemon's default `<data_dir>/models/potion-base-8M` | O2 |
| GitHub Release `models-multilingual-128M-v1` | Upload 3 files; pin 3 sha256 | O1 |

---

## Naming contract (pin these once — every task references them)

These identifiers appear across multiple tasks. Define once, reuse verbatim:

- Multilingual model id: `"minishlab/potion-multilingual-128M"`
- Data-dir models root: `<data_dir>/models/` (English default: `<data_dir>/models/potion-base-8M`; multilingual: `<data_dir>/models/minishlab/potion-multilingual-128M` — note the id contains a `/`, so the folder is nested; the resolver joins the id onto the root and this Just Works on both OSes)
- Local id-binding file: `air-model.json` = `{ "model_id": <string>, "safetensors_sha": <hex string> }`
- Signed config key: `"language_pack"`; record `LanguagePackRecord { model_id, safetensors_sha, migration: "in_progress"|"complete", consented_at }`
- GitHub Release tag: `models-multilingual-128M-v1`
- Env override (dev/harness only): `BOSSCLAWD_MODEL_DIR` (unchanged name)

---

# Phase A — Rust engine

## Task A1: Signed `language_pack` record (single source of truth — I2)

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (add consts + `ConfigFlag::LanguagePack` near `:209`/`:216-240`; add types + writer/reader near the reasoner-consent setter `:5104-5124` and reader `:5164-5187`)
- Test: `crates/bossclaw-core/tests/recall.rs`

- [ ] **Step 1: Write the failing test** (append to `crates/bossclaw-core/tests/recall.rs`)

```rust
// ── Rung 2: signed language_pack record (A1) ────────────────────────────────
use bossclaw_core::{LanguagePackRecord, MigrationState};

#[test]
fn language_pack_record_roundtrips_and_is_sticky_and_signed() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    // Absent by default (English default path — no record).
    assert!(log.language_pack_record().unwrap().is_none());

    // Write an in-progress record.
    let rec = LanguagePackRecord {
        model_id: "minishlab/potion-multilingual-128M".to_string(),
        safetensors_sha: "abc123".to_string(),
        migration: MigrationState::InProgress,
        consented_at: "2026-07-07T00:00:00Z".to_string(),
    };
    log.set_language_pack_record(&rec).unwrap();
    assert_eq!(log.language_pack_record().unwrap().unwrap(), rec);

    // Newest wins (flip to complete).
    let done = LanguagePackRecord { migration: MigrationState::Complete, ..rec.clone() };
    log.set_language_pack_record(&done).unwrap();
    assert_eq!(log.language_pack_record().unwrap().unwrap().migration, MigrationState::Complete);

    // It carries no model fields, so it must NOT disturb active_model().
    assert!(log.active_model().unwrap().is_none(), "language_pack record must not be seen as an active_model config");

    // Signed + chained.
    log.verify_chain().expect("chain verifies after two language_pack config events");
}
```

- [ ] **Step 2: Run it, expect FAIL**

Run: `cargo test -p bossclaw-core --test recall language_pack_record_roundtrips -- --nocapture`
Expected: FAIL — `cannot find type LanguagePackRecord in crate bossclaw_core` / `no method named set_language_pack_record`.

- [ ] **Step 3: Add the const + `ConfigFlag` arm.** In `crates/bossclaw-core/src/log.rs`, immediately after the `CLOUD_REASONER_CONSENT_KEY` const (`:209`):

```rust
/// Signed `config` key for the opt-in multilingual language pack (rung 2). Its presence +
/// `migration == Complete` is the SOLE authority for "load the multilingual model" (invariant I2);
/// `InProgress` records a consented-but-unfinished re-embed the daemon RESUMES on boot (I6). Written
/// only by [`EventLog::set_language_pack_record`]; absence means the English default (I7).
const LANGUAGE_PACK_KEY: &str = "language_pack";
```

In `pub enum ConfigFlag` (`:216-227`) add the variant after `CloudReasonerConsent`:

```rust
    /// The signed opt-in multilingual language-pack record ([`LANGUAGE_PACK_KEY`]).
    LanguagePack,
```

In `fn key(self)` (`:231-239`) add the arm:

```rust
            ConfigFlag::LanguagePack => LANGUAGE_PACK_KEY,
```

- [ ] **Step 4: Add the record types.** In `crates/bossclaw-core/src/log.rs`, after `pub struct ActiveModel { ... }` (`:97`):

```rust
/// Whether an opt-in language-pack migration has finished. `InProgress` means consent was
/// recorded and re-embedding started but the atomic end-flip has NOT run (recall keeps serving
/// the OLD model); `Complete` means the multilingual model is live (recall serves it). The daemon
/// resumes an `InProgress` migration on boot (invariant I6).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MigrationState {
    InProgress,
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
```

- [ ] **Step 5: Add the writer + reader.** After `set_cloud_reasoner_consent` (`:5124`):

```rust
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
```

- [ ] **Step 6: Export the new public types.** In `crates/bossclaw-core/src/lib.rs`, add `LanguagePackRecord` and `MigrationState` to the `pub use log::{...}` re-export list (find the existing `pub use log::` line that re-exports `ActiveModel`, `ReembedStats`, etc., and append the two names).

- [ ] **Step 7: Run it, expect PASS**

Run: `cargo test -p bossclaw-core --test recall language_pack_record_roundtrips -- --nocapture`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/src/lib.rs crates/bossclaw-core/tests/recall.rs
git commit -m "feat(bossclaw-core): signed language_pack record — the rung-2 single source of truth (I2)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task A2: Hardened all-or-nothing migration + entity-vector migration (I5, U4, U8)

The existing `reembed_migration` (`:1834-1908`) is NOT all-or-nothing (it GCs after a **best-effort** `rederive_pending` that silently skips embed failures) and never touches `entity_vectors`. This task adds two crash-safe primitives the daemon orchestrates around the signed-record flip, plus a shared `embed_and_upsert` helper (DRY with `rederive_pending`).

**Files:**
- Modify: `crates/bossclaw-core/src/log.rs` (extract `embed_and_upsert`; add `reembed_prepare`, `reembed_finalize_gc`, `rederive_entity_vectors_pending`, `gc_stale_vectors`)
- Test: `crates/bossclaw-core/tests/recall.rs`

- [ ] **Step 1: Write the failing tests** (append to `crates/bossclaw-core/tests/recall.rs`). These pin I5 (shortfall → Err + no GC + old vectors intact) and U8 (entity vectors migrate).

```rust
// ── Rung 2: all-or-nothing migration (A2 / I5 / U8) ─────────────────────────

/// An embedder that FAILS on any text containing the sentinel token, so an injected shortfall
/// is deterministic. model_id is "flaky-v2" so it partitions distinctly from mock-v1.
struct FlakyEmbedder {
    inner: MockEmbedder,
}
const FLAKY_MODEL_ID: &str = "flaky-v2";
const FAIL_TOKEN: &str = "FAILTOKEN";
impl FlakyEmbedder {
    fn new(dim: usize) -> Self { Self { inner: MockEmbedder::new(dim) } }
}
impl Embedder for FlakyEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError> {
        if texts.iter().any(|t| t.contains(FAIL_TOKEN)) {
            return Err(BossclawError::Embed("injected embed failure".into()));
        }
        self.inner.embed(texts)
    }
    fn dim(&self) -> usize { self.inner.dim() }
    fn model_id(&self) -> &str { FLAKY_MODEL_ID }
}

#[test]
fn reembed_prepare_shortfall_returns_err_and_gcs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    // 3 events under mock-v1; one will fail to re-embed under the flaky model.
    for t in ["ocean waves crashing", "FAILTOKEN broken event", "mountain peaks snowy"] {
        log.append(mk_memory_event(t)).unwrap();
    }
    let v1 = MockEmbedder::new(MID_DIM);
    log.rederive_pending(&v1).unwrap();
    let old_count = log.vectors_for_model(MOCK_MODEL_ID).unwrap().len();
    assert_eq!(old_count, 3);

    // Prepare under the flaky model → incomplete → Err.
    let flaky = FlakyEmbedder::new(MID_DIM);
    let err = log.reembed_prepare(&flaky, &mut |_done, _total| {}).unwrap_err();
    assert!(format!("{err}").contains("incomplete"), "shortfall must be reported: {err}");

    // I5: old vectors are INTACT — no GC ran.
    assert_eq!(
        log.vectors_for_model(MOCK_MODEL_ID).unwrap().len(),
        old_count,
        "a shortfall must leave every old vector intact (no GC)"
    );
}

#[test]
fn reembed_prepare_then_finalize_migrates_vectors_and_entity_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    for t in ["ocean waves crashing", "forest trees rustling"] {
        log.append(mk_memory_event(t)).unwrap();
    }
    let v1 = MockEmbedder::new(MID_DIM);
    log.rederive_pending(&v1).unwrap();
    // Seed an entity vector under mock-v1 so we can prove entity migration (U8).
    log.derive_entity_vector(&v1, "entity:01ARIA", "Aria Novak").unwrap();
    assert_eq!(log.entity_vectors_for_model(MOCK_MODEL_ID).unwrap().len(), 1);

    // Prepare + finalize under mock-v2 (both succeed).
    let v2 = MockEmbedderV2::new(MID_DIM);
    let mut progress_calls = 0usize;
    log.reembed_prepare(&v2, &mut |_done, _total| progress_calls += 1).unwrap();
    let stats = log.reembed_finalize_gc(&v2).unwrap();

    // New-id vectors cover all events; old-id vectors + entity vectors are GC'd.
    assert_eq!(log.vectors_for_model(MOCK_V2_MODEL_ID).unwrap().len(), 2);
    assert!(log.vectors_for_model(MOCK_MODEL_ID).unwrap().is_empty(), "old vectors GC'd");
    assert_eq!(log.entity_vectors_for_model(MOCK_V2_MODEL_ID).unwrap().len(), 1, "entity vector migrated (U8)");
    assert!(log.entity_vectors_for_model(MOCK_MODEL_ID).unwrap().is_empty(), "old entity vectors GC'd (U8)");
    assert!(progress_calls >= 2, "progress reported per embeddable event");
    assert_eq!(stats.gc_removed, 2, "2 old vector rows removed");
}
```

Note: `entity_vectors_for_model` is currently private (`:4906`). Step 3 makes it `pub` (it is a pure read, symmetric with the already-`pub` `vectors_for_model`).

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p bossclaw-core --test recall reembed_prepare -- --nocapture`
Expected: FAIL — `no method named reembed_prepare` / `entity_vectors_for_model is private`.

- [ ] **Step 3: Extract the shared per-event derive helper.** In `crates/bossclaw-core/src/log.rs`, add a private helper next to `rederive_pending` (`:1133`):

```rust
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
```

Then rewrite the body of `rederive_pending` (`:1133-1174`) to use it (preserving its best-effort contract exactly):

```rust
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
```

- [ ] **Step 4: Make `entity_vectors_for_model` public.** Change `fn entity_vectors_for_model` (`:4906`) to `pub fn entity_vectors_for_model` (pure read, symmetric with `vectors_for_model`). Its doc already describes it; no body change.

- [ ] **Step 5: Add the migration primitives.** In `crates/bossclaw-core/src/log.rs`, after `reembed_migration` (`:1908`):

```rust
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
        let embeddable = self.collect_embeddable_events_ordered()?;
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
        let missing = self.count_missing_vectors(embedder.model_id())?;
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
        conn.execute("DELETE FROM vectors WHERE model_id != ?1", rusqlite::params![keep_model_id])?;
        let removed = conn.changes() as usize;
        conn.execute("DELETE FROM entity_vectors WHERE model_id != ?1", rusqlite::params![keep_model_id])?;
        Ok(removed)
    }

    /// Count embeddable-with-text events that still lack a `vectors` row for `model_id`. Reuses the
    /// authoritative embeddable-with-text list so it and the completeness contract cannot drift.
    fn count_missing_vectors(&self, model_id: &str) -> Result<usize, BossclawError> {
        let embeddable = self.collect_embeddable_events_ordered()?; // (event_id, text), text non-empty
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
```

- [ ] **Step 6: Run, expect PASS**

Run: `cargo test -p bossclaw-core --test recall reembed_prepare -- --nocapture`
Expected: PASS (both tests).

- [ ] **Step 7: Confirm the existing migration tests still pass** (the `embed_and_upsert` extraction must be behaviour-preserving):

Run: `cargo test -p bossclaw-core --test recall reembed_migration -- --nocapture`
Expected: PASS (all four pre-existing `reembed_migration_*` tests).

- [ ] **Step 8: Commit**

```bash
git add crates/bossclaw-core/src/log.rs crates/bossclaw-core/tests/recall.rs
git commit -m "feat(bossclaw-core): all-or-nothing migration primitives + entity-vector migration (I5, U8)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task A3: Proto — `SetActiveModel` + `ModelStatus` ops (U4, U6)

Resolves spec §12 OQ1. **We add a dedicated `ModelStatus` op rather than extending `Response::Status`** (see Open-Questions section for the rationale — zero blast radius on every existing `Status` consumer + the invariance guard).

**Files:**
- Modify: `crates/bossclawd-proto/src/types.rs` (new wire structs/enums)
- Modify: `crates/bossclawd-proto/src/lib.rs` (`Request`/`Response` variants + roundtrip test)

- [ ] **Step 1: Write the failing test** — add to the `request_response_serde_roundtrip` list additions and a new focused test in `crates/bossclawd-proto/src/lib.rs` (in `mod protocol_tests`):

```rust
    /// The rung-2 ops round-trip through JSON (SetActiveModel request + every ModelState arm).
    #[test]
    fn rung2_ops_serde_roundtrip() {
        use crate::types::{ModelStateWire, ModelStatusWire, ReindexProgressWire};

        let req = Request::SetActiveModel {
            onboarded: true,
            model_id: "minishlab/potion-multilingual-128M".to_string(),
            safetensors_sha: "deadbeef".to_string(),
        };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(req, back);

        let status = Request::ModelStatus { onboarded: false };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&status).unwrap()).unwrap();
        assert_eq!(status, back);

        let responses = vec![
            Response::ModelStatus(ModelStatusWire { state: ModelStateWire::Ok, reindex: None }),
            Response::ModelStatus(ModelStatusWire {
                state: ModelStateWire::Missing { expected: "minishlab/potion-multilingual-128M".to_string() },
                reindex: Some(ReindexProgressWire { done: 220, total: 1043 }),
            }),
            Response::ModelStatus(ModelStatusWire {
                state: ModelStateWire::Mismatch {
                    expected: "minishlab/potion-multilingual-128M".to_string(),
                    loaded: "deadbeef".to_string(),
                },
                reindex: None,
            }),
        ];
        for resp in responses {
            let back: Response = serde_json::from_slice(&serde_json::to_vec(&resp).unwrap()).unwrap();
            assert_eq!(resp, back, "ModelStatus round-trips: {resp:?}");
        }
    }
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p bossclawd-proto rung2_ops_serde_roundtrip`
Expected: FAIL — `no variant SetActiveModel` / `ModelStatusWire not found`.

- [ ] **Step 3: Add the wire types** to `crates/bossclawd-proto/src/types.rs` (after `EngineStatusWire`, `:567`):

```rust
/// The loaded-vs-intended embedder state for the language-pack UI (invariants I3/U5/U6). `Ok` when
/// the daemon serves the intended model; `Missing`/`Mismatch` are the fail-loud states (recall
/// refuses rather than silently serving English) that the Settings card surfaces as re-download.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum ModelStateWire {
    Ok,
    /// The signed intent names a model whose folder is absent (e.g. profile copied to a new machine).
    Missing { expected: String },
    /// The intended model's folder exists but its safetensors sha does not match the signed sha.
    Mismatch { expected: String, loaded: String },
}

/// Re-index progress during a background migration (U6). `done`/`total` count embeddable events.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReindexProgressWire {
    pub done: u64,
    pub total: u64,
}

/// The language-pack status the Settings card polls (U6): the model state + optional live re-index
/// progress. Distinct from [`EngineStatusWire`] so every existing `Status` consumer is untouched.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ModelStatusWire {
    pub state: ModelStateWire,
    pub reindex: Option<ReindexProgressWire>,
}
```

- [ ] **Step 4: Add the `Request` variants** to `crates/bossclawd-proto/src/lib.rs` (after `EnableCloudReasoner`, `:142`):

```rust
    /// `EngineHandle::set_active_model` → `engine_set_active_model` (rung 2). Enables the
    /// multilingual language pack: validate folder+sha, write signed consent + in-progress marker,
    /// run the crash-safe re-embed migration in the background. Progress is polled via `ModelStatus`.
    SetActiveModel { onboarded: bool, model_id: String, safetensors_sha: String },
    /// `EngineHandle::model_status` → `engine_model_status` (rung 2). Loaded-vs-intended model state
    /// + live re-index progress. Polled by the Settings language-pack card.
    ModelStatus { onboarded: bool },
```

- [ ] **Step 5: Add the `Response` variant** to `crates/bossclawd-proto/src/lib.rs` (after `ReasonerReady(bool)`, `:206`); also add `ModelStatusWire` to the `use crate::types::{...}` import (`:33-37`):

```rust
    /// `ModelStatus` result (rung 2).
    ModelStatus(ModelStatusWire),
```

- [ ] **Step 6: Run, expect PASS**

Run: `cargo test -p bossclawd-proto`
Expected: PASS (the new test + the pre-existing proto tests).

- [ ] **Step 7: Commit**

```bash
git add crates/bossclawd-proto/src/lib.rs crates/bossclawd-proto/src/types.rs
git commit -m "feat(bossclawd-proto): SetActiveModel + ModelStatus wire ops (U4, U6)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task A4: Pull-based, sha-verified, swappable production embedder (U1, U2, I1, I3, I4, I7)

Rewrite `ResourceModel2Vec` (`crates/bossclawd/src/engine/embed.rs`) so the daemon resolves *which* model to load (env → signed record → bundled default), sha-verifies the signed multilingual model at load (I4), fails loud on missing/mismatch (I3), and holds a **swappable** cache so a migration can flip the served model without a restart. A test-only injectable loader lets every unit test run hermetically (no real safetensors). The pre-existing simple `ResourceModel2Vec::new(dir)` constructor is preserved verbatim so **memharness is byte-for-byte unaffected** (it resolves env itself and passes a fixed dir).

**Files:**
- Rewrite: `crates/bossclawd/src/engine/embed.rs`
- Test: inline `#[cfg(test)]` in `embed.rs`

- [ ] **Step 1: Write the failing tests** (replace the `#[cfg(test)]` block at the bottom of `embed.rs`, `:50-67`, with these):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bossclaw_core::{EventLog, LanguagePackRecord, MigrationState, MockEmbedder};
    use ed25519_dalek::SigningKey;
    use std::sync::Arc;

    const DEK: [u8; 32] = [42u8; 32];
    const KEY: [u8; 32] = [7u8; 32];

    fn open_log(dir: &std::path::Path) -> EventLog {
        EventLog::open(&dir.join("m.db"), &DEK, SigningKey::from_bytes(&KEY)).unwrap()
    }

    /// A loader that yields a MockEmbedder reporting the requested id, so resolution can be tested
    /// without real weights. It also records which (dir, id) it was asked to load.
    fn mock_loader() -> LoaderFn {
        Arc::new(|_dir: &std::path::Path, id: &str| {
            Ok(Arc::new(MockEmbedder::new(8)) as Arc<dyn bossclaw_core::Embedder>).map(|e| {
                // Wrap so model_id() reports the RESOLVED id, not mock-v1.
                Arc::new(IdOverride { inner: e, id: id.to_string() }) as Arc<dyn bossclaw_core::Embedder>
            })
        })
    }

    struct IdOverride { inner: Arc<dyn bossclaw_core::Embedder>, id: String }
    impl bossclaw_core::Embedder for IdOverride {
        fn embed(&self, t: &[String]) -> Result<Vec<Vec<f32>>, bossclaw_core::BossclawError> { self.inner.embed(t) }
        fn dim(&self) -> usize { self.inner.dim() }
        fn model_id(&self) -> &str { &self.id }
    }

    /// Write a valid model folder (fake safetensors + air-model.json) under `root/<id>` and return
    /// its sha256 so the signed record can bind to it.
    fn stage_model(root: &std::path::Path, id: &str, bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.safetensors"), bytes).unwrap();
        let sha = hex::encode(Sha256::digest(bytes));
        std::fs::write(
            dir.join("air-model.json"),
            serde_json::to_vec(&serde_json::json!({ "model_id": id, "safetensors_sha": sha })).unwrap(),
        )
        .unwrap();
        sha
    }

    #[test]
    fn no_record_resolves_bundled_english_default_i7() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let bundled = tmp.path().join("models/potion-base-8M");
        std::fs::create_dir_all(&bundled).unwrap();
        let p = ResourceModel2Vec::with_resolution(
            None, bundled.clone(), tmp.path().join("models"), MODEL_ID.to_string(),
        )
        .with_loader_for_test(mock_loader());
        let e = p.embedder_for(&log).unwrap();
        assert_eq!(e.model_id(), MODEL_ID, "no record → bundled English id (I7)");
        assert_eq!(p.model_state(), bossclaw_core_model_state_ok());
    }

    #[test]
    fn env_override_is_highest_priority() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let envdir = tmp.path().join("envmodel");
        std::fs::create_dir_all(&envdir).unwrap();
        let p = ResourceModel2Vec::with_resolution(
            Some(envdir.clone()), tmp.path().join("models/potion-base-8M"),
            tmp.path().join("models"), MODEL_ID.to_string(),
        )
        .with_loader_for_test(mock_loader());
        // Even with a Complete multilingual record present, env wins (dev/harness override, I1).
        let sha = stage_model(&tmp.path().join("models"), "ml/v1", b"weights");
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/v1".into(), safetensors_sha: sha, migration: MigrationState::Complete,
            consented_at: "t".into(),
        }).unwrap();
        let e = p.embedder_for(&log).unwrap();
        assert_eq!(e.model_id(), MODEL_ID, "env override wins over the signed record (I1)");
    }

    #[test]
    fn complete_record_loads_multilingual_when_sha_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let root = tmp.path().join("models");
        let sha = stage_model(&root, "ml/v1", b"weights");
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/v1".into(), safetensors_sha: sha, migration: MigrationState::Complete,
            consented_at: "t".into(),
        }).unwrap();
        let p = ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, MODEL_ID.to_string())
            .with_loader_for_test(mock_loader());
        assert_eq!(p.embedder_for(&log).unwrap().model_id(), "ml/v1");
    }

    #[test]
    fn complete_record_missing_folder_fails_loud_i3() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let root = tmp.path().join("models");
        std::fs::create_dir_all(&root).unwrap();
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/gone".into(), safetensors_sha: "abc".into(),
            migration: MigrationState::Complete, consented_at: "t".into(),
        }).unwrap();
        let p = ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, MODEL_ID.to_string())
            .with_loader_for_test(mock_loader());
        let err = p.embedder_for(&log).unwrap_err();
        assert!(matches!(err, EngineOpError::Embedder(_)), "missing folder must refuse (I3): {err:?}");
        assert!(matches!(p.model_state(), ModelState::Missing { .. }), "state reflects Missing");
    }

    #[test]
    fn complete_record_sha_mismatch_fails_loud_i4() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let root = tmp.path().join("models");
        stage_model(&root, "ml/v1", b"real-weights"); // real sha
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/v1".into(), safetensors_sha: "WRONGSHA".into(),
            migration: MigrationState::Complete, consented_at: "t".into(),
        }).unwrap();
        let p = ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, MODEL_ID.to_string())
            .with_loader_for_test(mock_loader());
        let err = p.embedder_for(&log).unwrap_err();
        assert!(matches!(err, EngineOpError::Embedder(_)), "sha mismatch must refuse (I4): {err:?}");
        assert!(matches!(p.model_state(), ModelState::Mismatch { .. }));
    }

    #[test]
    fn in_progress_record_serves_bundled_old_model() {
        let tmp = tempfile::tempdir().unwrap();
        let log = open_log(tmp.path());
        let root = tmp.path().join("models");
        let bundled = root.join("potion-base-8M");
        std::fs::create_dir_all(&bundled).unwrap();
        stage_model(&root, "ml/v1", b"weights");
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "ml/v1".into(), safetensors_sha: "x".into(),
            migration: MigrationState::InProgress, consented_at: "t".into(),
        }).unwrap();
        let p = ResourceModel2Vec::with_resolution(None, bundled, root, MODEL_ID.to_string())
            .with_loader_for_test(mock_loader());
        assert_eq!(p.embedder_for(&log).unwrap().model_id(), MODEL_ID,
            "during migration the OLD English model still serves");
    }

    fn bossclaw_core_model_state_ok() -> ModelState { ModelState::Ok }
}
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p bossclawd --lib engine::embed`
Expected: FAIL — `with_resolution` / `with_loader_for_test` / `ModelState` / `model_state` not found.

- [ ] **Step 3: Rewrite `crates/bossclawd/src/engine/embed.rs`** (full replacement of the production section; keep the `//!` header, adjust it):

```rust
//! The embedder seam: the production provider resolves WHICH model to load itself (invariant I1) —
//! env override (dev/harness) → the signed `language_pack` record → the bundled English default —
//! sha-verifies the signed multilingual model at load (I4), fails loud on missing/mismatch (I3),
//! and holds a SWAPPABLE cache so a consent-gated migration can flip the served model without a
//! daemon restart. The simple `ResourceModel2Vec::new(dir)` constructor is preserved for memharness
//! + tests (a fixed dir, no resolution). A `MockEmbedder` provider backs hermetic tests.

use crate::engine::EngineOpError;
use bossclaw_core::{Embedder, EventLog, MigrationState, Model2Vec};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The bundled English default model id (invariant I7: the default path reports THIS id and writes
/// identical vectors). Was the sole hardcoded seam pre-rung-2; now the default of a resolution order.
pub const MODEL_ID: &str = "minishlab/potion-base-8M";

/// The local id-binding file the downloader writes and the resolver reads (invariant I4).
const ID_BINDING_FILE: &str = "air-model.json";
/// The weights filename whose sha256 is the identity guard.
const SAFETENSORS_FILE: &str = "model.safetensors";

/// The loaded-vs-intended state surfaced to the UI (mirrors `ModelStateWire`; invariants I3/U5/U6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    Ok,
    Missing { expected: String },
    Mismatch { expected: String, loaded: String },
}

/// A pluggable model loader (production = `Model2Vec::from_pretrained`; tests inject a mock). Takes
/// the physical dir + the id to report, returns a built embedder.
pub type LoaderFn =
    Arc<dyn Fn(&Path, &str) -> Result<Arc<dyn Embedder>, EngineOpError> + Send + Sync>;

/// Builds (and caches) the embedder. Called on first ingest/recall, never at startup.
pub trait EmbedderProvider: Send + Sync {
    /// Legacy build with no resolution context (mocks + the fixed-dir constructor).
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError>;

    /// Resolution-aware build (production): reads the signed `language_pack` record from `log` to
    /// decide which model to load. Default: ignore `log`, delegate to [`Self::embedder`].
    fn embedder_for(&self, _log: &EventLog) -> Result<Arc<dyn Embedder>, EngineOpError> {
        self.embedder()
    }

    /// Build the migration TARGET model (load `<root>/<model_id>` + verify `sha`) WITHOUT publishing
    /// it to the live cache. Default: unsupported (mocks don't migrate).
    fn build_candidate(&self, _model_id: &str, _sha: &str) -> Result<Arc<dyn Embedder>, EngineOpError> {
        Err(EngineOpError::Embedder("this provider does not support model swap".into()))
    }

    /// Atomically replace the live cache with a candidate built by [`Self::build_candidate`]. Default: no-op.
    fn publish(&self, _embedder: Arc<dyn Embedder>) {}

    /// The current loaded-vs-intended state (default `Ok`; the production provider tracks the real one).
    fn model_state(&self) -> ModelState {
        ModelState::Ok
    }

    /// Set / read the live re-index progress (`(done, total)`), for `ModelStatus`. Defaults: no-op / None.
    fn set_reindex(&self, _progress: Option<(u64, u64)>) {}
    fn reindex(&self) -> Option<(u64, u64)> {
        None
    }
}

/// Production embedder provider.
pub struct ResourceModel2Vec {
    /// Resolution inputs (`None` env in the fixed-dir constructor; the fixed dir goes in `bundled_dir`).
    env_override: Option<PathBuf>,
    bundled_dir: PathBuf,
    bundled_id: String,
    data_models_root: PathBuf,
    /// When `true`, `embedder_for` ignores `log` and always loads `bundled_dir` as `bundled_id`
    /// (the `new(dir)` fixed-dir mode memharness uses). When `false`, full resolution runs.
    fixed: bool,
    loader: LoaderFn,
    cell: Mutex<Option<Arc<dyn Embedder>>>,
    state: Mutex<ModelState>,
    reindex: Mutex<Option<(u64, u64)>>,
}

impl ResourceModel2Vec {
    /// FIXED-DIR constructor (memharness + simple callers): always loads `dir` as [`MODEL_ID`], no
    /// signed-record resolution, no swap. Byte-for-byte the pre-rung-2 behaviour.
    pub fn new(model_dir: PathBuf) -> Self {
        Self {
            env_override: None,
            bundled_dir: model_dir.clone(),
            bundled_id: MODEL_ID.to_string(),
            data_models_root: model_dir,
            fixed: true,
            loader: default_loader(),
            cell: Mutex::new(None),
            state: Mutex::new(ModelState::Ok),
            reindex: Mutex::new(None),
        }
    }

    /// RESOLUTION-AWARE constructor (the daemon `main.rs`): env override → signed record → bundled.
    pub fn with_resolution(
        env_override: Option<PathBuf>,
        bundled_dir: PathBuf,
        data_models_root: PathBuf,
        bundled_id: String,
    ) -> Self {
        Self {
            env_override,
            bundled_dir,
            bundled_id,
            data_models_root,
            fixed: false,
            loader: default_loader(),
            cell: Mutex::new(None),
            state: Mutex::new(ModelState::Ok),
            reindex: Mutex::new(None),
        }
    }

    /// TEST-ONLY: inject a loader so resolution can be tested without real weights.
    #[cfg(test)]
    pub fn with_loader_for_test(mut self, loader: LoaderFn) -> Self {
        self.loader = loader;
        self
    }

    /// Resolve `(dir, id)` to load, or a fail-loud `ModelState`. Env highest (I1); else the signed
    /// record when `Complete` (verify sha, I3/I4); else the bundled default (I7). An `InProgress`
    /// record keeps serving the bundled/old model until the migration flips it to `Complete`.
    fn resolve(&self, log: &EventLog) -> Result<(PathBuf, String), EngineOpError> {
        if let Some(env) = &self.env_override {
            let id = read_binding_id(env).unwrap_or_else(|| self.bundled_id.clone());
            return Ok((env.clone(), id));
        }
        let rec = log
            .language_pack_record()
            .map_err(|e| EngineOpError::Embedder(e.to_string()))?;
        match rec {
            Some(r) if r.migration == MigrationState::Complete => {
                let dir = self.data_models_root.join(&r.model_id);
                let weights = dir.join(SAFETENSORS_FILE);
                if !weights.is_file() {
                    *self.state.lock().expect("model state poisoned") =
                        ModelState::Missing { expected: r.model_id.clone() };
                    return Err(EngineOpError::Embedder(format!(
                        "language pack '{}' is enabled but its files are missing — re-download",
                        r.model_id
                    )));
                }
                let actual = sha256_file(&weights).map_err(|e| EngineOpError::Embedder(e))?;
                if actual != r.safetensors_sha {
                    *self.state.lock().expect("model state poisoned") = ModelState::Mismatch {
                        expected: r.model_id.clone(),
                        loaded: actual,
                    };
                    return Err(EngineOpError::Embedder(format!(
                        "language pack '{}' failed its integrity check — re-download",
                        r.model_id
                    )));
                }
                *self.state.lock().expect("model state poisoned") = ModelState::Ok;
                Ok((dir, r.model_id))
            }
            // InProgress (old model still serving) OR no record → bundled English default.
            _ => {
                *self.state.lock().expect("model state poisoned") = ModelState::Ok;
                Ok((self.bundled_dir.clone(), self.bundled_id.clone()))
            }
        }
    }
}

impl EmbedderProvider for ResourceModel2Vec {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError> {
        // Fixed-dir mode: no log needed.
        let mut guard = self.cell.lock().expect("embedder cell poisoned");
        if let Some(e) = guard.as_ref() {
            return Ok(e.clone());
        }
        let e = (self.loader)(&self.bundled_dir, &self.bundled_id)?;
        *guard = Some(e.clone());
        Ok(e)
    }

    fn embedder_for(&self, log: &EventLog) -> Result<Arc<dyn Embedder>, EngineOpError> {
        if self.fixed {
            return self.embedder();
        }
        {
            let guard = self.cell.lock().expect("embedder cell poisoned");
            if let Some(e) = guard.as_ref() {
                return Ok(e.clone());
            }
        }
        let (dir, id) = self.resolve(log)?; // sets ModelState + returns Err on missing/mismatch (I3)
        let e = (self.loader)(&dir, &id)?;
        *self.cell.lock().expect("embedder cell poisoned") = Some(e.clone());
        Ok(e)
    }

    fn build_candidate(&self, model_id: &str, sha: &str) -> Result<Arc<dyn Embedder>, EngineOpError> {
        let dir = self.data_models_root.join(model_id);
        let weights = dir.join(SAFETENSORS_FILE);
        if !weights.is_file() {
            return Err(EngineOpError::Embedder(format!("model '{model_id}' files are missing")));
        }
        let actual = sha256_file(&weights).map_err(EngineOpError::Embedder)?;
        if actual != sha {
            return Err(EngineOpError::Embedder(format!("model '{model_id}' failed its integrity check")));
        }
        (self.loader)(&dir, model_id)
    }

    fn publish(&self, embedder: Arc<dyn Embedder>) {
        *self.cell.lock().expect("embedder cell poisoned") = Some(embedder);
        *self.state.lock().expect("model state poisoned") = ModelState::Ok;
    }

    fn model_state(&self) -> ModelState {
        self.state.lock().expect("model state poisoned").clone()
    }

    fn set_reindex(&self, progress: Option<(u64, u64)>) {
        *self.reindex.lock().expect("reindex poisoned") = progress;
    }

    fn reindex(&self) -> Option<(u64, u64)> {
        *self.reindex.lock().expect("reindex poisoned")
    }
}

/// The production loader: `Model2Vec::from_pretrained` (unchanged behaviour for the fixed path).
fn default_loader() -> LoaderFn {
    Arc::new(|dir: &Path, id: &str| {
        Model2Vec::from_pretrained(dir, id)
            .map(|m| Arc::new(m) as Arc<dyn Embedder>)
            .map_err(|e| EngineOpError::Embedder(e.to_string()))
    })
}

/// sha256 a file, hex-encoded (matches the downloader's `air-model.json` binding).
fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// Read the `model_id` from a folder's `air-model.json`, if present (used for the env-override id).
fn read_binding_id(dir: &Path) -> Option<String> {
    let raw = std::fs::read(dir.join(ID_BINDING_FILE)).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    v.get("model_id").and_then(|s| s.as_str()).map(str::to_string)
}

#[cfg(test)]
pub struct MockEmbedderProvider {
    dim: usize,
}

#[cfg(test)]
impl MockEmbedderProvider {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

#[cfg(test)]
impl EmbedderProvider for MockEmbedderProvider {
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError> {
        Ok(Arc::new(bossclaw_core::MockEmbedder::new(self.dim)))
    }
}
```

Then paste the `#[cfg(test)] mod tests` block from Step 1 below this. Add `sha2 = "0.10"` and `hex = "0.4"` to `crates/bossclawd/Cargo.toml` `[dependencies]` if not already present (check first: `grep -n 'sha2\|hex' crates/bossclawd/Cargo.toml`).

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p bossclawd --lib engine::embed`
Expected: PASS (all six resolution tests).

- [ ] **Step 5: Confirm memharness's provider construction still compiles** (it calls `ResourceModel2Vec::new(model_dir)`, unchanged):

Run: `cargo build -p memharness`
Expected: builds clean.

- [ ] **Step 6: Commit**

```bash
git add crates/bossclawd/src/engine/embed.rs crates/bossclawd/Cargo.toml
git commit -m "feat(bossclawd): pull-based, sha-verified, swappable embedder provider (U1/U2/I1/I3/I4/I7)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task A5: Wire resolution into the engine call sites + `main.rs` (U1, U5)

`EngineHandle` must call `embedder_for(&log)` (not `embedder()`) at the three call sites so the daemon serves the resolved model, and `main.rs` must build the resolution-aware provider.

**Files:**
- Modify: `crates/bossclawd/src/engine/mod.rs` (`run_ingest` `:452`, `ensure_indexed` `:502`; add `EngineHandle::model_state()` passthrough)
- Modify: `crates/bossclawd/src/main.rs` (`:121-133` provider construction; `:202-206` resolve helper)
- Test: `crates/bossclawd/src/engine/mod.rs` inline `#[cfg(test)]`

- [ ] **Step 1: Write the failing test** (add to the existing `#[cfg(test)] mod tests` in `mod.rs`; it proves recall refuses loudly when the signed model is Missing — I3/U5):

```rust
    #[tokio::test]
    async fn recall_refuses_loudly_when_signed_model_missing() {
        use crate::engine::embed::ResourceModel2Vec;
        use bossclaw_core::{LanguagePackRecord, MigrationState};
        // Build a resolution-aware provider whose data root has NO folder for the enabled model.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("models");
        std::fs::create_dir_all(root.join("potion-base-8M")).unwrap();
        let provider = std::sync::Arc::new(ResourceModel2Vec::with_resolution(
            None, root.join("potion-base-8M"), root, crate::engine::embed::MODEL_ID.to_string(),
        ));
        let handle = test_handle_with_provider(tmp.path().to_path_buf(), provider);
        // Onboard + record a Complete multilingual intent whose folder is absent.
        let log = handle.get_or_open(true).await.unwrap();
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "minishlab/potion-multilingual-128M".into(),
            safetensors_sha: "abc".into(),
            migration: MigrationState::Complete,
            consented_at: "t".into(),
        }).unwrap();
        let err = handle.recall(true, "anything".into(), 5).await.unwrap_err();
        assert!(matches!(err, crate::engine::EngineOpError::Embedder(_)),
            "recall must refuse when the signed model is missing (I3), got {err:?}");
    }
```

`test_handle_with_provider` is a helper this task adds (Step 3). The onboarding gate needs an identity file; reuse whatever the existing `mod.rs` tests use to make `get_or_open(true)` succeed (search the file for the existing test helper that builds an onboarded `EngineHandle` — e.g. a `test_handle(dir)` that seeds `identity.json` and an in-memory vault; add a `_with_provider` variant that takes the provider).

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p bossclawd --lib recall_refuses_loudly_when_signed_model_missing`
Expected: FAIL — `test_handle_with_provider` not found.

- [ ] **Step 3: Add the test helper** mirroring the existing onboarded-handle builder, parameterised on the provider. (In the `#[cfg(test)]` module of `mod.rs`, next to the existing handle builder.)

```rust
    fn test_handle_with_provider(
        home: std::path::PathBuf,
        provider: std::sync::Arc<dyn crate::engine::embed::EmbedderProvider>,
    ) -> EngineHandle {
        // Seed identity so `get_or_open(true)` opens (mirror the existing onboarded-handle helper).
        std::fs::write(home.join("identity.json"), b"{\"did\":\"did:test\"}").unwrap();
        let vault = std::sync::Arc::new(crate::server::TestVault::default()) as std::sync::Arc<dyn crate::secrets::SecretsVault>;
        let reasoner = std::sync::Arc::new(crate::server::TestReasonerProvider);
        EngineHandle::new(vault, home, provider, reasoner)
    }
```

(If `TestVault`/`TestReasonerProvider` are not visible from `mod.rs` tests, reuse the exact vault/reasoner the pre-existing `mod.rs` handle-builder test helper already constructs — copy its two lines.)

- [ ] **Step 4: Switch the call sites to `embedder_for`.** In `run_ingest` (`:451-454`), inside the `spawn_blocking` closure, change:

```rust
            let embedder = provider.embedder()?; // lazy, cached — built BEFORE the walk
```
to
```rust
            let embedder = provider.embedder_for(&log)?; // resolved (env → signed record → bundled)
```

(`log` is already moved into that closure.) In `ensure_indexed` (`:502`), change:

```rust
        let embedder = self.embedder_provider.embedder()?;
```
to
```rust
        let embedder = self.embedder_provider.embedder_for(log)?;
```

(`log: &Arc<EventLog>` is the method arg; `&**log` derefs to `&EventLog` — write `embedder_for(log)` and add `use std::ops::Deref;` if the coercion needs it, or `self.embedder_provider.embedder_for(log.as_ref())`.) `evolve_once` calls `ensure_indexed` (`:577`), so it inherits the resolved embedder — no change there.

- [ ] **Step 5: Add a `model_state` passthrough** on `EngineHandle` (used by A6's `model_status`). After `status` (`:387`):

```rust
    /// The loaded-vs-intended model state + live re-index progress (rung 2; U5/U6). A pure read of
    /// the provider's cells; the migration task and the loader guard keep them current.
    pub fn model_state(&self) -> (crate::engine::embed::ModelState, Option<(u64, u64)>) {
        (self.embedder_provider.model_state(), self.embedder_provider.reindex())
    }
```

- [ ] **Step 6: Build the resolution-aware provider in `main.rs`.** Replace `resolve_model_dir` (`:202-206`) with a resolver that returns the pieces, and update the provider construction (`:123`). New `resolve_model_dir` becomes:

```rust
    /// Bundled English model dir: `BOSSCLAWD_MODEL_DIR` (dev/harness override) if set, else the
    /// staged default `<data_dir>/models/potion-base-8M`.
    fn resolve_bundled_model_dir(data_dir: &std::path::Path) -> PathBuf {
        std::env::var_os(ENV_MODEL_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("models/potion-base-8M"))
    }
```

At `:84`, change `let model_dir = resolve_model_dir(&data_dir);` to:

```rust
        let env_override = std::env::var_os(ENV_MODEL_DIR).map(PathBuf::from);
        let bundled_dir = resolve_bundled_model_dir(&data_dir);
        let data_models_root = data_dir.join("models");
```

At `:123`, change:

```rust
        let embedder = Arc::new(bossclawd::engine::embed::ResourceModel2Vec::new(model_dir));
```
to
```rust
        let embedder = Arc::new(bossclawd::engine::embed::ResourceModel2Vec::with_resolution(
            env_override,
            bundled_dir,
            data_models_root,
            bossclawd::engine::embed::MODEL_ID.to_string(),
        ));
```

- [ ] **Step 7: Run, expect PASS**

Run: `cargo test -p bossclawd --lib recall_refuses_loudly_when_signed_model_missing`
Then: `cargo build -p bossclawd`
Expected: PASS + clean build.

- [ ] **Step 8: Commit**

```bash
git add crates/bossclawd/src/engine/mod.rs crates/bossclawd/src/main.rs
git commit -m "feat(bossclawd): serve the resolved model at every call site + fail-loud recall (U1/U5/I3)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task A6: `EngineHandle::set_active_model` + background migration + boot resume (U4, I5, I6)

The consent-gated orchestration: write the signed `InProgress` record, run the crash-safe migration in the background (prepare → flip record `Complete` → publish embedder → GC), and resume an interrupted one on boot. It never auto-migrates on a bare "zero vectors" heuristic (I6).

**Files:**
- Modify: `crates/bossclawd/src/engine/mod.rs` (new methods on `EngineHandle`)
- Modify: `crates/bossclawd/src/main.rs` (call boot resume after reseed, `:139`)
- Test: `crates/bossclawd/src/engine/mod.rs` inline

- [ ] **Step 1: Write the failing tests** (add to `mod.rs` `#[cfg(test)]`). These pin the enable path (I5 completion), consent-gating (I6: no auto-migrate), and boot resume (I6).

```rust
    /// Enable path: SetActiveModel drives the migration to completion (new vectors present, old GC'd,
    /// record Complete). Uses a resolution-aware provider + a staged mock model.
    #[tokio::test]
    async fn set_active_model_migrates_to_completion() {
        use crate::engine::embed::ResourceModel2Vec;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("models");
        std::fs::create_dir_all(root.join("potion-base-8M")).unwrap();
        let (id, sha) = stage_mock_model(&root, "ml/v1"); // helper Step 3
        let provider = std::sync::Arc::new(
            ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, crate::engine::embed::MODEL_ID.to_string())
                .with_loader_for_test(mock_loader_reporting_ids()),
        );
        let handle = test_handle_with_provider(tmp.path().to_path_buf(), provider);
        let log = handle.get_or_open(true).await.unwrap();
        for t in ["ocean waves", "forest trees"] { log.append(mk_test_memory(t)).unwrap(); }
        handle.run_ingest(true).await.unwrap(); // seeds English vectors

        handle.set_active_model(true, id.clone(), sha).await.unwrap();
        // set_active_model spawns a background task; await completion via the status poll.
        wait_until_active(&handle, &id).await;

        assert_eq!(log.vectors_for_model(&id).unwrap().len(), 2, "new-id vectors cover all events");
        assert!(log.vectors_for_model(crate::engine::embed::MODEL_ID).unwrap().is_empty(), "old GC'd");
        assert_eq!(log.language_pack_record().unwrap().unwrap().migration, bossclaw_core::MigrationState::Complete);
    }

    /// I6: a bare "zero vectors for the loaded model" state must NOT auto-migrate — only SetActiveModel does.
    #[tokio::test]
    async fn zero_vectors_never_auto_migrates() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_handle(tmp.path().to_path_buf()); // mock provider, no record
        let log = handle.get_or_open(true).await.unwrap();
        log.append(mk_test_memory("lonely event")).unwrap();
        // No SetActiveModel call. A recall/ingest must NOT write a language_pack record.
        let _ = handle.recall(true, "lonely".into(), 5).await;
        assert!(log.language_pack_record().unwrap().is_none(), "no consent → no migration record (I6)");
    }

    /// I6: an interrupted-but-consented migration (InProgress record + partial vectors) resumes on boot.
    #[tokio::test]
    async fn interrupted_migration_resumes_on_boot() {
        use crate::engine::embed::ResourceModel2Vec;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("models");
        std::fs::create_dir_all(root.join("potion-base-8M")).unwrap();
        let (id, sha) = stage_mock_model(&root, "ml/v1");
        // Simulate a crash mid-migration: InProgress record written, English vectors intact, NO new vectors.
        {
            let provider = std::sync::Arc::new(ResourceModel2Vec::new(root.join("potion-base-8M")));
            let handle = test_handle_with_provider(tmp.path().to_path_buf(), provider);
            let log = handle.get_or_open(true).await.unwrap();
            log.append(mk_test_memory("resume me")).unwrap();
            handle.run_ingest(true).await.unwrap();
            log.set_language_pack_record(&bossclaw_core::LanguagePackRecord {
                model_id: id.clone(), safetensors_sha: sha.clone(),
                migration: bossclaw_core::MigrationState::InProgress, consented_at: "t".into(),
            }).unwrap();
        }
        // Fresh handle (new process) with the resolution-aware provider → boot resume.
        let provider = std::sync::Arc::new(
            ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root.clone(), crate::engine::embed::MODEL_ID.to_string())
                .with_loader_for_test(mock_loader_reporting_ids()),
        );
        let handle = test_handle_with_provider(tmp.path().to_path_buf(), provider);
        handle.resume_migration_if_pending(true).await;
        wait_until_active(&handle, &id).await;
        let log = handle.get_or_open(true).await.unwrap();
        assert_eq!(log.language_pack_record().unwrap().unwrap().migration, bossclaw_core::MigrationState::Complete);
        assert_eq!(log.vectors_for_model(&id).unwrap().len(), 1, "resume finished the re-embed");
    }
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p bossclawd --lib set_active_model_migrates_to_completion`
Expected: FAIL — `no method named set_active_model` + missing helpers.

- [ ] **Step 3: Add the test helpers** (`mod.rs` `#[cfg(test)]`). `stage_mock_model` writes a folder + returns `(id, sha)`; `mock_loader_reporting_ids` is the `IdOverride`-style loader (reuse the one from A4's tests — lift it into the shared test module); `mk_test_memory` builds a memory event (reuse the file's existing memory-event helper if present, else define one that mirrors core's `mk_memory_event`); `wait_until_active` polls `model_state` until the cache reports the new id or a bounded timeout.

```rust
    fn stage_mock_model(root: &std::path::Path, id: &str) -> (String, String) {
        use sha2::{Digest, Sha256};
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = b"mock-weights";
        std::fs::write(dir.join("model.safetensors"), bytes).unwrap();
        let sha = hex::encode(Sha256::digest(bytes));
        (id.to_string(), sha)
    }

    async fn wait_until_active(handle: &EngineHandle, expected_id: &str) {
        for _ in 0..200 {
            if let Ok(Some(rec)) = handle.get_or_open(true).await.unwrap().language_pack_record() {
                if rec.migration == bossclaw_core::MigrationState::Complete && rec.model_id == expected_id {
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("migration did not complete within the bound");
    }
```

- [ ] **Step 4: Add `set_active_model`, the migration runner, and boot resume** to `EngineHandle` (`mod.rs`). The runner is factored so the enable path and the boot-resume path share it.

```rust
    /// Enable the multilingual language pack (rung 2; consent-gated — I6). Writes the signed
    /// `InProgress` record (the ONLY authority that starts a GC-bearing migration), then spawns the
    /// crash-safe migration in the background and returns immediately (the UI polls `model_status`).
    /// A folder/sha problem is surfaced synchronously (nothing is written) so the UI shows it at once.
    pub async fn set_active_model(&self, onboarded: bool, model_id: String, safetensors_sha: String) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        // Fail fast if the downloaded folder isn't loadable/verifiable (never write a record we can't honour).
        let _candidate = self.embedder_provider.build_candidate(&model_id, &safetensors_sha)?;
        // Record consent + the in-progress marker (signed). This is what authorizes the migration (I6).
        let record = bossclaw_core::LanguagePackRecord {
            model_id: model_id.clone(),
            safetensors_sha: safetensors_sha.clone(),
            migration: bossclaw_core::MigrationState::InProgress,
            consented_at: now_rfc3339(),
        };
        let log2 = log.clone();
        spawn_blocking(move || log2.set_language_pack_record(&record))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
            .map_err(|e| EngineOpError::Core(e.to_string()))?;
        // Run the migration in the background (see run_language_migration). Errors are surfaced via
        // model_state; the record stays InProgress on failure (retryable).
        self.spawn_migration(model_id, safetensors_sha);
        Ok(())
    }

    /// Spawn the background migration task. Extracted so both the enable path and boot-resume use it.
    fn spawn_migration(self: &Arc<Self>, model_id: String, sha: String) {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.run_language_migration(model_id, sha).await {
                eprintln!("bossclawd: language migration failed (old model still active): {e}");
                this.embedder_provider.set_reindex(None);
            }
        });
    }

    /// The crash-safe, all-or-nothing migration body (invariant I5). Prepare (re-embed new vectors +
    /// entity vectors, count-checked) → flip the signed record to `Complete` (the commit point) →
    /// publish the new embedder (atomic swap) → GC the old rows. On any failure BEFORE the flip:
    /// nothing is GC'd, the record stays InProgress, the old model keeps serving (retryable).
    async fn run_language_migration(&self, model_id: String, sha: String) -> Result<(), EngineOpError> {
        let log = self.get_or_open(true).await.map_err(EngineOpError::Open)?;
        let candidate = self.embedder_provider.build_candidate(&model_id, &sha)?;
        let provider = self.embedder_provider.clone();

        // Stage 1: re-embed (progress-reporting). No GC yet.
        let (log1, cand1, prov1) = (log.clone(), candidate.clone(), provider.clone());
        spawn_blocking(move || {
            let mut on = |done: u64, total: u64| prov1.set_reindex(Some((done, total)));
            log1.reembed_prepare(&*cand1, &mut on)
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
        .map_err(|e| EngineOpError::Core(e.to_string()))?;

        // Commit point: flip the signed record to Complete, then publish the new embedder.
        let done_record = bossclaw_core::LanguagePackRecord {
            model_id: model_id.clone(),
            safetensors_sha: sha,
            migration: bossclaw_core::MigrationState::Complete,
            consented_at: now_rfc3339(),
        };
        let log2 = log.clone();
        spawn_blocking(move || log2.set_language_pack_record(&done_record))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
            .map_err(|e| EngineOpError::Core(e.to_string()))?;
        self.embedder_provider.publish(candidate.clone());

        // Stage 2: GC the old vectors + entity vectors, rebuild indexes under the new model.
        let (log3, cand3) = (log.clone(), candidate);
        spawn_blocking(move || log3.reembed_finalize_gc(&*cand3))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
            .map_err(|e| EngineOpError::Core(e.to_string()))?;

        self.embedder_provider.set_reindex(None);
        // The recall index must reflect the newly-published model.
        *self.indexed.lock().await = false; // force a rebuild on next recall
        Ok(())
    }

    /// Boot-time resume (invariant I6): if a consented `InProgress` migration is recorded, finish it;
    /// if `Complete`, GC any stale rows a crash left behind (idempotent); if absent, do nothing. The
    /// ONLY boot-time migration — there is no un-consented heuristic.
    pub async fn resume_migration_if_pending(self: &Arc<Self>, onboarded: bool) {
        let log = match self.get_or_open(onboarded).await {
            Ok(l) => l,
            Err(_) => return, // not onboarded / open failure — nothing to resume
        };
        let rec = match log.language_pack_record() {
            Ok(Some(r)) => r,
            _ => return,
        };
        match rec.migration {
            bossclaw_core::MigrationState::InProgress => {
                self.spawn_migration(rec.model_id, rec.safetensors_sha);
            }
            bossclaw_core::MigrationState::Complete => {
                // A crash between the flip and the GC can leave stale old-model rows; sweep them.
                let keep = rec.model_id.clone();
                let _ = spawn_blocking(move || log.gc_stale_vectors(&keep)).await;
            }
        }
    }
```

Add a small time helper near the top of `mod.rs` (or reuse an existing RFC3339 helper if one exists — search for `rfc3339`/`chrono` in the crate first):

```rust
    /// Current time as an RFC3339 string (audit stamp for the consent record). Uses the same
    /// timestamp source the rest of the engine uses (search the crate for an existing helper and
    /// reuse it instead of adding a dependency).
    fn now_rfc3339() -> String {
        // If bossclaw-core exposes a timestamp helper, prefer it; otherwise std SystemTime.
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("{secs}") // seconds-since-epoch is sufficient for the audit stamp; swap for a real
                          // RFC3339 formatter if the crate already links one.
    }
```

Note: `set_active_model`/`spawn_migration`/`resume_migration_if_pending` take `self: &Arc<Self>`; callers already hold `Arc<EngineHandle>` (the daemon shares it as an Arc). If a caller holds `&EngineHandle`, adjust to pass the Arc.

- [ ] **Step 5: Call boot resume in `main.rs`.** After `reseed_reasoner_cell(...)` (`:139`), add:

```rust
        // (5b) Resume a consented-but-interrupted language migration (rung 2; I6). No-op unless a
        // signed InProgress record exists. Runs in the background; the UI polls model_status.
        engine.resume_migration_if_pending(onboarded).await;
```

- [ ] **Step 6: Run, expect PASS**

Run: `cargo test -p bossclawd --lib "set_active_model_migrates_to_completion|zero_vectors_never_auto_migrates|interrupted_migration_resumes_on_boot"`
Expected: PASS (all three).

- [ ] **Step 7: Commit**

```bash
git add crates/bossclawd/src/engine/mod.rs crates/bossclawd/src/main.rs
git commit -m "feat(bossclawd): consent-gated crash-safe migration + boot resume (U4/I5/I6)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task A7: Proto dispatch + client + facade for the two new ops (U4, U6)

**Files:**
- Modify: `crates/bossclawd/src/server.rs` (dispatch arms + `model_status_wire`)
- Modify: `crates/bossclawd/src/engine/client.rs` (`set_active_model`, `model_status`)
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (`Engine` facade delegation)
- Test: `crates/bossclawd/tests/roundtrip.rs`

- [ ] **Step 1: Write the failing roundtrip test** (append to `crates/bossclawd/tests/roundtrip.rs`, following its existing spawn-daemon + WireClient pattern):

```rust
#[tokio::test]
async fn model_status_roundtrips_over_the_socket() {
    // A fresh onboarded daemon with no language pack → ModelState::Ok, no reindex.
    let (client, _daemon) = spawn_onboarded_daemon().await; // reuse the file's existing spawn helper
    let status = client.model_status(true).await.expect("model_status");
    assert!(matches!(status.state, bossclawd::engine::embed::ModelState::Ok));
    assert!(status.reindex.is_none());
}
```

If the roundtrip file's helper returns a `WireClient` (not the desktop `EngineClient`), assert on the wire `Response::ModelStatus(ModelStatusWire { state: ModelStateWire::Ok, reindex: None })` instead — match the file's established client shape.

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p bossclawd --test roundtrip model_status_roundtrips`
Expected: FAIL — `no method model_status` / non-exhaustive `dispatch` match on the new `Request` variants.

- [ ] **Step 3: Add the dispatch arms** in `server.rs` `dispatch` (`:255-257`, after `EnableCloudReasoner`):

```rust
        // ── Language pack (rung 2). ──
        Request::SetActiveModel { onboarded, model_id, safetensors_sha } => {
            unit_result(engine.set_active_model(onboarded, model_id, safetensors_sha).await)
        }
        Request::ModelStatus { onboarded } => {
            Response::ModelStatus(model_status_wire(engine.model_status(onboarded).await))
        }
```

`engine.model_status(onboarded)` is a thin `EngineHandle` method (add it to `mod.rs`): it returns `(ModelState, Option<(u64,u64)>)` from `self.model_state()` but gated on onboarding — actually `model_state()` (A5 Step 5) is not gated; wrap it:

```rust
    /// Language-pack status for the UI poll (rung 2). Onboarding-gated only to avoid touching the
    /// engine before onboarding; a not-onboarded daemon reports `Ok`/no-progress.
    pub async fn model_status(&self, onboarded: bool) -> (crate::engine::embed::ModelState, Option<(u64, u64)>) {
        if !onboarded {
            return (crate::engine::embed::ModelState::Ok, None);
        }
        self.model_state()
    }
```

Add the `model_status_wire` mapper in `server.rs` (after `status_wire`, `:342`):

```rust
fn model_status_wire(
    (state, reindex): (crate::engine::embed::ModelState, Option<(u64, u64)>),
) -> bossclawd_proto::types::ModelStatusWire {
    use bossclawd_proto::types::{ModelStateWire, ModelStatusWire, ReindexProgressWire};
    let state = match state {
        crate::engine::embed::ModelState::Ok => ModelStateWire::Ok,
        crate::engine::embed::ModelState::Missing { expected } => ModelStateWire::Missing { expected },
        crate::engine::embed::ModelState::Mismatch { expected, loaded } => {
            ModelStateWire::Mismatch { expected, loaded }
        }
    };
    ModelStatusWire { state, reindex: reindex.map(|(done, total)| ReindexProgressWire { done, total }) }
}
```

Add `ModelStatusWire` (and the enum/struct) to the `use bossclawd_proto::types::{...}` import at `server.rs:25-29`.

- [ ] **Step 4: Add the client methods** in `client.rs` (after the reasoner methods; mirror `status`/`unit`):

```rust
    /// Mirrors `EngineHandle::set_active_model` (rung 2).
    pub async fn set_active_model(&self, onboarded: bool, model_id: String, safetensors_sha: String) -> Result<(), EngineOpError> {
        self.unit(Request::SetActiveModel { onboarded, model_id, safetensors_sha }).await
    }

    /// Mirrors `EngineHandle::model_status` (rung 2). Returns the loaded-vs-intended state + progress.
    pub async fn model_status(&self, onboarded: bool) -> Result<crate::engine::embed::ModelStatus, EngineOpError> {
        match self.request(Request::ModelStatus { onboarded }).await? {
            Response::ModelStatus(w) => Ok(model_status_from_wire(w)),
            other => Err(unexpected(other)),
        }
    }
```

Define the desktop-side `ModelStatus` type + the `from_wire` mapper in `client.rs` (or reuse `embed::ModelState`). Add near the top-level helpers:

```rust
/// Desktop-side language-pack status (client mirror of `ModelStatusWire`).
pub struct ModelStatus {
    pub state: crate::engine::embed::ModelState,
    pub reindex: Option<(u64, u64)>,
}

fn model_status_from_wire(w: bossclawd_proto::types::ModelStatusWire) -> ModelStatus {
    use bossclawd_proto::types::ModelStateWire;
    let state = match w.state {
        ModelStateWire::Ok => crate::engine::embed::ModelState::Ok,
        ModelStateWire::Missing { expected } => crate::engine::embed::ModelState::Missing { expected },
        ModelStateWire::Mismatch { expected, loaded } => {
            crate::engine::embed::ModelState::Mismatch { expected, loaded }
        }
    };
    ModelStatus { state, reindex: w.reindex.map(|p| (p.done, p.total)) }
}
```

Re-export `ModelStatus` from `client` where `EngineClient` is re-exported so the facade can name it. Note `bossclawd::engine::embed::ModelState` must be reachable from `client.rs` — it is (same crate). But `client.rs` (desktop) and `bossclawd` (daemon) are **two different `embed` modules**. The desktop's `client.rs` is `apps/desktop/src-tauri/src/engine/client.rs` and its `ModelState` must be the **desktop** copy. **Correction:** define a desktop `ModelState` mirror in the desktop `apps/desktop/src-tauri/src/engine/mod.rs` (next to the desktop `EngineError`), NOT the daemon's. Use that here. (The daemon `server.rs` uses the daemon's `embed::ModelState`; the desktop `client.rs` uses the desktop mirror — they only meet over `ModelStatusWire`.)

- [ ] **Step 5: Add the desktop `ModelState` mirror + facade delegation.** In `apps/desktop/src-tauri/src/engine/mod.rs`, add a `ModelState` enum (Ok/Missing{expected}/Mismatch{expected,loaded}) mirroring the wire, and on the `Engine` facade (`:241`) delegate:

```rust
    /// Mirrors `EngineClient::set_active_model` (rung 2).
    pub async fn set_active_model(&self, onboarded: bool, model_id: String, safetensors_sha: String) -> Result<(), EngineOpError> {
        self.client.set_active_model(onboarded, model_id, safetensors_sha).await
    }
    /// Mirrors `EngineClient::model_status` (rung 2).
    pub async fn model_status(&self, onboarded: bool) -> Result<client::ModelStatus, EngineOpError> {
        self.client.model_status(onboarded).await
    }
```

Point `client.rs`'s `ModelState` references at this desktop mirror (`crate::engine::ModelState`).

- [ ] **Step 6: Run, expect PASS**

Run: `cargo test -p bossclawd --test roundtrip model_status_roundtrips`
Then: `cargo build -p air_agent_desktop` (or the desktop crate's package name)
Expected: PASS + clean build.

- [ ] **Step 7: Commit**

```bash
git add crates/bossclawd/src/server.rs crates/bossclawd/src/engine/client.rs crates/bossclawd/src/engine/mod.rs apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/src/engine/client.rs
git commit -m "feat: wire SetActiveModel + ModelStatus through daemon dispatch, client, and facade (U4/U6)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task A8: Invariance guard (I7)

Prove the default no-multilingual path is output-identical to pre-change: same `model_id`, same vectors, same index.

**Files:**
- Test: `crates/bossclaw-core/tests/recall.rs`

- [ ] **Step 1: Write the test** (append to `recall.rs`). The core-level invariance is that with no `language_pack` record, `active_model()` and the stored vectors are the base model's — the resolution machinery does not exist in core, so this asserts the record's absence changes nothing about the existing vector pipeline. (The daemon-level resolution invariance is covered by `no_record_resolves_bundled_english_default_i7` in A4.)

```rust
#[test]
fn default_path_is_byte_identical_without_language_pack() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&dir.path().join("m.db"), &DEK, key).unwrap();

    for t in ["ocean waves crashing", "forest trees rustling"] {
        log.append(mk_memory_event(t)).unwrap();
    }
    let v1 = MockEmbedder::new(MID_DIM);
    log.rederive_pending(&v1).unwrap();
    log.set_active_model(v1.model_id(), v1.dim() as u32).unwrap();

    // No language_pack record was ever written.
    assert!(log.language_pack_record().unwrap().is_none());

    // The active model + vector bytes are exactly the base model's (I7 output identity).
    assert_eq!(log.active_model().unwrap().unwrap().active_model_id, MOCK_MODEL_ID);
    let before = log.vectors_for_model(MOCK_MODEL_ID).unwrap();
    assert_eq!(before.len(), 2);

    // Re-reading is stable (index rebuild is deterministic — same rows, same order).
    log.rebuild_indexes(&v1).unwrap();
    let after = log.vectors_for_model(MOCK_MODEL_ID).unwrap();
    assert_eq!(before, after, "vectors are byte-identical across a rebuild on the default path (I7)");
}
```

- [ ] **Step 2: Run, expect PASS** (no new impl — this asserts existing behaviour is preserved)

Run: `cargo test -p bossclaw-core --test recall default_path_is_byte_identical`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/bossclaw-core/tests/recall.rs
git commit -m "test(bossclaw-core): I7 invariance guard — default path unchanged without a language pack

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task A9: Phase A integration gate — the swap machinery end-to-end

The frozen quality A/B (+21 ko / −5 en, `air/rung2-multilingual-measurement-2026-07-06`) is **REUSED, not re-run** — it is a property of the *model*, already measured. This test proves only that the *swap machinery* produces the multilingual vectors, GCs the old ones (`vectors` **and** `entity_vectors`), keeps search working, resumes after a crash, and fails loud on a missing folder.

**Files:**
- Test: `crates/bossclawd/tests/roundtrip.rs` (or a new `crates/bossclawd/tests/language_pack.rs`)

- [ ] **Step 1: Write the failing integration test.** Drive a real in-process daemon (`spawn_for_test`/`test_engine_with_embedder`) with the **resolution-aware provider + injectable mock loader** so no real safetensors are needed; seed EN+KO memory events, run the enable flow, assert the four properties.

```rust
#[tokio::test]
async fn language_pack_enable_migrates_and_search_survives() {
    use bossclaw_core::MigrationState;
    // A daemon whose engine uses a resolution-aware provider + mock loader over a temp models root.
    let (handle, home) = onboarded_handle_with_resolution_provider().await; // helper: A6-style setup
    let root = home.join("models");
    let (id, sha) = stage_mock_model(&root, "minishlab/potion-multilingual-128M");

    let log = handle.get_or_open(true).await.unwrap();
    for t in ["the ocean waves crash on the shore", "바다 파도가 해안에 부딪힌다"] {
        log.append(mk_test_memory(t)).unwrap();
    }
    handle.run_ingest(true).await.unwrap();
    // Seed an entity vector so the entity-migration path is exercised (U8).
    log.derive_entity_vector(&*handle.embedder_provider.embedder_for(&log).unwrap(), "entity:01A", "Aria").unwrap();

    handle.set_active_model(true, id.clone(), sha).await.unwrap();
    wait_until_active(&handle, &id).await;

    // One vector per embeddable event under the new id; old id + entity vectors GC'd.
    assert_eq!(log.vectors_for_model(&id).unwrap().len(), 2);
    assert!(log.vectors_for_model(crate::engine::embed::MODEL_ID).unwrap().is_empty());
    assert_eq!(log.entity_vectors_for_model(&id).unwrap().len(), 1, "entity vector migrated (U8)");
    assert!(log.entity_vectors_for_model(crate::engine::embed::MODEL_ID).unwrap().is_empty());
    assert_eq!(log.language_pack_record().unwrap().unwrap().migration, MigrationState::Complete);

    // Search still works in both languages (machinery, not quality — the mock embeds both).
    assert!(!handle.recall(true, "ocean waves".into(), 5).await.unwrap().is_empty());
    assert!(!handle.recall(true, "바다 파도".into(), 5).await.unwrap().is_empty());
}
```

Add the fail-loud + resume assertions as two more tests in the same file, reusing A6's `interrupted_migration_resumes_on_boot` shape and A5's missing-folder shape but driven through `set_active_model`/`resume_migration_if_pending`.

- [ ] **Step 2: Run, expect FAIL then implement helpers** (`onboarded_handle_with_resolution_provider`, reused `stage_mock_model`/`wait_until_active`/`mk_test_memory` — lift the A6 helpers into a shared `tests/common` or duplicate minimally in this file).

Run: `cargo test -p bossclawd --test roundtrip language_pack_enable_migrates`
Expected: FAIL then PASS after helpers land.

- [ ] **Step 3: Phase A gate — run the full workspace gate**

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: clean clippy, all tests green. **This is the Phase A gate: do not proceed to Phase B until both are green.**

- [ ] **Step 4: Commit**

```bash
git add crates/bossclawd/tests/
git commit -m "test(bossclawd): integration gate — language-pack swap machinery end-to-end (U4/U8/I3/I5/I6)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

# Phase B — App downloader + UI

## Task B1: Downloader — preflight → fetch → sha-verify → atomic install → id-binding (U3, I4)

**Files:**
- Create: `apps/desktop/src-tauri/src/engine/language_pack.rs`
- Modify: `apps/desktop/src-tauri/src/engine/mod.rs` (add `pub mod language_pack;`)
- Modify: `apps/desktop/src-tauri/Cargo.toml` (add `fs2 = "0.4"` for the disk preflight)
- Test: inline `#[cfg(test)]` in `language_pack.rs`

- [ ] **Step 1: Write the failing tests** (hermetic — verify/install/binding operate on local temp files; no network). Create `apps/desktop/src-tauri/src/engine/language_pack.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_file_rejects_a_corrupt_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("model.safetensors");
        std::fs::write(&p, b"corrupt").unwrap();
        let err = verify_file(&p, "0000000000000000000000000000000000000000000000000000000000000000").unwrap_err();
        assert!(err.contains("check failed"), "{err}");
    }

    #[test]
    fn verify_file_accepts_a_matching_sha() {
        use sha2::{Digest, Sha256};
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("tokenizer.json");
        let bytes = b"{}";
        std::fs::write(&p, bytes).unwrap();
        let sha = hex::encode(Sha256::digest(bytes));
        assert!(verify_file(&p, &sha).is_ok());
    }

    #[test]
    fn install_verified_atomically_renames_and_writes_binding() {
        use sha2::{Digest, Sha256};
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".tmp-abc");
        std::fs::create_dir_all(&staging).unwrap();
        let weights = b"weights";
        std::fs::write(staging.join("model.safetensors"), weights).unwrap();
        std::fs::write(staging.join("tokenizer.json"), b"tok").unwrap();
        std::fs::write(staging.join("config.json"), b"cfg").unwrap();
        let sha = hex::encode(Sha256::digest(weights));
        let dest = tmp.path().join("minishlab/potion-multilingual-128M");

        install_verified(&staging, &dest, "minishlab/potion-multilingual-128M", &sha).unwrap();

        assert!(dest.join("model.safetensors").is_file(), "atomically renamed into place");
        assert!(!staging.exists(), "staging dir consumed by the rename");
        let binding: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dest.join("air-model.json")).unwrap()).unwrap();
        assert_eq!(binding["model_id"], "minishlab/potion-multilingual-128M");
        assert_eq!(binding["safetensors_sha"], sha, "id-binding written from the VERIFIED sha (I4)");
    }

    #[test]
    fn preflight_refuses_when_not_enough_free_space() {
        let tmp = tempfile::tempdir().unwrap();
        let err = preflight_disk(tmp.path(), u64::MAX).unwrap_err();
        assert!(err.contains("disk space"), "{err}");
        // A tiny requirement passes.
        assert!(preflight_disk(tmp.path(), 1).is_ok());
    }
}
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p air_agent_desktop --lib engine::language_pack`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement the downloader** (above the test module in `language_pack.rs`):

```rust
//! The multilingual language-pack downloader (rung 2, U3). Preflight disk → fetch the 3 files from
//! the pinned GitHub Release into a namespaced temp dir → per-file sha256 verify (fail-closed, rm on
//! mismatch) → atomic temp→rename into `<data_dir>/models/<id>/` → write the `air-model.json`
//! id-binding from the VERIFIED safetensors sha (invariant I4). No `bossclaw-core` dependency: this
//! only prepares files on disk; the daemon enables + migrates via `SetActiveModel`.

use std::path::{Path, PathBuf};

/// The enabled multilingual model id (the folder name under `<data_dir>/models/`).
pub const MULTILINGUAL_MODEL_ID: &str = "minishlab/potion-multilingual-128M";
/// The local id-binding filename (read by the daemon's resolver).
const ID_BINDING_FILE: &str = "air-model.json";
/// GitHub Release asset base (Ops task O1 uploads these three assets under this tag).
const RELEASE_BASE: &str =
    "https://github.com/AgentIdentityRegistry/air-note/releases/download/models-multilingual-128M-v1";
/// Headroom for the ~506 MB download plus a transient copy during the atomic rename (~1.5 GB).
const REQUIRED_FREE_BYTES: u64 = 1_500_000_000;

/// One pinned pack file. `sha256` is filled by Ops task O1 (safetensors cross-verified vs HF LFS).
struct PackFile {
    name: &'static str,
    sha256: &'static str,
}

/// The three files + their pinned sha256 (FILLED by Ops task O1 — placeholders here would fail
/// closed at verify, which is the correct default until O1 pins the real digests).
const PACK_FILES: &[PackFile] = &[
    PackFile { name: "model.safetensors", sha256: "REPLACE_WITH_PINNED_SHA_O1" },
    PackFile { name: "tokenizer.json", sha256: "REPLACE_WITH_PINNED_SHA_O1" },
    PackFile { name: "config.json", sha256: "REPLACE_WITH_PINNED_SHA_O1" },
];

/// Progress callback: `(bytes_done, bytes_total_or_zero)`.
pub type ProgressFn<'a> = dyn FnMut(u64, u64) + Send + 'a;

/// Refuse early if the destination volume lacks `required` free bytes (invariant: fail before any
/// network I/O). Uses `fs2::available_space` on the nearest existing ancestor of `dest_root`.
pub fn preflight_disk(dest_root: &Path, required: u64) -> Result<(), String> {
    let probe = existing_ancestor(dest_root);
    let free = fs2::available_space(&probe).map_err(|e| format!("could not read free disk space: {e}"))?;
    if free < required {
        return Err(format!(
            "not enough disk space (need ~{:.1} GB free, have {:.1} GB)",
            required as f64 / 1e9,
            free as f64 / 1e9
        ));
    }
    Ok(())
}

/// sha256 a file and compare (hex) to `expected`. `Err` (fail-closed) on any mismatch or read error.
pub fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let got = hex::encode(Sha256::digest(&bytes));
    if got != expected {
        return Err(format!("file check failed for {} (got {got}, want {expected})", path.display()));
    }
    Ok(())
}

/// Verify all 3 staged files, then ATOMICALLY rename the staging dir into `dest_dir` and write the
/// id-binding from the verified safetensors sha (I4). Fail-closed: on any verify failure the staging
/// dir is removed and nothing is installed.
pub fn install_verified(staging: &Path, dest_dir: &Path, model_id: &str, safetensors_sha: &str) -> Result<(), String> {
    for f in PACK_FILES {
        let expected = if f.name == "model.safetensors" { safetensors_sha } else { f.sha256 };
        if let Err(e) = verify_file(&staging.join(f.name), expected) {
            let _ = std::fs::remove_dir_all(staging);
            return Err(e);
        }
    }
    if let Some(parent) = dest_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    // If a previous partial install exists, remove it so the rename lands cleanly.
    if dest_dir.exists() {
        std::fs::remove_dir_all(dest_dir).map_err(|e| format!("clear stale {}: {e}", dest_dir.display()))?;
    }
    std::fs::rename(staging, dest_dir).map_err(|e| format!("atomic install rename failed: {e}"))?;
    let binding = serde_json::json!({ "model_id": model_id, "safetensors_sha": safetensors_sha });
    std::fs::write(dest_dir.join(ID_BINDING_FILE), serde_json::to_vec(&binding).map_err(|e| e.to_string())?)
        .map_err(|e| format!("write id-binding: {e}"))?;
    Ok(())
}

/// The full flow: preflight → fetch each file into a namespaced temp dir with progress → install.
/// Returns the installed model id (for the caller to pass to `SetActiveModel` with the sha). Async
/// (reqwest streaming). The safetensors sha comes from `PACK_FILES` and is echoed into the binding.
pub async fn download_and_install(models_root: &Path, on_progress: &mut ProgressFn<'_>) -> Result<(String, String), String> {
    preflight_disk(models_root, REQUIRED_FREE_BYTES)?;
    let staging = models_root.join(format!(".tmp-{}", uuid_like()));
    std::fs::create_dir_all(&staging).map_err(|e| format!("mkdir staging: {e}"))?;
    let client = reqwest::Client::new();
    let mut done: u64 = 0;
    for f in PACK_FILES {
        let url = format!("{RELEASE_BASE}/{}", f.name);
        let resp = client.get(&url).send().await.map_err(|e| format!("download {}: {e}", f.name))?;
        let resp = resp.error_for_status().map_err(|e| format!("download {}: {e}", f.name))?;
        let total = resp.content_length().unwrap_or(0);
        let mut out = std::fs::File::create(staging.join(f.name)).map_err(|e| format!("create {}: {e}", f.name))?;
        let mut stream = resp;
        // Stream chunks so progress + memory stay bounded on the 488 MB safetensors.
        while let Some(chunk) = stream.chunk().await.map_err(|e| format!("read {}: {e}", f.name))? {
            use std::io::Write;
            out.write_all(&chunk).map_err(|e| format!("write {}: {e}", f.name))?;
            done += chunk.len() as u64;
            on_progress(done, total);
        }
    }
    let safetensors_sha = PACK_FILES.iter().find(|f| f.name == "model.safetensors").unwrap().sha256.to_string();
    let dest = models_root.join(MULTILINGUAL_MODEL_ID);
    install_verified(&staging, &dest, MULTILINGUAL_MODEL_ID, &safetensors_sha)?;
    Ok((MULTILINGUAL_MODEL_ID.to_string(), safetensors_sha))
}

/// The nearest existing ancestor of `p` (so preflight can stat a real dir even before `models/` exists).
fn existing_ancestor(p: &Path) -> PathBuf {
    let mut cur = p;
    loop {
        if cur.exists() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return PathBuf::from("/"),
        }
    }
}

/// A collision-resistant temp suffix without adding a uuid dependency (time-nanos + pid).
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{}", std::process::id(), nanos)
}
```

Register the module in `apps/desktop/src-tauri/src/engine/mod.rs`: add `pub mod language_pack;`. Add `fs2 = "0.4"` to `apps/desktop/src-tauri/Cargo.toml` `[dependencies]`.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p air_agent_desktop --lib engine::language_pack`
Expected: PASS (all four tests). (The `PACK_FILES` placeholder shas do not affect these tests — `install_verified`/`verify_file` are driven with locally-computed shas; O1 fills the real pins for the live download path.)

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/engine/language_pack.rs apps/desktop/src-tauri/src/engine/mod.rs apps/desktop/src-tauri/Cargo.toml
git commit -m "feat(desktop): language-pack downloader — preflight, sha-verify, atomic install, id-binding (U3/I4)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task B2: Tauri commands — download + enable + status (U3, U7)

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/engine.rs` (three commands + a DTO)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register in `invoke_handler!`, `:124-236`)
- Test: inline `#[cfg(test)]` in `commands/engine.rs` (DTO shape) — the network path is covered by B1's unit tests; command wiring is validated by the build + the vitest in B5.

- [ ] **Step 1: Add the DTO + commands** in `apps/desktop/src-tauri/src/commands/engine.rs` (mirror `engine_enable_cloud_reasoner` `:582-592` + `engine_ollama_status` `:472-481`):

```rust
/// The language-pack status the Settings card polls (rung 2, U6). Payload-encoded like
/// `engine_ollama_status` so the card can poll without a throw. `state` is one of "ok"/"missing"/
/// "mismatch"; `expected`/`loaded` accompany the fail-loud states; `reindex_done`/`reindex_total`
/// are present only during a background migration.
#[derive(serde::Serialize)]
pub struct ModelStatusDto {
    pub state: String,
    pub expected: Option<String>,
    pub loaded: Option<String>,
    pub reindex_done: Option<u64>,
    pub reindex_total: Option<u64>,
}

/// Download + verify + install the multilingual language pack into `<data_dir>/models/` (U3), then
/// enable it via the daemon's `SetActiveModel` (which runs the consent-gated migration). Returns
/// once enable is ACCEPTED; the card polls `engine_model_status` for re-index progress.
#[tauri::command]
pub async fn engine_download_language_pack(state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    let models_root = state.identity_store.data_dir().join("models"); // data_dir accessor — see note
    let mut noop = |_done: u64, _total: u64| {};
    let (model_id, sha) =
        crate::engine::language_pack::download_and_install(&models_root, &mut noop).await?;
    state.engine.set_active_model(onboarded, model_id, sha).await.map_err(|e| e.to_string())
}

/// Enable an already-downloaded language pack (retry path / re-download recovery). Thin passthrough.
#[tauri::command]
pub async fn engine_set_active_model(model_id: String, safetensors_sha: String, state: State<'_, AppState>) -> Result<(), String> {
    let onboarded = state.identity_store.is_onboarded();
    state.engine.set_active_model(onboarded, model_id, safetensors_sha).await.map_err(|e| e.to_string())
}

/// Poll the loaded-vs-intended model state + re-index progress (U6). Never throws.
#[tauri::command]
pub async fn engine_model_status(state: State<'_, AppState>) -> Result<ModelStatusDto, String> {
    let onboarded = state.identity_store.is_onboarded();
    let status = state.engine.model_status(onboarded).await.map_err(|e| e.to_string())?;
    let (state_str, expected, loaded) = match status.state {
        crate::engine::ModelState::Ok => ("ok".to_string(), None, None),
        crate::engine::ModelState::Missing { expected } => ("missing".to_string(), Some(expected), None),
        crate::engine::ModelState::Mismatch { expected, loaded } => ("mismatch".to_string(), Some(expected), Some(loaded)),
    };
    let (reindex_done, reindex_total) = match status.reindex {
        Some((d, t)) => (Some(d), Some(t)),
        None => (None, None),
    };
    Ok(ModelStatusDto { state: state_str, expected, loaded, reindex_done, reindex_total })
}
```

Note on `data_dir()`: the `AppState`/`IdentityStore` already holds the data dir (main.rs `:56-64` constructs `IdentityStore::new(vault, data_dir)`). If there is no public accessor, add a small `pub fn data_dir(&self) -> &Path` to `IdentityStore` (it stores it), or thread the data dir into `AppState` alongside `engine`. Confirm by grepping `IdentityStore` for the stored dir field; reuse it.

- [ ] **Step 2: Register the commands** in `apps/desktop/src-tauri/src/main.rs` `invoke_handler!` (add after `engine_enable_cloud_reasoner`, `:218`):

```rust
            #[cfg(unix)]
            commands::engine::engine_download_language_pack,
            #[cfg(unix)]
            commands::engine::engine_set_active_model,
            #[cfg(unix)]
            commands::engine::engine_model_status,
```

- [ ] **Step 3: Build**

Run: `cargo build -p air_agent_desktop`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/engine.rs apps/desktop/src-tauri/src/main.rs
git commit -m "feat(desktop): download/enable/status Tauri commands for the language pack (U3/U6/U7)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task B3: Stage English into the data dir + stop pushing `BOSSCLAWD_MODEL_DIR` (I1)

For pull-resolution to work on the app-spawn path, the bundled English model must live at the daemon's default `<data_dir>/models/potion-base-8M` (not only in the read-only resource dir), and the app must **stop** pushing `BOSSCLAWD_MODEL_DIR` (else the env override would always win and block multilingual — I1).

**Files:**
- Modify: `apps/desktop/src-tauri/src/main.rs` (`:73-104` — stage English, drop the env push)
- Modify: `apps/desktop/src-tauri/src/engine/daemon.rs` (`:107-111`, `:119-168` — drop the model-dir env from spawn)

- [ ] **Step 1: Add an idempotent English-staging helper** in `apps/desktop/src-tauri/src/commands/engine.rs` (or a small `engine/staging.rs`), with a unit test:

```rust
/// Copy the bundled English model from `resource_models_dir` into `<data_dir>/models/potion-base-8M`
/// if the destination is missing (idempotent). This makes the daemon's DEFAULT resolution
/// (`<data_dir>/models/potion-base-8M`) work WITHOUT the app pushing `BOSSCLAWD_MODEL_DIR` (I1), so
/// a signed multilingual record can win. Best-effort: a copy failure is logged, not fatal (the app
/// still boots; memory features degrade until the model is present).
pub fn stage_bundled_english(resource_models_dir: &std::path::Path, data_models_root: &std::path::Path) {
    let src = resource_models_dir.join("potion-base-8M");
    let dst = data_models_root.join("potion-base-8M");
    if dst.join("model.safetensors").is_file() {
        return; // already staged
    }
    if let Err(e) = copy_dir_recursive(&src, &dst) {
        eprintln!("air-agent: could not stage bundled English model: {e} (memory features may be unavailable)");
    }
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
```

Test (inline):

```rust
#[cfg(test)]
mod staging_tests {
    use super::*;
    #[test]
    fn stages_english_when_absent_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let res = tmp.path().join("resources/models");
        std::fs::create_dir_all(res.join("potion-base-8M")).unwrap();
        std::fs::write(res.join("potion-base-8M/model.safetensors"), b"weights").unwrap();
        let data = tmp.path().join("data/models");
        stage_bundled_english(&res, &data);
        assert!(data.join("potion-base-8M/model.safetensors").is_file());
        // Second call is a no-op (does not error, does not re-copy over a user's staged model).
        stage_bundled_english(&res, &data);
        assert!(data.join("potion-base-8M/model.safetensors").is_file());
    }
}
```

- [ ] **Step 2: Call staging + drop the env push in `main.rs`.** In the `#[cfg(unix)]` engine block (`:73-104`), after computing `data_dir`, stage English and STOP passing a model dir to `ensure_started`:

```rust
                let resource_models = app.path().resource_dir().expect("resource dir").join("resources/models");
                let data_models_root = data_dir.join("models");
                crate::commands::engine::stage_bundled_english(&resource_models, &data_models_root);
                tauri::async_runtime::block_on(async {
                    let _up = crate::engine::daemon::ensure_started(&sock_path, &bin_path).await;
                });
```

(Remove the `let model_dir = app.path().resource_dir()...join("resources/models/potion-base-8M");` line and the `model_dir` argument.)

- [ ] **Step 3: Drop the env from the spawn command** in `apps/desktop/src-tauri/src/engine/daemon.rs`. Change `build_daemon_command(bin_path, model_dir)` (`:107-111`) to no longer set the env:

```rust
/// Build the `Command` used to spawn the daemon. With pull-based model resolution (rung 2, I1) the
/// app no longer pushes `BOSSCLAWD_MODEL_DIR`: the daemon resolves its model itself from the signed
/// log (multilingual) or its staged default `<data_dir>/models/potion-base-8M` (English). Env
/// remains a dev/harness-only override the daemon reads directly if a developer sets it.
fn build_daemon_command(bin_path: &Path) -> std::process::Command {
    std::process::Command::new(bin_path)
}
```

Update `ensure_started` signature to `pub async fn ensure_started(sock_path: &Path, bin_path: &Path) -> bool` and its `build_daemon_command(bin_path, model_dir)` call site to `build_daemon_command(bin_path)`. Delete the now-stale `build_daemon_command_sets_model_dir_env` test (`:285-303`) and the `model_dir` arg in the `ensure_started_returns_false_when_bin_missing_and_no_owner` test (`:268-283`).

- [ ] **Step 4: Run + build**

Run: `cargo test -p air_agent_desktop --lib "staging_tests|engine::daemon"` then `cargo build -p air_agent_desktop`
Expected: PASS + clean build.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/main.rs apps/desktop/src-tauri/src/engine/daemon.rs apps/desktop/src-tauri/src/commands/engine.rs
git commit -m "feat(desktop): stage English into data dir + stop pushing BOSSCLAWD_MODEL_DIR (I1)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task B4: TS API wrappers + DTOs (U7)

**Files:**
- Modify: `apps/desktop/src/api/engine.ts`

- [ ] **Step 1: Add the DTOs + wrappers** (after the reasoner wrappers `:66-73`):

```ts
export type ModelStatusDto = {
  state: "ok" | "missing" | "mismatch";
  expected: string | null;
  loaded: string | null;
  reindex_done: number | null;
  reindex_total: number | null;
};

/** Download + verify + install the multilingual pack, then enable it (starts the re-index). */
export const downloadLanguagePack = (): Promise<void> =>
  invoke<void>("engine_download_language_pack");
/** Enable an already-downloaded pack (retry / re-download recovery). */
export const setActiveModel = (modelId: string, safetensorsSha: string): Promise<void> =>
  invoke<void>("engine_set_active_model", { modelId, safetensorsSha });
/** Poll the language-pack state + re-index progress. Never throws (payload-encoded). */
export const modelStatus = (): Promise<ModelStatusDto> =>
  invoke<ModelStatusDto>("engine_model_status");
```

- [ ] **Step 2: Type-check**

Run: `cd apps/desktop && npm run typecheck` (or `npx tsc --noEmit`)
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/api/engine.ts
git commit -m "feat(desktop): TS wrappers for downloadLanguagePack/setActiveModel/modelStatus (U7)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task B5: Settings language-pack card (U7)

**Files:**
- Create: `apps/desktop/src/memory/LanguagePackCard.tsx`
- Create: `apps/desktop/src/memory/LanguagePackCard.test.tsx`
- Modify: `apps/desktop/src/memory/MemoryPanel.tsx` (render the card in the Evolve section; poll `modelStatus` alongside `refreshStatus`)

- [ ] **Step 1: Write the failing vitest** (`LanguagePackCard.test.tsx`). Mirror the repo's existing component tests (e.g. `MemoryPanel.test.tsx`) — mock `../api/engine`.

```tsx
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { LanguagePackCard } from "./LanguagePackCard";
import * as engine from "../api/engine";

vi.mock("../api/engine");

describe("LanguagePackCard", () => {
  beforeEach(() => vi.resetAllMocks());

  it("shows the Enable action when no pack is installed (state ok, no multilingual)", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue({
      state: "ok", expected: null, loaded: null, reindex_done: null, reindex_total: null,
    });
    render(<LanguagePackCard installed={false} />);
    expect(await screen.findByRole("button", { name: /enable multilingual/i })).toBeInTheDocument();
  });

  it("shows re-index progress while migrating", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue({
      state: "ok", expected: null, loaded: null, reindex_done: 220, reindex_total: 1043,
    });
    render(<LanguagePackCard installed={true} />);
    expect(await screen.findByText(/220\s*\/\s*1,?043/)).toBeInTheDocument();
  });

  it("shows a loud re-download prompt when the model is missing (I3)", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue({
      state: "missing", expected: "minishlab/potion-multilingual-128M",
      loaded: null, reindex_done: null, reindex_total: null,
    });
    render(<LanguagePackCard installed={true} />);
    expect(await screen.findByText(/missing/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /re-download/i })).toBeInTheDocument();
  });

  it("calls downloadLanguagePack when Enable is clicked", async () => {
    vi.mocked(engine.modelStatus).mockResolvedValue({
      state: "ok", expected: null, loaded: null, reindex_done: null, reindex_total: null,
    });
    vi.mocked(engine.downloadLanguagePack).mockResolvedValue();
    render(<LanguagePackCard installed={false} />);
    fireEvent.click(await screen.findByRole("button", { name: /enable multilingual/i }));
    await waitFor(() => expect(engine.downloadLanguagePack).toHaveBeenCalledOnce());
  });
});
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cd apps/desktop && npx vitest run src/memory/LanguagePackCard.test.tsx`
Expected: FAIL — `LanguagePackCard` not found.

- [ ] **Step 3: Implement `LanguagePackCard.tsx`** (mirror `SettingsSectionCard` + the Ollama hint block idiom `MemoryPanel.tsx:190-207`; poll `modelStatus` on an interval like `refreshStatus`):

```tsx
import { useEffect, useRef, useState } from "react";
import { SettingsSectionCard } from "../components/ui/SettingsSectionCard";
import { Button } from "../components/Button";
import { downloadLanguagePack, modelStatus, type ModelStatusDto } from "../api/engine";

/** How often the card refreshes the language-pack state while the tab is open. */
const POLL_MS = 2000;
/** Approximate download size shown to the user. */
const PACK_SIZE_LABEL = "≈500 MB";

type Props = {
  /** Whether the multilingual folder is already on disk (drives the Enable vs re-index copy). */
  installed: boolean;
};

export function LanguagePackCard({ installed }: Props) {
  const [status, setStatus] = useState<ModelStatusDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      setStatus(await modelStatus());
    } catch {
      // payload-encoded; a transport blip just leaves the last state.
    }
  };
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  useEffect(() => {
    void refreshRef.current();
    const id = setInterval(() => void refreshRef.current(), POLL_MS);
    return () => clearInterval(id);
  }, []);

  const onEnable = async () => {
    setBusy(true);
    setError(null);
    try {
      await downloadLanguagePack();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const reindexing =
    status?.reindex_total != null && status.reindex_done != null && status.reindex_total > 0;
  const missing = status?.state === "missing" || status?.state === "mismatch";

  return (
    <SettingsSectionCard
      title="Multilingual search"
      description="Search your memory in Korean and other languages. Adds a downloadable model; English search is unaffected until re-indexing finishes."
    >
      {missing ? (
        <div>
          <p style={{ fontSize: 13, color: "var(--error)" }}>
            {status?.state === "missing"
              ? "Model files missing — re-download to restore multilingual search."
              : "Model files failed their integrity check — re-download."}
          </p>
          <Button variant="primary" onClick={onEnable} disabled={busy}>
            {busy ? "Working…" : "Re-download"}
          </Button>
        </div>
      ) : reindexing ? (
        <p style={{ fontSize: 13 }}>
          Re-indexing your memories {status!.reindex_done!.toLocaleString()} /{" "}
          {status!.reindex_total!.toLocaleString()} — English search still works; Korean lights up when it finishes.
        </p>
      ) : installed ? (
        <p style={{ fontSize: 13, color: "var(--text-secondary)" }}>Multilingual active.</p>
      ) : (
        <div>
          <Button variant="primary" onClick={onEnable} disabled={busy}>
            {busy ? "Starting…" : `Enable multilingual (${PACK_SIZE_LABEL})`}
          </Button>
          {error ? <p style={{ fontSize: 13, color: "var(--error)" }}>{error}</p> : null}
        </div>
      )}
    </SettingsSectionCard>
  );
}
```

- [ ] **Step 4: Render it in `MemoryPanel.tsx`.** Import and place it in the Evolve section (after the Ollama hint block, before the Evolve buttons, `:207`):

```tsx
        <LanguagePackCard installed={false} />
```

(Import at top: `import { LanguagePackCard } from "./LanguagePackCard";`. `installed` can start `false`; a later refinement can derive it from a `modelStatus`-reported active id — out of scope for v1.)

- [ ] **Step 5: Run, expect PASS**

Run: `cd apps/desktop && npx vitest run src/memory/LanguagePackCard.test.tsx`
Expected: PASS (all four).

- [ ] **Step 6: Full frontend gate**

Run: `cd apps/desktop && npm run typecheck && npx vitest run && npm run lint`
Expected: clean typecheck, all vitest green, 0 lint warnings, 0 hardcoded colors (repo grep guard — the card uses only `var(--…)` tokens).

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/memory/LanguagePackCard.tsx apps/desktop/src/memory/LanguagePackCard.test.tsx apps/desktop/src/memory/MemoryPanel.tsx
git commit -m "feat(desktop): Settings language-pack card — enable/download/re-index/missing states (U7)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

# Ops (one-time)

## Task O1: Upload the 3 files to a GitHub Release + pin the 3 sha256

**Files:** `apps/desktop/src-tauri/src/engine/language_pack.rs` (`PACK_FILES` shas)

- [ ] **Step 1: Fetch the three files from Hugging Face** (source of truth for the model):

```bash
mkdir -p /tmp/ml-pack && cd /tmp/ml-pack
BASE="https://huggingface.co/minishlab/potion-multilingual-128M/resolve/main"
for f in model.safetensors tokenizer.json config.json README.md; do
  curl -fsSL "$BASE/$f" -o "$f"
done
```

- [ ] **Step 2: Compute + record the sha256 of each file**

```bash
cd /tmp/ml-pack && shasum -a 256 model.safetensors tokenizer.json config.json
```

- [ ] **Step 3: Cross-verify the safetensors sha against HF's independent LFS oid** (the `fetch-model.sh:11-17` precedent — the weights are the real attack surface):

```bash
curl -fsSL "https://huggingface.co/api/models/minishlab/potion-multilingual-128M?blobs=true" \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print([s for s in d['siblings'] if s['rfilename']=='model.safetensors'])"
```

Confirm the LFS `oid`/`sha256` equals the Step 2 digest for `model.safetensors`. The spec's partial digests to expect: safetensors `14b5eb39…`, tokenizer `19f19090…`, config `595e4cab…` — **re-confirm the full digests here**; do not ship the partials.

- [ ] **Step 4: Create the GitHub Release + upload the assets**

```bash
gh release create models-multilingual-128M-v1 \
  --repo AgentIdentityRegistry/air-note \
  --title "Multilingual embedder (potion-multilingual-128M) v1" \
  --notes "Opt-in language-pack model for rung-2 multilingual memory search. MIT (minishlab)." \
  /tmp/ml-pack/model.safetensors /tmp/ml-pack/tokenizer.json /tmp/ml-pack/config.json /tmp/ml-pack/README.md
```

- [ ] **Step 5: Pin the digests in code.** Replace the three `REPLACE_WITH_PINNED_SHA_O1` placeholders in `PACK_FILES` (Task B1) with the confirmed Step 2 hex digests. The safetensors is anchored (cross-verified); tokenizer/config are trust-on-first-download pins (per `fetch-model.sh:15-16`).

- [ ] **Step 6: Verify the live download path end-to-end** (against the real Release), then commit:

```bash
cargo test -p air_agent_desktop --lib engine::language_pack
git add apps/desktop/src-tauri/src/engine/language_pack.rs
git commit -m "ops(desktop): pin multilingual pack sha256 (safetensors cross-verified vs HF LFS)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task O2: De-pin `BOSSCLAWD_MODEL_DIR` from the installers + stage English (I1)

The launchd/systemd installer currently pins `BOSSCLAWD_MODEL_DIR` to the English bundle (`install-bossclawd.sh:42-44,85,179-191`). With env at highest priority (I1), that pin would block multilingual on the **installed** path. Remove it and instead stage English into the daemon's default `<data_dir>/models/potion-base-8M`.

**Files:** `scripts/install-bossclawd.sh`, `scripts/bossclawd.plist.in`, `scripts/bossclawd.service.in`

- [ ] **Step 1: Remove the `BOSSCLAWD_MODEL_DIR` env** from `bossclawd.plist.in` (its `EnvironmentVariables` dict) and `bossclawd.service.in` (its `Environment=` line). Grep both templates for `BOSSCLAWD_MODEL_DIR` and delete those entries.

- [ ] **Step 2: In `install-bossclawd.sh`, replace the env-templating step** with a COPY step that stages the resolved model dir (`--model-dir` / the macOS bundle path / the Linux default) into `<data_dir>/models/potion-base-8M`:

```bash
# Stage the bundled English model into the daemon's default resolution path so pull-based model
# resolution (rung 2) works without pinning BOSSCLAWD_MODEL_DIR (which would block the opt-in
# multilingual language pack). Idempotent: skip if already present.
staged="$(app_data_dir "${os}")/models/potion-base-8M"
if [ ! -f "${staged}/model.safetensors" ]; then
  mkdir -p "${staged}"
  cp -R "${MODEL_DIR}/." "${staged}/"
fi
```

(`${MODEL_DIR}` is the installer's already-resolved source; `app_data_dir` is the existing helper at `:179-191`.)

- [ ] **Step 3: Verify the templates no longer reference the env**

```bash
grep -rn "BOSSCLAWD_MODEL_DIR" scripts/ || echo "OK: no model-dir env pin remains"
```
Expected: only the daemon's own dev/harness reads remain (in Rust); no launcher pin.

- [ ] **Step 4: Commit**

```bash
git add scripts/install-bossclawd.sh scripts/bossclawd.plist.in scripts/bossclawd.service.in
git commit -m "ops: de-pin BOSSCLAWD_MODEL_DIR from launchers + stage English into data dir (I1)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Spec coverage (units U1–U8, invariants I1–I7)

| Spec item | Implemented / tested by |
|---|---|
| **U1** Pull-based model resolution | A4 (`ResourceModel2Vec::with_resolution` + `resolve`), A5 (`main.rs` construction + call-site wiring), A4 tests (env>signed>default), memharness preserved (A4 Step 5) |
| **U2** Verified model-identity binding | A4 (`build_candidate`/`resolve` sha-verify at load; `air-model.json` id read), B1 (`install_verified` writes binding from verified sha), A4 mismatch test |
| **U3** Downloader | B1 (`preflight_disk`/`verify_file`/`install_verified`/`download_and_install`), B2 (`engine_download_language_pack`), B4 (`downloadLanguagePack`) |
| **U4** Enable RPC + crash-safe migration | A3 (`Request::SetActiveModel`), A6 (`set_active_model`/`run_language_migration`), A7 (dispatch/client/facade), A2 (`reembed_prepare`/`reembed_finalize_gc`) |
| **U5** Fail-loud loaded-vs-intended guard | A4 (`ModelState` Missing/Mismatch in `resolve`), A5 (recall refuses test), A6/`model_status` |
| **U6** Progress + model-state reporting | A3 (`ModelStatusWire`/`ReindexProgressWire`), A6 (`model_status`, reindex cell), A7 (`model_status_wire`), B2/B4 (`engine_model_status`/`modelStatus`) |
| **U7** Settings UI card | B5 (`LanguagePackCard` + states), B2 DTO, B4 wrappers |
| **U8** `entity_vectors` in the migration | A2 (`rederive_entity_vectors_pending` + `gc_stale_vectors` GCs entity_vectors), A2 test, A9 integration assertion |
| **I1** Pull, not push | A4 (`resolve` reads signed log; env dev-only highest), A5, B3 (drop app push + stage English), O2 (drop launcher pin) |
| **I2** One source of truth | A1 (signed `language_pack` record is the sole authority) |
| **I3** Fail loud, never fall back | A4 (Missing/Mismatch → `Err`), A5 (`recall_refuses_loudly_when_signed_model_missing`), B5 (missing card state) |
| **I4** Verify, then name | A4 (sha-verify before load; `air-model.json` written after verify by B1), A4 mismatch test, O1 (cross-verify vs HF LFS) |
| **I5** All-or-nothing migration | A2 (`reembed_prepare` count-check → `Err` + no GC; `reembed_finalize_gc` only after complete), A2 `reembed_prepare_shortfall_returns_err_and_gcs_nothing` |
| **I6** Consent-gated, never auto | A6 (`set_active_model` writes the only enabling record; `resume_migration_if_pending` boot-only), A6 `zero_vectors_never_auto_migrates` + `interrupted_migration_resumes_on_boot` |
| **I7** English default output-identical | A4 (`no_record_resolves_bundled_english_default_i7`), A8 (`default_path_is_byte_identical_without_language_pack`) |

---

## Open questions carried from the spec (§12)

1. **Proto shapes for `SetActiveModel` + the Status extension (RESOLVED).** `SetActiveModel { onboarded, model_id, safetensors_sha } -> Response::Ok`. For the status side I **added a dedicated `ModelStatus` op** (`Request::ModelStatus -> Response::ModelStatus(ModelStatusWire { state: ModelStateWire, reindex: Option<ReindexProgressWire> })`) **instead of extending `EngineStatusWire`** as the spec's wording suggested. Rationale: the webview does not poll the general `engine_status` today (no `engineStatus()` wrapper exists in `api/engine.ts`), so there is no status DTO to extend; a dedicated op keeps every existing `Status` consumer and the invariance guard byte-identical (zero blast radius), and is simpler to test in isolation. This satisfies U6's stated purpose ("expose reindex progress + ModelState for polling"). Flagged for reviewer confirmation.
2. **`entity_vectors` inclusion (RESOLVED — included, U8).** On `main`, `entity_vectors` are populated only by evolve (`derive_entity_vector`, `:4871`) and are NOT touched by the old `reembed_migration`; a swap would orphan old-model rows and leave the new model's entity index empty (duplicate-entity risk on the next evolve tick). The migration therefore re-derives entity vectors under the new id (`rederive_entity_vectors_pending`) and GCs the old ones (`gc_stale_vectors` deletes both tables). The documented scope-out fallback was considered and rejected as leaving a real correctness hole.
3. **In-process swap vs shutdown+relaunch (RESOLVED — in-process swappable embedder).** Chosen because it is *simpler to test correctly* (the whole enable→migrate→flip runs in one process, exactly like the existing `roundtrip`/`memharness` in-process daemons) and avoids leaning on the two launchers' divergent relaunch semantics (the spec's #1 finding). Concurrency contract: the served embedder is an `Arc<dyn Embedder>` behind a `Mutex` in the production provider; the migration builds the candidate off to the side (`build_candidate`, no cache mutation), and only after the signed record is flipped to `Complete` does it `publish` (a single `Mutex` write) the new `Arc` and force a recall-index rebuild — so a concurrent recall always sees a *consistent* embedder (old until the flip, new after), never a half-built one.
4. **GitHub Release tag + redirect (RESOLVED for tag; redirect deferred).** Tag `models-multilingual-128M-v1`, direct asset URLs (O1). A client-transparent redirect indirection (so the asset can move without a client update) is **deferred** (YAGNI for v1) — noted as a follow-up.
5. **Re-confirm every `origin/main` line anchor (RESOLVED).** Done in "Verified current anchors" above; drift is listed below.

**Genuinely deferred (spec §11, unchanged):** reversibility ("revert to English"), language-detection nudge, zero-downtime migration (recall pauses to *serve old* during re-index but the flip is one-shot), dual-brain routing, Windows service specifics (the installed path here is launchd/systemd; app-spawn covers dev on all OSes — the Windows daemon is still un-gated per `apps/desktop/src-tauri/src/main.rs:5-8`).

---

## Spec-drift notes (where the tree differed from the spec's anchors)

- The spec's §3 line numbers were from `main`; the exact ones are re-listed in "Verified current anchors." No structural surprises, but note:
  - **`reembed_migration` GCs `vectors` only** (`log.rs:1873`), never `entity_vectors` — confirmed; U8 adds the entity half.
  - **The `MODEL_ID` const is at `embed.rs:16`** and is passed at the `from_pretrained` call `embed.rs:42` — confirmed as the seam.
  - **`resolve_model_dir` is `bossclawd/src/main.rs:202-206`** and defaults to `<data_dir>/models/potion-base-8M` — this is the pull-resolution default; the app currently overrides it via the `BOSSCLAWD_MODEL_DIR` push (`apps/desktop/.../daemon.rs:107-111` + `main.rs:86-90`), which B3 removes.
  - **The spec says "extend `Status`" (§6 U6, §12 OQ1); I diverged to a dedicated `ModelStatus` op** — see OQ1 resolution above. Reviewer should confirm this is acceptable.
  - **The two-launcher env-pin is real and load-bearing** (`install-bossclawd.sh:42-44,85`): with env at highest priority the installed path can never go multilingual unless the pin is removed — O2 handles this. The spec flagged the *restart* dependency as impossible but did not spell out that the *env pin itself* must be dropped for pull-resolution; this plan surfaces it as required scope.
```
