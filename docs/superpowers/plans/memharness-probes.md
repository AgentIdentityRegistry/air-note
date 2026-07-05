# memharness Phase 0 — live-reality probes (Task 1 findings) — Rev 2 — revised after architect+critic review

> **Purpose:** Task 1 of `docs/superpowers/plans/2026-07-03-air-agent-memharness-phase0.md` is a
> **read-only reality check** run on Peter's machine BEFORE any code is written. It pins the three
> external contracts the harness depends on — `gbrain query` CLI output, the Ollama HTTP surface, and
> the real mined-query count — so later tasks encode *verified* constants, not guesses.
>
> The implementer **fills this file in** while running the probe commands, then wires the confirmed
> values into the constants named below. If reality differs from a plan assumption, the implementer
> updates BOTH the constant in code AND the "Reconciliation" note here, and flags it in the Task 1
> commit message.

Status: **RUN** ✅  ·  Run date: 2026-07-03  ·  Machine: Peter's laptop (Darwin 25.5.0)  ·  `git rev` at probe time: `4889841`

> **⚠️ Headline reconciliations (read these before implementing any arm/judge task):**
> 1. **Peter's daily driver is `tokenmax` with the `zerank-2` reranker ON** — not "balanced,
>    reranker OFF" as the spec assumed (spec §4 premise is stale). Primary arm = the CONFIGURED
>    default pipeline; `balanced` demotes to the secondary reference arm (and since gbrain
>    v0.36.0.0 even `balanced` runs the reranker by default).
> 2. **The GBrain arm should use `gbrain call query` (JSON op bridge), not the human CLI output** —
>    same op surface the mined MCP calls hit, machine-parseable, no scraping.
> 3. **The skeleton's Probe C one-liner over-counts by ~175×** (20,662 vs the true 118) — it matches
>    tool-NAME listings embedded in transcripts. The miner must anchor on
>    `"name":"mcp__gbrain__…"` tool_use records.
> 4. **No valid `ANTHROPIC_API_KEY` is currently available on this machine** — the `~/.zshrc` line
>    is commented out AND the key it holds is stale (live probe → `invalid x-api-key`). Peter must
>    export a fresh key before any hybrid run (Task 46 runbook item).

---

## Probe A — `gbrain query` CLI output format

Feeds: `arms.rs::GBRAIN_ARM` chunk-extraction parser (Task 24) and its fixture (Task 23).

Commands run (output abridged; full shape captured in the fixture):

```
$ gbrain --version
gbrain 0.42.38.0

$ gbrain query "test" --limit 3        # human format — NOT what the arm will parse
[0.8716] signalytics/code-conventions -- Don't replace without understanding the svix flow …
[0.9498] projects/trading-edge/gemini-skills-catalog -- ### Nice-to-have …

$ GBRAIN_SOURCE=default gbrain call query '{"query":"test","limit":2}'   # op bridge — JSON array
[ { "slug": "signalytics/code-conventions", "page_id": 1191, "title": "Code Conventions",
    "type": "concept", "chunk_text": "…", "score": 0.9498, "rerank_score": …, "base_score": …,
    "chunk_id": …, "chunk_index": …, "chunk_source": "compiled_truth", "evidence": "high_vector_match",
    "effective_date": …, "effective_date_source": …, "backlink_boost": …, "reranker_delta": …,
    "create_safety": "exists", "stale": false, "source_id": "default" }, … ]
```

**Findings to pin:**

- Invocation that returns machine-parseable chunks: **`gbrain call query '{"query":<q>,"limit":<k>}'`**
  (the op bridge; JSON array on stdout). `gbrain query` has **no `--json`/`--format` flag** — its human
  output (`[score] slug -- text` + continuation lines) would need fragile scraping. The op bridge is
  also the SAME surface Peter's mined MCP calls hit (`mcp__gbrain__query` → the `query` op), so it is
  the more faithful arm, not just the sturdier one.
- **Required env: `GBRAIN_SOURCE=default`.** This machine's ambient env leaks `GBRAIN_SOURCE=__all__`,
  which `gbrain call` REJECTS (`Invalid GBRAIN_SOURCE value "__all__"`). The harness must set
  `GBRAIN_SOURCE=default` explicitly on the child process (never inherit).
- Output shape: **JSON array** (ranked order = array order).
- Per-hit fields available (19 keys, fixture captures all): page/slug identifier = `slug`,
  chunk text = `chunk_text`, score = `score` (final; `rerank_score`/`base_score` also present).
  Enum-ish metadata (`evidence`, `chunk_source`, `create_safety`) carries no page content.
  Parser contract: read `slug` + `chunk_text` + `score`, tolerate unknown keys.
- **Slug ↔ `~/brain`-relative-path convention** (known-item match normalization, Task 27):
  slug `air/session-start-protocol` ↔ file `air/session-start-protocol.md`. **Confirmed** by direct
  `ls` for two probed slugs (`signalytics/code-conventions.md`, `air/session-start-protocol.md`):
  path = slug + `.md`.
