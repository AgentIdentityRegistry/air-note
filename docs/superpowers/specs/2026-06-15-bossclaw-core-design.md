# bossclaw-core — Design Spec

**Status:** Draft for review · 2026-06-15
**Author:** Peter + Claude (brainstorm session, superpowers:brainstorming)
**Repo:** `~/air-note` (canonical) · new crate `crates/bossclaw-core`
**Supersedes for the memory layer:** the "Phase B — Brain" guesswork in [[air/forever-companion-architecture]] (all-MiniLM + sqlite-vec, 2026-05). This spec is the researched, decided version.

## 0. Revision log
- **Rev 2 (2026-06-15):** folded three independent adversarial reviews (architecture / security / critic, all **SHIP-WITH-FIXES**, all codebase-verified). Material changes: two-tier rebuild invariant (model output is committed as signed events, not recomputed); fully-specified event canonicalization + single serialized writer; honest reuse relabeling (the hash chain + at-rest encryption are NET-NEW, not reuse); v1 index default flipped to pure-Rust `hnsw_rs`; hardened file-access security (provenance, canonicalize-then-contain, content-aware never-touch, anti-fatigue confirm, taint-by-origin); key-custody named + truncation/rollback defense; new sections for versioning, backup/export, observability, first-run, supply-chain.
- **Rev 1 (2026-06-15):** initial design from the brainstorm + deep-research pass.

---

## 1. Purpose & scope

`bossclaw-core` is the **embeddable, local-first memory engine** for BossClaw — the part that *remembers everything, finds it again, links it, and lets a model reason + self-organize over it* — bound to the user's AIR identity (DID) by per-event cryptographic signatures.

This is the **first build** of the refined BossClaw direction (the universal never-forget memory hub). It is scoped to **the engine plus its first surfaces inside the BossClaw desktop app**, with the desktop as the only consumer and the user (Peter) as the first daily power-user (dogfooding).

### In scope (v1)
- The `bossclaw-core` Rust crate: encrypted store, signed append-only event log, recall (semantic + keyword + optional rerank), bi-temporal graph, a pluggable reasoner interface, and a minimal always-on "evolve" loop.
- Desktop integration as a Tauri command/event surface.
- **Local file access:** read-ingest of user-granted folders, **plus confirm-each-write** file actions (every write previewed + user-approved). *Milestone 6 (the actuator) is the explicit v1 cut-line — the memory engine ships complete without it if writes slip.*

### Explicitly NOT in scope (deferred sub-projects)
Universal hub wiring (history import + multi-tool shared memory) · tool-orchestration · sign/pay Mandate · **silent / unattended autonomous writes** · multi-device sync (CRDT) and its fork/merge semantics · self-training the local model. These consume or extend this engine.

---

## 2. Product context & the honest capability framing

BossClaw's claim is "a true second brain that remembers everything and can infer + evolve on its own." This spec commits to the **honest, buildable** version:

| User ask | What we deliver | Honesty line |
|---|---|---|
| **Remember everything** | Append-only signed log; nothing edited or deleted. | *Storage* guaranteed; *recall* excellent-not-perfect — a memory is never gone, at worst not-yet-surfaced. Remembers everything it is *given*; full capture is the later "hub" job. **Key loss = unrecoverable memory** (§8.2) — mitigated by a signed export. |
| **Infer + connect on its own** | Engine links structurally (graph + semantic recall + contradiction surfacing); a **language model reasons** over what it surfaces. | The *system* infers; the engine remembers+links+surfaces, the model reasons. Existence proof: GBrain does this daily. |
| **Think + evolve on its own** | An **always-on local-LLM loop** that continuously summarizes, links, retires contradictions, surfaces proactively — for ~$0, private, offline. | It is a model in a loop, not consciousness. "Evolve" = the *memory/graph* evolves (v1); model-weight self-training is a separate later phase. The loop's outputs are **recorded as signed events** (§4), not recomputed. |

**Tiered reasoning (roadmap B):** local model = always-on worker (extract/tag/link/summarize, ~$0); cloud frontier = rare hard-synthesis consultant. Baseline ≈ $0.

**Guaranteed vs. committed:** we *guarantee* the deterministic properties — never-forget storage + cryptographic ownership (every memory signed to the DID). We *commit to build* the connection/self-organization mechanisms and prove their quality by dogfooding + a recall fixture, not by promising a number.

---

