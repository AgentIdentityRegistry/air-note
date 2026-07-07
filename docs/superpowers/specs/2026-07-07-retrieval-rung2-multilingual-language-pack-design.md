# Retrieval Floor Phase 1 — Rung 2: Multilingual Embedder as an Opt-In Language Pack (Design)

**Status:** Draft 1 (post dual-review — architect SOUND-WITH-CHANGES + critic MAJOR-REWORK, both fixes folded in). Awaiting Peter's spec review → then `writing-plans`.

**Program:** Memory Hub Phase 1 Retrieval Floor, rung 2. Part of the ★ North Star strategy `air/memory-strategy-2026-07-03-beat-the-stack` (phase order: 0 measure → 1 retrieval floor → 2 multilingual → …). Rungs 0–1 shipped (main `f6c4cbc`). Rung 3 (chunking) built + measured + **shelved** as a dead end for this bag-of-words embedder.

**Goal:** Let a user gain multilingual memory-search (notably Korean) by opting into a downloadable **language pack** — the larger `minishlab/potion-multilingual-128M` embedder — from the desktop app's Settings, WITHOUT changing anything for the default English-only user. Enabling it re-embeds the user's existing memories under the new model so all recall works in their language.

---

## 1. Why (the measured win) and why opt-in

The frozen A/B harness (rung 0 protocol, paired per-case Wilcoxon on the frozen Phase 1 corpus + 396 cases) measured `potion-multilingual-128M` vs the current `potion-base-8M` by varying only `BOSSCLAWD_MODEL_DIR` (`air/rung2-multilingual-measurement-2026-07-06`):

| segment | n | potion-base-8M | multilingual-128M | Δ | paired p |
|---|---|---|---|---|---|
| synthetic·ko·known-item (gate) | 75 | 0.147 | **0.360** | **+0.213** | **0.0004** ✅ |
| synthetic·en·known-item (gate) | 225 | 0.778 | 0.729 | **−0.049** | **0.0423** ⚠️ |

This is a **real tradeoff, not a clean win**: multilingual more than doubles Korean recall but significantly regresses English by ~5 points (a multilingual-vs-specialist capacity split). A forced global swap would trip the frozen rung-2 gate ("synthetic·en no significant regression").

**The opt-in language-pack framing dissolves the tradeoff.** English-only users (the default) keep `potion-base-8M` and see zero change. Only a user who deliberately enables multilingual pays the −5% English cost — and they've decided the +21% Korean is worth it. The gate is satisfied by the architecture: the default experience is provably unchanged.

**Key enabling fact (confirmed by the frozen probe):** `potion-multilingual-128M` runs at **dimension 256 — identical to `potion-base-8M`**. So a model change requires **no vector-table reshape**; migration is purely "re-embed every event under the new `model_id`." The `vectors` schema already stores `dim` per-row and keys on `model_id`, so heterogeneous models coexist safely by construction.

## 2. Product decisions (locked with Peter, 2026-07-07 — do not relitigate)