- **(Rev 2) Mode default:** `--mode`/op `mode` param exists (`conservative|balanced|tokenmax`); when
  omitted, resolution is per-call opts → per-key config → `MODE_BUNDLES[cfg.search.mode]` →
  `MODE_BUNDLES.balanced` (source: `~/gbrain/src/core/search/mode.ts:11`). **Peter's live config:
  `search.mode = tokenmax`, `search.reranker.model = zeroentropyai:zerank-2`** (via
  `gbrain config get`). So the DAY-DRIVER pipeline = **tokenmax + zerank-2 reranker ON + autocut ON**
  — NOT the spec's assumed "balanced, reranker OFF". `MODE_BUNDLES` source also shows `balanced` has
  `reranker_enabled: true` since v0.36.0.0 (mode.ts:54–62); only `conservative` is reranker-off.
  - **Exact argv the PRIMARY arm must use (daily driver):**
    `gbrain call query '{"query":<q>,"limit":<k>}'` with `GBRAIN_SOURCE=default` — NO `mode` key, so
    the configured mode applies (what Peter's daily calls get). The run records
    `gbrain config get search.mode` + `search.reranker.model` output in the report as the pipeline
    fingerprint (so a future config change is visible, not silent).
  - **Autocut caveat (accepted, reported):** in the configured mode autocut may return FEWER than
    `limit` hits (score-cliff cut). That is the daily pipeline's own precision/recall trade and is
    measured as-is; per-arm returned-hit counts land in `PackStats` and known-item scoring treats a
    cut-away gold page as a miss.
- **Secondary arm:** `mode:"balanced"` in the same op JSON (verified accepted). This REPLACES the
  spec's optional `tokenmax` secondary — tokenmax is now the primary, `balanced` is the reference.
- **(Rev 2) Does GBrain index YAML frontmatter?** **STRIPS.** Three independent confirmations:
  (1) ingestion splits frontmatter off via gray-matter and stores it as metadata, body-only chunks
  (`~/gbrain/src/core/ingestion/sources/markdown-greenfield.ts:24–31`); (2) DB pages hold frontmatter
  as a separate field — `chunk_text`/`compiled_truth` never contain it (the `~/brain` files'
  frontmatter is GENERATED at export-to-fs time); (3) empirical: `captured_via` appears in every
  exported file's frontmatter, yet a keyword search returns exactly ONE hit (a page mentioning it in
  body text). → **`STRIP_FRONTMATTER = true`** (the harness strips, indexing the same text GBrain does).

**Reconciliation** (reality ≠ plan assumption `GBRAIN_QUERY_ARGS = ["query", <q>, "--limit", <k>]`):
**CHANGED** → `GBRAIN_QUERY_ARGS = ["call", "query", <op-json>]` + env `GBRAIN_SOURCE=default`;
parser = serde_json on the array (no text scraping). Primary arm = configured mode (tokenmax),
secondary = `mode:"balanced"`. Spec §4's "balanced (reranker OFF) = daily driver" premise is stale —
flagged in the Task 1 commit; report headline arm is labeled `gbrain-default (configured)` with the
recorded fingerprint.

- **Drift-count pin (Task 36, 2026-07-03):** indexed-page count for the drift check comes from
  `GBRAIN_SOURCE=default gbrain call get_stats '{}'` → JSON field `page_count` (live capture
  2026-07-03: `{"page_count": 895, "chunk_count": 2843, …}`). Any spawn/status/parse failure →
  `None` → the report renders "drift unknown", never a guess. The plan's Task-36 sketch
  (`gbrain stats` + text scraping) does not exist as a surface — reconciled like the query op.

---

## Probe B — Ollama HTTP surface

Feeds: `arms.rs`/`judge.rs`/`synth.rs` Ollama client (Task 12) + the availability preflight (Task 11).

Commands run:

```
$ curl -s http://127.0.0.1:11434/api/tags
{"models":[{"name":"qwen2.5:7b-instruct","model":"qwen2.5:7b-instruct", …,
  "details":{…,"parameter_size":"7.6B","quantization_level":"Q4_K_M","context_length":32768},
  "capabilities":["completion","tools"]}]}

$ time curl -s http://127.0.0.1:11434/api/generate \
    -d '{"model":"qwen2.5:7b-instruct","prompt":"Reply with exactly: OK","stream":false}'
→ response "OK", 5.6s wall (includes model load)
```

**Findings to pin:**

