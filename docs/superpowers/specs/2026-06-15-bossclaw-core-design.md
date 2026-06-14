# bossclaw-core — Design Spec

**Status:** Draft for review · 2026-06-15
**Author:** Peter + Claude (brainstorm session, superpowers:brainstorming)
**Repo:** `~/air-note` (canonical) · new crate `crates/bossclaw-core`
**Supersedes for the memory layer:** the "Phase B — Brain" guesswork in [[air/forever-companion-architecture]] (all-MiniLM + sqlite-vec, 2026-05). This spec is the researched, decided version.

---

## 1. Purpose & scope

`bossclaw-core` is the **embeddable, local-first memory engine** for BossClaw — the part that *remembers everything, finds it again, links it, and lets a model reason + self-organize over it* — bound to the user's AIR identity (DID) by per-event cryptographic signatures.

This is the **first build** of the refined BossClaw direction (the universal never-forget memory hub for all the user's AI tools). It is deliberately scoped to **the engine plus its first surfaces inside the BossClaw desktop app**, with the desktop as the only consumer and the user (Peter) as the first daily power-user (dogfooding).

### In scope (v1)
- The `bossclaw-core` Rust crate: encrypted store, signed append-only event log, recall (semantic + keyword + rerank), bi-temporal graph schema, a pluggable reasoner interface, and a minimal always-on "evolve" loop.
- Desktop integration of the engine as a Tauri command/event surface.
- **Local file access** through the engine: read-ingest of user-granted folders, **plus confirm-each-write** file actions (every write previewed + user-approved).

### Explicitly NOT in scope (deferred to later sub-projects)
- The universal hub wiring (importing history from + plugging Claude / Claude Code / Cowork / GPT / Codex / Gemini into one shared memory).
- Tool-orchestration / routing incoming work to the right tool.
- Sign/pay-on-your-behalf (the broader Mandate).
- **Silent / unattended autonomous file writes** (v1 writes are confirm-each only).
- Multi-device sync (CRDT).
- Self-training the local model on the user's data.

These are real and on the roadmap; they all *consume* or *extend* the engine this spec defines.

---

## 2. Product context & the honest capability framing

BossClaw's claim is "a true second brain that remembers everything and can infer + evolve on its own." This spec commits to the **honest, buildable** version of that claim:

| User ask | What we actually deliver | Honesty line |
|---|---|---|
| **Remember everything** | Append-only signed log; nothing is ever edited or deleted. | *Storage* is guaranteed; *recall* is excellent-not-perfect — but a memory is never gone, at worst not-yet-surfaced (raw log is always browsable). It remembers everything it is *given*; full capture is the later "hub" job. |
| **Infer + connect topics on its own** | The engine links topics structurally (graph + semantic recall + contradiction/anomaly surfacing); a **language model reasons** over what the engine surfaces. | The *system* infers; the engine remembers+links+surfaces, the model reasons on top. Existence proof: GBrain does exactly this daily. |
| **Think + evolve on its own** | An **always-on background loop driven by a local LLM** (Ollama/bundled) that continuously summarizes, links, retires contradictions, and proactively surfaces — for ~$0, privately, offline. | It is a model running in a loop, not independent consciousness. "Evolve" = the *memory/graph* evolves continuously (v1); the *model's weights* self-improving via training is a separate, optional, later phase. |

**Tiered reasoning (roadmap item B):** the **local model is the always-on worker** (extract / tag / link / summarize / spot-contradictions — the high-volume thinking, ~$0); a **cloud frontier model** is the rare "hard synthesis" consultant. Baseline run cost ≈ $0.

**Guaranteed vs. committed:** we *guarantee* the deterministic properties — never-forget storage and cryptographic ownership (every memory signed to the user's DID, no company can hold it hostage). We *commit to build* the proven mechanisms for connection + self-organization and prove their quality by dogfooding, not by promising a quality number.

---

## 3. Decisions locked in this brainstorm

1. **Scope:** engine-first; desktop is the only v1 consumer; hub / orchestration / sign-pay are later sub-projects.
2. **Substrate:** Rust-native, single language, no foreign runtime. **Adopt validated *designs*** (GBrain's hybrid ranking; Graphiti's bi-temporal graph) rather than embed GBrain's TS/Postgres runtime or rebuild from a blank page. *(Embedding GBrain wholesale was considered and rejected — it would ship a Node + WASM-Postgres engine inside a Rust/Tauri app, fighting the turnkey/offline/small-bundle goals; the 2026 Rust ecosystem makes it unnecessary.)*
3. **Embedding default:** `bge-small-en-v1.5` (quality-first; beats the all-MiniLM baseline at the same runtime), with `model2vec`/`potion-base-8M` bundled as the pure-Rust instant-offline fallback, and `EmbeddingGemma-300M` as the documented quality upgrade path.
4. **Reasoner:** pluggable + **local-first** (Ollama / bundled llama.cpp / cloud), first-class interface.
5. **Evolve loop:** a core component, not polish; v1 ships a *minimal* version so the user can watch the brain self-organize early.
6. **File access in v1:** **read-ingest + confirm-each writes** (user-chosen, overriding the more conservative recommendation). Every write is preview-and-approve with a human in the loop; silent/unattended writes remain deferred.
7. **Reuse:** Ed25519 signing from `air-rs/signing.rs`; keychain/secrets from the desktop's `vault.rs` + `secrets/`; the hash-chain pattern already shipped for the registry audit log.

---

## 4. Architecture overview

Two layers, cleanly separated so the engine stays focused and testable:

```
┌────────────────────────── BossClaw desktop (Tauri, Rust) ──────────────────────────┐
│                                                                                     │
│  CONNECTOR LAYER (app)                          REASONER BACKENDS                    │
│   • file reader   (ingest folders → events)      • local: Ollama / bundled llama.cpp │
│   • file actuator (confirm-each writes)          • cloud: frontier model (rare)      │
│   • grant manager (folder allowlist + OS perms)                                      │
│        │  signed memory events / queries / actions                                  │
│        ▼                                                                             │
│  ┌──────────────────────────── bossclaw-core (crate) ────────────────────────────┐  │
│  │  signed event log → derived: vector index · keyword index · bi-temporal graph │  │
│  │  recall pipeline (embed → hybrid → rerank → graph/recency boost)              │  │
│  │  reasoner interface (trait)         evolve loop (always-on, local-LLM-driven) │  │
│  │  encrypted SQLite store  ·  Ed25519 per-event signing (DID-bound)             │  │
│  └───────────────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

**Key invariant — the event log is the single source of truth.** Every vector, keyword token, and graph edge is *derived* from the signed event log and can be **rebuilt from scratch** by replaying it. Lose an index → lose performance, never history. (Same event-sourcing discipline as the messaging archive replayer.)

**The engine never touches the filesystem itself** (beyond its own DB files). File reading/writing lives in the connector layer, which feeds *events* in and receives *actions* out. This keeps `bossclaw-core` a pure, sandboxable library.

---

## 5. Components (each an isolated unit)

For each: what it does · its interface · what it depends on.

### 5.1 Store (`store`)
- **Does:** owns the encrypted SQLite database file(s); transactions; migrations.
- **Interface:** `open(path, key) -> Store`; transaction handles. No business logic.
- **Depends on:** `rusqlite` (bundled SQLite), the encryption layer (§8.1).

### 5.2 Signed event log (`events`) — the moat (Layer 10)
- **Does:** append-only writes of `{ id: ULID, ts, valid_time, type, content, prev_hash, hash, signed_by_did, signature }`. Per-event Ed25519 signature + hash chain (`hash = H(prev_hash ‖ canonical(event))`).
- **Interface:** `append(event) -> EventId`; `stream(since) -> Iterator<Event>`; `verify_chain() -> Result`. **No update/delete.**
- **Depends on:** `store`, `ed25519-dalek` (via `air-rs/signing.rs`), `sha2`.

### 5.3 Embedder (`embed`)
- **Does:** text → vector. Model-swappable.
- **Interface:** `Embedder` trait: `embed(&[Text]) -> Vec<Vector>`, `dim()`, `model_id()`.
- **Implementations:** `FastEmbed` (default, `bge-small-en-v1.5`; upgrade path to `EmbeddingGemma-300M`); `Model2Vec` (pure-Rust fallback). Selected by config.
- **Depends on:** `fastembed` (ort/ONNX backend) and/or `model2vec-rs`.

### 5.4 Vector index (`index`) — behind a swap-trait
- **Does:** approximate/Brute nearest-neighbour over embeddings.
- **Interface:** `VectorIndex` trait: `add(id, vec)`, `search(vec, k) -> [(id, score)]`, `remove(id)`, `rebuild(from_events)`.
- **Implementations:** `SqliteVec` (v1 default — lives in the same SQLite file, simplest; brute-force, fine at personal scale); `Hnsw` (`hnsw_rs`, pure-Rust ANN) as the drop-in for scale. *Do not marry the index — the trait is the contract.*
- **Depends on:** `sqlite-vec` or `hnsw_rs`.

### 5.5 Keyword index (`keyword`)
- **Does:** exact/lexical match (BM25-ish) via SQLite FTS5.
- **Interface:** `search(query, k) -> [(id, score)]`.
- **Depends on:** SQLite FTS5 (bundled).

### 5.6 Bi-temporal graph (`graph`) — truth-tracker (Layer 8)
- **Does:** entities + relationships with **two clocks** (valid-time `t_valid/t_invalid` vs ingestion-time `t_created/t_expired`); backlinks. Contradictions **invalidate-not-delete** (set `t_invalid`, keep history). Adopts Graphiti's design (not its Neo4j+LLM runtime).
- **Interface:** `link(a, b, rel, valid_time)`, `invalidate(edge, at)`, `neighbors(node)`, `as_of(time)`.
- **Depends on:** `store`. (LLM-driven edge *extraction* is the evolve loop's job, §5.9.)

### 5.7 Recall pipeline (`recall`)
- **Does:** the ranking recipe (adopted from GBrain): embed query → **hybrid** (vector + keyword) candidate set → **rerank** (cross-encoder) → boosts (graph proximity, recency-decay, user-pinned) → top-N with evidence labels.
- **Interface:** `recall(query, opts) -> [ScoredMemory]`.
- **Depends on:** `embed`, `index`, `keyword`, `graph`, the reranker (`fastembed` `TextRerank`, `bge-reranker-base`).

### 5.8 Reasoner (`reason`)
- **Does:** abstracts "ask a model to reason over retrieved memory."
- **Interface:** `Reasoner` trait: `complete(prompt, context) -> Text` (+ streaming). Untrusted context is **fenced** (§8.4).
- **Implementations:** `LocalReasoner` (Ollama HTTP / bundled llama.cpp) — default; `CloudReasoner` (frontier API) — opt-in, used only for escalated hard synthesis.
- **Depends on:** an HTTP client / local runtime; config for tiering.

### 5.9 Evolve loop (`evolve`)
- **Does:** the always-on background worker. On new events (and on an idle schedule): extract entities/links, write summary pages, retire contradicted facts, and queue proactive surfacing. v1 = the minimal pass (extract + link + summarize via the **local** reasoner). Deeper autonomy (proactive nudges, contradiction-resolution, cloud-tier synthesis) is phased on top.
- **Interface:** `tick()` / background task; emits new derived events (which are themselves signed).
- **Depends on:** `events`, `graph`, `reason` (local), a scheduler (throttled, idle/charging-aware).

### 5.10 File reader / ingest (`connector::reader`) — app layer
- **Does:** walks a *granted* folder, converts files to text (reuse desktop `markitdown.rs`), emits **signed memory events** (one per file/chunk) into the engine. Read-only.
- **Interface:** `ingest(grant) -> EventCount`; incremental re-scan by mtime/hash.
- **Depends on:** the grant manager (§5.12), `markitdown`, `events`.

### 5.11 File actuator (`connector::actuator`) — app layer, gated
- **Does:** performs file create/edit/delete **only** via a confirm-each flow: propose → preview diff → user approves → execute → record a signed action event → offer undo.
- **Interface:** `propose_write(op) -> Preview`; `confirm(preview_id) -> Result`. No path is writable without an active write-grant for it.
- **Depends on:** the grant manager, `events` (audit), the desktop confirm UI.

### 5.12 Grant manager (`connector::grants`) — app layer
- **Does:** the permission model. Folder allowlist; separate **read** and **write** grants; OS permission acquisition (macOS Full Disk / per-folder); the built-in **never-touch list**; one-click revoke.
- **Interface:** `grant(path, mode)`, `revoke(path)`, `is_allowed(path, mode) -> bool`, `grants() -> [Grant]`.
- **Depends on:** `store` (grants persisted as signed events), OS APIs.

---

## 6. Data flow

- **Write (remember):** caller → `events.append` → sign + chain → store → derive (enqueue embed + keyword index + graph extraction). Indexes update asynchronously; the event is durable the instant it's signed.
- **Read (recall):** query → `recall` (embed → hybrid → rerank → graph/recency boost) → top-N scored memories (with provenance/evidence).
- **Evolve:** background `evolve.tick` → local reasoner reads recent events → proposes summaries/links/invalidations → appended as new signed events → indexes/graph update.
- **Ingest:** `reader.ingest(grant)` → markitdown → chunk → `events.append` (each a signed `file_ingested` memory).
- **Act (write a file):** `actuator.propose_write` → preview/diff shown → user confirms → execute → `events.append` (`file_written` action event) → undo token retained.

---

## 7. Data model (initial — start narrow, derive the rest)

- `events` — the signed log (§5.2). The only authoritative table.
- `vectors` — `(event_id, embedding, model_id)`; derived. (sqlite-vec virtual table in v1.)
- `fts` — FTS5 over event text; derived.
- `nodes` / `edges` — bi-temporal graph (`t_valid, t_invalid, t_created, t_expired`); derived.
- `pages` — LLM-derived summary pages (from evolve); each backed by source event ids.
- `grants` — folder permissions (persisted as signed events; this table is a projection).

Resist widening `events`. Richer structure is derived, not stored on the event.

---

## 8. Security & safety

### 8.1 Encryption at rest
- A **data-encryption-key (DEK)** is stored in the OS keychain (reuse the desktop `vault.rs` / `secrets/`; hardware-backed on macOS). DB + any sidecar index files are encrypted with it.
- **KDF:** because the key lives in the keychain, no passphrase KDF is needed in v1 — use **HKDF** to derive purpose-specific subkeys from the identity/DEK. (Argon2id only if a user passphrase is ever added.)
- **Open spike (must do early):** confirm `sqlite-vec` loads inside an encrypted SQLite (e.g. SQLite3MultipleCiphers/SQLCipher). If incompatible, the vector index moves to a **separate encrypted sidecar file** (app-layer XChaCha20-Poly1305) — contained, since the index is derived + rebuildable.

### 8.2 Cryptographic ownership (the moat)
Every memory and every file action is Ed25519-signed to the user's DID and hash-chained. The store is therefore self-verifying and **portable + inalienable** — and doubles as a **tamper-evident audit trail** of everything the assistant read or wrote.

### 8.3 File permission model
- Scoped **allowlist** grants only — never blanket root access. Backed by the OS's own permission prompt.
- **Read and write are separate grants.** A read grant never implies write.
- Built-in **never-touch list:** `.ssh`, `.gnupg`, keychains, `.env`, `*.key/*.pem`, OS/system dirs, the BossClaw store itself.
- All grants **revocable in one click**; revocation is itself a signed event.

### 8.4 Injection defense (critical)
Ingested file content and retrieved memories are **data, never instructions.** The reasoner receives untrusted content inside an explicit fence (byte-identical discipline to the messaging `channel.mjs` / Phase B fence). A booby-trapped file therefore **cannot** make the model issue commands on its own — and even if a prompt tried, **every write is confirm-each**, so a human approves before anything touches disk. Defense in depth: fence + confirm gate + never-touch list + signed audit.

### 8.5 Write safety
- Confirm-each: propose → preview/diff → approve → execute → undo token.
- No autonomous/unattended writes in v1 (deferred, would require the broader Mandate).

---

## 9. The chosen stack (from deep research, 2026-06-15)

Sourced from a 109-agent adversarially-verified deep-research pass (23/25 claims confirmed). Full basis: GBrain `air/bossclaw-core-stack-research-2026-06-15`.

| Concern | Pick | Why / source |
|---|---|---|
| Store | `rusqlite` (bundled SQLite) | single file, FTS5, one language |
| Vector index (v1) | `sqlite-vec` (official Rust crate, static-linked via `cc`) | lowest friction, in-file; brute-force is fine at personal scale. *Pre-1.0 alpha — maturity flag.* |
| Vector index (scale) | `hnsw_rs` 0.3.4 (pure-Rust ANN, MIT/Apache-2.0) | true ANN; sqlite-vec degrades to "seconds" at 1M+ |
| Embedder runtime | `fastembed` v5.16.2 (ort/ONNX, no Python) | model-swappable; *ort wraps C++ ONNX Runtime — must bundle/static-link weights for true offline single binary (integration risk to design around)* |
| Default model | `bge-small-en-v1.5` | beats all-MiniLM at same runtime; fastembed's default |
| Quality upgrade | `EmbeddingGemma-300M` (Matryoshka 768→128, <200MB RAM) | multilingual quality jump; ~6–7× MiniLM size |
| Pure-Rust fallback | `model2vec` / `potion-base-8M` (`model2vec-rs`) | static embeddings, ~8–30MB, ~500× faster CPU, no neural runtime; ~8% quality cost |
| Hybrid + rerank | `fastembed` SPLADE + `bge-reranker-base` | dense+sparse+rerank in one crate, CPU |
| Graph/truth design | adopt **Graphiti** bi-temporal model | valid-time vs ingestion-time, invalidate-not-delete (design only) |
| Signing | `ed25519-dalek` via `air-rs/signing.rs` | reuse |
| Avoid | DuckDB VSS | re-serializes whole index on checkpoint; deletes only marked; WAL recovery broken — disqualifier for a rebuildable index |

---

## 10. Error handling
- **Store/IO errors** surface as typed errors (`thiserror`); never panic in library code.
- **Index/embed failures** degrade gracefully: recall falls back to keyword-only if the vector path is unavailable; a failed derive is retryable from the event log (source of truth intact).
- **Reasoner failures** (local model down): the evolve loop backs off and retries; recall still works (it doesn't need the reasoner). Cloud escalation failures fall back to local or surface a clear "couldn't synthesize" state.
- **File ops:** any actuator failure leaves the filesystem unchanged (write to temp + atomic rename); partial ingests are resumable.

## 11. Testing strategy
- **Unit:** each component against its trait. Event log: append/verify/replay; tamper a byte → chain verify fails.
- **Property/golden:** rebuild all indexes from the event log → identical recall results (the source-of-truth invariant).
- **Recall quality:** a small fixture corpus + labelled queries; track recall@k as models/rankers change (so "upgrades" are measured, not assumed).
- **Security:** injection-fence tests (a malicious ingested file must not alter reasoner behavior / must not produce an unconfirmed write); never-touch-list enforcement; encrypted-store round-trip.
- **Hermeticity:** temp homes only; never touch the real store under test (same discipline as the messaging suite's `bridgeHome` guard).
- **Spike test:** sqlite-vec under encrypted SQLite (§8.1) — gating.

## 12. Build sequence (internal milestones within v1)
1. **Bedrock:** crate skeleton + encrypted store + signed event log + verify/replay. (Proves "remember + yours.")
2. **Recall:** embedder (bge-small) + sqlite-vec + FTS5 + hybrid + rerank. (Proves "finds it again.")
3. **Graph:** bi-temporal schema + backlinks. 
4. **Reasoner + evolve (minimal):** local reasoner + the extract/link/summarize pass. (Proves "thinks/evolves on its own.")
5. **Ingest:** read-only folder ingest via grants + markitdown. (Fills the brain with real files.)
6. **Actuator:** confirm-each writes + undo + audit. (The "does things" capability.)
7. **Desktop surface:** Tauri commands/events; a Memory panel (plain words: "Journal", "Notes", "What does it know?").

Each milestone is independently demoable.

## 13. Deferred (separate later sub-projects)
Universal hub (history import + multi-tool shared memory) · tool-orchestration · sign/pay Mandate · **silent autonomous writes** · multi-device CRDT sync (cr-sqlite candidate) · model self-training · `EmbeddingGemma`/`hnsw_rs` swap-in at scale.

## 14. Open questions / spikes
1. **sqlite-vec under encryption** (§8.1) — gating spike; decides in-file vs sidecar index.
2. **ort static-linking / weight bundling** across macOS/Win/Linux for a true offline single binary — or make `model2vec-rs` the shipped default and `fastembed`+bge-small an opt-in download.
3. **Local reasoner backend:** require/detect Ollama vs bundle llama.cpp directly (turnkey vs flexibility).
4. **Evolve scheduling:** cadence + resource policy (idle/charging-aware throttling).

## 15. Research basis
Deep-research report (2026-06-15): 5 angles → 27 sources → 133 claims → 25 verified (23 confirmed, 2 killed). Primary sources: hnswlib-rs (`hnsw_rs`), USearch BENCHMARKS, sqlite-vec Rust docs, fastembed-rs, EmbeddingGemma (Google/HF/arXiv 2509.20354), model2vec (MinishLab), Graphiti (arXiv 2501.13956). To be saved to GBrain as `air/bossclaw-core-stack-research-2026-06-15`.
