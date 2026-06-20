# bossclaw-core Extraction-from-Files — Design Spec (Rev 2)

- **Date:** 2026-06-20
- **Status:** Rev 2 — **post-review**. Independent critic (verdict: NO-SHIP) + security (verdict: SHIP-WITH-FIXES, 1 Critical) reviewed Rev 1 and **converged on the same door**: the dossier path (Door C). Critic found it **unexecutable** (Task 5 called a non-existent `summarize_once`; the dossier test could not emit a page); security found it **unsound** (a page's taint was derived from the model's chosen citations, so a model could cite-around a file and launder the taint). Rev 2 adds **D8 (engine-anchored page taint)** to close the leak and reworks the plan to be executable. See §12 (Review log).
- **Milestone:** Extraction-from-Files — the consumer M5a/M5b deferred. Turns the externally-tainted `file_ingested` text M5a/M5b produce into **structured graph knowledge** (entities/links) + dossiers, **without laundering the external taint**.
- **Parent:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` (§5.10/§5.11 taint + lineage) and `2026-06-18-bossclaw-core-m5a-ingest-pipeline-design.md` (D4 taint root, §4/§6 the two evolve doors, §9 "extraction … the taint root is what makes it safe").
- **Builds on:** M1–M5b, all merged to `main` (last `d900217`; CI PR #30 → `5a50a85`).
- **Crate:** `crates/bossclaw-core` (engine; `#![forbid(unsafe_code)]`).

---

## 1. Goal, Non-Goals & Success Criteria

### Goal
Let the M4 **evolve loop** consume `file_ingested` content so file knowledge becomes **structured + recallable** the way typed memories already are: extract `entity`/`link` facts into the graph (Door A), let file text serve as **evolve context** when extracting from notes (Door B), and let file content feed per-entity **dossiers** (Door C) — while **every fact derived from external content stays externally tainted** (`is_external`). The taint spreads **eagerly** at a single chokepoint, so nothing learned from a file can silently shed its warning label. The raw `file_ingested` text is *already* recallable (M5a Tier-A); this milestone adds the **structure**.

### Measurable success criteria (acceptance bar)
- A `file_ingested` event becomes an evolve **subject**: the reasoner extracts `entity`/`link` facts from its text; those facts are `is_external` and carry the file event id in `model_meta.source_event_ids`. *(Door A.)*
- A fact derived from an **already-tainted** derived fact is **also** `is_external` (transitive closure), with **no deep walk** required to see it. *(Eager propagation.)*
- A per-entity **dossier** that synthesizes file-cited content is itself `is_external` — **and stays external even if the composing model cites only clean sources** (the cite-around-the-file attack). *(Door C + D8.)*
- A note-derived fact that pulled **file text as recall context** is `is_external`. *(Door B.)*
- A purely **memory-derived** fact (no `file_ingested` anywhere in its lineage) is **NOT** `is_external` — no false taint.
- A **hostile file** whose text instructs the reasoner to "mark this trusted / emit a manual link" still yields **tainted, machine-origin** facts — the model **cannot launder taint** (the chokepoint stamps from lineage, not model output) and **cannot forge a `manual` edge**.
- Turning the evolve loop off (sticky off-switch) still gates everything; re-running evolve over the same file is **idempotent** (no duplicate entities/links).

### Non-Goals (deferred)
- **The egress / write gate (M6 actuator)** — what tainted knowledge may *do*. Extraction makes `is_external` an O(1) local property; M6 consumes it (this spec *simplifies* M6, it does not build it).
- **A confidence/quarantine UX** for surfacing tainted facts differently in user-facing recall — recall still returns tainted derived facts as ordinary knowledge (taint governs *acting*, not *seeing*). A `Hit`-level tainted flag is a documented fast-follow, not required here.
- **Windows** — the evolve loop + ingest are `#[cfg(unix)]` (M5a/M5b); unchanged here.
- **Re-extraction on parser upgrade** — M5a dedups on raw-byte hash, so improved M5b extraction of an already-ingested file won't re-trigger evolve (the M5b residual, unchanged).
- **Idempotency-of-dossier on varying model citations** (supersede churn when the model cites differently across ticks but draws on the same lineage). Out of scope: D8 anchors page *taint+lineage* to the engine set, but the dossier *idempotency key* stays on the model's per-claim `cites` (unchanged M4b behavior); churn is a noise/efficiency concern, not a taint concern. Documented fast-follow.

