# M6 — The Actuator Program (Design)

- **Date:** 2026-06-20
- **Status:** **Program design — shape approved by Peter 2026-06-20.** This doc maps the *whole* program; **build is deferred**. Each brick (M6a → M6b → M6c) gets its own spec → independent critic+security review → Rev 2 → plan → subagent-driven build → PR, in its own session. **M6a is the next build.**
- **Milestone:** M6 — the actuator. **Brick 2 of [[air/vault-brain-architecture]]** (the fail-closed gate that consumes the taint root so nothing tainted acts without a gate).
- **Parent:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` (§5.11 file actuator, §5.12 grant manager, §8.4 confused-deputy threat, §24 M6 = v1 cut-line). **Consumes** the `is_external` O(1) taint signal from `2026-06-20-bossclaw-core-extraction-from-files-design.md` (§9 "M6 is simplified, not built — `is_external` is now sufficient for M6's fail-closed fast path; the lineage walk becomes a backstop/audit").
- **Crate:** `crates/bossclaw-core` (`#![forbid(unsafe_code)]`).

---

## 1. Goal & framing

The actuator is the first time `bossclaw-core` does anything other than remember + recall: it **writes**. M6 gives the brain the ability to **act on your files — eventually autonomously — without anything tainted or unauthorized slipping through.**

The full vision (Peter, 2026-06-20, "shoot for the moon"): the brain proposes **any supported file edit**, including under standing **mandates** it has been granted. That is a *program*, not a single milestone — so it is decomposed into three bricks (§5). The program is governed by **one pipeline and one security model**; the bricks only change *who proposes the write*.

The master design doc names M6 the **v1 cut-line**: the memory engine ships complete without it; the actuator is the first *action* capability layered on top.

---

## 2. Scope decisions (locked 2026-06-20)

| # | Decision | Choice |
|---|----------|--------|
| **D1** | **M6 core** | The **file-write actuator** — NOT the confidentiality/leak egress bouncer (recall→prompt tier-exclusion, A2A DLP). That is a *different* gate that needs sensitivity tiers; it is Vault-Brain **brick 3**, deferred. |
| **D2** | **Vision** | **Full autonomous proposer** ("any supported edit, incl. mandates") — decomposed into M6a/M6b/M6c so each session ships a clean cut. |
| **D3** | **Engine/app boundary** | The **engine** (`crates/bossclaw-core`) owns the gated mechanism + the proposer + the signed events. The **desktop app** owns the human preview/confirm UI. This program specs the **engine** side only (matching every prior engine-only milestone M1–M5b). |
| **D4** | **OS** | `#[cfg(unix)]` for v1 (Windows write path deferred, as M5a/M5b deferred Windows ingest). |
| **D5** | **Action surface** | **Files only** — create/edit/delete. No shell, process, or network actions. |
| **D6** | **Taint posture** | Tainted-origin writes are **allowed but loud** (a forced, un-bundleable confirm with provenance), NOT silently blocked — matching §5.11's "loud modal." The engine computes the *verdict*; the app renders the modal. |

---

## 3. The pipeline (every write, every brick)

```
propose  →  GATE  →  human confirm  →  execute  →  record  →  undo
            │                          │
            │                          └─ re-check target ⊆ write-grant AT EXECUTE TIME
            │                             (canonicalized, inside the rename critical
            │                             section → closes TOCTOU), then temp-write +
            │                             atomic rename
            └─ provenance · fail-closed taint-by-origin (is_external fast-path +
               lineage-walk backstop) · target ⊆ write-grant · secret/value diff-guard
```