1. **Opt-in language pack**, not a default swap. English (`potion-base-8M`, ~30 MB) stays bundled and default. Multilingual (~506 MB) is a user-initiated download.
2. **One-way for v1.** No in-app "turn multilingual back off / revert to English" path. (A user can reset via the existing Danger-Zone identity reset, which re-embeds from the event log under the then-default model. Documented, not a feature.)
3. **Download source = a GitHub Release** in the AIR org (we upload the 3 files once; we pin sha256 for each; the safetensors sha is cross-verified against Hugging Face's independent LFS oid, per the `fetch-model.sh` precedent). Fail-closed on any mismatch.
4. **Discovery = a manual toggle** in Settings (Brain → Search & Evolve). No language-detection nudge in v1.

## 3. Verified current-state reality (the ground the design stands on)

> These anchors were verified by two independent reviewers reading `origin/main` (`f6c4cbc`). The current working tree is on the **shelved** `feat-retrieval-rung3-chunking` branch; its `+chunks-v2` id, `chunk_ix` column, and `chunk_text` calls are branch-only and **will NOT be present** on the rung-2 base. **Every line number below is re-verified against `main` at plan-writing time** — treat them as pointers, not literals.

- **Two-program architecture.** The desktop app (`apps/desktop/src-tauri`) does **not** embed. It talks to a separate daemon `bossclawd` (`crates/bossclawd`) over a Unix-socket RPC (`bossclawd-proto`). The embedder + `EventLog` + vector store live in the daemon (via `bossclaw-core`).
- **The daemon has two launchers with different lifecycles — THIS is the load-bearing reality the design must respect:**
  - **Service-managed (installed) path:** `scripts/install-bossclawd.sh` writes a launchd plist / systemd unit that **hardcodes `BOSSCLAWD_MODEL_DIR` to the English bundle path** (`install-bossclawd.sh:85,240,311`) and runs the daemon under **launchd `KeepAlive: true` / systemd `Restart=always`** (`bossclawd.plist.in`, `bossclawd.service.in`). Killing it → the service manager relaunches it **with the English env**.
  - **App-spawn path:** `apps/desktop/src-tauri/src/engine/daemon.rs` `ensure_started` is probe-then-spawn only; its comment states *"we never kill or supervise the daemon."* The spawned child is moved to a detached reaper thread — **no kill handle is retained**. If a daemon already answers the socket, a fresh spawn is never attempted, and the single-owner advisory lock + `AddrInUse → exit 0` gate (`bossclawd/src/main.rs`) makes a second instance refuse to start.
  - **Consequence:** an app-driven "restart the daemon with a new env var" is **impossible on the installed path and unimplemented on the app-spawn path.** The design must not depend on it. (This is the #1 finding both reviewers converged on.)
- **Embedder loader:** `crates/bossclaw-core/src/model2vec.rs` — `Model2Vec::from_pretrained(dir, model_id)` takes the physical dir and the reported id as **separate** args; `model_id()` returns that string; `dim` is **probed at load** (no dim constant); mean-pooling StaticModel (no context window). The `hf-hub` network feature is **deliberately disabled** — there is no runtime-download path today.
- **Model id today:** `crates/bossclawd/src/engine/embed.rs` has a hardcoded `MODEL_ID` const (on main: the base id `"minishlab/potion-base-8M"`), passed as the reported id in `from_pretrained(&self.model_dir, MODEL_ID)`. **This const is the seam the design replaces.**
- **Re-embed engine (exists, unused):** `crates/bossclaw-core/src/log.rs` `reembed_migration(&dyn Embedder)` — appends a `config` event naming the new model **first**, re-embeds via `rederive_pending` (which **skips** individual embed failures with a warn, but propagates DB-write errors), then GCs (`DELETE FROM vectors WHERE model_id != ?1`), then `rebuild_indexes`. Returns `Ok(ReembedStats)`. **Zero production callers.** `set_active_model(model_id, dim)` records a signed `config` event only (no re-embed).
- **Recall is driven by the *loaded* embedder's `model_id`,** not by `active_model()`: `engine/mod.rs` recall → `ensure_indexed` → `rebuild_indexes(embedder)` → `vectors_for_model(embedder.model_id())`. A missing/empty vector arm degrades to keyword-only with only a `log::warn` (`log.rs` `resolve_arms`) — i.e. **wrong-model load is silent today.**
- **Vector store:** `log.rs` `vectors` table, PK includes `(event_id, model_id, …)` with per-row `dim`. A separate `entity_vectors` table (PK `(entity_id, model_id)`) exists and is **not** touched by `reembed_migration`.
- **RPC surface:** `crates/bossclawd-proto/src/lib.rs` `Request`/`Response` enums have **no** shutdown/reload/model-swap op (only `Teardown` = full identity reset). `crates/bossclawd/src/server.rs` `serve_connection` is a strict one-frame-request / one-frame-response loop — **no streaming**, so progress must be **polled** via `Status`.
- **Model provisioning today:** `scripts/fetch-model.sh` (build-time curl of `model.safetensors` / `tokenizer.json` / `config.json` + sha256 verify, fail-closed) → `tauri.conf.json` bundles the dir as a **read-only** app resource. A downloaded model must instead land in a **writable** app-data dir (`<data_dir>/models/…`; the daemon's own default fallback already lives under `<data_dir>/models/potion-base-8M`).
- **Settings UI home:** `apps/desktop/src/memory/MemoryPanel.tsx` (Brain → Search & Evolve). Its poll loop (`refreshStatus`), toggle idiom (`onToggleEvolve` + `toggling` flag), and the "local model: ready / pull" hint block are the templates to mirror. Reusable primitives: `components/ui/ToggleSwitch.tsx`, `components/ui/SettingsSectionCard.tsx`. API wrappers live in `apps/desktop/src/api/engine.ts`.

## 4. Design principles / invariants (every unit upholds these)

- **I1 — Pull, not push.** The daemon resolves *which brain to load itself*, from state in the shared `data_dir`, **not** from an env var pushed by whichever launcher started it. (Env remains a **dev/harness override** at highest priority so the frozen A/B mechanism keeps working.) This is the fix for the two-launchers problem.
- **I2 — One source of truth.** The **signed `config` event** in the encrypted log is the sole authority for "which model is enabled + its verified safetensors sha + user consent." There is **no** second app-local pointer that could drift. State changes are daemon-mediated (only the daemon holds the signing keystore).
- **I3 — Fail loud, never fall back.** If the intended (signed) model's folder is missing, or its safetensors sha does not match the signed sha, the daemon **refuses to serve recall/ingest** and surfaces a visible "model missing / mismatch" state. It must **never** silently serve a different brain than the one intended.
- **I4 — Verify, then name.** A model folder's `model_id` is bound to its **verified** `model.safetensors` sha by the downloader *after* the checksum passes — never trusted from a label shipped inside the release. (Both models are 256-dim, so the dim probe cannot catch a mislabel; the sha binding is the only real guard.)
- **I5 — All-or-nothing migration.** The old model's vectors are GC'd **only after** the new model's vectors are proven complete for every embeddable event. On any shortfall: no GC, old model stays active, return `Err`, surface to UI. A partial re-embed must never be reported as success and must never strip an event's only vector.
- **I6 — Consent-gated, never auto.** A GC-bearing migration runs **only** on explicit user consent recorded as a signed event — never auto-fired by a bare "zero vectors for the loaded model" heuristic (which would thrash the brain between languages on any hiccup). The only boot-time migration is **resuming** an already-consented, interrupted one.
- **I7 — English default is output-identical to today.** With no signed multilingual record, the daemon loads bundled English, reports the base id, writes identical vectors, builds an identical index, and runs no migration. A guard test asserts vector + id + index identity on the default path. (Scoped to *output* identity — a cheap signed-config read does run on the default path; that is not a behavior change.)

## 5. Architecture: pull-based resolution + a consent-gated, crash-safe reload

**Activation mechanism (the core change from the rejected first draft):** the daemon owns model selection. The app's only jobs are (a) download + verify the files and (b) send one RPC. Everything that mutates state happens inside the daemon, which already holds the keystore and the store.

**Happy-path flow when a user enables multilingual:**

1. **Preflight (app):** check free disk (need headroom for the ~506 MB download plus a transient copy during atomic rename). Refuse early with a clear message if insufficient.
2. **Download (app → Rust):** fetch the 3 files from the pinned GitHub Release into a temp dir under `<data_dir>/models/.tmp-…/`; verify each file's sha256 (fail-closed, `rm` on mismatch); on all-3-pass, **atomically rename** into `<data_dir>/models/potion-multilingual-128M/`; write the id-binding (`air-model.json` with `{model_id, safetensors_sha}`) **ourselves** from the verified sha (I4). Progress streamed to the UI throughout.
3. **Enable (app → daemon RPC):** a new `SetActiveModel { model_id, safetensors_sha }` request. The daemon validates the folder exists and its safetensors sha matches; on success it writes a **signed `config` event** recording consent + `active_model_id` + `safetensors_sha` + a durable **"migration in progress"** marker; then it starts the migration as a **background task** and returns immediately (the UI polls progress).
4. **Migrate (daemon, background, crash-safe — I5):** re-embed every embeddable event under the new `model_id`, writing new-id vectors **alongside** the existing old-id rows. During this one-time window the **old (English) model stays active for recall** — its vectors and embedder are still live until the flip — so search keeps working; new **ingest is deferred** with a "re-indexing" response, and the daemon stays responsive to `Status` for progress. Holding both embedders briefly costs ~530 MB resident (English is only ~30 MB) — acceptable. When every embeddable event has a new-id vector (verified by count), **atomically**: flip the serving embedder to the new model, clear the in-progress marker (mark complete), GC the old model's `vectors` **and** `entity_vectors` rows, rebuild indexes. On any failure before the flip: leave the old model active, keep old vectors, set a loud error state, GC nothing.
5. **Active.** Recall now serves multilingual; `Status` reports `Active`.

**Resolution order at load (I1):** `BOSSCLAWD_MODEL_DIR` env (dev/harness override) → **else** the signed `config` `active_model_id` resolved to `<data_dir>/models/<id>/` (verify sha vs signed; fail-loud on mismatch/missing, I3) → **else** bundled English default (base id). Because the embedder is built lazily (first embed/recall), the log is already open when resolution happens, so the signed intent is available.

**Boot with an interrupted migration (I6 exception):** if the daemon boots and finds a signed "migration in progress" marker with recorded consent, it **resumes** the same all-or-nothing migration (re-embed remaining → verify → flip → GC). This is crash-safe and needs no un-consented heuristic. If the marker says "complete," normal load. If absent, normal English/default load.

**Why not the alternatives (recorded, do not revisit without cause):**
- *App restarts the daemon with a new env dir (rejected first draft):* silently no-ops on the service-managed install path (env pinned to English, KeepAlive reverts); no kill handle or shutdown RPC exists. This is the flaw the second opinion caught.
- *Shutdown RPC + rely on auto-relaunch to pull-on-boot:* simpler core (no swappable embedder) but leans on differing relaunch semantics across the two launchers and adds a restart/Unavailable window. Kept as a documented fallback if the in-process reload proves too invasive at plan time; the pull-based resolution (I1) is what actually makes *either* mechanism correct.
- *Live hot-swap while serving (old Approach B):* rejected — in-process teardown under load for marginal UX. The reload here is a *controlled* one-time event — recall keeps serving the old model, progress is shown, and the new model goes live atomically at the end — not a mid-flight swap under load.
- *Dual-brain with language routing (old Approach C):* rejected — double storage + language detection + cross-model score fusion. YAGNI; Peter accepted the per-user tradeoff.

## 6. Units (each: purpose · interface · dependencies)

- **U1 — Pull-based model resolution** (`bossclawd/src/main.rs` `resolve_model_dir` + `engine/embed.rs`). *Purpose:* resolve the active model from env-override → signed config → bundled default, at embedder-build time. *Interface:* returns `(model_dir, model_id, expected_sha)`. *Depends on:* the opened `EventLog` (signed config reader), the model2vec loader. Replaces the hardcoded `MODEL_ID` const.
- **U2 — Verified model identity binding** (downloader + `engine/embed.rs` load check). *Purpose:* bind `model_id` to the verified safetensors sha (I4); the daemon reports the signed id and re-verifies the sha at load. *Interface:* `air-model.json {model_id, safetensors_sha}` written by us; loader asserts folder sha == signed sha. *Depends on:* U1, U3.
- **U3 — Downloader** (new Rust module + Tauri command; app `api/engine.ts` wrapper). *Purpose:* preflight disk → fetch 3 files from the pinned GitHub Release → per-file sha256 verify (fail-closed) → atomic temp→rename → write id-binding. *Interface:* `download_language_pack() -> progress events`; `Response` carries state (`Downloading{pct}` / `Verified` / `Failed{reason}`). *Depends on:* pinned URLs + shas (ops task §10); no `bossclaw-core` change.
- **U4 — Enable RPC + crash-safe migration** (`bossclawd-proto` new `SetActiveModel`; `bossclawd/src/server.rs` dispatch + background task; `bossclaw-core` hardened `reembed_migration`). *Purpose:* validate folder+sha, write signed consent + in-progress marker, run the all-or-nothing migration (I5) with the atomic end-flip, GC `vectors` **and** `entity_vectors`, rebuild indexes. *Interface:* `SetActiveModel{model_id, safetensors_sha} -> Response::Accepted`; progress via `Status`. *Depends on:* U1, U2, U6.
- **U5 — Fail-loud loaded-vs-intended guard** (`engine/mod.rs` boot/first-use). *Purpose:* if loaded model's verified sha/id ≠ signed intent (e.g. profile copied to a machine lacking the folder), refuse recall/ingest and surface a loud state (I3). *Interface:* a `ModelState` in `Status` (`Ok` / `Missing{expected}` / `Mismatch{expected, loaded}`). *Depends on:* U1, U6.
- **U6 — Progress + model-state reporting** (`bossclawd-proto` `Status` extension; `server.rs`). *Purpose:* expose reindex progress + `ModelState` for polling (no streaming exists). *Interface:* `Status` gains `{ model_state, reindex: Option<{done, total}> }`. *Depends on:* nothing new; consumed by U4/U5/U7.
- **U7 — Settings UI card** (`apps/desktop/src/memory/MemoryPanel.tsx` + `api/engine.ts`). *Purpose:* one `SettingsSectionCard` with states `Not-installed → [Enable multilingual (≈500 MB)] → Downloading{pct} → Re-indexing{done/total} → Active`, plus a loud `Model missing/mismatch` state. Mirrors the existing Ollama "model not installed / pull" card. *Interface:* polls `Status`; calls `downloadLanguagePack()` then `setActiveModel()`. *Depends on:* U3, U4, U6.
- **U8 — `entity_vectors` in the migration** (`bossclaw-core` `reembed_migration`). *Purpose:* GC + rebuild `entity_vectors` alongside `vectors` so entity resolution isn't left empty/orphaned under the new model. *Interface:* internal. *Depends on:* U4. (If the entity vectors are always re-derivable on demand and this proves large, the fallback is to explicitly scope it out with a documented consequence + follow-up — decided at plan time.)

## 7. User-facing states (Settings card)

`Not installed` → click Enable → `Checking space` → `Downloading 41%` → `Verifying` → `Re-indexing your memories 220/1,043` (English search still works; Korean lights up when it finishes) → `Multilingual active`. Failure states are explicit and actionable: `Not enough disk space (need ~1.5 GB free)`, `Download failed — retry`, `File check failed — retry` (fail-closed), `Model files missing — re-download` (I3/profile-copy), `Re-index didn't finish — retry` (I5, old model still active).

## 8. Error-handling matrix

| Failure | Behavior |
|---|---|
| Insufficient disk (preflight) | Refuse before download; clear message. |
| Download interrupted / network error | Temp dir discarded; card returns to `Not installed`; retry. Old model untouched. |
| sha256 mismatch on any file (I4) | Fail-closed: delete temp, do **not** install/enable; `File check failed`. |
| Crash mid-download | Temp dir orphaned (namespaced `.tmp-…`), swept on next enable; no partial model ever activated (atomic rename). |
| Crash mid-migration (I5/I6) | In-progress marker persists; old vectors intact (GC not yet run); on next boot the migration **resumes**; recall serves old model meanwhile. |
| Migration count shortfall (embed failures) | No GC; old model stays active; `Err` surfaced; `Re-index didn't finish`. |
| Signed intent = multilingual but folder gone (profile copy) | Fail-loud `Model missing` (I3), never silent English; offer re-download. |
| Wrong env override in dev | Env is dev-only + highest priority by design; not a production path. |

## 9. Testing strategy

- **Unit:** downloader rejects a corrupted file (sha mismatch → no install); atomic rename; preflight refuses on low disk. Model resolution order (env > signed > default) and fail-loud on sha mismatch/missing. Migration all-or-nothing: an injected embed shortfall → `Err` + **no** GC + old vectors intact. Id-binding written from the verified sha.
- **Integration (the real path — the frozen A/B does NOT exercise the swap machinery):** seed a brain with EN+KO events under English; run `SetActiveModel` → assert exactly one vector per embeddable event under the multilingual id, old id GC'd (`vectors` **and** `entity_vectors`), recall works in both languages, and Korean recall improves. Kill mid-migration → reboot → assert resume completes correctly and no event lost searchability. Signed-intent-but-missing-folder → assert `ModelState::Missing`, recall refuses loudly.
- **Invariance guard (I7):** default (no signed multilingual) path produces byte-identical vectors + `model_id` + index vs the pre-change tree.
- **Quality (reuse, do not re-run):** the +21 ko / −5 en number is a property of the *model*, already measured on the frozen harness — cite `air/rung2-multilingual-measurement-2026-07-06`; do not re-litigate it. The integration Korean-recall smoke above proves the *machinery* produces the multilingual vectors; that is a separate, cheaper assertion.
- **Gates before any measurement run:** `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` green, then the integration path, then (optionally) a single end-to-end frozen-harness confirmation via the real folder (not env-override) to prove pull-resolution matches the probe.

## 10. Delivery / ops (one-time)

- Upload `model.safetensors` (~488 MB), `tokenizer.json` (~18 MB), `config.json` to a **GitHub Release** in `AgentIdentityRegistry/air-note` (dedicated tag, e.g. `models-multilingual-128M-v1`). GitHub allows 2 GB/asset, so 488 MB fits.
- Pin the three sha256 in code (known from the frozen probe: safetensors `14b5eb39…`, tokenizer `19f19090…`, config `595e4cab…` — re-confirm exact full digests at plan time). **Cross-verify** the safetensors sha against Hugging Face's independent LFS oid before pinning (the `fetch-model.sh:16-19` precedent); tokenizer/config remain trust-on-first-download pins.
- License: `potion-multilingual-128M` is MIT — include its `README.md`/model card in the downloaded folder as we do for the bundled model.

## 11. Out of scope (v1) / deferred

- Reversibility (an in-app "revert to English" path).
- Language-detection nudge / auto-suggest.
- Zero-downtime migration (recall pauses during the one-time re-index, with progress; keeping the old model serving *during* migration is a future enhancement).
- Dual-brain / per-language routing.
- Windows service specifics (the installed path here is launchd/systemd; app-spawn covers dev on all OSes — confirm the Windows story at plan time if in scope).

## 12. Open questions to resolve during planning

1. Exact `bossclawd-proto` shapes for `SetActiveModel` + the `Status` extension (field names, enum variants).
2. Confirm `entity_vectors` inclusion (U8) vs documented scope-out, based on how it's populated on main.
3. In-process embedder swap (a guarded swappable `Arc<dyn Embedder>`) vs the shutdown+relaunch fallback — pick one and pin the concurrency contract.
4. GitHub Release tag name + whether to gate the download URL behind a small redirect we control (so the asset can move without a client update).
5. Re-confirm every `origin/main` line-number anchor in §3 (the working tree is currently the shelved rung-3 branch).

## 13. Sequencing / branch

Build **off `origin/main` (`f6c4cbc`)** on a **new** branch `feat-retrieval-rung2-multilingual`. Do **not** touch or build on the shelved `feat-retrieval-rung3-chunking` branch. TDD per unit (RED test committed before implementation), subagent-driven execution, two-stage (spec-fidelity then quality) review per task, whole-impl + dedicated egress/download-security review before the PR — matching the milestone-D / rung-1 process.
