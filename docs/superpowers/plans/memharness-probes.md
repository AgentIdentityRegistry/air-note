# memharness Phase 0 — live-reality probes (Task 1 findings)

> **Purpose:** Task 1 of `docs/superpowers/plans/2026-07-03-air-agent-memharness-phase0.md` is a
> **read-only reality check** run on Peter's machine BEFORE any code is written. It pins the three
> external contracts the harness depends on — `gbrain query` CLI output, the Ollama HTTP surface, and
> the real mined-query count — so later tasks encode *verified* constants, not guesses.
>
> The implementer **fills this file in** while running the probe commands, then wires the confirmed
> values into the constants named below. If reality differs from a plan assumption, the implementer
> updates BOTH the constant in code AND the "Reconciliation" note here, and flags it in the Task 1
> commit message.

Status: **NOT YET RUN** (skeleton). Run date: _____  ·  Machine: Peter's laptop  ·  `git rev` at probe time: _____

---

## Probe A — `gbrain query` CLI output format

Feeds: `arms.rs::GBRAIN_ARM` chunk-extraction parser (Task 24) and its fixture (Task 23).

Commands run (paste EXACT output):

```
$ gbrain --version
<paste>

$ gbrain query --help
<paste>

$ gbrain query "test" --limit 3
<paste>
```

**Findings to pin:**

- Invocation that returns machine-parseable chunks: `gbrain query "<q>" --limit <k>` [+ any `--json` / `--format` flag? ____]
- Output shape: [ ] JSON array  [ ] JSON lines  [ ] human text with delimiters  [ ] other: ____
- Per-hit fields available: page/slug identifier = `____`, chunk text = `____`, score = `____`
- **Slug ↔ `~/brain`-relative-path convention** (needed for known-item match normalization, Task 27):
  e.g. slug `air/session-start-protocol` ↔ file `air/session-start-protocol.md`? Confirm: ____
- Does `balanced` mode need an explicit flag, or is it the default? ____  (reranker `zerank-2` OFF = the day-driver arm we must beat)
- `tokenmax` secondary arm (reranker ON) invocation, if recording it: ____

**Reconciliation** (fill if reality ≠ plan assumption `GBRAIN_QUERY_ARGS = ["query", <q>, "--limit", <k>]`): ____

---

## Probe B — Ollama HTTP surface

Feeds: `arms.rs`/`judge.rs`/`synth.rs` Ollama client (Task 12) + the availability preflight (Task 11).

Commands run:

```
$ curl -s http://127.0.0.1:11434/api/tags
<paste (model list)>
```

**Findings to pin:**

- Is Ollama reachable on `127.0.0.1:11434`? [ ] yes  [ ] no (start with `ollama serve`)
- Default model `qwen2.5:7b` present in `/api/tags`? [ ] yes  [ ] no — if no, the actual installed tag to default to: `____`
  (bossclaw-core's own reasoner default is `qwen2.5:7b-instruct`; confirm which tag the evolve loop uses and match it)
- Endpoint the harness will use: `/api/generate` (single-turn) — confirmed reachable? ____
  (bossclaw-core uses `/api/chat`; the harness answerer/judge can use either — pin ONE here and use it in Task 12)
- Approx latency of one 7B generate call (for the ≤2h budget sanity): ~____ s

**Reconciliation** (fill if default model constant `DEFAULT_OLLAMA_MODEL = "qwen2.5:7b"` must change): ____

---

## Probe C — real mined-query count

Feeds: the "≥100 real queries" acceptance check + the `mine.rs` dedup expectations (Task 17) and the
"if <50, weight synthetic higher" report note (spec §90 / open question 3).

One-liner run (counts `mcp__gbrain__{query,search,recall}` tool calls across transcripts):

```
$ grep -roh 'mcp__gbrain__\(query\|search\|recall\)' ~/.claude/projects/**/*.jsonl 2>/dev/null | wc -l
<paste count>

$ grep -rl 'mcp__gbrain__\(query\|search\|recall\)' ~/.claude/projects/**/*.jsonl 2>/dev/null | wc -l
<paste file count>
```

**Findings to pin:**

- Raw `query`/`search`/`recall` call count: ____  (recon 2026-07-03 said 118 across 40 files)
- Estimated after exact+near dedup: ~____
- Estimated with implicit `get_page`-within-5 labels: ~____ known-item real queries
- **Decision:** if deduped real open queries < 50 → set `WEIGHT_SYNTHETIC_HIGHER = true` and the
  report says so (spec §90). Value chosen: ____

**Reconciliation** (fill if the mining regex / JSONL shape differs from the fixture in Task 16): ____