- **`propose`** produces a *proposal*: `(target_path, new_content_or_diff, source_event_ids, rationale)`. The proposer differs per brick (§5); the proposal shape does not.
- **`GATE`** is pure + engine-side: it computes provenance, the taint verdict, target eligibility, and the diff-guard flags. It never executes; it returns a verdict the app renders.
- **`human confirm`** is the app's job. The engine exposes the verdict + the diff; the app shows the modal and returns the user's decision. (No engine write happens without an explicit confirmed `execute` call.)
- **`execute`** re-canonicalizes the target and re-checks `target ⊆ write-grant` **inside the rename critical section** (a path that passed the gate can be swapped before execute — TOCTOU; the re-check closes it), then temp-writes + atomic-renames.
- **`record`** appends a signed `file_written` event to the same append-only, hash-chained log (consistency with M1's substrate; the write is itself auditable + lineage-bearing).
- **`undo`** retains enough to revert the last write (the pre-write bytes or a reverse patch) behind an undo token.

---

## 4. Security model — the confused-deputy threat is the whole point

A booby-trapped file **cannot command** the reasoner (untrusted content is fenced as data — the extraction-from-files Pass-A/compose fences, now breakout-hardened). But it **can socially-engineer a benign-looking edit proposal** (the *confused deputy*): "helpfully" steering the brain to write an attacker's payload to a sensitive file. No single control stops this, so the program is **defense-in-depth — every control required, none load-bearing alone** (§8.4: "raises the bar against direct injection; does **not** by itself stop confused-deputy proposals").

| Control | What it stops | Brick |
|---|---|---|
| **Write-grants** (separate from M5a read-grants; §5.12) | writing outside explicitly write-allowed folders | M6a |
| **Execute-time target re-check** (canonicalized, in the rename critical section) | TOCTOU swap-the-path races between gate and execute | M6a |
| **Fail-closed taint-by-origin** — `is_external` O(1) fast-path + signed-lineage-walk backstop; unknown/unresolvable provenance ⇒ tainted | a tainted-lineage edit sneaking through un-flagged; self-reported provenance being trusted | M6a |
| **Provenance display** ("this edit came from `~/x/README.md`, ingested 2026-06-14") | invisible influence — the user can't judge a proposal they can't trace | M6a |
| **Secret/value-shaped diff guard** (advisory, NOT a boundary) | escalates the loud confirm for diffs touching money / keys / URLs / `curl\|sh` / crontab / shell-rc. A denylist misses obfuscated payloads — load-bearing controls are target-restriction + taint + human confirm; this only raises the prompt | M6a |
| **Anti-fatigue** (no "approve all"; different-file proposals can't be bundled; deletes always get the loud modal) | rubber-stamping a batch that hides one bad write | M6a |
| **Atomic write + undo** | a bad write being partial or unrecoverable | M6a |

**Key invariant (carried from §5.11 + extraction-from-files):** every Tier-B event keeps a **mandatory, non-empty `source_event_ids`** (rejected at append otherwise). The taint gate walks *that signed lineage itself*; it never trusts an event's self-reported origin. The `is_external` stamp makes the common case O(1); the walk is the audit backstop.

---

## 5. The three bricks

Each brick is an independent session (spec → review → build → PR). M6b needs M6a; M6c needs M6a + M6b.

### M6a — "Safe hands" (the foundation; **next build**)
The gated write **mechanism**, with proposals from an **explicit caller** (no autonomous proposer yet).
- **Builds:** the write-grant model (§5.12, separate read/write); `propose_write(target, content, source_event_ids) → Proposal` (the §4 gate, pure); `execute_write(confirmed_proposal) → file_written` (execute-time re-check + temp/atomic-rename + signed event + undo); the verdict surface the app renders.
- **Consumes:** `is_external` (the fail-closed fast-path is just `any(is_external(src))` over the proposal's lineage — already O(1) after extraction-from-files).
- **Proves:** create/edit/delete are gated; an out-of-grant target is rejected; a swapped path is caught at execute; a tainted-lineage proposal yields the loud verdict; an atomic write is recoverable via undo; hermetic tests with scripted proposals + a TOCTOU race test.
- **Open questions (its own spec):** undo depth (last-write vs N-deep); diff vs whole-file content in the proposal; delete semantics (trash vs hard-delete); the exact `file_written` content shape + whether undo is itself a signed event.

### M6b — Reconciliation proposer (the first autonomous trigger)
The evolve loop emits `write_proposal` events when current knowledge **contradicts/supersedes** content in an ingested file.
- **Builds:** a new evolve-phase output (alongside entity/link/page) that, on an M4 `invalidate`/contradiction touching an ingested file's content, proposes the reconciling edit; `write_proposal` (Tier-B, signed, taint-stamped) → flows through M6a's mechanism for confirm + execute.
- **Reuses:** M4's contradiction/`invalidate` machinery (the "this fact superseded that one" signal already exists).
- **Open questions:** when does a contradiction warrant a *file* edit vs just a graph `invalidate`? how is the concrete diff synthesized (and fenced)? rate-limiting proposals; the live-model gate.

### M6c — General + mandate proposer (the moon shot)
The brain proposes **any supported edit**, including under standing **mandates** the user grants.
- **Builds:** a **mandate primitive** (what a mandate is, how it is granted / scoped / revoked — a signed, bounded standing goal); the general proposer that works toward a mandate using the whole knowledge graph.
- **Widest confused-deputy surface** — every §4 control matters most here.
- **Open questions:** the mandate schema + grant/revoke UX + signing; bounding autonomous proposal volume; how mandates compose with the taint gate (a mandate never widens write-grants or shed taint); this likely warrants its own brainstorming before a spec.

---

## 6. Data model (new, additive)

- **`file_written`** — signed Tier-B event in the existing append-only log: `{target (canonical), content_hash, prev_content_hash (for undo), byte_size, source_event_ids, producer, ...}`. Carries the taint stamp via the §4 chokepoint like any Tier-B event.
- **`write_proposal` / `write_rejected`** (M6b+) — the autonomous proposal and its outcome; `write_proposal` is Tier-B, taint-stamped, with the inducing `source_event_ids`.
- **Write-grant record** (M6a) — the grant manager gains a **mode** (read | write), distinct from M5a's read grants; `is_allowed(path, Write)` over the canonicalized real path with path-segment descent from a write-granted root.
- **Undo state** — the pre-write bytes or reverse patch, keyed by the `file_written` id.

---

## 7. Non-goals (whole program)

- **The confidentiality / "leak" egress bouncer** — recall→prompt tier-exclusion + A2A-send DLP. A *different* gate needing sensitivity tiers (Vault-Brain **brick 3**, vault-memory). M6 is the **integrity** gate (don't let tainted content *act*), not the **confidentiality** gate (don't let secrets *leave*).
- **Shell / process / network actions** — files only (D5).
- **Windows** write path (D4).
- **The desktop confirm UI** — the app's job (D3).

---

## 8. Build sequence

**M6a (next) → M6b → M6c.** Each brick: its own spec → independent **critic + security** review (the established AIR gate; the actuator is security-critical, so the security pass is mandatory) → Rev 2 → plan → subagent-driven build (per-task two-stage review + whole-impl Opus SHIP) → PR. M6c likely earns its own *brainstorming* before a spec (the mandate primitive is a fresh design surface).

---

## 9. Cross-links

[[air/vault-brain-architecture]] (parent program; M6 = brick 2; open q #7 "egress policy language" resolves across M6a/M6c) · `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` (§5.11/§5.12/§8.4/§24) · `docs/superpowers/specs/2026-06-20-bossclaw-core-extraction-from-files-design.md` (the `is_external` signal this consumes) · [[air/forever-companion-architecture]] (the "acts on your behalf under a Mandate" vision M6c realizes).
