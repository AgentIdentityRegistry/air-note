# Desktop Engine Ingest (SP2) — Design

**Status:** **Rev 2** (2026-06-22) — folds the independent **critic** (SHIP-WITH-FIXES) + **security** (SHIP; 0 Critical / 0 Important) second opinion. Both verified the design against the live engine code, and — unlike the SP1 review — every load-bearing engine-API claim checked out true. Sub-project **2 of 5** in the "engine-in-the-desktop" milestone. Under user review.

**Rev 2 changes from the review:**
- **Folder-picker snippet fixed** — `FilePath` (tauri v2) has no `Display`; convert via `into_path()` + `to_string_lossy`. A cancelled dialog *and* a dropped sender both resolve to `Ok(None)`.
- **Active-model `config` event now written in SP2** (was deferred to SP3) — closes a silent-empty-recall forward-trap. `MODEL_ID` is single-sourced as one `const`, and SP2 records the active model the moment vectors first exist. **This is the one scope adjustment: a small, bounded `bossclaw-core` addition** — see §"Recording the active model".
- **`default-features = false` made durable** — a `cargo-deny` ban on `hf-hub`/`ureq`/`reqwest` so the "embedder is network-free → needs no sandbox" property cannot silently regress (security finding #1).
- **Model-blob hygiene pinned** — the `.gitignore` rule lands in the same commit as `fetch-model.sh`; the sha256 pin is a committed constant; the fetch fails closed on mismatch; packaging CI asserts a clean tree.
- Smaller: matrix reason-strings de-quoted, `list_grants` revoked-filter stated, `FileRecordDto` keeps `file_event_id`, the picker-cancel + on-disk-deletion cases named, `ResourceModel2Vec` documented to cache for the process lifetime, and the three trivial `EngineHandle` method bodies shown.

## Context — the parent milestone

SP1 ([the engine spine](2026-06-22-desktop-engine-spine-design.md)) gave the AIR Agent desktop app a single live, encrypted `bossclaw_core::EventLog` — unlocked from the keychain, held in `AppState.engine` behind the `EngineHandle` chokepoint, opened **bare** (no recall index). SP2 is the first sub-project that *uses* that brain: it lets the user grant the engine read-access to folders, run ingest over them, and see what landed.

The 5 sub-projects (each its own spec → plan → build): **1 spine ✅ → 2 ingest (this doc) → 3 evolve-loop runtime → 4 confirm/preview UI → 5 mandate management.** Build order 1 → 2 → 3 → 4 (+5 alongside).

## Goal

Wire the engine's already-built M5a ingest pipeline into the desktop. After onboarding, the user can: **grant a folder** (a read-grant), **run ingest** (walk every active granted folder, parse text files, append signed `file_ingested` events, and derive **real searchable vectors** with a bundled local embedding model), and **list** both the granted folders and the ingested files. All access routes through SP1's `EngineHandle`. Ship a minimal but usable **Sources** panel so the flow is verifiable end-to-end.

The product point: this is the first moment the app turns the user's files into durable, searchable memory — fully offline.

## Non-goals (explicitly deferred)

- **Recall / semantic search UI → SP3.** SP2 *creates* the vectors (so ingest is real, not throwaway) and *records the active model* (below), but exposes **no query surface**. SP3 swaps SP1's bare open for `open_with_recall` (rebuilding the in-memory index from the vectors SP2 persisted, under the same `MODEL_ID`) and adds the search UI + the evolve loop.
- **Rich documents (PDF/docx/pptx/xlsx/msg) → a later brick.** The engine's M5b rich parser is behind the `markitdown` Cargo feature and needs a sandboxed Python venv + an egress-denial proof. SP2 ships **native UTF-8 text/markdown only** (`ParserRouter::native_only()`); non-text files are reported `skipped`, not failed.
- **Write-proposals / confirm UI → SP4. Mandate management → SP5.**
- **Per-file live progress, ingest cancellation, scheduled/auto ingest → out of scope.** `ingest_all` is one blocking call with no progress callback or cancel; the evolve/scheduler loop is SP3.
- **Windows → deferred to M7**, identical to SP1. All SP2 Rust (the new `EngineHandle` methods, commands, the embedder seam, the `set_active_model` engine method) and the Sources panel are `#[cfg(unix)]`-gated; Windows builds without them until M7 un-gates `bossclaw-core`.

## Architecture

### Dependencies — none new (the clean part)

SP2 adds **zero new dependencies**, Rust or JS:
- `model2vec-rs` is already in the tree (a default, non-feature-gated dep of `bossclaw-core` since SP1). `Model2Vec` compiles today.
- `tauri-plugin-dialog` is already a Rust dep **and already registered** in `main.rs` (`.plugin(tauri_plugin_dialog::init())`). The folder picker is reached from Rust (below), so **no `@tauri-apps/plugin-dialog` npm package and no capability change are needed** (the capability set stays `core:default` + `opener:default` + `dialog:default`, with no `fs:` permission).
- The embedding model ships as a **bundled data resource**, not a crate.
- `cargo-deny` (a dev/CI tool, not a runtime dep) is introduced for the network-ban guard below.

The `markitdown` / `fastembed` / `ollama` features stay OFF.

### The embedder seam (testability + lazy, cached model load)

`ingest_all(router, embedder)` requires a real `&dyn Embedder`. SP2 introduces one seam, mirroring SP1's `EngineKeystore`/`SecretsVault` injection:

```rust
// apps/desktop/src-tauri/src/engine/embed.rs  (new)
pub const MODEL_ID: &str = "minishlab/potion-base-8M";   // single source of truth (SP2 + SP3)

pub trait EmbedderProvider: Send + Sync {
    /// Build (and cache) the embedder. Called on first ingest, not at startup.
    fn embedder(&self) -> Result<Arc<dyn Embedder>, EngineOpError>;
}
```

- **Production:** `ResourceModel2Vec { model_dir: PathBuf, cell: Mutex<Option<Arc<dyn Embedder>>> }` lazily loads `Model2Vec::from_pretrained(&model_dir, MODEL_ID)` on first call and **caches it for the process lifetime** (the `cell`). Model load (~30 MB, a few hundred ms) is paid once per process, on first ingest — never at startup (preserving SP1's laziness), and not re-paid on subsequent "Ingest now" clicks.
- **Tests:** `MockEmbedderProvider { dim }` returns an `Arc<MockEmbedder>` — hermetic, no model files, no network.

The provider is injected into the handle: `EngineHandle::new(vault, data_dir, embedder_provider)`. (SP1's `EngineHandle::new(vault, data_dir)` call site in `main.rs` and its `engine/mod.rs` tests are updated to pass a provider — the only SP1 churn.)

### Model delivery

- **Bundle:** ship `minishlab/potion-base-8M` (MIT-licensed; pure-Rust static embeddings — no code execution, no network, no subprocess) as a Tauri resource. `tauri.conf.json` → `bundle.resources` includes `resources/models/potion-base-8M/` (the three files `from_pretrained` needs: `model.safetensors`, `tokenizer.json`, `config.json`). Runtime path: `app.path().resource_dir()?.join("models/potion-base-8M")`, passed into `ResourceModel2Vec` when the engine is constructed in `main.rs`.
- **Never committed to git.** A `scripts/fetch-model.sh` downloads the three files from HuggingFace and **verifies a committed, in-repo sha256 constant** for each (the pin is reviewed like any other code change — NOT read from the same HF response it validates), writing them to the gitignored `resources/models/potion-base-8M/`. The script **fails closed** on any hash mismatch (never ships an unverified blob). The `.gitignore` rule (`apps/desktop/src-tauri/resources/models/`) lands **in the same commit** as `fetch-model.sh`, and the packaging CI job runs `git status --porcelain` after the fetch to assert the tree stays clean (so a 30 MB blob can never slip into history). Run once by a developer and in the **packaging** CI job before `tauri build`. Rationale: a ~30 MB binary blob doesn't belong in git history; a hash-pinned fetch gives reproducibility (TOFU on first pin — acceptable for a quality-only asset with no code path) without the bloat or git-LFS's per-clone smudge dependency. Mirrors how the engine's own tests obtain this exact model.
- **Hermetic tests never need the model** (they use `MockEmbedderProvider`), so the normal `cargo test` / CI test path stays fast + offline. The real model path is covered by one `#[ignore]`-gated integration test + manual launch.

### Recording the active model (closes the SP3 divergence trap)

`MODEL_ID` is a single Rust `const` (in `engine/embed.rs`) consumed by both SP2's `ResourceModel2Vec` and, later, SP3's recall-open — so the vectors SP2 persists under `embedder.model_id()` and the vectors SP3's `rebuild_indexes` reads back via `vectors_for_model(MODEL_ID)` match **by construction**. That alone prevents a divergent `model_id`.

But `active_model()` would still return `None` after ingest (only `reembed_migration` writes a `config` event today, `log.rs:1732`), leaving SP3 unable to *discover* the model from the log — and `rebuild_indexes` silently reads **zero** vectors on any `model_id` mismatch (the worst kind of failure: empty recall, no error). So **SP2 records the active model the moment vectors first exist:** after a successful `run_ingest`, if `active_model()` is `None` (or its `model_id` differs), append one signed `config` event carrying `{ active_model_id: MODEL_ID, dim: embedder.dim(), schema_version: SCHEMA_VERSION }`. It is **idempotent** (written once), signed by the **brain key** via the engine's normal `signer_did()` path (NOT `reembed_migration`'s hardcoded migration DID), and makes `active_model()` truthful from vector-birth.

If the engine exposes no public setter, SP2 adds a minimal **`EventLog::set_active_model(model_id, dim)`** that appends the `config` event + refreshes any projection, mirroring `add_grant` — **the one small, additive `bossclaw-core` touch in SP2** (a pure addition; no change to existing `ingest_all` behavior). (Resolves the critic's Major finding + Open Question 1.)

### The `EngineHandle` operational methods (the chokepoint pattern)

Five new methods on the SP1 `EngineHandle`, each get-or-opening through the onboarding gate then doing the blocking work in `spawn_blocking` — exactly how SP1's `status()` is shaped, so SP3/SP4 can't bypass the gate. A new `ingest_lock: tokio::sync::Mutex<()>` field is added to the struct (the in-flight guard).

The three trivial methods follow `status()`'s shape exactly — gate, then one blocking call (note `EventLog::add_grant` takes `&Path`, so `&path`):

```rust
impl EngineHandle {                                  // all #[cfg(unix)]
    async fn add_grant(&self, onboarded: bool, path: PathBuf) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || log.add_grant(&path).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string())))
            .await.map_err(|e| EngineOpError::Join(e.to_string()))?
    }
    // revoke_grant / list_grants / list_files: the identical shape over
    // EventLog::revoke_grant / grants / current_files (the list_* methods map to DTOs).
}
```

`run_ingest` is the only non-trivial one:

```rust
async fn run_ingest(&self, onboarded: bool) -> Result<IngestReport, EngineOpError> {
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    // In-flight guard: a second concurrent ingest returns Busy immediately (try_lock,
    // not lock). The UI also disables the button. Guard is Send, held across the await.
    let _guard = self.ingest_lock.try_lock().map_err(|_| EngineOpError::Busy)?;
    let provider = self.embedder_provider.clone();
    let report = tokio::task::spawn_blocking(move || {
        let embedder = provider.embedder()?;           // lazy, cached model load — BEFORE the walk
        let router = ParserRouter::native_only();
        let report = log.ingest_all(&router, &*embedder).map_err(|e| EngineOpError::Core(e.to_string()))?;
        // Record the active model once, now that vectors exist (idempotent).
        if log.active_model().ok().flatten().is_none() {
            log.set_active_model(MODEL_ID, embedder.dim()).map_err(|e| EngineOpError::Core(e.to_string()))?;
        }
        Ok::<_, EngineOpError>(report)
    })
    .await.map_err(|e| EngineOpError::Join(e.to_string()))??;
    Ok(report)
}
```

The embedder is built **before** the walk, so a missing/corrupt model fails fast with **nothing ingested** (no partial state).

### Error type — `EngineOpError` (zero SP1 churn)

SP1's `EngineError` + `map_err_state` + `EngineState` are **untouched** (its exhaustive, no-wildcard match stays a status-only concern). The operational layer gets its own enum that wraps the open path:

```rust
pub enum EngineOpError {
    Open(EngineError),  // gate/open failed (NotOnboarded, KeystoreDbMismatch, …) — reuse SP1 Display
    Core(String),       // a BossclawError from grant/revoke/ingest/list/set_active_model
    Embedder(String),   // model load failed (resource missing/corrupt)
    Busy,               // an ingest is already running
    Join(String),       // spawn_blocking join failure
}
```

`EmbedderProvider::embedder()` returns `EngineOpError` (the `Embedder` variant on load failure). Commands map `EngineOpError` → `String` via `Display`. Keeping the two enums separate means SP2 never has to invent a bogus `EngineState` mapping for operational errors.

### Commands (thin) + DTOs

`apps/desktop/src-tauri/src/commands/engine.rs` gains thin handlers (the `EngineHandle` does the work, like SP1's `engine_status`), registered in `main.rs`'s `generate_handler!`:

```rust
engine_add_grant(path: String)   -> Result<(), String>
engine_revoke_grant(path: String)-> Result<(), String>
engine_list_grants()             -> Result<Vec<GrantDto>, String>   // ALL grants (active + revoked)
engine_run_ingest()              -> Result<IngestReportDto, String>
engine_list_files()              -> Result<Vec<FileRecordDto>, String>
engine_pick_folder()             -> Result<Option<String>, String>  // native dialog, below
```

The core types (`Grant`, `FileRecord`, `IngestReport`) are **not** `Serialize`, so SP2 defines serde DTOs and maps them:
- `GrantDto { canonical_root, granted_at, revoked }` — `list_grants` returns **all** grants (the core `grants()` returns active *and* revoked); the **frontend filters to active** for the main list (revoked rows carry `revoked: true`).
- `FileRecordDto { canonical_path, file_event_id, content_hash, grant_root }` — keeps `file_event_id` (free now; SP4's confirm/preview will want it to reference a file).
- `IngestReportDto { ingested, superseded, deduped, skipped: Vec<SkipDto>, failed: Vec<SkipDto> }` where `SkipDto { path: String, reason: String }` (maps the core's `Vec<(PathBuf, String)>`). The reason strings come verbatim from the engine — the frontend renders them but **must not assert on their exact text** (they are engine-owned).

### Folder picker — Rust-side, threading-safe (no JS dep)

`engine_pick_folder` opens the native directory chooser via the already-registered Rust dialog plugin, using the **non-blocking callback** form bridged to async with a `oneshot` (NOT `blocking_pick_folder`, which must not run on the main thread). `FilePath` (tauri v2) is an enum with no `Display`, so convert via `into_path()`; both a cancelled dialog (`None`) and a dropped sender resolve to `Ok(None)`:

```rust
#[tauri::command]
pub async fn engine_pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |p| { let _ = tx.send(p); });
    let picked = rx.await.ok().flatten();              // dropped sender → None (no error)
    Ok(picked.and_then(|p| p.into_path().ok()).map(|pb| pb.to_string_lossy().into_owned()))
}
```

It returns a path string only — **canonicalization + containment happen in the engine** (`add_grant` canonicalizes; the walk does the no-symlink contained open). The desktop never reads granted files itself. (Alternative considered: the JS `@tauri-apps/plugin-dialog` `open({directory:true})` — rejected for SP2 to avoid a new npm dep + a capability-permission edit, since the callback+oneshot Rust form is equally threading-safe; the reviewer confirmed `pick_folder` dispatches to the main thread internally.)

### Frontend — the Sources panel (`src/sources/`)

A `SettingsSectionCard` titled **"Sources"** (rendered in the settings area), built from the existing component kit (`Button`, `SettingsSectionCard`, `StatusBadge`, `Loading`), talking to a new `src/api/engine.ts` (typed `invoke` wrappers matching `src/api/tauri.ts`):

- **Add folder** → `engine_pick_folder()` → if a path comes back, `engine_add_grant(path)` → refresh the grant list. (Cancel → no-op.)
- **Granted folders** list — `engine_list_grants()` **filtered to active** (`!revoked`), each with a **Revoke** (`engine_revoke_grant`).
- **Ingest now** button → `engine_run_ingest()`; disabled + shows an indeterminate **"Ingesting…"** state while awaiting; on return, renders a one-line summary from `IngestReportDto` (e.g. *"3 added · 1 updated · 12 unchanged · 2 skipped · 0 failed"*) plus an expandable skip/fail reason list.
- **Ingested files** list (`engine_list_files`) — path + grant root.
- A small empty/disabled state when `engine_status` reports `NotOnboarded`.

Pure helpers (the `IngestReportDto` → summary string; sorting/grouping the file list) live in plain modules with `vitest` `.test.ts` files, matching `src/inbox/*.test.ts`.

## Data flow

1. User clicks **Add folder** → `engine_pick_folder` (native dialog) → path → `engine_add_grant(path)` → `EngineHandle.add_grant` → `get_or_open` (gate) → `spawn_blocking(EventLog::add_grant)` (canonicalize + append `grant` + rebuild grants projection).
2. User clicks **Ingest now** → `engine_run_ingest` → `EngineHandle.run_ingest` → gate → in-flight guard → `spawn_blocking`: build/cache `Model2Vec` → `ingest_all(native_only, embedder)` (walk every active grant: contained no-symlink open → UTF-8 parse → dedup/supersede → append signed `file_ingested` (origin `"external"`) → derive + persist vectors → rebuild in-memory ANN/FTS index) → record the active-model `config` event (once) → `IngestReport`.
3. Panel renders the report summary + refreshes `engine_list_files` / `engine_list_grants`.

## Failure / partial-state matrix

(Reason strings below are descriptive — the engine owns the exact text; the UI must not assert on it.)

| Scenario | Result |
|---|---|
| Not onboarded → any `engine_*` | `get_or_open` gate → `Open(NotOnboarded)`; panel shows "set up your identity first" |
| Folder picker cancelled / dialog dismissed | `engine_pick_folder` → `Ok(None)` → UI no-ops (no grant) |
| `add_grant` on a non-existent/unresolvable path | core `canonicalize` fails closed → `Core` error; no grant appended |
| `run_ingest` with no active grants | `ingest_all` walks nothing → empty `IngestReport` (all zeros) — success, not an error |
| `run_ingest` while one is already running | `Busy` (button is disabled anyway) |
| Model resource missing/corrupt at first ingest | `Embedder` error ("memory model unavailable"); **nothing ingested** (embedder built before the walk) |
| File not valid UTF-8 (binary/rich doc) | per-file **skipped** (non-UTF-8 reason) — not a failure |
| File over the byte cap | **skipped** (oversize) |
| Symlink / containment refusal / TOCTOU swap | per-file **failed** with the engine's containment reason (fail-closed) |
| Grant → ingest → revoke → ingest again | revoked root not walked; prior `file_ingested` events stay in the log but are excluded from recall (SP3) |
| Re-ingest an unchanged folder | every file **deduped** (same path + same content hash) — no new events |
| A file's content changed since last ingest | **superseded** (old version retired, new appended) |
| A previously-ingested file deleted on disk | the walk won't re-encounter it; its `FileRecord` stays current (no tombstone) — the panel may list a now-absent file (named in Known limitations) |

## Security invariants

- **Taint root preserved.** Every `file_ingested` event the engine appends is stamped `origin: "external"` (M5a/D5, single-sourced at `ingest.rs:617`; `append_event_in_tx` re-signs engine-side, overwriting any caller signature/origin). SP2 introduces no second file-reading path; its only write to the log is `ingest_all` (+ the one `set_active_model` config event, which carries no file content). The M6 actuator's fail-closed lineage walk still consumes the taint.
- **Read-grants are the only authorization.** `ingest_all` walks **only active** grants (`ingest.rs:590`) and re-checks `!revoked` before every append (`ingest.rs:667`); a concurrent revoke stops mid-run. SP2 surfaces grant state honestly and never widens it.
- **The desktop never reads granted files itself.** The picker returns a path string; all file access (canonicalize, no-symlink contained `openat`/`openat2(RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS)` walk, post-open `fstat`, byte cap, dedup) is the engine's M5a pipeline. The webview has **no `fs:` capability**, so the frontend physically cannot read granted files. TOCTOU: the grant string is an authorization label, not a capability the walk dereferences blindly — a symlink swapped in after grant-time is refused as `Containment` (per-file `failed`), never followed out of tree.
- **In-process embedder, no sandbox needed — and the property is locked.** `Model2Vec` is pure-Rust static embeddings: safetensors load (no pickle/`torch.load` gadget), no subprocess, no network. Confirmed at the dependency boundary — `bossclaw-core` declares `model2vec-rs` with `default-features = false`, so its `hf-hub`/`ureq`/`reqwest` fetch path is **not compiled** (`cargo tree -i hf-hub` ⇒ absent from the engine graph). To keep this from silently regressing (a future edit re-enabling defaults would compile a network fetcher into the engine and invalidate the no-jail justification), SP2 adds a **`cargo-deny` ban** (`deny.toml`) on `hf-hub`/`ureq`/`reqwest` entering the `bossclaw-core` graph, enforced in CI. This is *why* native+model2vec needs no jail (unlike markitdown/M5b's Python venv).
- **Model integrity** is hash-pinned at fetch time (committed sha256 constant, fail-closed script). A swapped model could degrade recall quality but cannot escalate (no code path). Shipped read-only as a resource.
- **Keys unchanged.** SP2 adds no new secrets and does not touch the SP1 keystore. DEK/brain-key handling is unchanged. The `config` event is signed by the brain key (the normal `signer_did()` path).
- **Info exposure acceptable.** DTOs carry canonical paths + content hashes; error strings carry paths — all data the user already owns, rendered back into the user's own UI with no network egress. (Hardening note for later: if any ingest summary is ever logged/exported off-device, render paths relative to the grant root.)
- **No new `unsafe`**; no new runtime dependencies (smaller audit surface than SP1). Pre-existing GTK/Tauri-stack advisories (`unic-*` unmaintained, `glib` unsound) are not introduced by SP2 and not on the ingest path — record them as accepted in an audit baseline so the signal stays clean.

## Known limitations (named, accepted for SP2)

- **No per-file progress and no cancellation.** `ingest_all` is one blocking call → the UI shows an indeterminate "Ingesting…" until it returns. A large folder can take seconds (responsive throughout via `spawn_blocking`). Incremental/streaming progress would need an engine change (deferred; the engine doc already earmarks an incremental `index_event` path for M7).
- **First-ingest model-load latency** (~hundreds of ms, once per process). Accepted; not shown as separate progress.
- **Stale file list after on-disk deletion.** A file deleted from disk after ingest keeps its `FileRecord` (no tombstone) until/unless superseded; the panel may show it. Engine-documented behavior; accepted for SP2.
- **`add_grant` assumes a directory.** `EventLog::add_grant` canonicalizes any existing path without an `is_dir()` check; the picker only returns folders, so this is unreachable via the UI (a granted *file* would just walk-as-root and ingest nothing).
- **Poison-on-panic** (inherited from SP1): `EventLog`'s internal mutex uses `.expect(POISON)`; a prior engine panic poisons later calls.
- **Windows:** no Sources panel until M7 (Unix-gated).
- **Rich documents are skipped**, with a clear per-file reason — not silently dropped.

## Testing

- **Rust `#[cfg(unix)]` `EngineHandle` tests** (`engine/mod.rs`, extending SP1's): `MockVault` + `MockEmbedderProvider(dim)` + a temp `app_data_dir` + a temp source folder containing a couple of `.txt`/`.md` files →
  - `add_grant` → `list_grants` shows it (active) → `run_ingest` → `IngestReport.ingested == N` → `list_files` returns the N paths → **`active_model()` is now `Some(MODEL_ID)`** (config event written).
  - Re-`run_ingest` → all `deduped`; `active_model()` still written once (idempotent — no duplicate config event).
  - Change a file's bytes → `run_ingest` → one `superseded`.
  - A non-UTF-8 file → `skipped` (not failed).
  - `run_ingest` with no grants → empty report.
  - `revoke_grant` → `list_grants` shows it `revoked` → `run_ingest` skips that root.
  - Not-onboarded → every op returns `Open(NotOnboarded)`; no DB/keys created.
- **Engine unit test** (`bossclaw-core`): `set_active_model` writes a `config` event signed by the log's own DID; `active_model()` returns it; a second call is idempotent.
- **DTO mapping unit tests** (`Grant`→`GrantDto`, `IngestReport`→`IngestReportDto` incl. `(PathBuf,String)`→`SkipDto`, `FileRecord`→`FileRecordDto` keeps `file_event_id`).
- **Frontend `vitest`**: the `IngestReportDto` → summary-string formatter; file-list helpers; the active-grant filter.
- **One `#[ignore]`-gated real-model integration test** (env-var-pointed model dir, mirroring `bossclaw-core/tests/recall.rs`) exercising `ResourceModel2Vec` → `Model2Vec::from_pretrained` → `ingest_all` end-to-end with real vectors.
- **Manual launch:** onboard → Add folder → Ingest → see summary + file list → Revoke → re-Ingest.
- **Gates:** `cargo build/test/clippy -p air_agent_desktop` + `cargo test -p bossclaw-core` green; `npm run typecheck --workspace @air-agent/desktop` + `vitest` green; **`cargo deny check` passes** (the network-ban guard); `cargo audit` against the accepted baseline. Native-only ⇒ no Python/jail in the test path.

## New constants / resources / commands / engine touch

- `MODEL_ID = "minishlab/potion-base-8M"` — one `const` in `engine/embed.rs`; the engine-side `model_id` tag on every vector AND the `active_model_id` in the config event; SP3's recall-open reuses it.
- Resource dir: `resources/models/potion-base-8M/{model.safetensors,tokenizer.json,config.json}` (gitignored; fetched by `scripts/fetch-model.sh`, committed-sha256-pinned, fail-closed; listed in `tauri.conf.json` `bundle.resources`).
- New Tauri commands: `engine_add_grant`, `engine_revoke_grant`, `engine_list_grants`, `engine_run_ingest`, `engine_list_files`, `engine_pick_folder`.
- New modules: `engine/embed.rs` (the provider seam + `MODEL_ID`), `src/sources/*` + `src/api/engine.ts` (frontend).
- New `deny.toml` (cargo-deny config) banning `hf-hub`/`ureq`/`reqwest` from the `bossclaw-core` graph.
- **Engine addition (bounded):** `EventLog::set_active_model(model_id, dim)` if no public setter exists — appends a `config` event via the normal signed `append` path.

## Resolved by review (was "Open questions")

1. **Active-model `config` event — SP2 or SP3?** → **Write it in SP2** (critic Major). `active_model()` returns `None` until a `config` event exists, and `rebuild_indexes` silently reads zero vectors on a `model_id` mismatch — a silent-empty-recall trap one milestone forward. Recording it at vector-birth (idempotent, brain-signed) eliminates the forward-dependency. `MODEL_ID` is additionally single-sourced.
2. **In-flight guard.** → **Keep it** (both reviewers), framed as UX/efficiency, not safety (the engine serializes writes + re-checks revoke per append). ~3 lines, prevents redundant concurrent walks + double-reports; the disabled button alone is insufficient (a second window / re-entrant call could fire).
3. **Model delivery: fetch-script vs LFS.** → **Hash-pinned fetch-script** (both reviewers). Keeps integrity without permanent history bloat or LFS's per-clone dependency; the pin is a committed constant, the fetch fails closed, packaging CI asserts a clean tree, and tests never need the blob.
4. **Picker P2 (Rust) vs P1 (JS).** → **P2** (both reviewers); `pick_folder` dispatches to the main thread internally, so it's deadlock-safe — no new npm dep / capability edit for zero gain. Conversion bug fixed above.

## Future hooks (NOT built here)

- **SP3** swaps SP1's bare `open` for `open_with_recall` (rebuilding the index from the vectors SP2 persisted — **same `MODEL_ID` const**, and `active_model()` is now truthful so SP3 can discover the model from the log), holds the embedder at open (instead of per-ingest), and adds the recall/search UI + the evolve-loop runtime. The `EmbedderProvider` seam + the bundled model resource SP2 introduces are reused as-is.
- **SP4** adds the write-proposal queue + confirm/preview UI, routed through the same `EngineHandle` (and will use `FileRecordDto.file_event_id`).
- The single-`EngineHandle` chokepoint continues to hold the onboarding gate + single-instance invariant as SP3 adds the evolve writer and SP4 adds confirm readers.