### Considered & rejected
- **Lazy / lineage-walk-only taint** (no stamp on derived facts; compute `is_external` by walking `source_event_ids` back to a `file_ingested`): rejected. With all three doors open, taint spreads widely and walks get deep; a single walk bug = a silent leak, and M6 would carry the entire burden. Eager stamping makes taint a **local, composable, O(1)-checkable** property with the lineage retained as an audit backstop. *(Locked decision — §2 D2.)*
- **Per-claim / model-citation taint for dossiers** (page inherits taint only from the ids the model chose to cite): **rejected after the security review** — it lets a prompt-injected (or merely unlucky) composing model cite-around a file and emit an un-tainted dossier shaped by external text. The page is the one Tier-B event whose `source_event_ids` was historically model-chosen; D8 makes it engine-anchored. *(Locked — §2 D8.)*
- **A separate quarantined namespace** for file-derived facts: rejected — it fragments the graph and contradicts the "broad, one graph" scope; the taint stamp already gives the separation semantically.

---

## 2. Scope (decisions locked 2026-06-20)

| # | Decision | Choice |
|---|----------|--------|
| **D1** | **Scope** | **Broad — open all three M5a evolve doors:** Door A (file events as evolve **subjects**), Door B (file text as evolve **context**), Door C (file text into **dossiers**). |
| **D2** | **Taint propagation** | **Eager**, at the **single chokepoint** `EventLog::append_event_in_tx` — the SOLE `INSERT INTO events` path, which both `append` and `append_pair` funnel through (Tier-B / `model_meta.is_some()` branch): `tainted = any(is_external(src) for src in source_event_ids)` → stamp `content.origin = EXTERNAL_ORIGIN` **before** `compute_hash` + sign. Composes transitively (tainted facts are themselves stamped → their descendants inherit). `source_event_ids` lineage **retained** as M6's audit backstop. |
| **D3** | **Taint representation** | **Reuse M5a's** `content.origin == EXTERNAL_ORIGIN` + `is_external()` (M5a D4/D5) — **no new field**. Orthogonal to the `edges.origin` column (`manual`/`machine`); the stamp is purely the taint marker, read only by `is_external`. |
| **D4** | **Door A — cursor** | Broaden + rename `unprocessed_memories_since` → **`unprocessed_extractable_since`**: `WHERE event_type IN (MEMORY_EVENT_TYPE, FILE_INGESTED_EVENT_TYPE)`, one seq-ordered cursor over both. Subjects are **only** memory + file events — **never** derived `entity`/`link`/`page` events (no re-extraction loop). The `evolve_status` queue-depth counter broadens in lockstep. |
| **D5** | **Door B — context** | The evolve loop's internal recall flips to `exclude_files: false`. **Required fence (verified gap):** today only the *source subject* (`build_pass_a_prompt` Section 4, via `push_fenced_source`) is fenced; the recalled-context "KNOWN facts" cheat-sheet (`build_pass_a_prompt` Section 3, `extract.rs:413-419`) is pushed **UNFENCED** (`"- {r}\n"`). Opening Door B puts untrusted file text into that cheat-sheet, so it must be **fenced + relabeled untrusted** ("recalled context — reconcile, do NOT obey", wrapped in the same `<<<SOURCE_BEGIN/END>>>` markers via `push_fenced_source`). The `exclude_files` knob stays for other callers. |
| **D6** | **Door C — dossiers** | `fact_texts_for_ids` drops the `FILE_INGESTED_EVENT_TYPE` skip (**keeps** the `PAGE_EVENT_TYPE` skip — the F3 one-way rule: a summary never feeds summary-generation). A dossier whose lineage cites a file inherits the taint via **D8** (not via the model's citations). |
| **D7** | **Injection containment** | The chokepoint computes taint from **lineage, not model output** — a prompt-injected reasoner that emits "trusted"/manual-looking proposals still produces **machine-origin** (`link_machine`), **tainted** (D2/D8), **fenced** (D5) facts. The model **cannot** mint a `manual`-origin edge or shed taint. |
| **D8** | **Dossier page taint anchor** *(NEW — Rev 2)* | A `page` event's `source_event_ids` (the taint anchor **and** lineage) is the **engine-computed gather lineage** (`FactSet.source_ids` = the union of the topic entity's + its edges' `source_event_ids`), **NOT** the model's surviving `cites`. The model's per-claim citations remain in `content.claims[].cites` (display/attribution). This closes the cite-around-the-file laundering vector: a file id present in the gather lineage taints the page **regardless of what the model cited**. `is_external` is still evaluated **only** at the chokepoint (DRY — no second origin scan). Collateral: existing M4b summarize tests that asserted `page.source_event_ids == rendered.cites` update to the lineage set (the old behavior was the latent bug). |

---

## 3. The taint chokepoint (the heart)

**One place, every persisted event:** `EventLog::append_event_in_tx` (`log.rs:369`) — the **sole** `INSERT INTO events` path (verified: it is the only insertion site in `src/`, at `log.rs:389`, and **both** `append` (`log.rs:330`) and `append_pair` (`log.rs:346-347`) funnel through it). The eager stamp goes here, before `compute_hash` (`log.rs:382`), so it is part of the signed bytes. (The existing non-empty-`source_event_ids` `reject_empty_tier_b` guard stays at the entry points; the stamp is a separate, finalize-time step.)

```
append_event_in_tx(tx, mut event):                      # the sole INSERT path
    if event.model_meta is Some:                        # Tier-B (derived)
        tainted = any( is_external(load(src, tx)) for src in event.model_meta.source_event_ids )
        if tainted:
            event.content["origin"] = EXTERNAL_ORIGIN    # the eager stamp
    # …then the EXISTING finalize: id, ts, prev_hash, compute_hash, Ed25519 sign, INSERT
```

- **Single chokepoint, can't be bypassed:** every `entity`/`link`/`invalidate` (`link_machine` → `append`) and every dossier `supersede`+`page` (`emit_page` → `append`/`append_pair`) is persisted through `append_event_in_tx`. The stamp is applied to *all* Tier-B events uniformly — a §8 test asserts no path bypasses it (proven through BOTH entry points). Source `is_external` reads use `tx`, so they see prior committed events **and** any uncommitted earlier event in the same `append_pair` transaction.
- **Stamp is part of the signed record:** applied **before** JCS canonicalization + signing, so taint is durable, rebuild-stable, and tamper-evident (the frozen canonicalization vector set gains a tainted-Tier-B vector — both a `link` and, for D8, a `page`).
- **Transitive, automatically:** a tainted derived fact carries `origin=external`, so when it is later a source, the chokepoint sees `is_external(src)=true` and stamps the new fact. The transitive closure is maintained at write time; no recursion.
- **The page is the one Tier-B event whose `source_event_ids` was model-chosen.** For `link`/`entity`/`invalidate`, `source_event_ids = read_set` (engine-built at `evolve_once`, `log.rs:3183-3187`) — taint is already lineage-derived. For the `page`, `emit_page` historically stamped `source_event_ids = rendered.cites` (the model's surviving citations). **D8 fixes this**: the page's `source_event_ids` becomes the engine-computed gather lineage, so the chokepoint sees every file in the lineage no matter what the model cited.
- **Tier-A untouched:** `model_meta: None` events (user memories; the `file_ingested` root itself, already stamped by `file_ingested_content`) are **not** re-stamped. Base case = the file root; inductive step = the chokepoint.
- **Cost:** one indexed `events`-by-id read per source to evaluate `is_external` (sources per derivation are few — the memory/file subject + the bounded recall read-set, or the bounded gather lineage). Runs inside the serialized writer; bounded, noted in §10.

---

## 4. Opening the three doors

- **Door A (subjects).** `unprocessed_extractable_since` (D4) yields `memory` **and** `file_ingested` events to `evolve_once`. The reasoner extracts from `content.text` (both kinds carry it); the subject's event id is in the derived facts' `source_event_ids` → §3 stamps them external. (Inverts the M5a "cursor is memory-only" door; broadens `evolve_status` in lockstep.)
- **Door B (context).** `evolve_once`'s context recall passes `exclude_files: false` (D5). File text can now surface as extraction context; if a file hit enters the read-set, the derived fact's lineage includes the file → tainted. File text is fenced in the prompt (D5). (Inverts the M5a `exclude_files: true` door.)
- **Door C (dossiers).** `fact_texts_for_ids` no longer skips `file_ingested` rows (D6); a per-entity dossier can synthesize file content. The page's taint comes from **D8** (the engine gather lineage carries the file ids), not from the model's citations. (Inverts the M5a defense-in-depth file skip; keeps the page-skip F3 rule.)

---

## 5. Data flow

`evolve_once` (off-switch ok) → `unprocessed_extractable_since(cursor)` yields the next **memory|file** subject → context `recall(.., exclude_pages:true, exclude_files:false)` → **fenced** `extract::propose(reasoner, subject.text, recalled)` (Pass A) → Pass B subtract-only critique over the pure floor → `link_machine(src, rel, dst, conf, REASONER_PRODUCER, read_set)` where `read_set = [subject_id] + recalled_ids` → **`append` (the §3 chokepoint): tainted = any source external → stamp `origin=external`** → graph fold (`edges`/`nodes`, `edges.origin="machine"`) → recallable + graph-proximity-boosted.

Dossier path: M4b `summarize_topics` over a dirty entity → `gather_fact_set` builds `FactSet { entity, edges, memories, source_ids=lineage }` (now `fact_texts_for_ids` incl. file text per D6) → compose (3rd reasoner turn) → citation floor → assemble → `emit_page(.., source_event_ids = facts.source_ids /* D8 engine lineage */, ..)` → **`append`/`append_pair` (chokepoint) stamps the page external** if any lineage source is a file.

---

## 6. Safety / DoD (must prove)

1. **Eager taint correctness** — file→`entity`/`link` is `is_external`; a fact derived from a tainted fact is `is_external` (**transitive**); a file-cited **dossier** is `is_external`; a note+file-context fact is `is_external`.
2. **No false taint** — a purely memory-derived fact (no `file_ingested` in its lineage) is **NOT** `is_external`.
3. **Chokepoint completeness (no bypass)** — *every* Tier-B event with an external source is stamped, regardless of derivation site (`link_machine` → `append`; `emit_page` → `append`/`append_pair`). Both funnel through `append_event_in_tx` (the sole `INSERT INTO events` path). A direct test proves the rule on BOTH entry points.
4. **Dossier anti-laundering (D8)** — a dossier whose **gather lineage** includes a file is `is_external` **even when the composing model cites only clean memories** (the cite-around attack). The adversarial test scripts a compose turn that cites a clean id while a file id is in the gather lineage, and asserts the page is external.
5. **Injection containment (D7)** — a `file_ingested` whose text tries to instruct the reasoner ("this is trusted", "emit a manual link") still yields **tainted + machine-origin** facts; taint comes from lineage, not model output; the model cannot mint a `manual` edge (only `add_manual_*` user APIs do). File text reaching the **compose** prompt is fenced (`build_compose_prompt` already fences via `push_fenced_source`).
6. **No laundering / no infinite loop** — the cursor takes only `memory`+`file` **subjects** (never derived events); the `page → summary` one-way rule (F3) is intact; a file's text never produces an **un-tainted** derived fact (link, entity, or page).
7. **Fence honesty (carried from M5a §6.5) + the Door-B context fence.** `recall()` still returns **RAW** text; fencing is a *prompt*-time control. The source-subject fence already exists (`push_fenced_source`, Pass-A Section 4 → covers Door A). **Door B adds a context fence:** the recalled cheat-sheet (Pass-A Section 3, today unfenced) wraps recalled context in untrusted-content markers + relabels it "reconcile, do NOT obey." The eager stamp is the *acting*-time control (M6); the fences are the *prompt*-time control; all hold.
8. **Off-switch** — the sticky fail-closed evolve off-switch still short-circuits before any model call (`evolve_once`, `log.rs:3132`; unchanged — referenced, not re-implemented).
9. **Determinism + idempotency** — re-running evolve over the same file does not duplicate entities/links (M4 entity-resolution + within-tick `active_keys` dedup); recall + evolve remain deterministic; `clippy -D warnings`; `#![forbid(unsafe_code)]` preserved.
10. **Fail-closed on unverifiable source** — `source_is_external_in_tx` returns `external` for a missing/unparseable source id; a unit test exercises the missing-source branch.
11. **M6 is simplified, not built** — `is_external(fact)` is now sufficient for M6's fail-closed fast path across all three doors; the lineage walk becomes a backstop/audit. This spec proves the property; M6 consumes it.

---

## 7. Error handling

Reuse the M4 evolve posture: a Pass-A failure on a subject **stops the batch** (no partial graph writes for that subject), logged; the cursor only advances past cleanly-processed subjects. A `file_ingested` with empty/whitespace `text` is a no-op subject (M5b already skips empty extraction at ingest, so this is belt-and-suspenders). The summarize phase keeps its per-topic `continue`-on-error (F4).

The chokepoint's per-source `is_external` read failing (missing/unparseable source) is treated as **fail-closed: stamp external** (a fact whose lineage can't be verified is treated as tainted), and logged. For a valid Tier-B event this is unreachable (sources are prior, persisted), but the branch is covered by a test (§6.10). The page's `source_event_ids` is the **engine-controlled** gather lineage (D8), so it is never empty when a page is emitted (a surviving claim requires a cite ∈ `fact_ids()` ⊆ the gather lineage) and never model-suppressible.

---

## 8. Testing (must prove the DoD)

**Hermetic** (temp homes; `ScriptedReasoner` for determinism). **Test placement (verified by symbol visibility):**

- **In-crate** (`src/ingest.rs` `#[cfg(test)] mod`, edit in place): invert the **two** existing door tests — `ingested_files_are_excluded_from_the_evolve_cursor` → file events **appear** in the evolve queue (Door A); `evolve_context_recall_excludes_file_text` → evolve-context recall **surfaces** file text (Door B). These use the in-crate `run_ingest`/`MockEmbedder` helpers.
- **Integration** (`tests/extraction.rs`, NEW — mirrors `tests/evolve.rs`; public API only: `open_with_recall`, `add_grant`, `ingest_all`, `link_machine`, `event_by_id` (new pub), `is_external`, `evolve_once`, `gather_fact_set`, `recall`, `ScriptedReasoner` + `build_pass_a/b_prompt` + `build_compose_prompt`; the file event id is obtained via a unique-token `recall`, not the `pub(crate)` `current_file_for_path`): the chokepoint test, Door-A end-to-end + transitive, no-loop, idempotency-via-dedup, Door-C dossier + the **adversarial cite-around** test, injection containment, no-false-taint, fail-closed.
- **Integration** (`tests/extract.rs`, append): the Pass-A context-fence string assertion on `build_pass_a_prompt` (already `pub`, already tested there).
- **Integration** (`tests/vectors.rs`, append): the frozen canonicalization vectors for a tainted `link` AND a tainted `page` (D8).
- **Integration** (`tests/live_ollama.rs`, append): the feature-gated `#[ignore]` live-Ollama extraction gate.

**The "third door test" correction.** Rev 1 said "invert the three M5a door tests." There are **two** invertible door tests (cursor, context recall). The `fact_texts_for_ids` `file_ingested` skip is M5a *defense-in-depth with no dedicated test* — Door C's tests (§6.4 dossier + adversarial) are its **first** coverage.

**Coverage map (DoD → test):**
- §6.1 file→entity/link external + transitive → chokepoint test (`link_machine` + `event_by_id`) + Door-A e2e (scripted reasoner over an ingested file) + a transitive `link_machine` on the tainted link.
- §6.1 note+file-context fact external → a memory subject whose Door-B recall surfaces a file → derived fact external.
- §6.1 file-cited dossier external + §6.4 anti-laundering → Door-C dossier (real `evolve_once` → `summarize_topics`; compose turn scripted via `gather_fact_set`) asserts page external; the **adversarial** variant scripts the compose to cite only a clean memory and still asserts external.
- §6.2 no false taint → pure-memory derived fact NOT external.
- §6.3 chokepoint completeness → `append` (link) + `append_pair`/`append` (page) both stamped.
- §6.5 injection containment → hostile file → `is_external` link + `edges.origin == "machine"` (via `neighbors().origin`); plus assert file text lands inside the `<<<SOURCE_BEGIN/END>>>` fence in `build_compose_prompt`.
- §6.6 no-loop → derived events never re-enter the cursor (`evolve_status().queue_depth == 0` after a tick).
- §6.7 Door-B fence → string assertion on `build_pass_a_prompt` (recalled context inside fence markers, relabeled untrusted).
- §6.8 off-switch → reference the existing M4 off-switch test (`evolve_once` short-circuits when disabled); unchanged path.
- §6.9 idempotency-via-dedup → reset the evolve cursor and re-run `evolve_once`; assert active-edge count unchanged (exercises M4 dedup, not just the cursor no-op).
- §6.10 fail-closed → a Tier-B event whose `source_event_ids` references a non-existent id is stamped external.
- §6 frozen vectors → tainted `link` + tainted `page` canonicalization frozen.
- **Live gate** (the M4a/M4b pattern): real Ollama `qwen2.5:7b` extracts entities from a real ingested file; the derived facts come out `is_external`; a contradiction between a file fact and a memory fact still drives `invalidate` — with the file-derived side tainted. (Feature-gated `#[ignore]`, run in the existing live-model CI leg.)

---

## 9. Deferred

- **M6 actuator** — the fail-closed egress/write gate that *consumes* `is_external` (now an O(1) local check) + the confused-deputy write defenses. Brick 2 of the Vault-Brain program.
- **`Hit`-level tainted flag** for differentiated user-facing surfacing (fast-follow).
- **Dossier idempotency on varying model citations** (supersede churn) — §1 Non-Goals; D8 anchors page taint to the engine set but leaves the idempotency key on model `cites`.
- **Re-extraction on M5b parser upgrade** (M5b dedup-on-byte-hash residual, unchanged).
- **Windows** evolve/ingest.

## 10. Risks

- **Chokepoint read cost.** One `events`-by-id read per source per Tier-B append. Bounded (few sources/derivation) and inside the existing writer lock; if profiling shows it hot, batch the lookups or cache `is_external` per id within a batch. Documented, not pre-optimized.
- **Taint over-spread (false positives), by design.** Opening Door B taints a note-extraction that *happens* to recall a file as context even if the file text was irrelevant. D8 likewise widens a page's `source_event_ids` from "what the model cited" to "the full gather lineage" — so a clean-only dossier's lineage set grows (more audit ids), and a dossier touching any file becomes tainted. This is the **safe** direction (over-taint, never under-taint) and matches the "leak nothing" thesis; the cost is some conservative gating by M6. Acceptable; precise per-claim taint is out of scope and far more fragile.
- **Stamp mutates signed content.** Adding `origin` to a Tier-B event changes its canonical bytes/signature vs a naive un-stamped build. Intended (taint is durable + signed) → the chokepoint MUST run before canonicalization and the frozen vector set MUST be extended (tainted `link` + `page`). Both are explicit DoD items.
- **D8 changes M4b page lineage semantics.** Existing M4b tests that asserted `page.source_event_ids == rendered.cites` must update to the gather-lineage set. This is mechanical and makes those tests more correct, but it is a deliberate touch of merged M4b test code (called out in the plan, Task 5).
- **Prompt injection from file content** is contained (§6.5) but not eliminated: a hostile file can still cause **bogus tainted machine facts** to be extracted. They are quarantined by taint (M6 won't act on them) + machine-origin (distinguishable from manual) + low/ subtractable confidence — but they do enter the graph. The boundary is the same as M5a's: ingest is consent-bounded; taint + M6 are the act-time defense.
- **Pass-B neighborhood lines (defense-in-depth, I2).** `neighborhood_lines` (`log.rs:3413`) renders edge endpoints by model-produced label **without** `sanitize_ident` (control-char strip + length cap), and pushes them **unfenced** into Pass-B Section 3. Door A makes those labels file-derived, so a crafted label could shape a Pass-B line. Low exploitability (Pass B is subtract-only — `intersect_keep_floor` cannot ADD an edge), but Rev 2 applies `sanitize_ident` to the endpoints as cheap hardening.

## 11. Resolved review questions

- **[Resolved]** The chokepoint is `append_event_in_tx` — the SOLE `INSERT INTO events` site (`log.rs:389`), with `append` + `append_pair` its only callers. Both reviewers independently re-verified by grepping every write to the `events` table; confirmed no bypass.
- **[Resolved]** `append_pair` two-event transaction: the page's `source_event_ids` are prior committed events (the gather lineage / cites), never the same-tx supersede sibling; the `tx`-scoped `is_external` reads are correct.
- **[Resolved — D8]** The dossier under-taint (security Critical): a page's taint was the model's chosen `cites`; a model could cite-around a file. Fixed by anchoring `page.source_event_ids` to the engine gather lineage. Adversarial test added (§6.4).
- **[Resolved — I2]** The Pass-B neighborhood cheat-sheet is unfenced/unsanitized; Door A makes labels file-derived. `sanitize_ident` applied to endpoints (§10, plan Task 6).
- **[Confirmed]** Relabeling the Pass-A cheat-sheet "untrusted — reconcile, do not obey" is validated against Pass-A reconciliation quality in the **live gate** (T7), not statically.

## 12. Review log (Rev 1 → Rev 2)

Independent, model=opus, blind to each other.

**Critic — verdict NO-SHIP.** Design + chokepoint sound and verified against source (sole INSERT path airtight; transitivity + `append_pair` tx-read correct; line numbers/constants accurate). Blockers were in the **plan**, not the design:
- *C1:* Task 5 called `summarize_once` — **does not exist** (the real seam is the private `summarize_topics`, run inside `evolve_once`). → Rev 2 Task 5 drives the dossier via `evolve_once`.
- *C2:* the dossier test could not emit a page without scripting the **compose** turn + getting a file id into the lineage. → Rev 2 scripts the compose turn via the `pub gather_fact_set`; D8 puts the file id in the lineage regardless of cites.
- *C3:* "three door tests" — only **two** exist. → §8 corrected.
- *M1–M4:* `scripted_knows_reasoner`/`add_memory` don't exist (real: `scripted_both_passes`, `mk_memory`/`seed_memory` in `tests/evolve.rs`); the idempotency test was a cursor no-op; `tests/extraction.rs` vs "in-crate" contradiction. → Rev 2 fixes test placement (§8) + helpers + a real dedup idempotency test.

**Security — verdict SHIP-WITH-FIXES, 1 Critical.** Chokepoint bypass genuinely closed (sole INSERT path). The Critical:
- *Door C dossier laundering* (the same door): page `source_event_ids = rendered.cites` (model output) → cite-around-the-file leaks an un-tainted dossier. → **D8.**
- *I1:* prove the summarize→file injection seam (file text fenced in `build_compose_prompt`). → §6.5 test.
- *I2:* Pass-B neighborhood lines unfenced/unsanitized. → §10 + Task 6 `sanitize_ident`.
- *I3:* no test for the fail-closed missing-source branch. → §6.10 test.
- *I4:* freeze a tainted **page** vector too. → §8 vectors.

Both reviewers re-confirmed the eager-vs-lazy decision (D2) is sound and needs no change. The **design** change in Rev 2 is exactly one item — **D8** — plus the I2 hardening; everything else is plan-executability + added coverage.