- Is Ollama reachable on `127.0.0.1:11434`? **[x] yes**
- Default model `qwen2.5:7b` present in `/api/tags`? **[ ] no** — the ONLY installed tag is
  **`qwen2.5:7b-instruct`** (which is also bossclaw-core's evolve-loop default tag) → default to it.
- Endpoint the harness will use: **`/api/generate`** (single-turn, `"stream": false`) — **confirmed
  reachable**, returns `{"response": …}`. Pinned as THE endpoint for answerer/judge/synth (Task 12).
- Approx latency of one 7B generate call: **~5.6 s** cold (incl. load); budget math should assume
  ~5–15 s per call with real prompts. ~100 open queries × (2 answers + 2 judge calls) ≈ 400 calls ≈
  35–100 min + synth generation ≈ within the ≤2h budget, tight but plausible; known-item queries are
  mechanical (no LLM calls).

**Reconciliation** (default model constant `DEFAULT_OLLAMA_MODEL = "qwen2.5:7b"` must change):
**CHANGED** → `DEFAULT_OLLAMA_MODEL = "qwen2.5:7b-instruct"` (only installed tag; matches evolve loop).

---

## Probe C — real mined-query count

Feeds: the "≥100 real queries" acceptance check + the `mine.rs` dedup expectations (Task 17) and the
"if <50, weight synthetic higher" report note (spec §90 / open question 3).

Commands run:

```
$ grep -roh 'mcp__gbrain__\(query\|search\|recall\)' ~/.claude/projects --include='*.jsonl' | wc -l
20662        # ← the skeleton one-liner: WRONG measure, see reconciliation

$ grep -roh '"name":"mcp__gbrain__\(query\|search\|recall\)"' ~/.claude/projects --include='*.jsonl' | wc -l
118          # actual tool_use invocations
$ grep -rl  '"name":"mcp__gbrain__\(query\|search\|recall\)"' ~/.claude/projects --include='*.jsonl' | wc -l
40
$ grep -roh '"name":"mcp__gbrain__get_page"' ~/.claude/projects --include='*.jsonl' | wc -l
645          # candidate implicit-label events
$ …tool_use lines | grep -o '"query":"[^"]\{1,200\}"' | sort -u | wc -l
104          # unique query strings after exact dedup
```

**Findings to pin:**

- Raw `query`/`search`/`recall` call count: **118 across 40 files** (matches the 2026-07-03 recon
  exactly — once counted correctly, see reconciliation).
- Estimated after exact+near dedup: **104 exact-unique; ~95–104 after near-dup merge** (top dup runs
  are only 2×). `get_page` label-candidate events: 645.
- Estimated with implicit `get_page`-within-5 labels: computed at mine time (645 candidates is
  plentiful); if labeled+open real queries land <100 total, the report states the shortfall against
  acceptance criterion 2 honestly.
- **Decision:** deduped real open queries ≥ 50 → **`WEIGHT_SYNTHETIC_HIGHER = false`**.

**Reconciliation** (the mining regex / JSONL shape differs from the skeleton one-liner):
**CHANGED** — the skeleton's bare-name grep counts 20,662 (~175× over) because transcripts embed
tool-NAME listings (deferred-tool reminders, tool defs) on non-invocation lines. `mine.rs` MUST anchor
on tool_use records — `"name":"mcp__gbrain__query|search|recall"` — and extract the adjacent
`input.query` field; the Task 21 fixture must include a decoy tool-name-listing line to lock this in.

- **Shape reconciliation (Task 21, 2026-07-03):** real lines nest tool calls under
  `message.content[]` (`type=="tool_use"`, `name`, `input`) keyed by line-level `sessionId` —
  NOT the flat recon shape. `mine.rs` parses the nested shape (two-layer: line → flattened
  tool calls; string-form `content` lines skip cleanly); the committed fixture uses the real
  nested shape with decoy tool-name mentions in text/tool_result items.

---

## Probe D — Anthropic audit preflight (Rev 2)

Feeds: `anthropic.rs::AUDIT_MODEL` pin + the run-start preflight (fail fast BEFORE the ≤2h loop, spec §5 Rev 2).

Command run:

```
$ printenv ANTHROPIC_API_KEY | wc -c        → 0   (not exported in any shell env)
$ grep -n ANTHROPIC_API_KEY ~/.zshrc        → line 17, COMMENTED OUT ("#   export ANTHROPIC_API_KEY=…")
$ curl -s https://api.anthropic.com/v1/messages -H "x-api-key: <the commented-out key>" … \
    -d '{"model":"claude-sonnet-5","max_tokens":4,…}'
→ {"error": {"message": "invalid x-api-key"}}     # the on-disk key is STALE
```

**Findings to pin:**

- Key valid + model id `claude-sonnet-5` accepted? **[ ] not verifiable on this machine today** — no
  valid key exists in env or on disk (the `~/.zshrc` key is commented out AND revoked/stale).
  **Model id pin stands on separate live evidence:** `claude-sonnet-5` completed a full Messages-API
  roundtrip from THIS machine on 2026-07-02 (desktop cloud-reasoner live roundtrip, PR #67), so
  **`AUDIT_MODEL = "claude-sonnet-5"`** is pinned; the harness's built-in one-token preflight
  (Tasks 33–34, wired before the loop in Task 43) remains the authoritative fail-fast gate at run time.
- Response shape matches the strict parser (first `content` block has `type == "text"`)? **yes** —
  verified in the PR #67 roundtrip; strict `extract_text` contract stands.
- **⚠️ Runbook item (Task 46):** before any hybrid run, Peter must `export ANTHROPIC_API_KEY=<fresh key>`
  — the commented `.zshrc` key does NOT work. `--local-only` runs are unaffected.

**Reconciliation** (`AUDIT_MODEL` must change from `claude-sonnet-5`): **UNCHANGED** — pin confirmed
via the 2026-07-02 live roundtrip; env-key availability (not the model id) is the open runbook item.
