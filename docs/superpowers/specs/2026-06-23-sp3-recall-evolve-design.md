# Desktop Engine — Recall + Evolve Loop (SP3) — Design

**Status:** **Rev 2** (2026-06-23) — folds the independent **critic** (SHIP-WITH-FIXES) + **security** (SHIP-WITH-FIXES; 0 Critical / 2 Important) second opinion. Both reviewers verified every load-bearing engine/desktop API claim against current `main` (`bbf1f03`); unlike the SP1 review, **no false engine-API claim was found** — the foundation is accurate. Sub-project **3 of 5** in the "engine-in-the-desktop" milestone. Under user review.

**Rev 2 changes from the review:**
- **Network-guard CI bug fixed (Critical, both reviewers).** The Rev 1 inline `… | grep -q … && exit 1` form *fails CI on the clean/safe case* under GitHub's `bash -eo pipefail` (a no-match `grep -q` exits 1 → the pipeline exits 1 → the step reds a healthy build). Rewritten to the `if … then exit 1; fi` idiom (matching the existing SP2 guard) for both graphs, plus the promised `--features ollama` clippy leg (which is **not** in CI today).
- **`ensure_indexed` flag-poison fixed (Major).** Rev 1 set `indexed = true` *before* the rebuild; a rebuild error left it `true` forever → silent keyword-only recall for the session (the exact "silent empty recall" class SP2 fought). Now a `tokio::Mutex<bool>` set **only after a successful rebuild**.
- **Mandate emitter closed (security Important #1).** `set_proposals_enabled(false)` kills only the **M6b** proposal path; the **M6c mandate** path (`log.rs:5729`) is gated by `mandates_enabled` alone. `prime_switches` now forces **all three** flags off (first-class, not "defense-in-depth"). The zero-proposals test registers a dummy mandate so it isn't vacuous. *(Disk writes are structurally impossible in SP3 regardless — `execute_write` has no evolve/command caller — but the off-switch story is now airtight.)*
- **`prime_switches` placement LOCKED into `get_or_open`'s first-open block** (not "or `ensure_indexed`"), so switches are primed the moment the engine opens regardless of entry point (status / recall / scheduler). Failure reuses the existing open-failure path — **no new `EngineError` variant**.
- **Scheduler runtime fixed (Major).** `tauri::async_runtime::spawn` (a bare `tokio::spawn` panics in Tauri `.setup()`); `interval` uses `MissedTickBehavior::Skip` so a slow (minutes-long) tick can't burst.
- **`IdentityStore` gains `#[derive(Clone)]`** so the scheduler can hold an onboarding read across the spawn boundary (both fields already cheaply clone).
- **`EngineOpError::Busy(&'static str)`** generalized so ingest vs evolve name themselves (Rev 1 reused the hardcoded `"an ingest is already running"`).
- Smaller: `event_by_id` prose corrected to `Result<Option<Event>>`; the `new` 4th-arg churn counted honestly (~8 test sites + `main.rs`); `run_ingest` gains one line (set `indexed`); telemetry lock poison-recovers + `last_error` is length-capped; dim-mismatch + slow-tick named limitations; "first network surface" reframed (the *engine's* first loopback-only capability — the desktop already has egress); the scripted-evolve test template cited.

## Context — the parent milestone

SP1 ([spine](2026-06-22-desktop-engine-spine-design.md)) put one live, encrypted `bossclaw_core::EventLog` behind the `EngineHandle` chokepoint, opened **bare** (no recall index). SP2 ([ingest](2026-06-22-desktop-engine-ingest-design.md)) let the user grant folders, run ingest, persist **real `model2vec` vectors** under a single-sourced `MODEL_ID`, and recorded the active model at vector-birth (`set_active_model`) so a later milestone could discover it.

The 5 sub-projects (each its own spec → plan → build): **1 spine ✅ → 2 ingest ✅ → 3 recall + evolve loop (this doc) → 4 confirm/preview UI → 5 mandate management.**

SP3 is the payoff of SP1+SP2: it makes the persisted memory **searchable** and lets a **local** model **curate** it (entities, links, dossiers) on a background cadence — still fully offline, still onboarding-gated, still single-writer.

## Goal

After onboarding (and at least one ingest), the user can:
- **Search their memory** — type a query in a new top-level **"Memory"** tab and get back the most relevant remembered items (hybrid semantic + keyword recall over SP2's persisted vectors).
- **Let the brain tidy itself** — flip on a background **evolve loop** that, when a local **Ollama** model is present, periodically reads unprocessed memories/files and extracts structured knowledge (entities, machine links, dossiers). **OFF by default**, with a manual **"Evolve now"** button and an honest status readout (enabled · Ollama reachable · queue depth · last tick · errors).

The product point: this is the first moment the user's offline memory becomes **queryable** and **self-organizing**.

## Non-goals (explicitly deferred)

- **File-rewrite proposals + the confirm/preview UI → SP4.** The evolve loop in SP3 **only extracts knowledge** (entities/links/dossiers via M4a/M4b). It emits **zero** `write_proposal`s — the M6b reconciliation **and** M6c mandate proposers stay disabled (see §"Autonomous writes forced OFF", a hard safety requirement because the engine defaults these flags **on**).
- **Cloud Reasoner → later.** SP3 ships the `ReasonerProvider` seam + a local `OllamaReasonerProvider` only. A cloud-bridging provider drops in behind the same seam with zero rework.
- **Mandate management → SP5.**
- **Reranker → later.** The engine's `NoopReranker` (identity) stays; no cross-encoder in SP3.
- **Incremental / persisted ANN index → M7 perf.** SP3 rebuilds the in-memory index from persisted vectors (cheap — no re-embedding) on first recall and after each ingest/evolve. A persisted HNSW graph + idle/charging/thermal scheduler throttling are M7 (the engine comments earmark exactly this).
- **Per-user model-tag override / model auto-pull → later.** The reasoner model tag is one default `const`; SP3 detects + instructs, it does not download Ollama models.
- **Windows → M7**, identical to SP1/SP2. All new Rust (the `ReasonerProvider`, the recall/evolve `EngineHandle` methods, the scheduler, the new commands) and the Memory tab's engine calls are `#[cfg(unix)]`-gated.

## Architecture

### Dependencies — one engine **feature**, no new crates

- **Enable `bossclaw-core/ollama` on the desktop's Unix dependency edge** (`apps/desktop/src-tauri/Cargo.toml`, the `cfg(unix)` `bossclaw-core` line → `features = ["ollama"]`). This compiles `bossclaw_core::OllamaReasoner` + its loopback `ureq` backend (`ollama = ["dep:ureq"]`, `Cargo.toml:53`; `ureq` is `optional`, `default-features=false`, `features=["json","tls"]`). **No new desktop crate.**
- **The Ollama *detection* probe reuses the desktop's existing `reqwest`** (already a desktop dep, `rustls-tls`) — a plain `GET http://127.0.0.1:11434/api/tags`. It is **desktop-side**, so it does **not** enter `bossclaw-core`'s dependency graph (the network guard is engine-scoped — §"Network-posture guard").
- The `EmbedderProvider` seam + the bundled `potion-base-8M` resource from SP2 are **reused as-is** for recall.
- No new JS dependency; the Memory tab is built from the existing component kit.

### A. Recall-readiness — lazy index rebuild (refines the handoff's "open_with_recall at open")

For semantic recall to work, the in-memory HNSW must be rebuilt from SP2's persisted vectors (a bare `open` leaves it `None` → the vector arm degrades to keyword-only). The engine exposes `open_with_recall(path, dek, key, embedder)` (`log.rs:897` = `open` + `rebuild_indexes` + `rebuild_graph`).

**Decision (OQ1, confirmed by review) — rebuild lazily on first recall/evolve, NOT inside `get_or_open`.** SP1's `get_or_open` stays a **bare** `open` (its `EngineError` type unchanged), and a new idempotent `EngineHandle::ensure_indexed()` does the rebuild the first time it's needed:

```rust
// EngineHandle gains: indexed: tokio::sync::Mutex<bool>   (false at construction)
async fn ensure_indexed(&self, log: &Arc<EventLog>) -> Result<Arc<dyn Embedder>, EngineOpError> {
    let embedder = self.embedder_provider.embedder()?;        // cached for the process (SP2)
    let mut done = self.indexed.lock().await;                 // serializes — no double rebuild
    if !*done {
        let (log, emb) = (log.clone(), embedder.clone());
        spawn_blocking(move || -> Result<(), EngineOpError> {
            log.rebuild_indexes(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
            log.rebuild_graph().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(())
        }).await.map_err(|e| EngineOpError::Join(e.to_string()))??;
        *done = true;   // set ONLY after a successful rebuild — a rebuild error leaves it
                        // false so the next recall/evolve retries (no silent-empty-recall trap)
    }
    Ok(embedder)
}
```

**Why lazy, not `open_with_recall` in `get_or_open`:**
- `get_or_open` is also `engine_status`'s path; forcing a model load (~30 MB / few hundred ms) + a full index rebuild on a *status check* would regress SP1/SP2's startup laziness (`status()` today only does `verify_chain` + `count`). Lazy defers the cost to the first **search/evolve**.
- `get_or_open` returns `EngineError` (an exhaustive, no-wildcard status enum); building the embedder there would need a new variant + a `map_err_state` arm. Keeping the rebuild in the op-layer (`EngineOpError`) avoids that churn.
- **Same outcome:** recall reads SP2's persisted vectors. "Embedder held for the session" is satisfied by SP2's `ResourceModel2Vec` cache (the `Arc` is built once, reused).
- The `tokio::Mutex<bool>` (not an `AtomicBool`) serializes concurrent first-recalls **and** makes the "set true only on success" guarantee race-free.

**Index currency.** `*indexed == true` means "the in-memory index reflects persisted vectors this session." The only writers of embeddable events are ingest + evolve, both of which leave the index current: `ingest_all` already rebuilds after writing (SP2) — `run_ingest` additionally sets `*indexed = true` (**one new line** in SP2's method) to skip a redundant first-recall rebuild; `evolve_once` does **not** rebuild (engine survey confirms), so SP3's evolve method rebuilds after each tick (§E). So recall always sees the latest.

### B. Recall — handle method, command, DTO, Memory tab

**Engine call:** `recall(embedder, query, k, &RecallOptions) -> Vec<Hit>` (`log.rs:1321`). `Hit { event_id, score, sources, kind }` carries **no text** (`recall.rs:35`) — SP3 fetches each snippet via `event_by_id(id) -> Result<Option<Event>, BossclawError>` (`log.rs:739`) and pulls `content["text"]`. User-facing recall uses `RecallOptions::default()` (`pinned`/`graph_seeds` empty, `exclude_pages`/`exclude_files` **false** — the user wants dossiers and file hits).

**`EngineHandle::recall`** (the chokepoint shape, mirrors SP2's ops):
```rust
async fn recall(&self, onboarded: bool, query: String, k: usize)
    -> Result<Vec<HitWithText>, EngineOpError>
{
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    let embedder = self.ensure_indexed(&log).await?;
    spawn_blocking(move || -> Result<Vec<HitWithText>, EngineOpError> {
        let hits = log.recall(&*embedder, &query, k, &RecallOptions::default())
            .map_err(|e| EngineOpError::Core(e.to_string()))?;
        hits.into_iter().map(|h| {                       // snippet hydration (best-effort)
            let text = log.event_by_id(&h.event_id).ok().flatten()
                .and_then(|e| e.content.get("text").and_then(|t| t.as_str()).map(str::to_owned))
                .unwrap_or_default();
            Ok(HitWithText { hit: h, text })
        }).collect()
    }).await.map_err(|e| EngineOpError::Join(e.to_string()))?
}
```

**Command + DTO** (`commands/engine.rs`, the SP2 `#[derive(Serialize)] + From` convention):
```rust
engine_recall(query: String, k: usize) -> Result<Vec<HitDto>, String>
// HitDto { event_id, score, kind, sources: Vec<"vector"|"keyword">, text }
```
`k` is clamped server-side (e.g. `1..=50`). `RecallSource` serializes `#[serde(rename_all="snake_case")]` → TS `"vector" | "keyword"`.

**Memory tab (frontend).** Extend `App.tsx`'s `type View` (currently `"identity" | "inbox" | "settings"`, App.tsx:29) with `"memory"`, add a nav `<Button>` (mirroring `InboxNavButton`, which can surface evolve `queue_depth` as a badge), and a branch in the body **ternary** (App.tsx:47 — a nested ternary, not a switch; the new branch slots before the `settings` else) rendering a new `apps/desktop/src/memory/MemoryPanel.tsx` built from the kit (`Button`, `SettingsSectionCard`, `StatusBadge`, `Loading`), talking to new `api/engine.ts` wrappers:
- A **search box** → `recall(query, k)` → a results list (each: snippet text, `kind` badge — memory/page/file, `sources` chips, score). Empty query → no call; no results → friendly empty state.
- An **Evolve status/control** card (§E/§F).
- Pure render helpers (a hit→display-row mapper, the status formatter) live in sibling modules with `vitest` `.test.ts` files (mirrors `sources/ingestSummary.ts` + `.test.ts`).

### C. The `ReasonerProvider` seam (mirrors `EmbedderProvider` exactly)

New `apps/desktop/src-tauri/src/engine/reason.rs`:
```rust
pub const REASONER_MODEL_ID: &str = "qwen2.5:7b-instruct";   // single source of truth

pub trait ReasonerProvider: Send + Sync {
    /// Build (and cache) the reasoner. Called on first evolve, never at startup.
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError>;
}

pub struct OllamaReasonerProvider { cell: Mutex<Option<Arc<dyn Reasoner>>> }   // lazy + cached
impl ReasonerProvider for OllamaReasonerProvider {
    fn reasoner(&self) -> Result<Arc<dyn Reasoner>, EngineOpError> {
        let mut g = self.cell.lock().expect("reasoner cell poisoned");
        if let Some(r) = g.as_ref() { return Ok(r.clone()); }
        let r: Arc<dyn Reasoner> = Arc::new(bossclaw_core::OllamaReasoner::new(REASONER_MODEL_ID));
        *g = Some(r.clone());
        Ok(r)
    }
}

#[cfg(test)]                       // mock yields a ScriptedReasoner, like MockEmbedderProvider
pub struct MockReasonerProvider { /* canned (system,prompt)->response turns */ }
```
- `OllamaReasoner::new(REASONER_MODEL_ID)` (`ollama.rs:51`) targets the default `OLLAMA_LOOPBACK_URL = "http://127.0.0.1:11434/api/chat"` (`ollama.rs:26`) and is **fail-closed loopback-only**: `is_loopback_url` runs *before* any socket (`ollama.rs:80`); only numeric `127.0.0.0/8` + `::1` pass (bare `localhost`, `0.0.0.0`, int/hex/IPv4-mapped forms are rejected — compiler-verified by the security review). SP3 supplies **no** user URL → no SSRF surface.
- The provider is injected into the handle: `EngineHandle::new(vault, data_dir, embedder_provider, reasoner_provider)`. **Churn:** the SP2 call site in `main.rs` + ~8 `EngineHandle::new` test sites in `engine/mod.rs` + the `air/identity.rs` site each gain the 4th arg (mechanical).
- Tests inject `MockReasonerProvider` → `ScriptedReasoner` (`reason.rs:56`), keyed on SHA-256 of `(system, prompt)` — hermetic, no Ollama, no network.

### D. Ollama detection — `engine_ollama_status`

A desktop-side probe (reqwest → `GET http://127.0.0.1:11434/api/tags`, short timeout, **hardcoded host**) returns:
```rust
engine_ollama_status() -> Result<OllamaStatusDto, String>   // never errors (payload-encoded)
// OllamaStatusDto { reachable: bool, model_present: bool, model_tag: String /* = REASONER_MODEL_ID */ }
```
- `reachable` = the probe got 200; `model_present` = `REASONER_MODEL_ID` appears in `models[].name`. Any connect error → `{ reachable:false, model_present:false }`.
- The Memory tab disables the evolve toggle + shows an **"install Ollama and `ollama pull qwen2.5:7b-instruct`"** hint until both are true. The scheduler also gates on this (§E).

### E. The evolve scheduler — background timer + "Evolve now", OFF by default

The engine has **no running loop** — `evolve_once(embedder, reasoner)` (`log.rs:5463`) is a single tick (≤ `EVOLVE_BATCH = 16` events); the running scheduler is explicitly "M7". SP3 builds the driver.

**`EvolveScheduler`** — a task spawned once at startup in `main.rs` `setup` via **`tauri::async_runtime::spawn`** (a bare `tokio::spawn` panics — `.setup()` is not inside a tokio reactor). It owns `Arc<EngineHandle>` + a cloned `IdentityStore` (now `#[derive(Clone)]`) for the onboarding read. The ticker uses `MissedTickBehavior::Skip` so a slow tick can't burst:
```rust
let mut ticker = tokio::time::interval(EVOLVE_INTERVAL);          // ~5 min
ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
loop {
    ticker.tick().await;
    if !identity.is_onboarded() { continue; }                     // 1
    if !engine.evolve_enabled().await.unwrap_or(false) { continue; }   // 2 (off by default)
    let oll = probe_ollama().await;                               // 3
    if !(oll.reachable && oll.model_present) { continue; }
    match engine.evolve_status(true).await { Ok(s) if s.queue_depth > 0 => {}, _ => continue } // 4
    let _ = engine.evolve_once(true).await;                       // 5 (records telemetry inside)
}
```
**"Evolve now"** → `engine_evolve_now()` → `handle.evolve_once()` directly (one pass), independent of the interval.

**`EngineHandle::evolve_once`** — gated, **serialized by a new `evolve_lock: tokio::Mutex<()>`** (a manual click during a scheduled tick — or vice-versa — gets `Busy`; the scheduler skips if locked), builds embedder + reasoner, runs the tick, then **rebuilds indexes + graph** so recall reflects the new entities/links/dossiers:
```rust
async fn evolve_once(&self, onboarded: bool) -> Result<EvolveReport, EngineOpError> {
    let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
    let _guard = self.evolve_lock.try_lock().map_err(|_| EngineOpError::Busy("evolve"))?;
    let embedder = self.ensure_indexed(&log).await?;
    let reasoner = self.reasoner_provider.reasoner()?;            // EngineOpError::Reasoner on build fail
    let t0 = std::time::Instant::now();
    let result = spawn_blocking({ let log = log.clone(); let emb = embedder.clone(); move || {
        let r = log.evolve_once(&*emb, &*reasoner).map_err(|e| EngineOpError::Core(e.to_string()))?;
        log.rebuild_indexes(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;  // fold new vectors
        log.rebuild_graph().map_err(|e| EngineOpError::Core(e.to_string()))?;         // fold new edges
        Ok::<_, EngineOpError>(r)
    }}).await.map_err(|e| EngineOpError::Join(e.to_string()))?;
    self.record_tick(t0.elapsed(), &result);                     // telemetry (§F)
    result
}
```

**Off by default.** `evolve_enabled()` returns `true` only if never set (`log.rs:4832`); SP1/SP2 never set it. `prime_switches` forces it **OFF at first engine open** (§"Autonomous writes forced OFF") so the loop is genuinely opt-in. The toggle (`engine_set_evolve_enabled(bool)`) appends the sticky config event.

### F. Evolve status — merge engine live fields with scheduler telemetry

The engine's `EvolveStatus` (`evolve.rs:66`) has only **two live** fields (`queue_depth`, `enabled`); `last_tick_ms` / `error_count` / `last_error` are hardcoded stubs (`None/0/None`, `log.rs:6406`). So SP3 owns the real telemetry:
```rust
// EngineHandle gains: evolve_tel: std::sync::Mutex<EvolveTelemetry>
struct EvolveTelemetry { last_tick_ms: Option<u128>, error_count: usize, last_error: Option<String> }
// Lock poisoning (a panicked tick) must NOT wedge status: read via
//   self.evolve_tel.lock().unwrap_or_else(|p| p.into_inner())
// record_tick() caps last_error to ~512 bytes before storing (engine error strings can
// embed paths / reasoner output and flow to the webview DTO).

engine_evolve_status() -> Result<EvolveStatusDto, String>        // never errors
// merges: queue_depth + enabled            (EventLog::evolve_status)
//       + last_tick_ms/error_count/last_error  (EngineHandle.evolve_tel)
//       + ollama_reachable/model_present    (the §D probe — so the UI is one round-trip)
```
Both `evolve_once` paths (scheduled + manual) update `evolve_tel`. The DTO is the single source the Memory tab renders.

### Autonomous writes forced OFF (HARD safety requirement)

The engine defaults **all three** of `evolve_enabled`, `proposals_enabled`, `mandates_enabled` to `true` when never set (`log.rs:4832/4892/4949`), all sticky/fail-closed (a later flag-less config never re-arms a flag). Critically, `evolve_once` has **two independent** proposal emitters: the **M6b** reconciliation path gated by `proposals_enabled` (`log.rs:5657`) **and** the **M6c** mandate path gated by `mandates_enabled` **alone** (`log.rs:5729` → `run_mandate` emits at `log.rs:6100`). So SP3 must pin **all three** off:

```rust
// Runs ONCE inside get_or_open's first-open closure (NOT ensure_indexed) — so the switches
// are primed the moment the engine opens, regardless of which entry point (status / recall /
// scheduler) touches it first. Idempotent: each setter is sticky, so it writes at most once.
fn prime_switches(log: &EventLog) -> Result<(), BossclawError> {
    if log.evolve_enabled()?    { log.set_evolve_enabled(false)?; }     // opt-in loop
    if log.proposals_enabled()? { log.set_proposals_enabled(false)?; }  // kills the M6b emitter
    if log.mandates_enabled()?  { log.set_mandates_enabled(false)?; }   // kills the M6c emitter
    Ok(())
}
```
- **Placement:** in `get_or_open`'s first-open `spawn_blocking` closure, right after `EventLog::open`. Any failure maps to the **existing** open-failure path (`EngineError::KeystoreDbMismatch`) — **no new `EngineError` variant, no `map_err_state` churn**. Side-effect: the first engine open (often the first `engine_status`) appends ≤3 `config` events; idempotent thereafter (each flag already `false` → no write).
- **Why all three:** `set_proposals_enabled(false)` alone leaves the M6c mandate emitter armed. No mandate is registered anywhere in SP3 (`add_mandate` has zero non-test callers, so `active_mandates()` is provably empty and the M6c loop never iterates) — but forcing `mandates_enabled(false)` makes the safety property **structural**, not dependent on the runtime fact "no mandate exists." (Backstop: even an emitted proposal writes **no file** — `execute_write` has no evolve/command caller in SP3.)

### Error type — one new `EngineOpError` variant + one generalization

SP1's `EngineError` is untouched. The op-layer enum gains `Reasoner(String)` (mirrors `Embedder` — Ollama/reasoner build failure, Display → `"reasoner unavailable: {m}"`), and `Busy` is generalized to **`Busy(&'static str)`** so ingest and evolve name themselves (Display → `"an {op} is already running"`; the SP2 ingest site passes `Busy("ingest")`, evolve passes `Busy("evolve")`). Commands map `EngineOpError -> String` via `Display` as today.

### Commands + DTOs (added to `commands/engine.rs`, registered `#[cfg(unix)]` in `main.rs`)

```rust
engine_recall(query: String, k: usize)   -> Result<Vec<HitDto>, String>
engine_evolve_status()                    -> Result<EvolveStatusDto, String>   // never errors
engine_set_evolve_enabled(enabled: bool)  -> Result<(), String>
engine_evolve_now()                       -> Result<EvolveReportDto, String>    // manual one-pass
engine_ollama_status()                    -> Result<OllamaStatusDto, String>    // never errors
```
DTOs (serde, `From<bossclaw_core::…>`): `HitDto`, `EvolveStatusDto`, `EvolveReportDto`, `OllamaStatusDto`. Hand-mirrored TS twins added to `api/engine.ts` (snake_case preserved), each a one-line `invoke<T>("…", { args })`.

### Network-posture guard (refined — security-critical)

SP2's guard banned `hf-hub|ureq|reqwest` in `bossclaw-core`'s **default** graph. SP3 intentionally adds the loopback LLM client, so the guard becomes a **two-graph check** (`.github/workflows/build.yml`, `bossclaw-core` job) — written in the `if … then exit 1; fi` idiom so a clean graph does **not** red CI under `bash -eo pipefail`:
```bash
- name: Engine network-posture guard (two-graph)
  run: |
    if cargo tree -p bossclaw-core -e normal --prefix none \
         | grep -qE '^(hf-hub|ureq|reqwest)( |$)'; then
      echo "FORBIDDEN: a network crate is in the DEFAULT bossclaw-core graph"; exit 1; fi
    if cargo tree -p bossclaw-core -e normal --features ollama --prefix none \
         | grep -qE '^(hf-hub|reqwest)( |$)'; then
      echo "FORBIDDEN: hf-hub/reqwest in the ollama graph (only ureq allowed)"; exit 1; fi
    echo "network posture OK: default=zero-client, ollama=ureq-only"
- name: Clippy bossclaw-core (ollama feature)         # NOT in CI today — add it
  run: cargo clippy -p bossclaw-core --features ollama --all-targets -- -D warnings
```
Rationale: the embedder stays provably network-free (default graph: zero clients — model2vec is `default-features=false`, no `hf-hub`); the **only** network capability the engine gains is `ureq`, used **solely** by the fail-closed loopback `OllamaReasoner` (its transitive deps are TLS-only: `rustls*`, no HTTP-egress client). The anchored `^name( |$)` pattern is kept (a version always follows a space, so it won't false-positive on `reqwest-*`). `fastembed` stays off.

## Data flow

**Search:** Memory tab → `engine_recall(query, k)` → `EngineHandle.recall` → gate → `ensure_indexed` (first time: build embedder + rebuild HNSW from persisted vectors + rebuild graph) → `spawn_blocking(log.recall)` → hydrate snippets via `event_by_id` → `Vec<HitDto>` → results list.

**Evolve (scheduled):** `EvolveScheduler` tick → onboarded ✓ → `evolve_enabled()` ✓ → Ollama reachable+model ✓ → `queue_depth>0` ✓ → `EngineHandle.evolve_once` → gate → `evolve_lock` → build embedder+reasoner → `spawn_blocking`: `log.evolve_once` (≤16 events: extract entities/links/invalidates/dossiers via Ollama, signed appends; **no proposals** — all three flags off) → `rebuild_indexes` + `rebuild_graph` → telemetry. **Manual "Evolve now":** same path via `engine_evolve_now`.

**Status:** Memory tab polls `engine_evolve_status` → merges engine `{queue_depth,enabled}` + handle telemetry `{last_tick_ms,error_count,last_error}` + probe `{reachable,model_present}`.

**First open (any entry point):** `get_or_open` first-open closure → `EventLog::open` → `prime_switches` (force evolve/proposals/mandates off, idempotent) → cached.

## Failure / partial-state matrix

| Scenario | Result |
|---|---|
| Not onboarded → any `engine_*` | `get_or_open` gate → `Open(NotOnboarded)`; Memory tab shows "set up your identity first" |
| Recall before any ingest (no vectors) | `ensure_indexed` rebuilds an empty index → recall returns `[]` — success, not an error |
| Recall, vector index empty but keyword has hits | vector arm degrades (engine `resolve_arms`) → keyword-only results — success |
| Index rebuild **errors** at first recall/evolve | `ensure_indexed` returns the error AND leaves `indexed=false` → the next call retries (no permanent silent-empty trap) |
| Model resource missing/corrupt at first recall/evolve | `EngineOpError::Embedder` ("memory model unavailable"); recall/evolve fail cleanly; nothing mutated |
| Ollama not running / model not pulled | `engine_ollama_status` → `{reachable:false,…}`; toggle disabled with install hint; scheduler skips; manual "Evolve now" → `Reasoner` error (loopback refused) |
| Evolve enabled, queue empty | scheduler tick no-ops (`queue_depth==0`); "Evolve now" → empty `EvolveReport` |
| Manual "Evolve now" during a scheduled tick (or vice-versa) | `EngineOpError::Busy("evolve")` (`evolve_lock` `try_lock`) — this race is exactly why the lock exists (a disabled button alone can't cover the scheduled-vs-manual overlap) |
| A reasoner/graph error on one memory mid-tick | engine logs + breaks the batch, cursor unmoved → retries next tick; `error_count`/`last_error` updated; recall/storage untouched |
| A single poisoned memory wedges the queue | `queue_depth` stops advancing across ticks → surfaced in the status card (named limitation) |
| A tick takes longer than the interval (cold 7B) | `MissedTickBehavior::Skip` + `evolve_lock` → no burst, no overlap; the next tick fires one interval after completion |
| Evolve toggled off mid-run | engine off-switch is checked before any model call; the in-flight ≤16 batch finishes, no new tick starts |
| Proposals/mandates somehow enabled (regression) | caught by the test asserting zero `write_proposal` events after evolve **with a dummy mandate registered**; `prime_switches` forces all three `false` at open |

## Security invariants

- **Loopback-only reasoner, fail-closed (verified).** `OllamaReasoner.complete_json` verifies `is_loopback_url` **before** opening any socket (`ollama.rs:80`); only numeric `127.0.0.0/8` + `::1` pass (`localhost`, `0.0.0.0`, int/hex/IPv4-mapped forms rejected — compiler-checked by the security review). SP3 constructs it via `new` with the default `OLLAMA_LOOPBACK_URL` and supplies **no** user URL → no SSRF / confused-deputy. *(The `pub OllamaReasoner::with_url` escape hatch is never called in SP3 prod; a future `pub(crate)`/`#[cfg(test)]` tightening is noted as a deferred engine hardening so it stays unreachable.)*
- **Embedder stays provably network-free.** model2vec is `default-features=false` (no `hf-hub`). The two-graph guard proves: default graph = **zero** network clients; ollama graph = **only** `ureq`. `hf-hub`/`reqwest` can never enter `bossclaw-core`.
- **No autonomous file writes.** `prime_switches` forces `evolve_enabled`/`proposals_enabled`/`mandates_enabled` all `false` at first open (the engine defaults them **on**). The evolve loop emits only `entity`/`link`/`invalidate`/`page`/`supersede` events — **zero** `write_proposal`s (both the M6b and M6c emitters are gated off). M6 actuator + confirm UI is SP4. Even if a proposal were emitted, `execute_write` (the only FS-mutating path, `log.rs:3127`) has **no** evolve/command caller → no disk write is reachable in SP3. Verified by a non-vacuous test (dummy mandate registered).
- **Taint preserved.** Evolve reads `file_ingested` (external-tainted) + `memory` content; the engine's eager D2 taint + D8 lineage rules keep file-derived facts `is_external` and dossier `source_event_ids` engine-gathered (not model-cited). SP3 adds no new file-read path; reasoner output is parsed as `Proposals` (data), never authority. Untrusted text is engine-fenced (`push_fenced_source` + zero-width-space fence-breakout neutralization, `extract.rs:185`) + capped (`MAX_INPUT_TEXT_BYTES = 16 KB`, `extract.rs:95`).
- **Off-switch honored before any model call.** `evolve_once` checks `evolve_enabled()` first (`log.rs:5470`); the scheduler additionally gates on enabled + Ollama + queue. Off by default.
- **Detection probe is hardcoded loopback.** `engine_ollama_status` GETs a fixed `127.0.0.1:11434/api/tags`, no user input. (Note: the desktop *already* has broad outbound egress via `web_fetch_public`, so this probe does not widen the app's surface — it adds the *engine's* first loopback-only capability. The pre-existing `web_fetch_public` SSRF is a separate, out-of-scope ticket.)
- **Single writer + chokepoint intact.** `evolve_once` is a normal signed `append`; `evolve_lock` serializes manual + scheduled ticks; the onboarding gate + single-instance cell still hold. No second DB writer.
- **No new secrets.** Local Ollama needs no API key (no `Authorization` header, no vault reference in `ollama.rs`); the keychain is untouched.
- **Resource bounds.** ≤16 events/tick (`EVOLVE_BATCH`); ~5 min interval with `Skip`; `spawn_blocking` keeps the UI responsive; `evolve_lock` prevents concurrent ticks; recall `k` clamped; `last_error` length-capped.
- **Provenance note (inherited):** the engine does not yet verify `signed_by_did` against the user owner (engine TODO, M7) — out of SP3 scope; recorded for the audit baseline. The reasoner model tag is unpinned for SP3 (OQ2) — accepted; provenance pinning matters more when the cloud provider lands.

## Known limitations (named, accepted for SP3)

- **First recall/evolve latency** — model load (~hundreds of ms, once/process) + an index rebuild from persisted vectors (linear in vector count). Brief loading state; incremental rebuild is M7.
- **Post-evolve full rebuild** — `rebuild_indexes` + `rebuild_graph` after each tick is O(vectors); a tick processes ≤16 events yet rebuilds the whole index, so at large vector counts the rebuild dominates tick cost. Fine for SP3 brain sizes; the M7 incremental path addresses it.
- **Recall snippet hydration is N+1** (`event_by_id` per hit) — fine for `k ≤ 50`; batch fetch is a later optimization.
- **`last_tick_ms`/`error_count`/`last_error` are session-scoped** (scheduler memory, not persisted — matching the engine's own M7-deferred stubs); they reset on app restart.
- **A poisoned memory can wedge the evolve queue** (engine retries it forever) — surfaced via a non-advancing `queue_depth`; auto-quarantine is a later engine change.
- **Fixed ~5 min cadence**, no idle/charging/thermal throttle (M7).
- **One model tag, no per-user override / auto-pull** — the user installs Ollama + pulls the model; SP3 detects + instructs.
- **Embedder/vector dim mismatch** — if a future model swap changes `dim` from what SP2 persisted, `rebuild_indexes` reads zero matching vectors → empty semantic recall (keyword still works). Out of SP3 scope (single `MODEL_ID`); named so it isn't a surprise.
- **Windows:** no Memory tab / evolve until M7 (Unix-gated).

## Testing

- **Hermetic `#[cfg(unix)]` `EngineHandle` tests** (`MockVault` + `MockEmbedderProvider` + `MockReasonerProvider`/`ScriptedReasoner` + tempdir):
  - `open` → ingest a couple `.txt` → `recall("…")` returns the expected hit(s) with hydrated text + `kind`/`sources`.
  - recall before any ingest → `[]`; recall with only keyword overlap → keyword-only sources.
  - `evolve_once` (scripted reasoner) → `EvolveReport` with the expected `entities_minted`/`links_emitted`; a follow-up `recall` surfaces the new dossier (`kind=="page"`). **Template:** mirror `crates/bossclaw-core/tests/evolve.rs` — `scripted_both_passes` (line 70, primes Pass A + Pass B + adjudication + summarize turns) + `evolve_once_emits_entities_and_a_link_then_advances_the_cursor` (line 117). The `ScriptedReasoner` is keyed on exact SHA-256 of `(system,prompt)` — reuse the engine's prompt builders so keys match; budget this (it is the heaviest test).
  - **Autonomous writes stay off (non-vacuous):** `prime_switches` then **register a dummy mandate** (`add_mandate`), run `evolve_once`, assert **zero** `write_proposal` events AND `report.proposals_emitted == 0` AND `evolve_enabled()/proposals_enabled()/mandates_enabled()` all `false`.
  - `ensure_indexed` flag-reset: inject a rebuild failure → error returned AND a subsequent call retries (flag stayed `false`).
  - `evolve_lock`: concurrent `evolve_once` → one `Busy("evolve")`.
  - not-onboarded → every op `Open(NotOnboarded)`; no DB/keys created.
- **Scheduler gating unit tests** — given (onboarded, enabled, ollama-reachable, queue_depth) the tick decision is correct (off → no-op; on+queue+ollama → runs; ollama-down → skip; queue 0 → skip). Telemetry updates on run; `MissedTickBehavior::Skip` asserted.
- **Ollama probe parsing** — unit-test the `/api/tags` JSON → `OllamaStatusDto` mapping (present / absent / connect-error), no live Ollama.
- **DTO mapping unit tests** — `Hit`→`HitDto`, `EvolveStatus`(+telemetry)→`EvolveStatusDto`, `EvolveReport`→`EvolveReportDto`, probe→`OllamaStatusDto`.
- **Frontend `vitest`** — the hit→display-row mapper; the evolve-status formatter (enabled/queue/last-tick/errors); empty/disabled states.
- **One `#[ignore]`-gated live-Ollama evolve test** (real `qwen2.5:7b-instruct`, mirroring `crates/bossclaw-core/tests/live_ollama.rs`): enable → `evolve_once` → entities/links minted → recall surfaces them.
- **Gates:** `cargo build/test/clippy -p air_agent_desktop` + `cargo test -p bossclaw-core` + `cargo clippy -p bossclaw-core --features ollama -- -D warnings` green; `npm run typecheck --workspace @air-agent/desktop` + `vitest` green; the **two-graph network guard** green (default zero-network; ollama = ureq-only).
- **Manual launch:** onboard → ingest → search in Memory tab → (with Ollama up) flip Evolve on → "Evolve now" → watch queue drain + status update → search again, see a dossier.

## New constants / modules / commands / touch

- `REASONER_MODEL_ID = "qwen2.5:7b-instruct"` — one `const` in `engine/reason.rs`.
- New modules: `engine/reason.rs` (the `ReasonerProvider` seam), `src/memory/*` + new `api/engine.ts` recall/evolve wrappers.
- `EngineHandle` gains: `reasoner_provider` field + `new` arg, `evolve_lock: tokio::Mutex<()>`, `indexed: tokio::Mutex<bool>`, `evolve_tel: std::sync::Mutex<EvolveTelemetry>`, and methods `recall`, `evolve_once`, `evolve_status`, `set_evolve_enabled`, `ensure_indexed`, `record_tick`, `prime_switches`. `run_ingest` gains one line (`*indexed = true`).
- `EngineError`: **unchanged** (prime-switches failure reuses the `KeystoreDbMismatch` open-failure path).
- `EngineOpError`: new `Reasoner(String)`; `Busy` → `Busy(&'static str)` (updates the SP2 ingest call site + Display).
- `IdentityStore`: gains `#[derive(Clone)]` (both fields already clone) — for the scheduler's onboarding read.
- New Tauri commands: `engine_recall`, `engine_evolve_status`, `engine_set_evolve_enabled`, `engine_evolve_now`, `engine_ollama_status`.
- New `EvolveScheduler` (a `tauri::async_runtime::spawn` task, started in `main.rs` setup).
- `Cargo.toml` (desktop, unix): `bossclaw-core` `features = ["ollama"]`.
- CI: the two-graph network-posture guard (idiom-fixed) + the new ollama-feature clippy step.
- **No `bossclaw-core` code change** — SP3 consumes the engine's existing recall/evolve/reasoner/off-switch surface as-is (the rare milestone with zero engine edits; all churn is in the desktop crate).

## Resolved by review (was "Open questions")

1. **Lazy index rebuild vs `open_with_recall` at open → LAZY** (keeps `engine_status` cheap, no SP1 error churn), **with the mandatory flag-reset fix** (set `indexed` only after a successful rebuild) — which neutralizes lazy's only downside vs eager (loud-failure-on-open).
2. **Reasoner model-tag pinning → UNPINNED for SP3.** The loopback guard removes the egress threat; the digest pin is provenance, and the user pulls whatever `ollama pull` gives them (no committed digest to pin to). Revisit when the cloud provider lands.
3. **`mandates_enabled` → FORCE `false` (first-class).** It gates a *second*, independent proposal emitter (M6c); `set_proposals_enabled(false)` alone is one flag short. Cheap, makes the property structural.
4. **Evolve telemetry persistence → SESSION-SCOPED**, matching the engine's own non-persisted stubs. Persisting it is M7.
5. **Post-evolve full rebuild → ACCEPTABLE for SP3** (bounded note added to Known limitations); do **not** pull M7's incremental indexing forward.

## Future hooks (NOT built here)

- **SP4** adds the write-proposal queue + confirm/preview UI: flips `proposals_enabled` on behind the confirm surface, reuses the same `EngineHandle` chokepoint + `FileRecordDto.file_event_id`, consumes the M6 actuator. The `ReasonerProvider` + scheduler from SP3 are reused unchanged.
- **SP5** adds mandate management (flips `mandates_enabled`, registers mandates via `add_mandate`).
- A **cloud `ReasonerProvider`** drops behind the SP3 seam (BYO-key, still data-not-authority) with zero rework.
- **Deferred engine hardening** (separate tickets, not SP3): make `OllamaReasoner::with_url` `pub(crate)`/`#[cfg(test)]` so a non-loopback reasoner can't be constructed; and the pre-existing desktop `web_fetch_public` SSRF (scheme-only URL validation).
- **M7:** persisted ANN index, incremental indexing, idle/charging/thermal throttling, persisted evolve telemetry, signer-DID verification, Windows.