## 3. Decisions locked in this brainstorm (+ review)

1. **Scope:** engine-first; desktop = only v1 consumer; hub / orchestration / sign-pay are later.
2. **Substrate:** Rust-native, single language. **Adopt validated *designs*** (GBrain hybrid ranking; Graphiti bi-temporal graph), not GBrain's TS/Postgres runtime, not a from-scratch rebuild.
3. **Default stack favors constraint-coherence over peak benchmark** (review correction):
   - **Index default = `hnsw_rs`** (pure-Rust ANN). `sqlite-vec` is demoted to opt-in/experimental — it is `0.1.10-alpha.4` and its maintainer warns of breaking *on-disk storage-format* changes pre-1.0, which is unacceptable inside a never-forget store.
   - **Embedder default = `bge-small-en-v1.5`** via fastembed (quality-first — recall *is* the product). This bundles **one** native lib (ONNX Runtime via `ort`) — acknowledged, not hidden. **`model2vec`/`potion` (pure-Rust)** ships as the always-available offline fallback and the basis of a fully-pure-Rust build. A **recall@k fixture (§11) is the empirical tiebreak**, not the quoted MTEB delta.
4. **Reasoner:** pluggable + local-first (Ollama / bundled llama.cpp / cloud).
5. **Evolve loop:** core, not polish; minimal in v1; its outputs are signed events.
6. **File access in v1:** **read-ingest + confirm-each writes** (user-chosen). Hardened per §8. Silent/unattended writes deferred.
7. **Reuse — stated honestly (review correction):**
   - **Real reuse:** `ed25519-dalek` + the `serde_jcs` (JCS) canonicalization *discipline* from `air-rs/signing.rs`; the OS-keychain *secret store* in `vault.rs`/`secrets/` (holds string secrets today); the untrusted-content **fence pattern** from `agent-bridge-mcp/channel.mjs`; the replay/bi-temporal lessons in `air-rs/inbox/replay.rs`.
   - **NET-NEW (not reuse — priced into §12):** the hash-chained append-only log (the registry audit log lives in `~/air-site`, in TypeScript, with a *different* recipe — a reference design, not shared Rust code); at-rest **DB encryption + a data-encryption-key** (`vault.rs` has no DEK today); the file **never-touch list + path containment** (no such code exists yet); per-event raw-bytes signing (`signing.rs` only signs the `Envelope` struct).

---

## 4. Architecture overview

Two layers, cleanly separated so the engine stays focused, sandboxable, and testable:

```
┌────────────────────────── BossClaw desktop (Tauri, Rust) ──────────────────────────┐
│  CONNECTOR LAYER (app)                          REASONER BACKENDS                    │
│   • file reader   (ingest → signed events)       • local: Ollama / bundled llama.cpp │
│   • file actuator (confirm-each writes)          • cloud: frontier model (rare)      │
│   • grant manager (allowlist + OS perms)                                             │
│        │  signed events / queries / proposals                                        │
│        ▼                                                                             │
│  ┌──────────────────────────── bossclaw-core (crate) ────────────────────────────┐  │
│  │  SIGNED EVENT LOG (single serialized writer)                                  │  │
│  │    ├─ Tier A (deterministic, rebuildable): vector index · FTS · graph-fold    │  │
│  │    └─ Tier B (model-derived): summary/link/invalidate events — REPLAYED, not  │  │
│  │       recomputed (they ARE log entries)                                        │  │
│  │  recall (embed → hybrid → optional rerank → graph/recency boost)              │  │
│  │  reasoner trait     evolve loop (always-on, local-LLM-driven)                  │  │
│  │  encrypted store  ·  per-event Ed25519 signing (DID-bound)                     │  │
│  └───────────────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

**The two-tier source-of-truth invariant (review fix C1 — load-bearing).**
- **Tier A — mechanically derived, rebuildable byte-for-byte from the log:** the vector index (under a *fixed* active embedding model), the FTS keyword index, and the **graph table (a deterministic fold over signed `link`/`invalidate` events)**. Lose these → rebuild exactly → lose performance, never history.
- **Tier B — model-derived (non-deterministic): summary pages, LLM-extracted links/invalidations.** An LLM's output is **not** a pure function of the log (model version, sampling, kernel nondeterminism). Therefore the evolve loop **commits its conclusions as first-class signed events**; on replay they are **re-loaded as recorded, never regenerated.** They are part of the log, not derived from it.

The §11 "rebuild" test asserts byte-identity for Tier A and replay-fidelity for Tier B — never "re-run the LLM and get the same bytes."

**Single-writer invariant (review fix C3).** The hash chain needs a total order. `events.append` is **strictly serialized through one writer** (a single writer task fed by a channel, or `BEGIN IMMEDIATE` + read-tip→hash→insert in one critical section under a process `Mutex`). The evolve loop is **not** a privileged writer — it enqueues to the same serialized path. Two concurrent appends can never fork the chain.

**The engine never touches the filesystem itself** (beyond its own store files). File I/O lives in the connector layer (which feeds events in and receives proposals out) — so `bossclaw-core` is a pure, sandboxable library. *(Note: the connector's ingest does run an external parser — see §8.6.)*

---

## 5. Components (each an isolated unit)

### 5.1 Store (`store`)
- **Does:** owns the encrypted SQLite DB + the encrypted vector sidecar; transactions; migrations.
- **Interface:** `open(path, dek) -> Store`. The DEK is taken as a `Zeroizing<[u8;32]>` and zeroized on drop; never logged.
- **Depends on:** `rusqlite` (bundled), the encryption layer (§8.1), `zeroize`.

### 5.2 Signed event log (`events`) — the moat (Layer 10)
- **Does:** append-only signed log; the only authoritative store.
- **Event:** `{ id: ULID, ts, valid_time?, type, content, model_meta?, prev_hash, hash, signed_by_did, signature }`. `model_meta` (for Tier-B events) = `{ model_id, prompt_hash, source_event_ids }`.
- **Canonicalization (fully specified — review fix C2, frozen before milestone 1):**
  1. `canon = serde_jcs(event_without { hash, signature })` (JCS, matching the house standard).
  2. `hash = SHA256( prev_hash_bytes ‖ canon )`, where `prev_hash_bytes` = the 32 raw bytes of the previous event's hash; **genesis `prev_hash_bytes` = 32 zero bytes.**
  3. `signature = Ed25519_sign(signing_key, hash)` — signing the hash binds content + chain position.
  4. `verify` = recompute `hash` from `canon` + `prev_hash`, assert equal; verify `signature` over `hash` against the pubkey resolved from `signed_by_did` (pinned).
  - A **frozen test vector** (one known event → known hash → known signature) lands in §11, the same way air-rs pins cross-language vectors.
- **Event types:** `memory`, `file_ingested`, `page` (Tier-B summary), `link` / `invalidate` (Tier-B graph ops), `supersede`, `config` (active model, schema version), `grant` / `revoke`, `file_written`.
- **Interface:** `append(event) -> EventId` (serialized, §4); `stream(since)`; `verify_chain()`; `head_checkpoint()`.
- **Truncation/rollback defense (review fix C3-sec):** maintain a **signed high-water-mark** `{ tip_id, count, tip_hash }` persisted to the keychain (and updated per append). On open, if the live tip is behind the stored high-water → **truncation/rollback detected** → open read-only + alert (§10). A plain chain detects edits but *not* deletion-of-the-tail; the high-water closes that.
- **Depends on:** `store`, `ed25519-dalek`, `serde_jcs`, `sha2`, `zeroize`.

### 5.3 Embedder (`embed`)
- **Does:** text → vector; model-swappable; exactly one **active** model per store (recorded as a `config` event).
- **Interface:** `Embedder` trait: `embed(&[Text]) -> Vec<Vector>`, `dim()`, `model_id()`.
- **Implementations:** `FastEmbed` (default `bge-small-en-v1.5`; upgrade path `EmbeddingGemma-300M`) — bundles the ONNX Runtime native lib; `Model2Vec` (pure-Rust, no neural runtime) — the offline fallback / fully-pure-Rust build.
- **Depends on:** `fastembed` and/or `model2vec-rs`.

### 5.4 Vector index (`index`) — behind a swap-trait
- **Does:** nearest-neighbour over embeddings of the **active** model only.
- **Interface:** `VectorIndex` trait: `add(id, vec)`, `search(vec, k)` *(filtered to `model_id == active` — never score mixed models, review fix C4)*, `remove(id)`, `rebuild(from_events)`, `last_indexed() -> EventId`.
- **Implementations:** `Hnsw` (`hnsw_rs`, pure-Rust ANN) — v1 default, persisted as an encrypted sidecar; `SqliteVec` — opt-in/experimental only (alpha, §3.3).
- **Sidecar crash-consistency (review fix I3):** on open, if `last_indexed()` lags the log tip, **rebuild the tail from the log** (cheap, bounded). The sidecar stores its `last_indexed` as a checkpoint.
- **Depends on:** `hnsw_rs` (or `sqlite-vec`), the encryption layer.

### 5.5 Keyword index (`keyword`)
- FTS5 over event text; Tier-A derived. `search(query, k)`.

### 5.6 Bi-temporal graph (`graph`) — truth-tracker (Layer 8)
- **Does:** entities + relationships with **two clocks** (valid-time vs ingestion-time); backlinks; contradictions **invalidate-not-delete**. Adopts Graphiti's design (not its Neo4j+LLM runtime).
- **Key point (review fix I4/C1):** the graph **table is a deterministic fold** over signed `link`/`invalidate` events (apply in log order → current graph) → **Tier A, rebuildable.** The *extraction* that creates those events is non-deterministic and lives in evolve (§5.9).
- **Interface:** `neighbors(node)`, `as_of(time)`; mutation only via appending `link`/`invalidate` events.

### 5.7 Recall pipeline (`recall`)
- **Does:** embed query → **hybrid** (vector + keyword) → **optional rerank** → boosts (graph proximity, recency-decay, pinned) → top-N with evidence labels. Adopts GBrain's ranking recipe.
- **Reranker behind a trait (review fix I2):** `Reranker` with a **no-op default**, so v1 ships hybrid-without-rerank and adds the cross-encoder (`bge-reranker`, another bundled ONNX model) behind the trait later. *(Dense SPLADE/sparse fusion deferred; if added, sparse vectors live in their own Tier-A derived table.)*
- **Depends on:** `embed`, `index`, `keyword`, `graph`, optional reranker.

### 5.8 Reasoner (`reason`)
- **Does:** "ask a model to reason over retrieved memory." Untrusted context is **fenced** (§8.4); reasoner output is **data, never authority**.
- **Interface:** `Reasoner` trait: `complete(prompt, context)` (+ streaming).
- **Local backend hardening (§8.5/I2):** Ollama bound **loopback-only**; verify the port's owner; **no network egress** from the evolve loop / local reasoner in v1 except an explicit opt-in cloud call to a pinned endpoint; pulled-model **digest-pinning**.
- **Implementations:** `LocalReasoner` (default), `CloudReasoner` (opt-in, escalated hard synthesis only).

### 5.9 Evolve loop (`evolve`) — the "curator"
- **Does:** always-on background worker. On new events + on idle: extract entities/links, write summary pages, retire contradicted facts, queue proactive surfacing. **All outputs are appended as signed Tier-B events** (§4) via the serialized writer. v1 = the minimal pass (extract + link + summarize via the **local** reasoner).
- **Resource policy (review gap):** idle/charging-aware throttle, a hard **off switch**, proposal rate-limiting. Observability: `last_tick`, queue depth, error counts surfaced to the desktop (§ Observability).
- **Depends on:** `events` (serialized append), `graph`, `reason` (local), a scheduler.

### 5.10 File reader / ingest (`connector::reader`) — app layer
- **Does:** walks a *granted* folder, converts files to text (external parser, §8.6), emits **signed `file_ingested` events**. Read-only.
- **Safety (review fix C2-sec):** `WalkDir::follow_links(false)`; **canonicalize each path and assert it stays inside the grant root** (reject symlinked components that escape); apply the **content-aware never-touch filter during the walk** (skip + log `.env`, `*.pem`, `*.key`, `id_*`, `*.p12`, `.aws/`, `.npmrc`, `.git/`, `credentials`, `*.kdbx`, plus a high-entropy-line heuristic).
- **Dedup/supersede (review gap):** incremental re-scan by mtime/hash; a re-ingested edited file appends a `supersede` event so recall surfaces only the latest (no stale duplicates).
- **Signs the canonical post-conversion text** (conversion is lossy-by-design; documented).
- **Depends on:** grant manager, the sandboxed parser (§8.6), `events`.

### 5.11 File actuator (`connector::actuator`) — app layer, gated
- **Does:** create/edit/delete **only** via confirm-each: propose → preview diff → user approves → execute (temp-write + atomic rename) → signed `file_written` event → undo token.
- **Hardening (review fix C1-sec):**
  - **Provenance on every proposal:** show which source event(s) induced it ("this edit was suggested by content from `~/repo/x/README.md`, ingested 2026-06-14").
  - **Write-target ⊆ explicitly write-granted paths**, enforced in the actuator, **canonicalized and re-checked at *execute* time inside the rename critical section** (closes TOCTOU), never widened by model output.
  - **Secret/value-shaped diff guard:** louder confirm for diffs touching money amounts, URLs, keys, `curl|sh`, crontab, shell rc / `.command` / `.sh`.
  - **Taint-by-origin:** any proposal whose causal chain touches ingested (non-user-authored) content → "untrusted-origin" → loud modal.
  - **Anti-fatigue:** no "approve all"; proposals touching different files can't be bundled; deletes always get the loud modal.

### 5.12 Grant manager (`connector::grants`) — app layer
- **Does:** the permission model. Allowlist; **separate read/write grants**; OS permission acquisition (macOS Full Disk / per-folder); the never-touch list; one-click revoke (a signed `revoke` event).
- **Containment:** `is_allowed(path, mode)` operates on the **canonicalized real path** (symlinks resolved) and requires path-segment descent from a granted root.
- **Honest trust boundary (review):** "A read grant ingests *everything* in that subtree, including secrets you forgot were there. We mitigate with pattern excludes + no-symlink-follow, but you are trusting us with the whole tree."

---

## 6. Data flow
- **Remember:** caller → **serialized** `events.append` (sign + chain) → store → enqueue Tier-A derive (embed/FTS/graph-fold).
- **Recall:** query → `recall` (embed → hybrid → optional rerank → graph/recency boost) → top-N with provenance.
- **Evolve:** background tick → local reasoner over recent events → **append signed Tier-B events** (pages/links/invalidations) via the serialized writer.
- **Ingest:** walk grant (no symlink follow, contain, never-touch filter) → sandboxed convert → `supersede`-aware → append signed `file_ingested`.
- **Act:** `propose_write` (provenance + taint + diff guard) → user confirms → **re-canonicalize + re-check target ⊆ grant at execute** → temp+rename → signed `file_written` → undo.

---

## 7. Data model (start narrow; derive the rest)
- `events` — the signed log (§5.2). **Only authoritative table.** Carries all the event types incl. Tier-B (`page`/`link`/`invalidate`) and `config`/`grant`.
- `vectors` — `(event_id, embedding, model_id, dim)`; Tier-A derived; queries filter to the active model.
- `fts` — FTS5; Tier-A derived.
- `nodes` / `edges` — bi-temporal graph; **projection (fold) of `link`/`invalidate` events**; Tier-A.
- `pages` — **projection of `page` events** (the events are authoritative; the table is a convenience view).
- `grants` — projection of `grant`/`revoke` events.
- `config` — active embedding model + `schema_version` (projection of `config` events).
Resist widening `events`; derive richer structures.

---

## 8. Security & safety

### 8.1 Encryption at rest (NET-NEW; builds on existing primitives)
- A **DEK** lives in the OS keychain (the keychain *store* is reused from `vault.rs`; the DEK + DB encryption are new). DB + the **vector sidecar** are encrypted; **HKDF** derives per-purpose subkeys; keys held in `Zeroizing` buffers.
- **The sidecar/index is content-sensitive (review fix I3):** embeddings are invertible, FTS tokens are literally the words, page titles may carry secrets. It is encrypted with a DEK subkey, **no exceptions** ("derived ≠ non-sensitive"). XChaCha20-Poly1305, random nonce per write.
- **Gating spike (§14):** prove that **no plaintext index/FTS pages ever hit disk** under the chosen scheme (SQLCipher/SQLite3MultipleCiphers for the DB, app-AEAD for the hnsw sidecar) — not merely "does it load."
- **No passphrase KDF** in v1 (key is in the keychain); Argon2id only if a user passphrase is ever added.

### 8.2 Cryptographic ownership (the moat) — key custody named (review fix C3-sec)
- **Memory events are signed with the BossClaw memory signing key** (keychain, hardware-backed on macOS, loaded into the engine process at runtime). The plaintext daemon `identity.json` seed in `air-rs/inbox/stores.rs` is **NOT** used for memory signing (and is flagged for migration to the vault).
- **In-process exposure acknowledged + bounded:** the private key is in RAM while signing; hold it in a `Zeroizing`/locked buffer, minimal lifetime, zeroize on drop (no `zeroize` exists in the codebase today — added as a requirement).
- **Truncation/rollback** handled by the signed high-water-mark (§5.2). **Fork/merge** is an open question gated to the deferred CRDT sync.
- **Key-loss honesty:** "lose the keychain DEK with no export → the memory is cryptographically unrecoverable. This is the cost of inalienability." A **signed, portable export** (§ Backup) makes "portable + inalienable" real rather than asserted.

### 8.3 File permission model — see §5.10/§5.12
Canonicalize-then-contain (propose **and** execute); read/write separated; content-aware never-touch during the walk; no symlink follow; honest "we see the whole granted subtree" statement.

### 8.4 Injection defense — honest scope (review fix C1-sec)
Ingested content + retrieved memories are **data, never instructions**, fenced into the reasoner (the `channel.mjs` fence pattern). **But that fence is not, by itself, sufficient here:** in `channel.mjs` the fenced path is summary-only with a human-gated send; in bossclaw the same bytes feed a reasoner that can *propose disk writes*. So a booby-trapped file can't *command* the model but **can socially-engineer a benign-looking write proposal** (confused deputy). Defense-in-depth, all required: **fence + write-target restriction + execute-time re-check + provenance display + taint-by-origin + secret-shaped-diff guard + anti-fatigue confirm**. §8.4 prose says "raises the bar against direct injection; does **not** by itself stop confused-deputy proposals" — never "cannot."

### 8.5 Write safety
Confirm-each with the §5.11 hardening; no autonomous writes in v1.

### 8.6 Supply chain & the ingest parser (review fix I1/M3)
- **Ingest shells out to an external document parser** (`markitdown`, a Python stack) on **attacker-controlled files** — a real parser/zip-bomb/XXE surface **outside** the Rust sandbox. Run it in a restricted child: **timeout, memory cap, output-size cap, no network**, reject/expand archives under a depth+size budget.
- **Audit all trust roots in CI:** `cargo-deny`/`cargo audit` (Rust), `pip-audit` (the parser venv), the `ort`/ONNX native blob, and **digest-pinned** model weights.

---

## 9. The chosen stack (from deep research + review)

Deep-research basis: 109-agent adversarially-verified pass (23/25 claims). Full record: GBrain `air/bossclaw-core-stack-research-2026-06-15`.

| Concern | Pick | Notes |
|---|---|---|
| Store | `rusqlite` (bundled) + at-rest encryption | one file (+ encrypted sidecar) |
| Vector index (default) | **`hnsw_rs` 0.3.4** (pure-Rust ANN) | encrypted sidecar; no alpha/foreign-runtime risk |
| Vector index (opt-in) | `sqlite-vec` | **alpha `0.1.10`, storage-format may break — not the default** |
| Embedder (default) | `fastembed` `bge-small-en-v1.5` | quality-first; bundles ONNX Runtime (one native lib) |
| Embedder (fallback / pure-Rust build) | `model2vec` / `potion-base-8M` | ~8% below all-MiniLM, **~10–15% below bge-small** (recall fixture decides) |
| Embedder (upgrade) | `EmbeddingGemma-300M` | Matryoshka, multilingual; re-embed migration required |
| Rerank | `bge-reranker` behind a no-op-default trait | another bundled ONNX model; optional in v1 |
| Graph/truth design | **Graphiti** bi-temporal (design only) | table = fold over signed link/invalidate events |
| Signing | `ed25519-dalek` + `serde_jcs` discipline | raw-event signing is net-new (not `sign_envelope`) |
| Avoid | DuckDB VSS | whole-index re-serialize on checkpoint; broken WAL recovery |

**Single-binary claim, stated honestly:** the **quality default bundles one native lib** (ONNX Runtime); a **fully pure-Rust build exists** (`model2vec` + `hnsw_rs`, no native deps). Not "zero native libraries."

---

## 10. Error handling
- Typed errors (`thiserror`); no panics in library code.
- **`verify_chain()` / high-water failure → open read-only + alert the user** (a tamper/rollback that's silently ignored is no protection); never auto-proceed.
- Index/embed failure degrades to keyword-only recall; a failed derive is retryable from the log.
- **First-run / no local model installed (review gap):** ingest + recall still work; the evolve loop **queues** and surfaces "waiting for local model" rather than blocking or silently dropping.
- Reasoner-down: evolve backs off; recall unaffected. Cloud-escalation failure → local fallback or a clear "couldn't synthesize."
- File ops: failure leaves the FS unchanged (temp + atomic rename); partial ingests resume.

## 11. Testing strategy
- **Two-tier rebuild (review fix C1):** Tier A → rebuild from log is **byte-identical**; Tier B → replay reproduces recorded events (never "re-run the LLM and match bytes").
- **Canonicalization:** the frozen event test vector (§5.2).
- **Security:** injection confused-deputy (a malicious file must not yield an *unconfirmed* write, and its proposal must show untrusted provenance); never-touch + symlink/`..` traversal enforcement; **truncation/rollback detection**; **no-plaintext-index-on-disk** (the §8.1 spike, as a test).
- **Recall quality:** a labelled fixture corpus; `recall@k` tracked across model/ranker changes — **this is the empirical embedder-default gate**.
- **Hermeticity:** temp homes only; never touch the real store (the messaging suite's `bridgeHome` guard discipline).
- **Supply chain:** `cargo-deny` + `pip-audit` in CI.

## 12. Build sequence (milestones; each demoable)
1. **Bedrock:** crate + encrypted store + signed log. **DoD includes the C1/C3 decisions** — the event schema carries Tier-B event types and the serialized-writer append path + the frozen canonicalization vector. (These are the most expensive things to change later.)
2. **Recall:** embedder (bge-small) + `hnsw_rs` + FTS5 + hybrid. *(A tiny throwaway ingest exists here to feed the recall fixture.)* **Go/no-go gate:** the §8.1 encryption spike + the ort-bundling spike resolve here; if either fails, fall back to the pure-Rust path (`model2vec`).
3. **Graph:** bi-temporal fold over link/invalidate events + backlinks.
4. **Reasoner + evolve (minimal):** local reasoner + extract/link/summarize emitting signed Tier-B events; resource policy + off switch + observability.
5. **Ingest:** read-only folder ingest via grants (contain + never-touch + dedup) + sandboxed parser.
6. **Actuator (v1 CUT-LINE):** confirm-each writes + provenance + taint + execute-time containment + undo. *A slip here never blocks shipping the engine (1–5).*
7. **Desktop surface:** Tauri commands/events; a plain-words Memory panel ("Journal", "Notes", "What does it know?").

## 13. Deferred
Universal hub · tool-orchestration · sign/pay Mandate · **silent autonomous writes** · multi-device CRDT sync **and its fork/merge semantics** (cr-sqlite candidate) · model self-training · SPLADE/sparse fusion · HyDE/ColBERT · `EmbeddingGemma`/`sqlite-vec` opt-ins at scale.

## 14. Open questions / spikes
1. **Encryption — no plaintext index/FTS on disk** (§8.1): the gating spike; decides SQLCipher-vs-app-AEAD and DB-plus-sidecar layout.
2. **ort offline bundling** across macOS/Win/Linux vs making `model2vec` the shipped default (resolves at the milestone-2 gate).
3. **Local reasoner backend:** require/detect Ollama vs bundle llama.cpp.
4. **Evolve scheduling policy** specifics (cadence, thermal/battery thresholds).
5. **Tier-B determinism (optional):** could a pinned greedy local model make some Tier-B output reproducible enough to keep a weak golden test? Spike before finalizing §11 wording.

## 15. New cross-cutting concerns (added in review)
- **Versioning & migration:** one active embedding model per store (config event); a model change triggers a **re-embed migration** (replay → re-embed → atomic active-model switch → GC stale vectors) — the one *expensive* Tier-A rebuild; budget it. A `schema_version` gates store format.
- **Backup / recovery / export:** a **signed, portable export** of the event log (makes §8.2 "portable" real); documented restore + corruption-recovery (rebuild Tier-A from the log). For a never-forget product this is first-tier, not optional.
- **Observability:** evolve `last_tick` / queue depth / error counts; recall latency; index lag vs log tip — surfaced to the desktop so Peter (dogfooding) can see it's alive, stalled, or thrashing.
- **First-run:** defined behavior before any local model is installed (§10).

## 16. Research & review basis
- Deep-research (2026-06-15): 5 angles → 27 sources → 25 verified (23 confirmed). GBrain `air/bossclaw-core-stack-research-2026-06-15`.
- Independent review (2026-06-15): three adversarial reviewers (architecture / security / critic), all **SHIP-WITH-FIXES**, codebase-verified. Their CRITICAL/IMPORTANT findings are folded into Rev 2 above.
