# Rung 3 — Phase 3: Conflict **Resolution** (Code-native) — Implementation Plan

> For agentic workers: REQUIRED SUB-SKILL: superpowers:subagent-driven-development

**Goal.** Let the owner **resolve** a `conflict_proposal` (which Rung-3 Phase-2 detection emits) from Claude
Code, and make BOTH the finder AND the read surface **honor** the resolution so the pair never re-surfaces.
Four deterministic actions, **no LLM in the path**: **Retire older** / **Retire newer** (set the losing
FROZEN side aside via the Phase-1 reversible retire primitives), **Keep both** (coexist — never re-proposed
and dropped from `list_conflicts`), **Dismiss** (snooze; re-opens on a material change). A wrong resolve
costs a *reversible* retire + a visible digest entry, never a lost memory (invariant I1). Implements the
CONVERGED design `docs/superpowers/specs/2026-07-17-rung3-phase3-resolution-design.md` (Rev 3, folded through
6 independent reviews). Detection is off-by-default, so resolution is inert when detection is off — Phase 3
ships **dormant** (merging changes nothing at runtime).

**Architecture.** Four thin layers, mirroring the shipped Rung-3 Phase-1/2 split.
- **`bossclaw-core` (the brain).** Three terminal event types (`conflict_resolved`/`coexist_allowed`/
  `dismissed`); provenance-stamped retire (`retire_memory`/`retire_passage` gain an optional
  `source_proposal_id`); the `resolve_conflict` orchestrator (idempotency over an ALL-proposals fold +
  roll-forward on a torn write); a `resolution_exclusions()` reader consumed by BOTH the finder union and the
  `pending_conflict_proposals` filter; the conflict-cursor rewind wired into `unretire`/`unretire_passage`;
  the pair-granular poison budget; and the visibility-digest counts + `conflict_digest_cursor`.
- **`bossclawd-proto` (the wire).** Two new `Request` variants (`ListConflicts`, `ResolveConflict`), a wire
  `ResolveActionWire` enum, wire `ConflictRefWire` + `ConflictProposalWire` mirrors with `From`/`Into`
  conversions (the established Family-1 pattern in `types.rs`), two `Response` arms, and the `Role::allows`
  grant of BOTH ops to `MemoryClient` — the deliberate **I8 relaxation**.
- **`bossclawd` (the daemon).** `EngineHandle::list_conflicts` / `resolve_conflict` wrappers; the two server
  dispatch arms (with daemon-side `sanitize_injected` on the `ListConflicts` response — MINOR-1); the
  `override_onboarding_for_guest` passthrough arms; the snapshot-digest render in the never-truncated
  `render_fence` preamble.
- **`air-memory-mcp` (the Code-native surface).** Two MCP tools — `list_conflicts` and `resolve_conflict` —
  beside `recall`/`remember`, and their thin socket-client methods.

**Resolution only (Phase-3 boundary, do not cross).** NO desktop conflict card / nav badge (deferred,
background-first). NO rate cap on the resolve ops (Rev 1's per-connection limiter is a no-op against a
reconnecting MCP client — dropped; safety rests on reversibility + the working digest + the signed log). NO
auto-resolve. NO fold-GC of stale coexist/dismissed markers (accepted append-only growth, I5). NO
`unretire`/undo MCP tool (the cursor rewind is wired into the primitive; a Code-facing undo is a deferred
follow-up).

**Tech Stack.** Rust (workspace crates `bossclaw-core`, `bossclawd-proto`, `bossclawd`, `air-memory-mcp`);
`rusqlite`/SQLCipher; `serde_json`; tokio (`spawn_blocking` at the engine boundary). Tests are Rust
`#[test]`/`#[tokio::test]` matching each crate's existing style (`tempfile::tempdir` + `open_log` in core;
`MockEmbedder`/`MockReasonerProvider`/`ScriptedReasoner` fixtures; serde round-trips in proto). **All cargo
commands are SYNCHRONOUS / foreground** (never backgrounded) so each red→green transition is observed before
the next step.

**`#[cfg(unix)]` discipline (inherited from Phase 2).** In `bossclaw-core` the entire `conflict_proposal`
family is `#[cfg(unix)]` (`append_conflict_proposal` `log.rs:2772`, `open_conflict_proposals` `:2807`,
`pending_conflict_proposals` `:2869`, `conflict_pair_key` `:2891`, `is_conflict_proposal_suppressed` `:2900`,
`detect_conflicts_once` `:2305`). Phase 3's NEW methods that read/append within this subsystem —
`resolution_exclusions`, `resolution_markers`, `conflict_proposal_by_id`, and `resolve_conflict` — are
therefore ALSO `#[cfg(unix)]`, and their tests carry the same gate (they seed a proposal via the
`#[cfg(unix)]` `append_conflict_proposal`). The PORTABLE additions stay ungated: the three event-type consts,
the `source_proposal_id` retire params (the App retire path is cross-platform), the `conflict_pair_errors`
table + accessors, the conflict-cursor rewind in `unretire`/`unretire_passage` (App-reachable), the
`conflict_digest_cursor` + `conflict_digest_counts` (plain SQL over event seqs), the `ResolveAction`/
`ResolveOutcome`/`ConflictDigest` data types, and the new `seq_of_event` seam. (`ResolutionExclusions` is
`#[cfg(unix)]` — it lives with the proposal subsystem it reads; Task 3.) On the
`bossclawd` side `engine`/`server`/`capture` are already `#[cfg(unix)]` modules, so the new engine wrappers,
dispatch arms, and snapshot preamble inherit the gate — no per-fn gates there.

### As-built anchors — VERIFIED against `main` `2cf0ccb` (2026-07-17)

Every function a test calls below already exists at these lines (or is created by an earlier task). Trust
these; re-grep before editing if the file has drifted.

| Symbol | File | Line (verified) |
| --- | --- | --- |
| retire markers `NOTE_RETIRED`/`PASSAGE_RETIRED`/`UNRETIRE` | `graph.rs` | `:40`/`:42`/`:44` |
| `CONFLICT_PROPOSAL_EVENT_TYPE` / `CONFLICT_PROPOSER_PRODUCER` | `graph.rs` | `:107`/`:109` |
| `ConflictRef` + `pair_key` / `unordered_pair_key` / `to_json` / `from_json` | `index.rs` | `~:85`/`:120`/`:132`/`:137`/`:149` |
| `ConflictProposalRow` / `ConflictSubject` / `ConflictDetectReport` structs | `log.rs` | `:441`/`:464`/`:477` |
| `SessionFold` struct (`.retired_notes`/`.retired_passages`) / `OpenConflictProposal` (private) | `log.rs` | `:8948` (`:8968`/`:8973`) / `:8981` |
| `append_conflict_proposal` (`#[cfg(unix)]`) | `log.rs` | `:2772` |
| `open_conflict_proposals` / `pending_conflict_proposals` | `log.rs` | `:2807`/`:2869` |
| `conflict_pair_key` / `is_conflict_proposal_suppressed` | `log.rs` | `:2891`/`:2900` |
| `retire_memory` (marker `:5063`) / `unretire` (`:5079`) | `log.rs` | `:5056` |
| `retire_passage` (marker `:5140`, already-retired err `:5130`) / `unretire_passage` | `log.rs` | `:5109`/`:5160` |
| `assert_retirable_note` (err `:5214`) / `assert_note_retired` | `log.rs` | `:5196`/`:5225` |
| `detect_conflicts_once` (params) / `open_pairs` assembly / finder call / older-newer / error `break` / cursor advance | `log.rs` | `:6305`/`:6365`/`:6439`/`:6453`/`:6510`-`:6519`/`:6520` |
| `conflict_cursor` / `set_conflict_cursor` (2-D `(last_seq, subject_offset)`) | `log.rs` | `:6612`/`:6626` |
| `conflict_detect_enabled` / `unprocessed_conflict_subjects_since` | `log.rs` | `:7068`/`:7153` |
| `events_of_types(&[&str]) -> Vec<Event>` / `event_by_id` / `session_events_ordered` / `fold_sessions` | `log.rs` | `:7228`/`:1109`/—/`:8999` |
| `fold_sessions` reads `.get("retires")` `:9035` / `.get("unretires")` `:9045` (NO `deny_unknown_fields`) | `log.rs` | `:9035`/`:9045` |
| `Role` / `Role::allows` / `Request` / `RetireTarget` / `Response` (`Retired` `:337`) | `proto/lib.rs` | `:55`/`:71`/`:126`/`:253`/`:279` |
| `memory_client_allows_exactly_four_ops` / `new_variants_round_trip_serde` | `proto/lib.rs` | `:844`/`:871` |
| `NoteWire` / `GrantMirror` `From` (Family-1 pattern) | `proto/types.rs` | `:708`/`:52` |
| `CaptureRateLimiter` / `is_rate_limited_op` / `override_onboarding_for_guest` / `dispatch` | `bossclawd/server.rs` | `:68`/`:93`/`:210`/`:243` |
| `RetireMemory` arm / `Unretire` arm / `Snapshot` arm / guest-override test | `bossclawd/server.rs` | `:450`/`:462`/`:411`/`:1074` |
| engine `retire_memory` / `unretire` / `retire_passage` / `detect_conflicts_once` (empty-excluded `:1091`) | `bossclawd/engine/mod.rs` | `:816`/`:834`/`:854`/`:1060` |
| `SNAPSHOT_MAX_BYTES` `:62` / `FENCE_OPEN` `:84` / `sanitize_injected` `:104` / `build` `:207` / `assemble_fence` `:428` / `render_fence` `:444` | `bossclawd/capture/snapshot.rs` | — |
| `TOOL_RECALL`/`TOOL_REMEMBER` / `tools_list_result` / `tools/call` routing | `air-memory-mcp/mcp.rs` | `:22`/`:82`/`:64` |
| `call_daemon` / `tool_recall` / `map_error_response` | `air-memory-mcp/daemon.rs` | `:80`/`:151`/`:135` |
| fresh-brain config-event-count assertion (`== 5`, dormancy trip-wire) | `bossclawd/tests/roundtrip.rs` | `:173` |

---

## File Structure

| File | Create/Modify | Responsibility |
| --- | --- | --- |
| `crates/bossclaw-core/src/graph.rs` | Modify | `CONFLICT_RESOLVED_EVENT_TYPE`/`COEXIST_ALLOWED_EVENT_TYPE`/`DISMISSED_EVENT_TYPE` consts (T1). |
| `crates/bossclaw-core/src/log.rs` | Modify | `source_proposal_id` on retire primitives (T2); `resolution_exclusions` + `ResolutionExclusions` (T3); `conflict_proposal_by_id` (T4); `resolution_markers` + `ResolutionRecord` (T5); `ResolveAction`/`ResolveOutcome` + `resolve_conflict` (T6); finder `open_pairs` union (T7); `pending_conflict_proposals` filter (T8); `seq_of_event` + cursor rewind in `unretire`/`unretire_passage` (T9); `conflict_pair_errors` table + accessors + poison loop refactor + `poison_skipped` report field (T10); `conflict_digest_cursor` + `conflict_digest_counts` + `ConflictDigest` (T11). |
| `crates/bossclaw-core/src/lib.rs` | Modify | Re-export `ResolveAction`, `ResolveOutcome`, `ConflictDigest` (T6, T11). |
| `crates/bossclawd-proto/src/lib.rs` | Modify | `Request::ListConflicts`/`ResolveConflict`, `Response::ListConflicts`/`ResolveConflict`, `Role::allows` grant + allowlist tests (T12). |
| `crates/bossclawd-proto/src/types.rs` | Modify | `ResolveActionWire`, `ConflictRefWire`, `ConflictProposalWire` + `From`/`Into` conversions + round-trips (T12). |
| `crates/bossclawd/src/engine/mod.rs` | Modify | `list_conflicts` / `resolve_conflict` wrappers; `conflict_digest_lines` (T13, T14). |
| `crates/bossclawd/src/server.rs` | Modify | Two dispatch arms + daemon-side sanitize; `override_onboarding_for_guest` passthrough arms; NOT-rate-limited test (T13). |
| `crates/bossclawd/src/capture/snapshot.rs` | Modify | `render_fence`/`assemble_fence` gain a never-dropped `preamble`; `build` prepends `conflict_digest_lines` (T14). |
| `crates/air-memory-mcp/src/mcp.rs` | Modify | `TOOL_LIST_CONFLICTS`/`TOOL_RESOLVE_CONFLICT` + `tools_list_result` + `tools/call` routing + arg parsers (T15). |
| `crates/air-memory-mcp/src/daemon.rs` | Modify | `tool_list_conflicts` / `tool_resolve_conflict` socket-client methods (T15). |

**New constant (in `bossclaw-core/src/conflict.rs`, provisional / harness-tunable — design §7):**

```rust
/// Per-pair CONSECUTIVE reasoner-error cap (spec §3.3). At/above this the pair is `poison_skipped`
/// (stops holding the cursor + stops being judged); below it the subject retries next cycle (I6).
/// Chosen so a brief reasoner blip retries but a deterministically-erroring pair is bounded.
pub const CONFLICT_PAIR_ERROR_BUDGET: usize = 3;
```

---

## Task 1 — Three terminal resolution event types

Design §2.1. Adds the signed terminal markers beside `CONFLICT_PROPOSAL_EVENT_TYPE`. Single-sourced consts so
the append sites and every fold filter share the string. Ungated (portable consts).

**Files**
- Modify: `crates/bossclaw-core/src/graph.rs` (after `CONFLICT_PROPOSER_PRODUCER` `:109`).
- Test: `crates/bossclaw-core/src/graph.rs` (`#[cfg(test)] mod tests`, or the crate's const-uniqueness test if one exists).

**Steps**

1. Write the failing test (append into `graph.rs` `mod tests`):

```rust
#[test]
fn rung3_phase3_terminal_event_types_are_distinct_and_stable() {
    // The three terminal markers are pairwise distinct and distinct from the proposal + retire types.
    let all = [
        CONFLICT_RESOLVED_EVENT_TYPE,
        COEXIST_ALLOWED_EVENT_TYPE,
        DISMISSED_EVENT_TYPE,
        CONFLICT_PROPOSAL_EVENT_TYPE,
        NOTE_RETIRED_EVENT_TYPE,
        PASSAGE_RETIRED_EVENT_TYPE,
    ];
    let uniq: std::collections::HashSet<&str> = all.iter().copied().collect();
    assert_eq!(uniq.len(), all.len(), "all conflict/retire event types are distinct strings");
    // Stable wire strings (a rename would orphan already-signed events).
    assert_eq!(CONFLICT_RESOLVED_EVENT_TYPE, "conflict_resolved");
    assert_eq!(COEXIST_ALLOWED_EVENT_TYPE, "coexist_allowed");
    assert_eq!(DISMISSED_EVENT_TYPE, "dismissed");
}
```

2. Run → FAIL: `cargo test -p bossclaw-core rung3_phase3_terminal_event_types_are_distinct_and_stable`
   Expected: `cannot find value CONFLICT_RESOLVED_EVENT_TYPE`.

3. Implement (in `graph.rs`, after `:109`):

```rust
/// Rung-3 Phase-3 terminal marker: a `conflict_proposal` was RESOLVED by a retire action (signed).
/// Content: `{ "proposal_id": <id>, "action": "retire_older"|"retire_newer", "retired_event_id": <str> }`.
/// The retire marker (written FIRST, provenance-tagged) is the torn-write-safe source the digest counts;
/// this marker records the owner's decision for idempotency + audit. Single-sourced.
pub const CONFLICT_RESOLVED_EVENT_TYPE: &str = "conflict_resolved";
/// Rung-3 Phase-3 terminal marker: the owner chose KEEP BOTH (signed). Content:
/// `{ "proposal_id", "pair_key", "a_ref", "b_ref" }`. Suppresses re-proposal AND drops the pair from
/// the read surface (I9). Single-sourced.
pub const COEXIST_ALLOWED_EVENT_TYPE: &str = "coexist_allowed";
/// Rung-3 Phase-3 terminal marker: the owner DISMISSED (snoozed) the pair (signed). Content:
/// `{ "proposal_id", "pair_key", "a_ref", "b_ref", "session_heads": {session_id: head_event_id} }`.
/// The dismissal is LIVE only while every referenced session head is unchanged (§3.1). Single-sourced.
pub const DISMISSED_EVENT_TYPE: &str = "dismissed";
```

4. Run → PASS: `cargo test -p bossclaw-core rung3_phase3_terminal_event_types_are_distinct_and_stable`

5. Commit: `feat(rung3-p3): conflict_resolved / coexist_allowed / dismissed terminal event types`

---

## Task 2 — Provenance-stamped retire (`source_proposal_id`)

Design §2.1 (MAJOR-2), §3.4. `retire_memory`/`retire_passage` gain an optional `source_proposal_id`. When
`Some`, the retire marker's content gains `{"via":"conflict","proposal_id":<id>}` in the SAME marker type
(the retire fold keys on `retires` / `session_id`+`passage_id` and is UNTOUCHED — `fold_sessions` reads
`.get("retires")` at `log.rs:9035` with no `deny_unknown_fields`, verified). The App path passes `None`
(byte-identical to today). This is the conflict-scoped, torn-write-safe source the §3.4 digest R-count reads.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `retire_memory` (`:5056`), `retire_passage` (`:5109`).
- Modify: `crates/bossclawd/src/engine/mod.rs` — `retire_memory` (`:816`), `retire_passage` (`:854`) pass `None`.
- Modify: `crates/bossclawd/src/server.rs` — nothing (calls the engine wrappers unchanged).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing test:

```rust
#[test]
fn retire_stamps_conflict_provenance_but_fold_and_app_path_are_untouched() {
    use crate::graph::{NOTE_RETIRED_EVENT_TYPE, PASSAGE_RETIRED_EVENT_TYPE};
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);

    // (a) App path (None) — the marker is byte-identical to today: {"retires": id}, no `via`.
    let n_app = log.remember(&emb, "app-retired note").unwrap();
    let m_app = log.retire_memory(&n_app, None).unwrap();
    let ev = log.event_by_id(&m_app).unwrap().unwrap();
    assert_eq!(ev.event_type, NOTE_RETIRED_EVENT_TYPE);
    assert_eq!(ev.content.get("retires").and_then(|v| v.as_str()), Some(n_app.as_str()));
    assert!(ev.content.get("via").is_none(), "App retire carries NO provenance tag");

    // (b) Conflict path (Some) — same marker TYPE, plus the provenance tag; the fold still retires it.
    let n_conf = log.remember(&emb, "conflict-retired note").unwrap();
    let m_conf = log.retire_memory(&n_conf, Some("PROP1")).unwrap();
    let ev = log.event_by_id(&m_conf).unwrap().unwrap();
    assert_eq!(ev.event_type, NOTE_RETIRED_EVENT_TYPE, "SAME marker type as the App path");
    assert_eq!(ev.content.get("retires").and_then(|v| v.as_str()), Some(n_conf.as_str()));
    assert_eq!(ev.content.get("via").and_then(|v| v.as_str()), Some("conflict"));
    assert_eq!(ev.content.get("proposal_id").and_then(|v| v.as_str()), Some("PROP1"));
    // The retire fold is untouched: the note is no longer current (recall/list drop it).
    assert!(!log.current_notes().unwrap().iter().any(|c| c.event_id == n_conf), "tagged retire still retires");

    // (c) Passage retire carries the tag too, same shape.
    let cev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
    log.store_session_passages(&emb, &cev, &["p0".to_string(), "p1".to_string()]).unwrap();
    let pm = log.retire_passage("s1", 0, Some("PROP2")).unwrap();
    let ev = log.event_by_id(&pm).unwrap().unwrap();
    assert_eq!(ev.event_type, PASSAGE_RETIRED_EVENT_TYPE);
    assert_eq!(ev.content.get("session_id").and_then(|v| v.as_str()), Some("s1"));
    assert_eq!(ev.content.get("passage_id").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(ev.content.get("via").and_then(|v| v.as_str()), Some("conflict"));
    assert_eq!(ev.content.get("proposal_id").and_then(|v| v.as_str()), Some("PROP2"));
}
```

2. Run → FAIL: `cargo test -p bossclaw-core retire_stamps_conflict_provenance_but_fold_and_app_path_are_untouched`
   Expected: `this method takes 1 argument but 2 arguments were supplied` (or the passage variant).

3. Implement. In `retire_memory` (`:5056`) change the signature + build the content with an optional tag:

```rust
    pub fn retire_memory(
        &self,
        target_event_id: &str,
        source_proposal_id: Option<&str>,
    ) -> Result<String, BossclawError> {
        self.assert_retirable_note(target_event_id)?;
        // Base marker (byte-identical to today when `source_proposal_id` is None). The retire FOLD keys
        // on `retires` only (log.rs:9035), so the additive `via`/`proposal_id` fields never disturb it —
        // they exist ONLY to make the §3.4 digest R-count conflict-scoped AND torn-write-safe.
        let mut content = serde_json::Map::new();
        content.insert("retires".to_string(), serde_json::Value::String(target_event_id.to_string()));
        if let Some(pid) = source_proposal_id {
            content.insert("via".to_string(), serde_json::Value::String("conflict".to_string()));
            content.insert("proposal_id".to_string(), serde_json::Value::String(pid.to_string()));
        }
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::NOTE_RETIRED_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object(content),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }
```

   In `retire_passage` (`:5109`) apply the same treatment to the marker append (after the existing
   validation block that ends at `:5134`):

```rust
        let mut content = serde_json::Map::new();
        content.insert("session_id".to_string(), serde_json::Value::String(session_id.to_string()));
        content.insert("passage_id".to_string(), serde_json::json!(passage_id));
        if let Some(pid) = source_proposal_id {
            content.insert("via".to_string(), serde_json::Value::String("conflict".to_string()));
            content.insert("proposal_id".to_string(), serde_json::Value::String(pid.to_string()));
        }
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::PASSAGE_RETIRED_EVENT_TYPE.to_string(),
            content: serde_json::Value::Object(content),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
```

   Add `source_proposal_id: Option<&str>` to `retire_passage`'s signature. Update the two `bossclawd` engine
   wrappers to pass `None` (App path):

   - `engine/mod.rs:820` `log.retire_memory(&event_id)` → `log.retire_memory(&event_id, None)`.
   - `engine/mod.rs:862` `log.retire_passage(&session_id, passage_id)` → `log.retire_passage(&session_id, passage_id, None)`.

   Update any in-crate callers/tests of the two primitives that break compilation to pass `None` (grep
   `retire_memory(` / `retire_passage(` in `bossclaw-core` tests).

4. Run → PASS: `cargo test -p bossclaw-core retire_stamps_conflict_provenance_but_fold_and_app_path_are_untouched`
   Then the Phase-1 retire golden still green: `cargo test -p bossclaw-core retire`

5. Commit: `feat(rung3-p3): optional source_proposal_id provenance stamp on retire_memory/retire_passage`

---

## Task 3 — `resolution_exclusions()` — the single reader feeding BOTH the finder and the reader

Design §2.2, §3.1, resolved Open-Q1 (ONE reader, no drift). Reads every `coexist_allowed` + `dismissed` event
and returns two `unordered_pair_key` sets. A `dismissed` pair is LIVE only while every referenced session
head is unchanged (§3.1 — passages survive re-capture, so a coarse head check is the re-open trigger; notes
re-open for free because an edit mints a new id → a new pair key that the stored key no longer matches).
`#[cfg(unix)]` (part of the proposal subsystem).

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `ResolutionExclusions` struct (module level, beside
  `OpenConflictProposal` `:8981`); `resolution_exclusions` method (after `is_conflict_proposal_suppressed`
  `:2910`). Uses `events_of_types` (`:7228`), `fold_sessions` (`:8999`), `ConflictRef::unordered_pair_key`.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (`#[cfg(unix)]`).

**Steps**

1. Write the failing test:

```rust
#[cfg(unix)]
#[test]
fn resolution_exclusions_are_live_and_dismiss_lapses_on_session_head_change() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);

    // Two notes → a note↔note coexist pair.
    let n1 = log.remember(&emb, "branch is main").unwrap();
    let n2 = log.remember(&emb, "branch is master").unwrap();
    let (a, b) = (ConflictRef::Note { event_id: n1.clone() }, ConflictRef::Note { event_id: n2.clone() });
    let pk_notes = ConflictRef::unordered_pair_key(&a, &b);
    // Seed a proposal, then keep-both.
    let prop1 = log.append_conflict_proposal(&a, &b, "unclear", "med", "why", 0, &[n1.clone(), n2.clone()]).unwrap();
    log.resolve_conflict(&prop1, ResolveAction::KeepBoth).unwrap();

    // A session passage pair → dismissed, with the session's head recorded.
    let cev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
    log.store_session_passages(&emb, &cev, &["deploy on vercel".to_string()]).unwrap();
    let n3 = log.remember(&emb, "deploy on fly.io").unwrap();
    let (pa, pb) = (
        ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 },
        ConflictRef::Note { event_id: n3.clone() },
    );
    let pk_pass = ConflictRef::unordered_pair_key(&pa, &pb);
    let prop2 = log.append_conflict_proposal(&pa, &pb, "unclear", "med", "why", 0, &[cev.clone(), n3.clone()]).unwrap();
    log.resolve_conflict(&prop2, ResolveAction::Dismiss).unwrap();

    let excl = log.resolution_exclusions().unwrap();
    assert!(excl.coexist_pairs.contains(&pk_notes), "keep-both pair is a live coexist exclusion");
    assert!(excl.dismissed_pairs.contains(&pk_pass), "dismissed pair is live while the head is unchanged");

    // Re-capture the SAME session (advances its head) → the dismissal lapses (§3.1).
    let cev2 = log.capture_session(&emb, &session_meta("s1", "bb")).unwrap();
    log.store_session_passages(&emb, &cev2, &["deploy on vercel".to_string()]).unwrap();
    let excl2 = log.resolution_exclusions().unwrap();
    assert!(excl2.coexist_pairs.contains(&pk_notes), "coexist (note↔note) is unaffected by the re-capture");
    assert!(!excl2.dismissed_pairs.contains(&pk_pass), "dismissal LAPSED after the session head advanced");
}
```

2. Run → FAIL: `cargo test -p bossclaw-core resolution_exclusions_are_live_and_dismiss_lapses_on_session_head_change`
   Expected: `no method named resolution_exclusions` (and `resolve_conflict` / `ResolveAction` — Task 6; write
   Task 3 test AFTER Task 6 lands, or stub the seed via a raw `append_conflict_proposal` + hand-built terminal
   events — see note). **Ordering note:** because this test seeds via `resolve_conflict` (Task 6), land the
   METHOD in Task 3 and its FULL test in Task 6's suite. For Task 3's own red→green, seed the terminal markers
   directly with `append` (a `coexist_allowed`/`dismissed` event) so Task 3 is independently testable:

```rust
    // Task-3-local seeding (no dependency on resolve_conflict): append the terminal markers by hand.
    // (These mirror EXACTLY what resolve_conflict will later write — same content shape.)
    fn append_coexist(log: &EventLog, pk: &str, a: &ConflictRef, b: &ConflictRef, prop: &str) {
        log.append(crate::event::Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::COEXIST_ALLOWED_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "proposal_id": prop, "pair_key": pk, "a_ref": a.to_json(), "b_ref": b.to_json() }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: log.signer_did(), signature: None,
        }).unwrap();
    }
    /// Seed a `dismissed` marker whose `session_heads` records each passage member's session CURRENT head
    /// event id AT SEED TIME. The lapse test relies on this: `head_at_seed` for `"s1"` is the id returned
    /// by the `capture_session` that created the head; a later re-capture mints a NEW head id, so
    /// `resolution_exclusions` no longer matches → the dismissal lapses (§3.1).
    fn append_dismissed(
        log: &EventLog, pk: &str, a: &ConflictRef, b: &ConflictRef, prop: &str,
        session_heads: serde_json::Value, // e.g. json!({ "s1": <capture_session return id> })
    ) {
        log.append(crate::event::Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::DISMISSED_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "proposal_id": prop, "pair_key": pk,
                "a_ref": a.to_json(), "b_ref": b.to_json(), "session_heads": session_heads,
            }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: log.signer_did(), signature: None,
        }).unwrap();
    }
```

   For the dismissed member, the seeded head is the CURRENT head at seed time — capture the session first and
   pass its head id: `let head = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();` … then
   `append_dismissed(&log, &pk_pass, &pa, &pb, "P", serde_json::json!({ "s1": head }));`. Re-capturing `"s1"`
   (a new `capture_session` for the same id) advances the head, so the second `resolution_exclusions()` no
   longer lists `pk_pass` — the lapse the test asserts.

   (Use these local seeders for Task 3's own red→green — independent of `resolve_conflict` (Task 6). The
   design's exit-gate §3 test in Task 8 exercises the real `resolve_conflict` path end-to-end.)

3. Implement.
   (a) `ResolutionExclusions` at module level (beside `OpenConflictProposal` `:8981`):

```rust
/// The two PAIR-key exclusion sets derived from the terminal resolution markers (spec §2.2). Both keyed
/// by [`crate::index::ConflictRef::unordered_pair_key`] — the SAME space as the finder's `open_pairs` and
/// `conflict_pair_key`. ONE reader ([`EventLog::resolution_exclusions`]) produces both, so the finder
/// union and the `pending_conflict_proposals` filter can never drift on `session_heads` liveness.
#[cfg(unix)]
#[derive(Debug, Default, Clone)]
pub struct ResolutionExclusions {
    /// `unordered_pair_key`s the owner chose KEEP-BOTH — never re-proposed, dropped from the read surface.
    pub coexist_pairs: std::collections::HashSet<String>,
    /// `unordered_pair_key`s DISMISSED and still LIVE (every referenced session head unchanged, §3.1).
    pub dismissed_pairs: std::collections::HashSet<String>,
}
```

   (b) The reader (after `is_conflict_proposal_suppressed` `:2910`):

```rust
    /// The live coexist + dismissed PAIR exclusions (spec §2.2 / §3.1). ONE fold-derived read consumed by
    /// BOTH the finder's `open_pairs` union (Task 7) AND `pending_conflict_proposals` (Task 8), so the two
    /// honor resolution identically (resolved Open-Q1). A `coexist_allowed` pair is permanent; a
    /// `dismissed` pair is included ONLY while every session in its stored `session_heads` still has that
    /// exact current head (a re-capture advances the head → the dismissal lapses → the pair may
    /// re-propose). Notes need no head: an edit mints a new event id → a new `unordered_pair_key` the
    /// stored key no longer matches, so the stale key becomes inert. Restart-safe (pure fold, no cursor).
    #[cfg(unix)]
    fn resolution_exclusions(&self) -> Result<ResolutionExclusions, BossclawError> {
        let fold = fold_sessions(&self.session_events_ordered()?);
        let head_of: std::collections::HashMap<String, String> =
            fold.current.iter().map(|cs| (cs.session_id.clone(), cs.event_id.clone())).collect();
        let mut out = ResolutionExclusions::default();
        for ev in self.events_of_types(&[
            crate::graph::COEXIST_ALLOWED_EVENT_TYPE,
            crate::graph::DISMISSED_EVENT_TYPE,
        ])? {
            let Some(pk) = ev.content.get("pair_key").and_then(|v| v.as_str()) else {
                continue; // malformed — never excludes
            };
            if ev.event_type == crate::graph::COEXIST_ALLOWED_EVENT_TYPE {
                out.coexist_pairs.insert(pk.to_string());
                continue;
            }
            // dismissed: live only while every stored session head is unchanged.
            let live = match ev.content.get("session_heads").and_then(|v| v.as_object()) {
                None => true, // no passage members (note↔note) → no head to lapse; key is inert on edit
                Some(map) => map.iter().all(|(sid, stored)| {
                    head_of.get(sid).map(String::as_str) == stored.as_str()
                }),
            };
            if live {
                out.dismissed_pairs.insert(pk.to_string());
            }
        }
        Ok(out)
    }
```

4. Run → PASS: `cargo test -p bossclaw-core resolution_exclusions_are_live_and_dismiss_lapses_on_session_head_change`

5. Commit: `feat(rung3-p3): resolution_exclusions reader (coexist + live-dismissed pair keys)`

---

## Task 4 — `conflict_proposal_by_id` — the ALL-proposals by-id reader

Design §2.3, §2.1 (MAJOR-1), Open-Q7. A retire withdraws the proposal from the OPEN set (`open_conflict_proposals`
drops a non-current ref), so `resolve_conflict`'s idempotency + roll-forward must recover `a_ref`/`b_ref` from a
by-id read that ignores open-ness. `#[cfg(unix)]`.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `conflict_proposal_by_id` (after `open_conflict_proposals` `:2862`).
  Uses `event_by_id`, `ConflictRef::from_json`.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (`#[cfg(unix)]`).

**Steps**

1. Write the failing test:

```rust
#[cfg(unix)]
#[test]
fn conflict_proposal_by_id_recovers_refs_even_after_a_ref_is_retired() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);
    let n1 = log.remember(&emb, "branch is main").unwrap();
    let n2 = log.remember(&emb, "branch is master").unwrap();
    let (a, b) = (ConflictRef::Note { event_id: n1.clone() }, ConflictRef::Note { event_id: n2.clone() });
    let prop = log.append_conflict_proposal(&a, &b, "newer", "high", "why", 7, &[n1.clone(), n2.clone()]).unwrap();

    // Before any retire: recovers both refs.
    let (ra, rb) = log.conflict_proposal_by_id(&prop).unwrap().expect("proposal exists");
    assert_eq!(ra, a);
    assert_eq!(rb, b);

    // Retire a_ref (withdraws the proposal from the OPEN set) — the by-id reader STILL recovers it.
    log.retire_memory(&n1, Some(&prop)).unwrap();
    assert!(log.open_conflict_proposals_len_for_test().unwrap_or(0) == 0 || true); // open-set may be empty now
    let (ra2, rb2) = log.conflict_proposal_by_id(&prop).unwrap().expect("still readable by id after retire");
    assert_eq!(ra2, a, "a_ref recovered by id regardless of open-ness (MAJOR-1)");
    assert_eq!(rb2, b);

    // Unknown / wrong-type id → None.
    assert!(log.conflict_proposal_by_id("NOPE").unwrap().is_none());
    assert!(log.conflict_proposal_by_id(&n2).unwrap().is_none(), "a memory id is not a proposal id");
}
```

   (Drop the `open_conflict_proposals_len_for_test` line if no such helper exists — it is illustrative; the
   load-bearing assertions are the two `conflict_proposal_by_id` recoveries.)

2. Run → FAIL: `cargo test -p bossclaw-core conflict_proposal_by_id_recovers_refs_even_after_a_ref_is_retired`
   Expected: `no method named conflict_proposal_by_id`.

3. Implement (after `open_conflict_proposals` `:2862`):

```rust
    /// Recover `(a_ref, b_ref)` for a `conflict_proposal` by id, REGARDLESS of open-ness (spec §2.3,
    /// MAJOR-1). `open_conflict_proposals` withdraws a proposal whose ref went non-current (a retire), so
    /// the idempotency/roll-forward path in [`EventLog::resolve_conflict`] cannot read refs from the open
    /// set. This reads the raw `conflict_proposal` event by id. `None` for an unknown id or a non-proposal
    /// event (a `memory` id must never resolve here). `#[cfg(unix)]`.
    #[cfg(unix)]
    pub fn conflict_proposal_by_id(
        &self,
        proposal_id: &str,
    ) -> Result<Option<(crate::index::ConflictRef, crate::index::ConflictRef)>, BossclawError> {
        use crate::index::ConflictRef;
        let Some(ev) = self.event_by_id(proposal_id)? else {
            return Ok(None);
        };
        if ev.event_type != crate::graph::CONFLICT_PROPOSAL_EVENT_TYPE {
            return Ok(None);
        }
        let (Some(a), Some(b)) = (
            ev.content.get("a_ref").and_then(ConflictRef::from_json),
            ev.content.get("b_ref").and_then(ConflictRef::from_json),
        ) else {
            return Ok(None); // malformed proposal — treat as unknown
        };
        Ok(Some((a, b)))
    }
```

4. Run → PASS: `cargo test -p bossclaw-core conflict_proposal_by_id_recovers_refs_even_after_a_ref_is_retired`

5. Commit: `feat(rung3-p3): conflict_proposal_by_id all-proposals reader (open-ness independent)`

---

## Task 5 — `resolution_markers()` — the terminal fold keyed by `proposal_id`

Design §2.1 (idempotency universe). Folds ALL `conflict_resolved`/`coexist_allowed`/`dismissed` events into
`proposal_id -> ResolutionRecord{ action_kind, retired_event_id? }` — the SAME-vs-DIFFERENT-action terminal
guard and the digest read both consume it. First resolution wins (a second, different marker is ignored — the
guard rejects the second call before it can append). `#[cfg(unix)]`.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `ResolutionKind` enum + `ResolutionRecord` struct (module level,
  beside `ResolutionExclusions`); `resolution_markers` method (after `resolution_exclusions`).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (`#[cfg(unix)]`).

**Steps**

1. Write the failing test (seed terminal markers by hand — same local seeder pattern as Task 3):

```rust
#[cfg(unix)]
#[test]
fn resolution_markers_key_by_proposal_and_first_marker_wins() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    // Append a conflict_resolved (retire_older) for PROP1 and a coexist for PROP2.
    log.append(crate::event::Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::CONFLICT_RESOLVED_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "proposal_id": "PROP1", "action": "retire_older", "retired_event_id": "E1" }),
        model_meta: None, prev_hash: String::new(), hash: None, signed_by_did: log.signer_did(), signature: None,
    }).unwrap();
    log.append(crate::event::Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::COEXIST_ALLOWED_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "proposal_id": "PROP2", "pair_key": "PK", "a_ref": {"kind":"note","event_id":"a"}, "b_ref": {"kind":"note","event_id":"b"} }),
        model_meta: None, prev_hash: String::new(), hash: None, signed_by_did: log.signer_did(), signature: None,
    }).unwrap();

    let m = log.resolution_markers().unwrap();
    let r1 = m.get("PROP1").expect("PROP1 resolved");
    assert_eq!(r1.kind, ResolutionKind::RetireOlder);
    assert_eq!(r1.retired_event_id.as_deref(), Some("E1"));
    assert_eq!(m.get("PROP2").unwrap().kind, ResolutionKind::KeepBoth);
    assert!(m.get("PROP_NONE").is_none());
}
```

2. Run → FAIL: `cargo test -p bossclaw-core resolution_markers_key_by_proposal_and_first_marker_wins`
   Expected: `no method named resolution_markers`.

3. Implement.
   (a) Types (module level, beside `ResolutionExclusions`):

```rust
/// Which terminal action resolved a proposal (spec §2.1). Ordered so `conflict_resolved`'s `action`
/// string maps here. Portable data type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionKind {
    /// `conflict_resolved` with `action == "retire_older"` — a_ref retired.
    RetireOlder,
    /// `conflict_resolved` with `action == "retire_newer"` — b_ref retired.
    RetireNewer,
    /// `coexist_allowed` — keep both.
    KeepBoth,
    /// `dismissed` — snoozed.
    Dismiss,
}

/// One proposal's terminal record (spec §2.1). `retired_event_id` is present only for the two retire
/// kinds. Internal to the resolution fold.
#[cfg(unix)]
#[derive(Debug, Clone)]
struct ResolutionRecord {
    kind: ResolutionKind,
    retired_event_id: Option<String>,
}
```

   (b) The fold (after `resolution_exclusions`):

```rust
    /// Fold ALL terminal markers into `proposal_id -> ResolutionRecord` (spec §2.1). The idempotency +
    /// terminal-state guard in [`EventLog::resolve_conflict`] reads this over ALL proposals (NOT the open
    /// set — a retire withdrew the proposal from open, MAJOR-1). FIRST marker per proposal wins (a second,
    /// different action is rejected by the guard before it can append, so a well-formed log has at most one
    /// per id; the fold defensively keeps the earliest). `#[cfg(unix)]`.
    #[cfg(unix)]
    fn resolution_markers(&self) -> Result<std::collections::HashMap<String, ResolutionRecord>, BossclawError> {
        let mut out: std::collections::HashMap<String, ResolutionRecord> = std::collections::HashMap::new();
        for ev in self.events_of_types(&[
            crate::graph::CONFLICT_RESOLVED_EVENT_TYPE,
            crate::graph::COEXIST_ALLOWED_EVENT_TYPE,
            crate::graph::DISMISSED_EVENT_TYPE,
        ])? {
            let Some(pid) = ev.content.get("proposal_id").and_then(|v| v.as_str()) else {
                continue;
            };
            if out.contains_key(pid) {
                continue; // earliest (seq ASC) wins — events_of_types is seq-ordered
            }
            let record = match ev.event_type.as_str() {
                t if t == crate::graph::CONFLICT_RESOLVED_EVENT_TYPE => {
                    let kind = match ev.content.get("action").and_then(|v| v.as_str()) {
                        Some("retire_older") => ResolutionKind::RetireOlder,
                        Some("retire_newer") => ResolutionKind::RetireNewer,
                        _ => continue, // malformed conflict_resolved — ignore
                    };
                    ResolutionRecord {
                        kind,
                        retired_event_id: ev
                            .content
                            .get("retired_event_id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    }
                }
                t if t == crate::graph::COEXIST_ALLOWED_EVENT_TYPE => {
                    ResolutionRecord { kind: ResolutionKind::KeepBoth, retired_event_id: None }
                }
                _ => ResolutionRecord { kind: ResolutionKind::Dismiss, retired_event_id: None },
            };
            out.insert(pid.to_string(), record);
        }
        Ok(out)
    }
```

   Re-export `ResolutionKind` in `lib.rs` only if a test outside `log.rs` needs it (Task 6's tests are in
   `log.rs`, so no re-export needed yet — keep it crate-internal `pub`).

4. Run → PASS: `cargo test -p bossclaw-core resolution_markers_key_by_proposal_and_first_marker_wins`

5. Commit: `feat(rung3-p3): resolution_markers terminal fold keyed by proposal_id`

---

## Task 6 — `resolve_conflict` orchestrator (idempotency + roll-forward + frozen loser)

Design §2.1, §3.4, Open-Q7/Q9. The one op: `resolve_conflict(proposal_id, action) -> ResolveOutcome`. Owns
its idempotency via the ALL-proposals guard; retires the FROZEN loser (`RetireOlder`=a_ref, `RetireNewer`=b_ref
— NO ts recompute); rolls a torn write forward (loser already retired, `conflict_resolved` missing → append the
missing marker, no primitive re-call). `#[cfg(unix)]`.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `ResolveAction`/`ResolveOutcome` enums (module level, beside
  `ConflictSubject` `:464`); `resolve_conflict` (after `resolution_markers`). Uses `conflict_proposal_by_id`
  (T4), `resolution_markers` (T5), `fold_sessions().retired_notes`/`retired_passages`, `retire_memory`/
  `retire_passage` (T2), `ConflictRef::unordered_pair_key`.
- Modify: `crates/bossclaw-core/src/lib.rs` — re-export `ResolveAction`, `ResolveOutcome`.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (`#[cfg(unix)]`).

**Steps**

1. Write the failing test (covers each action + idempotent repeat AFTER the open-set withdrawal +
   different-action reject + unknown id + torn-write roll-forward):

```rust
#[cfg(unix)]
#[test]
fn resolve_conflict_retires_frozen_loser_and_is_idempotent_and_rolls_forward() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);

    // ── RetireOlder retires the FROZEN a_ref (no ts recompute) ──
    let older = log.remember(&emb, "branch is main").unwrap();
    let newer = log.remember(&emb, "branch is master").unwrap();
    let (a, b) = (ConflictRef::Note { event_id: older.clone() }, ConflictRef::Note { event_id: newer.clone() });
    let prop = log.append_conflict_proposal(&a, &b, "newer", "high", "why", 0, &[older.clone(), newer.clone()]).unwrap();

    let out = log.resolve_conflict(&prop, ResolveAction::RetireOlder).unwrap();
    assert!(matches!(out, ResolveOutcome::Applied(_)), "first resolution applies");
    assert!(!log.current_notes().unwrap().iter().any(|c| c.event_id == older), "a_ref (older) retired");
    assert!(log.current_notes().unwrap().iter().any(|c| c.event_id == newer), "b_ref (newer) survives");
    // The conflict_resolved marker exists AND the retire marker carries the conflict provenance tag.
    let resolved = log.resolution_markers_for_test(&prop); // helper: reads resolution_markers().get(prop).cloned()
    assert!(resolved.is_some(), "conflict_resolved recorded");

    // Idempotent repeat of the SAME action — EVEN THOUGH the retire withdrew the proposal from the open
    // set — is a clean no-op success (no primitive Err bubbles up).
    let again = log.resolve_conflict(&prop, ResolveAction::RetireOlder).unwrap();
    assert!(matches!(again, ResolveOutcome::NoOp), "same-action repeat = no-op success");

    // A DIFFERENT action on a resolved proposal is rejected (first resolution wins).
    let diff = log.resolve_conflict(&prop, ResolveAction::KeepBoth);
    assert!(matches!(diff, Err(BossclawError::InvalidInput(_))), "different action on resolved = reject");

    // Unknown proposal id → error.
    assert!(matches!(log.resolve_conflict("NOPE", ResolveAction::Dismiss), Err(BossclawError::InvalidInput(_))));

    // ── KeepBoth + Dismiss append their markers ──
    let k1 = log.remember(&emb, "x=1").unwrap();
    let k2 = log.remember(&emb, "x=2").unwrap();
    let kp = log.append_conflict_proposal(
        &ConflictRef::Note { event_id: k1.clone() }, &ConflictRef::Note { event_id: k2.clone() },
        "unclear", "med", "why", 0, &[k1.clone(), k2.clone()]).unwrap();
    assert!(matches!(log.resolve_conflict(&kp, ResolveAction::KeepBoth).unwrap(), ResolveOutcome::Applied(_)));
    assert_eq!(log.resolution_markers_for_test(&kp).unwrap(), ResolutionKind::KeepBoth);

    // ── Torn-write roll-forward: loser retired, conflict_resolved MISSING → append it, no-op success ──
    let o2 = log.remember(&emb, "y=old").unwrap();
    let n2b = log.remember(&emb, "y=new").unwrap();
    let (ra, rb) = (ConflictRef::Note { event_id: o2.clone() }, ConflictRef::Note { event_id: n2b.clone() });
    let torn = log.append_conflict_proposal(&ra, &rb, "newer", "high", "why", 0, &[o2.clone(), n2b.clone()]).unwrap();
    // Simulate the crash window: the tagged retire marker landed, the conflict_resolved did NOT.
    log.retire_memory(&o2, Some(&torn)).unwrap();
    assert!(log.resolution_markers_for_test(&torn).is_none(), "precondition: no conflict_resolved yet");
    let rolled = log.resolve_conflict(&torn, ResolveAction::RetireOlder).unwrap();
    assert!(matches!(rolled, ResolveOutcome::NoOp), "roll-forward returns no-op success (no primitive Err)");
    assert_eq!(log.resolution_markers_for_test(&torn).unwrap(), ResolutionKind::RetireOlder, "missing marker appended");

    // ── DISCRIMINATING roll-forward: the frozen loser is already retired by a DIFFERENT source (a MANUAL
    // App retire, via=None), NOT by this proposal. The gate is retired-SET MEMBERSHIP (§3.4), NOT
    // tag-equality — so a regression to a "was-this-proposal's-tag" gate would wrongly re-call the
    // fail-loud primitive here and bubble an Err. resolve_conflict must still roll forward to a clean NoOp.
    let o3 = log.remember(&emb, "z=old").unwrap();
    let n3c = log.remember(&emb, "z=new").unwrap();
    let (za, zb) = (ConflictRef::Note { event_id: o3.clone() }, ConflictRef::Note { event_id: n3c.clone() });
    let cross = log.append_conflict_proposal(&za, &zb, "newer", "high", "why", 0, &[o3.clone(), n3c.clone()]).unwrap();
    log.retire_memory(&o3, None).unwrap(); // MANUAL retire of the frozen loser — a DIFFERENT source, no tag
    assert!(log.resolution_markers_for_test(&cross).is_none(), "precondition: proposal not yet resolved");
    let crossed = log.resolve_conflict(&cross, ResolveAction::RetireOlder)
        .expect("must NOT bubble a fail-loud `already retired` Err — the gate is retired-set membership");
    assert!(matches!(crossed, ResolveOutcome::NoOp), "roll-forward on a differently-retired loser = no-op");
    assert_eq!(log.resolution_markers_for_test(&cross).unwrap(), ResolutionKind::RetireOlder, "missing marker appended");
}
```

   Add the tiny test helper next to the test (or inline the two-line body):

```rust
#[cfg(unix)]
impl EventLog {
    #[cfg(test)]
    fn resolution_markers_for_test(&self, prop: &str) -> Option<ResolutionKind> {
        self.resolution_markers().unwrap().get(prop).map(|r| r.kind)
    }
}
```

2. Run → FAIL: `cargo test -p bossclaw-core resolve_conflict_retires_frozen_loser_and_is_idempotent_and_rolls_forward`
   Expected: `cannot find type ResolveAction` / `no method named resolve_conflict`.

3. Implement.
   (a) Public enums (module level, beside `ConflictSubject` `:464`):

```rust
/// One of the four deterministic resolution actions (spec §1). NO LLM in the path. Portable data type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveAction {
    /// Retire the FROZEN older side (a_ref) — reversible.
    RetireOlder,
    /// Retire the FROZEN newer side (b_ref) — reversible.
    RetireNewer,
    /// Both memories coexist — never re-proposed, dropped from the read surface.
    KeepBoth,
    /// Snooze the pair — re-opens on a material change to a member (§3.1).
    Dismiss,
}

/// The outcome of a [`EventLog::resolve_conflict`] call (spec §2.1). Portable data type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// The action was applied for the first time; carries the id of the terminal marker just appended.
    Applied(String),
    /// Idempotent no-op success: the proposal was already resolved by the SAME action, OR a torn-write
    /// roll-forward completed the missing `conflict_resolved`. No fail-loud primitive was (re-)called.
    NoOp,
}
```

   (b) The orchestrator (after `resolution_markers`):

```rust
    /// Resolve a detected `conflict_proposal` (spec §2.1). Deterministic, no LLM, no egress. Owns its
    /// idempotency via the ALL-proposals guard (a retire withdraws the proposal from the OPEN set, so the
    /// guard must NOT key off open membership — MAJOR-1). The retire actions retire the FROZEN loser
    /// (`RetireOlder`=a_ref, `RetireNewer`=b_ref — detection fixed older→a_ref at `log.rs:6453`; NO ts
    /// recompute here, since a passage's ts tracks its session head, which a re-capture can flip). A
    /// torn-write retry (loser already in the retired set, no `conflict_resolved`) rolls forward: append
    /// the missing marker, return `NoOp` — never re-call the fail-loud primitive (`Err("already retired")`,
    /// `log.rs:5214`/`:5130`). `#[cfg(unix)]`.
    #[cfg(unix)]
    pub fn resolve_conflict(
        &self,
        proposal_id: &str,
        action: ResolveAction,
    ) -> Result<ResolveOutcome, BossclawError> {
        use crate::index::ConflictRef;
        // (1) Load refs by id (open-ness independent) — unknown id ⇒ error.
        let Some((a_ref, b_ref)) = self.conflict_proposal_by_id(proposal_id)? else {
            return Err(BossclawError::InvalidInput(format!("unknown conflict proposal {proposal_id}")));
        };
        let want = action_kind(action);
        // (2) Terminal-state guard over ALL resolution markers.
        if let Some(existing) = self.resolution_markers()?.get(proposal_id) {
            if existing.kind == want {
                return Ok(ResolveOutcome::NoOp); // idempotent same-action repeat
            }
            return Err(BossclawError::InvalidInput(format!(
                "conflict proposal {proposal_id} is already resolved"
            )));
        }
        // (3) No terminal marker yet — apply.
        match action {
            ResolveAction::RetireOlder | ResolveAction::RetireNewer => {
                let loser = if matches!(action, ResolveAction::RetireOlder) { &a_ref } else { &b_ref };
                let retired_event_id = retired_id_of(loser);
                // Roll-forward gate: retired-SET membership (regardless of who retired it — §3.4). If the
                // frozen loser is ALREADY retired, the primitive would fail loud → append the missing
                // conflict_resolved instead and return no-op success.
                let fold = fold_sessions(&self.session_events_ordered()?);
                let already_retired = match loser {
                    ConflictRef::Note { event_id } => fold.retired_notes.contains(event_id),
                    ConflictRef::Passage { session_id, passage_id } => {
                        fold.retired_passages.contains(&(session_id.clone(), *passage_id))
                    }
                };
                if !already_retired {
                    // Retire the frozen loser with conflict provenance (§2.1 MAJOR-2), written FIRST.
                    match loser {
                        ConflictRef::Note { event_id } => {
                            self.retire_memory(event_id, Some(proposal_id))?;
                        }
                        ConflictRef::Passage { session_id, passage_id } => {
                            self.retire_passage(session_id, *passage_id, Some(proposal_id))?;
                        }
                    }
                }
                let marker_id = self.append_conflict_resolved(proposal_id, want, &retired_event_id)?;
                // A fresh retire → Applied; a roll-forward (loser was already retired) → NoOp success.
                if already_retired {
                    Ok(ResolveOutcome::NoOp)
                } else {
                    Ok(ResolveOutcome::Applied(marker_id))
                }
            }
            ResolveAction::KeepBoth => {
                let id = self.append_pair_terminal(
                    crate::graph::COEXIST_ALLOWED_EVENT_TYPE, proposal_id, &a_ref, &b_ref, None,
                )?;
                Ok(ResolveOutcome::Applied(id))
            }
            ResolveAction::Dismiss => {
                // Record the current head of every referenced session so the dismissal lapses on
                // re-capture (§3.1). Notes contribute no head.
                let fold = fold_sessions(&self.session_events_ordered()?);
                let head_of: std::collections::HashMap<&str, &str> =
                    fold.current.iter().map(|cs| (cs.session_id.as_str(), cs.event_id.as_str())).collect();
                let mut heads = serde_json::Map::new();
                for r in [&a_ref, &b_ref] {
                    if let ConflictRef::Passage { session_id, .. } = r {
                        if let Some(h) = head_of.get(session_id.as_str()) {
                            heads.insert(session_id.clone(), serde_json::Value::String((*h).to_string()));
                        }
                    }
                }
                let id = self.append_pair_terminal(
                    crate::graph::DISMISSED_EVENT_TYPE, proposal_id, &a_ref, &b_ref,
                    Some(serde_json::Value::Object(heads)),
                )?;
                Ok(ResolveOutcome::Applied(id))
            }
        }
    }

    /// Append a `conflict_resolved{proposal_id, action, retired_event_id}` terminal marker (§2.1). Plain
    /// signed `append` (like the retire markers — NOT `build_proposer_event`). Written AFTER the retire
    /// marker (§3.4 ordering). `#[cfg(unix)]`.
    #[cfg(unix)]
    fn append_conflict_resolved(
        &self,
        proposal_id: &str,
        kind: ResolutionKind,
        retired_event_id: &str,
    ) -> Result<String, BossclawError> {
        let action = match kind {
            ResolutionKind::RetireOlder => "retire_older",
            ResolutionKind::RetireNewer => "retire_newer",
            _ => unreachable!("append_conflict_resolved is only for retire kinds"),
        };
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: crate::graph::CONFLICT_RESOLVED_EVENT_TYPE.to_string(),
            content: serde_json::json!({
                "proposal_id": proposal_id, "action": action, "retired_event_id": retired_event_id,
            }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }

    /// Append a `coexist_allowed` / `dismissed` PAIR terminal marker with the shared shape
    /// `{proposal_id, pair_key, a_ref, b_ref}` (+ optional `session_heads` for `dismissed`). §2.1.
    /// `#[cfg(unix)]`.
    #[cfg(unix)]
    fn append_pair_terminal(
        &self,
        event_type: &str,
        proposal_id: &str,
        a_ref: &crate::index::ConflictRef,
        b_ref: &crate::index::ConflictRef,
        session_heads: Option<serde_json::Value>,
    ) -> Result<String, BossclawError> {
        let mut content = serde_json::Map::new();
        content.insert("proposal_id".to_string(), serde_json::Value::String(proposal_id.to_string()));
        content.insert(
            "pair_key".to_string(),
            serde_json::Value::String(crate::index::ConflictRef::unordered_pair_key(a_ref, b_ref)),
        );
        content.insert("a_ref".to_string(), a_ref.to_json());
        content.insert("b_ref".to_string(), b_ref.to_json());
        if let Some(h) = session_heads {
            content.insert("session_heads".to_string(), h);
        }
        self.append(Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: event_type.to_string(),
            content: serde_json::Value::Object(content),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: self.signer_did(),
            signature: None,
        })
    }
```

   Free helpers at module level:

```rust
/// Map a `ResolveAction` to its terminal `ResolutionKind` (the two retire actions map to the two retire
/// kinds; KeepBoth/Dismiss map to their own).
fn action_kind(a: ResolveAction) -> ResolutionKind {
    match a {
        ResolveAction::RetireOlder => ResolutionKind::RetireOlder,
        ResolveAction::RetireNewer => ResolutionKind::RetireNewer,
        ResolveAction::KeepBoth => ResolutionKind::KeepBoth,
        ResolveAction::Dismiss => ResolutionKind::Dismiss,
    }
}

/// The `retired_event_id` recorded in `conflict_resolved` (Open-Q7): well-formed from the proposal refs on
/// BOTH the fresh and roll-forward paths. A Note → its event id; a Passage → a stable `session#passage`
/// composite (informational; the digest R-count reads the tagged retire MARKERS, not this field).
fn retired_id_of(loser: &crate::index::ConflictRef) -> String {
    match loser {
        crate::index::ConflictRef::Note { event_id } => event_id.clone(),
        crate::index::ConflictRef::Passage { session_id, passage_id } => {
            format!("{session_id}#{passage_id}")
        }
    }
}
```

   Re-export in `lib.rs`: extend the `pub use log::{...}` block with `ResolveAction, ResolveOutcome`.

4. Run → PASS: `cargo test -p bossclaw-core resolve_conflict_retires_frozen_loser_and_is_idempotent_and_rolls_forward`
   Also re-run Task 3's real-path seeding now that `resolve_conflict` exists.

5. Commit: `feat(rung3-p3): resolve_conflict orchestrator — frozen loser, idempotency, torn-write roll-forward`

---

## Task 7 — Finder union: honor coexist/dismissed in `detect_conflicts_once`

Design §2.2 item 1 (BLOCKER). Union `resolution_exclusions().{coexist_pairs ∪ dismissed_pairs}` into the
finder's `open_pairs` set at `log.rs:6365` (same `unordered_pair_key` space). Do NOT touch the single-ref
`resolution_excluded_refs` param (it feeds `decide_conflict_sweep`'s single-ref `excluded_refs`, which never
matches a pair key). `#[cfg(unix)]` (inside `detect_conflicts_once`).

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `detect_conflicts_once` `open_pairs` assembly (`:6365`).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (`#[cfg(unix)]`, reuse Phase-2 seeded/stubbed finder style).

**Steps**

1. Write the failing test (assert the ENGINE INVARIANT — a coexist/dismissed pair is not re-proposed — not ANN
   counts, per the Phase-2 lesson):

```rust
#[cfg(unix)]
#[test]
fn finder_union_suppresses_a_coexist_pair_with_no_open_proposal() {
    // REAL test-double API (verified `reason.rs:56-112`, Phase-2 ref `log.rs:10620-10668`).
    use crate::index::ConflictRef;
    use crate::conflict::{build_conflict_prompt, CONFLICT_SYSTEM};
    use crate::reason::ScriptedReasoner;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    // MUST be 64: at dim=8 these near-dups fall below CANDIDATE_SIM_MIN → zero candidate pairs → the test
    // would trivially pass for the wrong reason.
    let emb = MockEmbedder::new(64);

    // Proven near-duplicate texts (one token apart) that clear the similarity floor. n1 is remembered
    // first, so ref_ts makes it the OLDER side; register BOTH orderings so the pair surfaces from either
    // endpoint the finder judges first.
    let t1 = "the default deploy target is vercel";
    let t2 = "the default deploy target is fly";
    let n1 = log.remember(&emb, t1).unwrap();
    let n2 = log.remember(&emb, t2).unwrap();
    log.set_conflict_detect_enabled(true).unwrap();

    // The judge WOULD rule this pair a conflict (verdict keyed on SHA of (CONFLICT_SYSTEM, prompt)).
    let verdict = serde_json::json!({
        "contradicts": true, "winner": "newer", "confidence": 92, "why": "conflicting deploy targets"
    });
    let reasoner = ScriptedReasoner::new("test")
        .with_response(CONFLICT_SYSTEM, &build_conflict_prompt(t1, t2), verdict.clone())
        .with_response(CONFLICT_SYSTEM, &build_conflict_prompt(t2, t1), verdict);
    let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
    let empty = std::collections::HashSet::new();

    // A `coexist_allowed` marker exists for this exact pair, but NO open `conflict_proposal` does. This
    // ISOLATES the finder union from open-set membership: without the Task-7 union, `open_pairs` is empty,
    // the judge returns a conflict, and a proposal is minted (`proposed == 1`) — the RED failure. With the
    // union, `coexist_pairs` holds this pk → the finder screens it out BEFORE judging → `proposed == 0`.
    let (a, b) = (ConflictRef::Note { event_id: n1.clone() }, ConflictRef::Note { event_id: n2.clone() });
    let pk = ConflictRef::unordered_pair_key(&a, &b);
    log.append(crate::event::Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::COEXIST_ALLOWED_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "proposal_id": "P", "pair_key": pk, "a_ref": a.to_json(), "b_ref": b.to_json() }),
        model_meta: None, prev_hash: String::new(), hash: None, signed_by_did: log.signer_did(), signature: None,
    }).unwrap();

    let r = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 100).unwrap();
    assert_eq!(r.proposed, 0, "the coexist pair is never (re-)proposed by the finder (open_pairs union, I9)");
    assert!(log.pending_conflict_proposals().unwrap().is_empty(), "no proposal materializes for a coexist pair");
}
```

   Companion (locks the READER side of I9): the end-to-end `detect → resolve_conflict(KeepBoth) → the proposal
   is gone from `pending_conflict_proposals`` flow is Task 8's `keep_both_and_dismiss_drop_the_proposal_from_the_read_surface`
   (that filter lands in Task 8). Together the two tests lock both halves of I9 (finder + reader).

2. Run → FAIL: `cargo test -p bossclaw-core finder_union_suppresses_a_coexist_pair_with_no_open_proposal`
   Expected: `assertion failed: r.proposed == 0` — WITHOUT the union the judge is called and a proposal is
   minted (`proposed == 1`), because there is no open proposal to suppress it via open-set membership.

3. Implement. In `detect_conflicts_once`, at the `open_pairs` assembly (`:6365`), after building `open_pairs`
   from `opens`, union the resolution exclusions:

```rust
        let opens = self.open_conflict_proposals()?;
        let mut open_pairs: std::collections::HashSet<String> =
            opens.iter().map(|p| Self::conflict_pair_key(&p.a_ref, &p.b_ref)).collect();
        // Rung-3 Phase-3 (§2.2 item 1): a kept-both / live-dismissed pair must never be re-proposed. Union
        // its `unordered_pair_key` into `open_pairs` (the SAME space the finder screens against) so the
        // pure finder needs zero reshape. NOT the single-ref `resolution_excluded_refs` param, which feeds
        // `decide_conflict_sweep`'s single-ref `excluded_refs` and would silently never match a pair key.
        let resolution = self.resolution_exclusions()?;
        open_pairs.extend(resolution.coexist_pairs.iter().cloned());
        open_pairs.extend(resolution.dismissed_pairs.iter().cloned());
        let mut open_count = opens.len();
```

4. Run → PASS: `cargo test -p bossclaw-core finder_union_suppresses_a_coexist_pair_with_no_open_proposal`
   Phase-2 detection goldens still green: `cargo test -p bossclaw-core detect_conflicts`

5. Commit: `feat(rung3-p3): finder honors coexist/dismissed via open_pairs union (I9 re-proposal suppression)`

---

## Task 8 — Reader filter: `pending_conflict_proposals` drops coexist/dismissed (stop-nagging)

Design §2.2 item 2 (BLOCKER). KeepBoth/Dismiss retire nothing, so both refs stay current and the proposal
stays in the open set forever. Filter the reader (the read behind `ListConflicts` AND the snapshot count) by
the SAME `resolution_exclusions()` live set. `#[cfg(unix)]`.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `pending_conflict_proposals` (`:2869`).
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (`#[cfg(unix)]`).

**Steps**

1. Write the failing test:

```rust
#[cfg(unix)]
#[test]
fn keep_both_and_dismiss_drop_the_proposal_from_the_read_surface() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);
    let n1 = log.remember(&emb, "x=1").unwrap();
    let n2 = log.remember(&emb, "x=2").unwrap();
    let (a, b) = (ConflictRef::Note { event_id: n1.clone() }, ConflictRef::Note { event_id: n2.clone() });
    let prop = log.append_conflict_proposal(&a, &b, "unclear", "med", "why", 0, &[n1.clone(), n2.clone()]).unwrap();
    assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1, "open before resolution");

    // KeepBoth retires nothing — both refs stay current — but the reader must drop it (I9).
    log.resolve_conflict(&prop, ResolveAction::KeepBoth).unwrap();
    assert!(log.pending_conflict_proposals().unwrap().is_empty(), "kept-both drops from the read surface");

    // A dismissed passage pair drops while live; re-appears if the head advances (covered in Task 3).
    let cev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
    log.store_session_passages(&emb, &cev, &["deploy vercel".to_string()]).unwrap();
    let n3 = log.remember(&emb, "deploy fly").unwrap();
    let pa = ConflictRef::Passage { session_id: "s1".into(), passage_id: 0 };
    let pb = ConflictRef::Note { event_id: n3.clone() };
    let prop2 = log.append_conflict_proposal(&pa, &pb, "unclear", "med", "why", 0, &[cev.clone(), n3.clone()]).unwrap();
    assert_eq!(log.pending_conflict_proposals().unwrap().len(), 1);
    log.resolve_conflict(&prop2, ResolveAction::Dismiss).unwrap();
    assert!(log.pending_conflict_proposals().unwrap().is_empty(), "dismissed drops while the head is unchanged");
}
```

2. Run → FAIL: `cargo test -p bossclaw-core keep_both_and_dismiss_drop_the_proposal_from_the_read_surface`
   Expected: `assertion failed` (the kept-both proposal still lists).

3. Implement. In `pending_conflict_proposals` (`:2869`) apply the exclusion filter:

```rust
    #[cfg(unix)]
    pub fn pending_conflict_proposals(&self) -> Result<Vec<ConflictProposalRow>, BossclawError> {
        // Rung-3 Phase-3 (§2.2 item 2): a retire drops via the currency-GC in `open_conflict_proposals`;
        // KeepBoth/Dismiss retire NOTHING, so the SAME live coexist/dismissed set that suppresses the
        // finder (Task 7) must also drop them here — or the pending count / `ListConflicts` nags forever.
        let excluded = self.resolution_exclusions()?;
        Ok(self
            .open_conflict_proposals()?
            .into_iter()
            .filter(|p| {
                let pk = crate::index::ConflictRef::unordered_pair_key(&p.a_ref, &p.b_ref);
                !excluded.coexist_pairs.contains(&pk) && !excluded.dismissed_pairs.contains(&pk)
            })
            .map(|p| ConflictProposalRow {
                id: p.id,
                a_ref: p.a_ref,
                b_ref: p.b_ref,
                winner_hint: p.winner_hint,
                confidence_band: p.confidence_band,
                why: p.why,
                detected_at: p.detected_at,
            })
            .collect())
    }
```

4. Run → PASS: `cargo test -p bossclaw-core keep_both_and_dismiss_drop_the_proposal_from_the_read_surface`

5. Commit: `feat(rung3-p3): pending_conflict_proposals drops coexist/dismissed pairs (stop-nagging, I9)`

---

## Task 9 — `seq_of_event` + conflict-cursor rewind in `unretire`/`unretire_passage`

Design §3.2, Open-Q5. Un-retiring makes a memory current again, but the conflict cursor already swept past it.
Rewind the 2-D `(last_seq, subject_offset)` cursor to the lexicographic `min` so the next sweep re-examines it.
Append the unretire marker FIRST, then rewind (a torn write leaves the cursor un-rewound → benign delay).
Monotone (never advances); idempotent upsert (works on a brain that never ran detection). Portable (App-reachable).

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `seq_of_event` (new, beside `event_by_id`); a private
  `rewind_conflict_cursor(seq, within)` helper; call it at the tail of `unretire` (`:5079`) and
  `unretire_passage` (`:5160`). Uses `conflict_cursor`/`set_conflict_cursor` (`:6612`/`:6626`), `fold_sessions`.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing test:

```rust
#[test]
fn unretire_rewinds_the_conflict_cursor_to_re_examine_the_memory() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);

    // A note, retired, then the cursor advanced well past it (as detection would).
    let note = log.remember(&emb, "branch is main").unwrap();
    let note_seq = log.seq_of_event(&note).unwrap().expect("note has a seq");
    let m = log.retire_memory(&note, None).unwrap();
    log.set_conflict_cursor(note_seq + 100, 5).unwrap();

    // Unretire rewinds to (note_seq, 0) — the lexicographic min — never advances.
    log.unretire(&note).unwrap();
    assert_eq!(log.conflict_cursor().unwrap(), (note_seq, 0), "cursor rewound to the un-retired note");

    // A rewind never ADVANCES: unretire when the cursor is already behind the note is a no-op on the cursor.
    let _ = m; // (marker id unused)
    log.set_conflict_cursor(0, 0).unwrap();
    // re-retire + unretire again; cursor stays at/below the note position (min semantics).
    log.retire_memory(&note, None).unwrap();
    log.unretire(&note).unwrap();
    assert_eq!(log.conflict_cursor().unwrap(), (0, 0), "rewind is monotone: never advances past current");

    // Passage rewind: unretire_passage rewinds to (capture_seq, passage_id).
    let cev = log.capture_session(&emb, &session_meta("s1", "aa")).unwrap();
    log.store_session_passages(&emb, &cev, &["p0".to_string(), "p1".to_string()]).unwrap();
    let cap_seq = log.seq_of_event(&cev).unwrap().unwrap();
    log.retire_passage("s1", 1, None).unwrap();
    log.set_conflict_cursor(cap_seq + 50, 0).unwrap();
    log.unretire_passage("s1", 1).unwrap();
    assert_eq!(log.conflict_cursor().unwrap(), (cap_seq, 1), "passage unretire rewinds to (capture_seq, passage_id)");
}
```

2. Run → FAIL: `cargo test -p bossclaw-core unretire_rewinds_the_conflict_cursor_to_re_examine_the_memory`
   Expected: `no method named seq_of_event` (then the cursor assertions).

3. Implement.
   (a) `seq_of_event` (beside `event_by_id`):

```rust
    /// The append `seq` of the event with `event_id`, or `None` if no such event. A thin indexed lookup
    /// (`events.id` is unique); used by the Rung-3 conflict-cursor rewind (§3.2) to map an un-retired
    /// memory's event id back to its cursor coordinate.
    pub fn seq_of_event(&self, event_id: &str) -> Result<Option<i64>, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        Ok(store
            .conn()
            .query_row("SELECT seq FROM events WHERE id = ?1", [event_id], |r| r.get::<_, i64>(0))
            .optional()?)
    }
```

   (b) The rewind helper (private, beside `set_conflict_cursor` `:6638`):

```rust
    /// Rewind the conflict cursor to the lexicographic `min` of its current position and `(seq, within)`
    /// (§3.2). MONOTONE — only ever moves the cursor BACK (never advances), so a re-examination is
    /// re-scheduled without ever skipping unrelated newer subjects. Idempotent upsert via
    /// [`Self::set_conflict_cursor`], so it works on a brain that never ran detection (cursor defaults to
    /// `(0, 0)`). Caller appends the unretire marker FIRST; a torn write here leaves the cursor un-rewound
    /// (a benign delay — the memory is current but re-examined only at the next natural sweep past it).
    fn rewind_conflict_cursor(&self, seq: i64, within: usize) -> Result<(), BossclawError> {
        let current = self.conflict_cursor()?;
        let target = (seq, within);
        if target < current {
            self.set_conflict_cursor(seq, within)?;
        }
        Ok(())
    }
```

   (c) At the TAIL of `unretire` (`:5079`), after the `self.append(...)?` that returns the marker id, capture
   the id, rewind, then return the id:

```rust
    pub fn unretire(&self, retired_event_id: &str) -> Result<String, BossclawError> {
        self.assert_note_retired(retired_event_id)?;
        let marker_id = self.append(Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: crate::graph::UNRETIRE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "unretires": retired_event_id }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: self.signer_did(), signature: None,
        })?;
        // Rung-3 Phase-3 (§3.2): the note is current again but the cursor swept past it. Rewind (marker
        // written first, so a torn write is benign). A note is one subject at within-seq id 0.
        if let Some(seq) = self.seq_of_event(retired_event_id)? {
            self.rewind_conflict_cursor(seq, 0)?;
        }
        Ok(marker_id)
    }
```

   (d) At the TAIL of `unretire_passage` (`:5160`), rewind to the session's current-head capture seq +
   `passage_id`:

```rust
        let marker_id = self.append(Event { /* ...existing passage-unretire marker... */ })?;
        // Rung-3 Phase-3 (§3.2): rewind to (current-head capture seq, passage_id). Resolve the head via the
        // post-append fold so the un-retired passage is included.
        let fold = fold_sessions(&self.session_events_ordered()?);
        if let Some(cs) = fold.current.iter().find(|cs| cs.session_id == session_id) {
            if let Some(seq) = self.seq_of_event(&cs.event_id)? {
                self.rewind_conflict_cursor(seq, passage_id)?;
            }
        }
        Ok(marker_id)
```

4. Run → PASS: `cargo test -p bossclaw-core unretire_rewinds_the_conflict_cursor_to_re_examine_the_memory`
   Phase-1 unretire goldens still green: `cargo test -p bossclaw-core unretire`

5. Commit: `feat(rung3-p3): conflict-cursor rewind on unretire/unretire_passage (2-D lexicographic min)`

---

## Task 10 — Poison-pair budget: pair-granular + persistent + cursor-advance rule

Design §3.3 (MAJOR), Open-Q3 (`conflict_pair_errors` table). Today a pair `Err` breaks the whole cycle WITHOUT
advancing (`log.rs:6510`-`:6519`), so a deterministically-erroring pair stalls the sweep forever and hides the
subject's other pairs. Fix: per-pair persistent consecutive-error counter; on `Err`, skip ONLY that pair; at
`CONFLICT_PAIR_ERROR_BUDGET` mark it `poison_skipped` (stops holding the cursor); below budget the subject
retries next cycle (cursor does NOT advance past it — preserves I6). Reset the counter on any successful judge.

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `conflict_pair_errors` DDL (beside the `conflict_cursor` DDL);
  `conflict_pair_error_count` / `bump_conflict_pair_error` / `reset_conflict_pair_error` accessors; refactor the
  judge loop in `detect_conflicts_once` (`:6451`-`:6521`); add `poison_skipped: usize` to `ConflictDetectReport`
  (`:477`).
- Modify: `crates/bossclaw-core/src/conflict.rs` — `CONFLICT_PAIR_ERROR_BUDGET` const.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests` (`#[cfg(unix)]`).

**Steps**

1. Write the failing test (a stub that errors on ONE ordering but judges the subject's other pair):

```rust
#[cfg(unix)]
#[test]
fn poison_pair_is_skipped_after_budget_and_frees_the_cursor() {
    // REAL test-double API. An ERRORING pair is created by simply NOT registering its
    // (CONFLICT_SYSTEM, build_conflict_prompt(a, b)) response — an unregistered prompt returns
    // `Err(Reasoner(..))` naturally (reason.rs:106-111). NO `err_on` builder exists.
    use crate::conflict::CONFLICT_PAIR_ERROR_BUDGET;
    use crate::reason::ScriptedReasoner;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(64); // 64 — a lower dim drops the pair below CANDIDATE_SIM_MIN

    // EXACTLY ONE candidate pair: two near-duplicate notes (one token apart). A 2-note graph guarantees a
    // single pair — the architect traced that a 3-note {anchor, good, bad} graph produces STAGGERED poison
    // pairs (`bad` neighbours both), so `poison_skipped` on a fixed cycle count can be 0. Keep it to one pair.
    log.remember(&emb, "the default deploy target is vercel").unwrap();
    log.remember(&emb, "the default deploy target is fly").unwrap();
    log.set_conflict_detect_enabled(true).unwrap();

    // NO responses registered → every judge of this pair returns Err → a DETERMINISTIC poison pair.
    let reasoner = ScriptedReasoner::new("test");
    let no_passages = |_sid: &str, _pid: usize| -> Option<String> { None };
    let empty = std::collections::HashSet::new();

    // Sub-budget cycles: the pair keeps erroring and the cursor does NOT advance past the subject (I6 — a
    // transient reasoner outage must retry next cycle, not be dropped). Assert the INVARIANTS each cycle,
    // not "on the Nth call".
    let mut r = None;
    for _ in 0..CONFLICT_PAIR_ERROR_BUDGET {
        r = Some(log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 100).unwrap());
        assert!(r.as_ref().unwrap().reasoner_errors >= 1, "the poison pair errored this cycle");
    }
    // On the budget-th consecutive error the pair is poison_skipped, stops holding the cursor, and the
    // sweep advances — a permanent stall becomes a bounded dropped-counter on ONE pair.
    assert!(r.unwrap().poison_skipped >= 1, "poison pair skipped once it reaches CONFLICT_PAIR_ERROR_BUDGET");
    let (cseq, _off) = log.conflict_cursor().unwrap();
    assert!(cseq > 0, "cursor advanced past the poisoned subject (sweep no longer stalls)");

    // It is truly STOPPED being judged (the top-of-loop poison check, not merely re-erroring): rewind the
    // cursor to re-scan the same subject and confirm NO fresh reasoner error is attributed to the pair.
    log.set_conflict_cursor(0, 0).unwrap();
    let after = log.detect_conflicts_once(&emb, &reasoner, &no_passages, &empty, 100).unwrap();
    assert_eq!(after.reasoner_errors, 0, "a fully-poisoned pair is skipped BEFORE the judge, not re-judged");
}
```

   (Load-bearing assertions: `poison_skipped >= 1` after `CONFLICT_PAIR_ERROR_BUDGET` consecutive-error
   cycles, the cursor advancing, and a fully-poisoned pair no longer reaching the judge. If you ALSO want to
   prove "a poison pair does not hide the subject's OTHER real pair," option (b): add a third note crafted so
   it neighbours the anchor above the floor but the poison note BELOW it — but MockEmbedder's bag-of-words
   near-dups cluster, so guaranteeing `sim(good, bad) < CANDIDATE_SIM_MIN` is fragile; the pair-granular
   `continue` in the impl already secures that invariant structurally. Prefer the robust 2-note graph here.)

2. Run → FAIL: `cargo test -p bossclaw-core poison_pair_is_skipped_after_budget_and_frees_the_cursor`
   Expected: `no field poison_skipped` on `ConflictDetectReport`, then (after the field is added) the sweep
   stalls (cursor stays `(0, 0)` — the pre-fix whole-cycle `break` never advances).

3. Implement.
   (a) `CONFLICT_PAIR_ERROR_BUDGET` in `conflict.rs` (the const from §File Structure).
   (b) `poison_skipped` field on `ConflictDetectReport` (`:477`):

```rust
    /// Pairs abandoned this run after `CONFLICT_PAIR_ERROR_BUDGET` consecutive reasoner errors (§3.3) — a
    /// bounded dropped counter on ONE pair, never a frozen sweep, never a hidden sibling conflict.
    pub poison_skipped: usize,
```

   (c) DDL beside the `conflict_cursor` CREATE TABLE:

```rust
        // Rung-3 Phase-3 (§3.3): per-pair CONSECUTIVE reasoner-error counter. Re-derivable progress state
        // (NOT a Tier-A fold): losing it only re-tries a poison pair. Keyed by `unordered_pair_key`.
        store.exec(
            "CREATE TABLE IF NOT EXISTS conflict_pair_errors (
                pair_key           TEXT PRIMARY KEY,
                consecutive_errors INTEGER NOT NULL
            )",
        )?;
```

   (d) Accessors (beside `set_conflict_cursor`):

```rust
    /// Read a pair's consecutive-error count (0 if absent). §3.3.
    fn conflict_pair_error_count(&self, pair_key: &str) -> Result<usize, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let n: Option<i64> = store
            .conn()
            .query_row("SELECT consecutive_errors FROM conflict_pair_errors WHERE pair_key = ?1", [pair_key], |r| r.get(0))
            .optional()?;
        Ok(n.unwrap_or(0) as usize)
    }

    /// Increment a pair's consecutive-error count, returning the NEW value (§3.3).
    fn bump_conflict_pair_error(&self, pair_key: &str) -> Result<usize, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO conflict_pair_errors (pair_key, consecutive_errors) VALUES (?1, 1)
             ON CONFLICT(pair_key) DO UPDATE SET consecutive_errors = consecutive_errors + 1",
            [pair_key],
        )?;
        let n: i64 = store.conn().query_row(
            "SELECT consecutive_errors FROM conflict_pair_errors WHERE pair_key = ?1", [pair_key], |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Reset a pair's consecutive-error count to 0 (on any successful judge of that pair — §3.3).
    fn reset_conflict_pair_error(&self, pair_key: &str) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute("DELETE FROM conflict_pair_errors WHERE pair_key = ?1", [pair_key])?;
        Ok(())
    }
```

   (e) Refactor the judge loop in `detect_conflicts_once` (`:6451`-`:6521`). Replace the whole-subject
   `reasoner_failed` break with the pair-granular rule. Key changes: compute `pk` per pair up front; reset the
   counter on any `Ok(_)`; on `Err`, bump + branch on budget; advance the cursor only if no sub-budget error
   remains for the subject:

```rust
            let mut subject_blocked = false; // an outstanding SUB-budget pair error holds the cursor (I6)
            for (a, b) in pairs {
                let (older, newer) = if ref_ts(&a) <= ref_ts(&b) { (a, b) } else { (b, a) };
                let pk = Self::conflict_pair_key(&older, &newer);
                // §3.3: a pair already at the budget is POISON — stop judging it entirely (do not consume
                // budget, do not re-error). It no longer holds the cursor (`subject_blocked` stays false).
                if self.conflict_pair_error_count(&pk)? >= CONFLICT_PAIR_ERROR_BUDGET {
                    continue;
                }
                let (Some(ot), Some(nt)) = (ref_text(&older), ref_text(&newer)) else {
                    report.dropped += 1;
                    continue;
                };
                report.judged += 1;
                budget_left -= 1;
                match crate::conflict::judge_pair(reasoner, &ot, &nt) {
                    Ok(verdict) => {
                        // Any successful judge (conflict or not) clears the pair's error streak (§3.3).
                        self.reset_conflict_pair_error(&pk)?;
                        match verdict {
                            Some(v) => {
                                log::debug!("conflict verdict: winner={} confidence={}", winner_str(v.winner), v.confidence);
                                if open_count >= CONFLICT_OPEN_CEILING { report.ceiling_hit = true; continue; }
                                if self.is_conflict_proposal_suppressed(&older, &newer)? { continue; }
                                let why = templated_why(winner_str(v.winner), confidence_band(v.confidence), ref_kind(&older), ref_kind(&newer));
                                let sources: Vec<String> = [ref_source_event(&older), ref_source_event(&newer)].into_iter().flatten().collect();
                                self.append_conflict_proposal(&older, &newer, winner_str(v.winner), confidence_band(v.confidence), &why, detected_at, &sources)?;
                                open_pairs.insert(pk);
                                open_count += 1;
                                report.proposed += 1;
                            }
                            None => report.dropped += 1,
                        }
                    }
                    Err(_) => {
                        // Pair-granular (§3.3): skip ONLY this pair; keep judging the subject's others so a
                        // poison pair never hides a real sibling conflict.
                        report.reasoner_errors += 1;
                        let n = self.bump_conflict_pair_error(&pk)?;
                        if n >= CONFLICT_PAIR_ERROR_BUDGET {
                            report.poison_skipped += 1; // give up on this pair; it no longer holds the cursor
                        } else {
                            subject_blocked = true; // sub-budget: retry this subject next cycle (I6)
                        }
                        continue;
                    }
                }
            }
            if subject_blocked {
                // A transient outage → do NOT advance past this subject; next cycle re-judges it. Once every
                // erroring pair reaches the budget (poison_skipped), `subject_blocked` stays false → advance.
                break;
            }
            self.set_conflict_cursor(cs.seq, cs.within_seq_id + 1)?;
```

   Add `CONFLICT_PAIR_ERROR_BUDGET` to the `use crate::conflict::{...}` import at the top of
   `detect_conflicts_once`. Remove the now-dead `reasoner_failed` variable and the outer `if reasoner_failed
   { break; }`.

4. Run → PASS: `cargo test -p bossclaw-core poison_pair_is_skipped_after_budget_and_frees_the_cursor`
   Phase-2 sweep goldens still green: `cargo test -p bossclaw-core detect_conflicts`

5. Commit: `feat(rung3-p3): pair-granular persistent poison budget + cursor-advance rule (§3.3)`

---

## Task 11 — Visibility digest: `conflict_digest_cursor` + `conflict_digest_counts`

Design §2.4, §3.4, Open-Q4/Q8. The R-count reads the `via=="conflict"`-tagged retire markers (conflict-scoped
AND torn-write-safe); D/K read `dismissed`/`coexist_allowed`. The window is a SEQ boundary
(`conflict_digest_cursor`) so a torn write between the retire marker and `conflict_resolved` cannot slip the
retire marker out of the counted window. Portable (plain SQL over event seqs; the daemon-side wiring is Task 14).

**Files**
- Modify: `crates/bossclaw-core/src/log.rs` — `ConflictDigest` struct (module level); `conflict_digest_cursor`
  / `set_conflict_digest_cursor` (beside the conflict cursor accessors); `conflict_digest_counts(since_seq)`.
- Modify: `crates/bossclaw-core/src/lib.rs` — re-export `ConflictDigest`.
- Test: `crates/bossclaw-core/src/log.rs` `mod tests`.

**Steps**

1. Write the failing test:

```rust
#[test]
fn conflict_digest_counts_scope_to_via_conflict_and_advance_by_seq() {
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(8);

    assert_eq!(log.conflict_digest_cursor().unwrap(), 0, "digest cursor defaults to 0");

    // A MANUAL App retire (via=None) must NOT be counted.
    let manual = log.remember(&emb, "manually retired").unwrap();
    log.retire_memory(&manual, None).unwrap();
    // A CONFLICT retire (via="conflict") IS counted.
    let conf = log.remember(&emb, "conflict retired").unwrap();
    log.retire_memory(&conf, Some("PROP")).unwrap();

    let d = log.conflict_digest_counts(0).unwrap();
    assert_eq!(d.retired, 1, "only the via=conflict retire is counted (manual App retire excluded)");
    assert!(d.max_seq > 0);

    // Advance the cursor to max_seq → the next window sees nothing until new markers appear.
    log.set_conflict_digest_cursor(d.max_seq).unwrap();
    let d2 = log.conflict_digest_counts(log.conflict_digest_cursor().unwrap()).unwrap();
    assert_eq!(d2.retired, 0, "no new conflict retires since the cursor");

    // A dismissed + a coexist marker after the cursor count as D and K.
    log.append(crate::event::Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::DISMISSED_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "proposal_id": "P", "pair_key": "K", "a_ref": {"kind":"note","event_id":"a"}, "b_ref": {"kind":"note","event_id":"b"} }),
        model_meta: None, prev_hash: String::new(), hash: None, signed_by_did: log.signer_did(), signature: None,
    }).unwrap();
    log.append(crate::event::Event {
        id: String::new(), ts: String::new(), valid_time: None,
        event_type: crate::graph::COEXIST_ALLOWED_EVENT_TYPE.to_string(),
        content: serde_json::json!({ "proposal_id": "P2", "pair_key": "K2", "a_ref": {"kind":"note","event_id":"c"}, "b_ref": {"kind":"note","event_id":"d"} }),
        model_meta: None, prev_hash: String::new(), hash: None, signed_by_did: log.signer_did(), signature: None,
    }).unwrap();
    let d3 = log.conflict_digest_counts(log.conflict_digest_cursor().unwrap()).unwrap();
    assert_eq!((d3.dismissed, d3.kept), (1, 1), "dismissed + coexist counted since the cursor");
}
```

   Second test (Open-Q9 — the accepted both-sides torn-write edge is DIGEST-VISIBLE, which is what makes it
   acceptable). Depends on `resolve_conflict` (Task 6) + `conflict_digest_counts` (this task):

```rust
#[cfg(unix)]
#[test]
fn both_sides_torn_write_edge_is_digest_visible() {
    use crate::index::ConflictRef;
    let dir = tempfile::tempdir().unwrap();
    let log = open_log(dir.path());
    let emb = MockEmbedder::new(64);
    let older = log.remember(&emb, "the default deploy target is vercel").unwrap();
    let newer = log.remember(&emb, "the default deploy target is fly").unwrap();
    let (a, b) = (ConflictRef::Note { event_id: older.clone() }, ConflictRef::Note { event_id: newer.clone() });
    let prop = log.append_conflict_proposal(&a, &b, "newer", "high", "why", 0, &[older.clone(), newer.clone()]).unwrap();

    // Torn RetireOlder: the tagged retire marker for a_ref landed, but conflict_resolved was lost (crash).
    log.retire_memory(&older, Some(&prop)).unwrap();
    // A deliberate RetireNewer then proceeds (b_ref is not yet retired) and retires the newer side too — the
    // accepted both-sides edge. Both retires carry via=="conflict", so the digest counts BOTH.
    log.resolve_conflict(&prop, ResolveAction::RetireNewer).unwrap();
    let d = log.conflict_digest_counts(0).unwrap();
    assert_eq!(d.retired, 2, "both conflict-driven retires are digest-visible (Open-Q9 acceptability)");
}
```

2. Run → FAIL: `cargo test -p bossclaw-core conflict_digest_counts_scope_to_via_conflict_and_advance_by_seq`
   Expected: `no method named conflict_digest_cursor`.

3. Implement.
   (a) `ConflictDigest` (module level):

```rust
/// The visibility-digest counts since the last session boundary (spec §2.4). `max_seq` is the store's
/// current max event seq — the daemon advances `conflict_digest_cursor` to it after serving a snapshot,
/// so the next session's window starts fresh. Portable data type.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConflictDigest {
    /// `note_retired`/`passage_retired` markers with `via=="conflict"` since the cursor (torn-write-safe).
    pub retired: usize,
    /// `dismissed` markers since the cursor.
    pub dismissed: usize,
    /// `coexist_allowed` (keep-both) markers since the cursor.
    pub kept: usize,
    /// The store's current max event seq (the new cursor position).
    pub max_seq: i64,
}
```

   (b) Cursor DDL (beside the `conflict_cursor` / `conflict_pair_errors` DDL):

```rust
        // Rung-3 Phase-3 (§2.4): the "since last session" digest window boundary (a SEQ, Open-Q8). Single
        // row (id = 0). Advanced when a snapshot is served (Task 14). Re-derivable progress state.
        store.exec(
            "CREATE TABLE IF NOT EXISTS conflict_digest_cursor (
                id       INTEGER PRIMARY KEY CHECK (id = 0),
                last_seq INTEGER NOT NULL
            )",
        )?;
```

   (c) Getter/setter (beside `conflict_cursor`):

```rust
    /// Read the digest window boundary (`0` if never set). §2.4.
    pub fn conflict_digest_cursor(&self) -> Result<i64, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        Ok(store
            .conn()
            .query_row("SELECT last_seq FROM conflict_digest_cursor WHERE id = 0", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0))
    }

    /// Advance the digest window boundary (idempotent single-row upsert). §2.4.
    pub fn set_conflict_digest_cursor(&self, last_seq: i64) -> Result<(), BossclawError> {
        let store = self.inner.lock().expect(POISON);
        store.conn().execute(
            "INSERT INTO conflict_digest_cursor (id, last_seq) VALUES (0, ?1)
             ON CONFLICT(id) DO UPDATE SET last_seq = ?1",
            rusqlite::params![last_seq],
        )?;
        Ok(())
    }
```

   (d) The counts (beside the cursor accessors):

```rust
    /// Count conflict-resolution ACTIVITY strictly after `since_seq` (spec §2.4/§3.4). R = retire markers
    /// with `via=="conflict"` (conflict-scoped — a manual App retire is tagless and NOT counted; AND
    /// torn-write-safe — the tagged retire marker is written before `conflict_resolved`). D = `dismissed`,
    /// K = `coexist_allowed`. Enumerates the four marker types by seq so the SEQ window boundary cannot
    /// drop a retire marker on a torn write (Open-Q8). `max_seq` is the store's current max event seq.
    pub fn conflict_digest_counts(&self, since_seq: i64) -> Result<ConflictDigest, BossclawError> {
        let store = self.inner.lock().expect(POISON);
        let conn = store.conn();
        let mut d = ConflictDigest::default();
        let mut stmt = conn.prepare(
            "SELECT event_type, payload FROM events
             WHERE event_type IN (?1, ?2, ?3, ?4) AND seq > ?5 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![
                crate::graph::NOTE_RETIRED_EVENT_TYPE,
                crate::graph::PASSAGE_RETIRED_EVENT_TYPE,
                crate::graph::DISMISSED_EVENT_TYPE,
                crate::graph::COEXIST_ALLOWED_EVENT_TYPE,
                since_seq,
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        for row in rows {
            let (etype, payload) = row?;
            let ev: Event = serde_json::from_str(&payload)?;
            match etype.as_str() {
                t if t == crate::graph::NOTE_RETIRED_EVENT_TYPE
                    || t == crate::graph::PASSAGE_RETIRED_EVENT_TYPE =>
                {
                    if ev.content.get("via").and_then(|v| v.as_str()) == Some("conflict") {
                        d.retired += 1;
                    }
                }
                t if t == crate::graph::DISMISSED_EVENT_TYPE => d.dismissed += 1,
                _ => d.kept += 1,
            }
        }
        d.max_seq = conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?;
        Ok(d)
    }
```

   Re-export `ConflictDigest` in `lib.rs`.

4. Run → PASS: `cargo test -p bossclaw-core conflict_digest_counts_scope_to_via_conflict_and_advance_by_seq both_sides_torn_write_edge_is_digest_visible`

5. Commit: `feat(rung3-p3): conflict_digest_cursor + conflict_digest_counts (via=conflict-scoped, seq window)`

---

## Task 12 — Proto: two wire ops, DTO mirrors, and the `Role::allows` I8 relaxation

Design §2.3. Adds `Request::ListConflicts` + `Request::ResolveConflict{proposal_id, action}`, a wire
`ResolveActionWire`, wire `ConflictRefWire` + `ConflictProposalWire` mirrors with `From`/`Into` (the Family-1
`types.rs` pattern), two `Response` arms, and grants BOTH ops to `MemoryClient` (the I8 relaxation, commented).

**Files**
- Modify: `crates/bossclawd-proto/src/lib.rs` — `Request` variants (after `CaptureEnabled` `:244`); `Response`
  arms (after `Retired` `:337`); `Role::allows` (`:71`); allowlist tests (`:844`, `:871`).
- Modify: `crates/bossclawd-proto/src/types.rs` — `ResolveActionWire`, `ConflictRefWire`,
  `ConflictProposalWire` + conversions (Family-1 block); round-trip tests.
- Test: `crates/bossclawd-proto/src/lib.rs` + `types.rs` `mod tests`.

**Steps**

1. Write the failing tests.
   (a) In `types.rs` `mod tests` (round-trip + core↔wire conversion, mirroring `grant_mirror_roundtrip`):

```rust
    #[test]
    fn conflict_ref_and_proposal_wire_roundtrip_both_ways() {
        use bossclaw_core::index::ConflictRef;
        for r in [
            ConflictRef::Note { event_id: "n1".into() },
            ConflictRef::Passage { session_id: "s1".into(), passage_id: 4 },
        ] {
            let wire: ConflictRefWire = r.clone().into();
            let bytes = serde_json::to_vec(&wire).unwrap();
            let back: ConflictRefWire = serde_json::from_slice(&bytes).unwrap();
            let core: ConflictRef = back.into();
            assert_eq!(core, r, "ConflictRef survives core → wire → serde → core");
        }
        let row = bossclaw_core::ConflictProposalRow {
            id: "P1".into(),
            a_ref: ConflictRef::Note { event_id: "a".into() },
            b_ref: ConflictRef::Passage { session_id: "s".into(), passage_id: 0 },
            winner_hint: "newer".into(),
            confidence_band: "high".into(),
            why: "templated".into(),
            detected_at: 42,
        };
        let wire: ConflictProposalWire = row.clone().into();
        let bytes = serde_json::to_vec(&wire).unwrap();
        let back: ConflictProposalWire = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.id, "P1");
        assert_eq!(back.winner_hint, "newer");
        assert_eq!(back.detected_at, 42);
    }

    #[test]
    fn resolve_action_wire_maps_to_core_both_ways() {
        use bossclaw_core::ResolveAction;
        for (w, c) in [
            (ResolveActionWire::RetireOlder, ResolveAction::RetireOlder),
            (ResolveActionWire::RetireNewer, ResolveAction::RetireNewer),
            (ResolveActionWire::KeepBoth, ResolveAction::KeepBoth),
            (ResolveActionWire::Dismiss, ResolveAction::Dismiss),
        ] {
            let core: ResolveAction = w.into();
            assert_eq!(core, c);
            let back: ResolveActionWire = c.into();
            assert_eq!(back, w);
        }
    }
```

   (b) In `lib.rs` `mod tests` (extend the allowlist tests + a request round-trip):

```rust
    #[test]
    fn memory_client_allows_the_two_resolution_ops() {
        use Request::*;
        // I8 RELAXATION (owner decision 2026-07-17): the resolve ops are guest-reachable.
        assert!(Role::MemoryClient.allows(&ListConflicts { onboarded: true }));
        assert!(Role::MemoryClient.allows(&ResolveConflict {
            onboarded: true, proposal_id: "P".into(), action: crate::types::ResolveActionWire::KeepBoth,
        }));
    }

    #[test]
    fn resolution_ops_round_trip_serde() {
        let req = Request::ResolveConflict {
            onboarded: true, proposal_id: "P1".into(), action: crate::types::ResolveActionWire::RetireOlder,
        };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(back, req);
        let req = Request::ListConflicts { onboarded: true };
        let back: Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(back, req);
    }
```

2. Run → FAIL: `cargo test -p bossclawd-proto conflict_ref_and_proposal_wire_roundtrip_both_ways`
   Expected: `cannot find type ConflictRefWire`.

3. Implement.
   (a) `types.rs` — the wire mirrors + conversions (Family-1 block, beside `NoteWire`):

```rust
/// Wire mirror of [`bossclaw_core::index::ConflictRef`]. Externally tagged so both variants are
/// self-describing on the wire; converts BOTH ways (the daemon builds it from core, the client reads it).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum ConflictRefWire {
    /// A memory note by event id.
    Note { event_id: String },
    /// A session passage by session id + ordinal.
    Passage { session_id: String, passage_id: usize },
}

impl From<bossclaw_core::index::ConflictRef> for ConflictRefWire {
    fn from(r: bossclaw_core::index::ConflictRef) -> Self {
        match r {
            bossclaw_core::index::ConflictRef::Note { event_id } => ConflictRefWire::Note { event_id },
            bossclaw_core::index::ConflictRef::Passage { session_id, passage_id } => {
                ConflictRefWire::Passage { session_id, passage_id }
            }
        }
    }
}

impl From<ConflictRefWire> for bossclaw_core::index::ConflictRef {
    fn from(r: ConflictRefWire) -> Self {
        match r {
            ConflictRefWire::Note { event_id } => bossclaw_core::index::ConflictRef::Note { event_id },
            ConflictRefWire::Passage { session_id, passage_id } => {
                bossclaw_core::index::ConflictRef::Passage { session_id, passage_id }
            }
        }
    }
}

/// Wire mirror of [`bossclaw_core::ConflictProposalRow`] — one pending conflict for the read surface. The
/// string fields (`winner_hint`/`confidence_band`/`why`) are daemon-sanitized when the `ListConflicts`
/// response is BUILT (server-side), so the client always receives single-line, bounded text (MINOR-1).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ConflictProposalWire {
    pub id: String,
    pub a_ref: ConflictRefWire,
    pub b_ref: ConflictRefWire,
    pub winner_hint: String,
    pub confidence_band: String,
    pub why: String,
    pub detected_at: i64,
}

impl From<bossclaw_core::ConflictProposalRow> for ConflictProposalWire {
    fn from(p: bossclaw_core::ConflictProposalRow) -> Self {
        ConflictProposalWire {
            id: p.id,
            a_ref: p.a_ref.into(),
            b_ref: p.b_ref.into(),
            winner_hint: p.winner_hint,
            confidence_band: p.confidence_band,
            why: p.why,
            detected_at: p.detected_at,
        }
    }
}

/// Wire mirror of [`bossclaw_core::ResolveAction`] (the four deterministic actions). Converts both ways.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResolveActionWire {
    RetireOlder,
    RetireNewer,
    KeepBoth,
    Dismiss,
}

impl From<ResolveActionWire> for bossclaw_core::ResolveAction {
    fn from(a: ResolveActionWire) -> Self {
        match a {
            ResolveActionWire::RetireOlder => bossclaw_core::ResolveAction::RetireOlder,
            ResolveActionWire::RetireNewer => bossclaw_core::ResolveAction::RetireNewer,
            ResolveActionWire::KeepBoth => bossclaw_core::ResolveAction::KeepBoth,
            ResolveActionWire::Dismiss => bossclaw_core::ResolveAction::Dismiss,
        }
    }
}

impl From<bossclaw_core::ResolveAction> for ResolveActionWire {
    fn from(a: bossclaw_core::ResolveAction) -> Self {
        match a {
            bossclaw_core::ResolveAction::RetireOlder => ResolveActionWire::RetireOlder,
            bossclaw_core::ResolveAction::RetireNewer => ResolveActionWire::RetireNewer,
            bossclaw_core::ResolveAction::KeepBoth => ResolveActionWire::KeepBoth,
            bossclaw_core::ResolveAction::Dismiss => ResolveActionWire::Dismiss,
        }
    }
}
```

   (Ensure `bossclaw_core::index::ConflictRef` and `ConflictProposalRow`/`ResolveAction` are re-exported from
   core's `lib.rs` — `ConflictRef`/`ConflictProposalRow` already are; `ResolveAction` was added in Task 6.)

   (b) `lib.rs` — import `ResolveActionWire`, `ConflictProposalWire` in the `types` re-export block (`:36`);
   add the two `Request` variants (after `CaptureEnabled` `:244`):

```rust
    /// Rung-3 Phase-3: list the pending conflict proposals (already excludes coexist/dismissed). Reachable
    /// from `MemoryClient` (the I8 relaxation) so Claude Code can review conflicts. Sanitized daemon-side.
    ListConflicts { onboarded: bool },
    /// Rung-3 Phase-3: resolve one conflict proposal with a deterministic action. Reachable from
    /// `MemoryClient` (the I8 relaxation). No LLM, no egress; the retire actions are reversible.
    ResolveConflict { onboarded: bool, proposal_id: String, action: ResolveActionWire },
```

   Add the `Response` arms (after `Retired(String)` `:337`):

```rust
    /// `ListConflicts` result — the pending (coexist/dismissed-filtered), daemon-sanitized proposals.
    ListConflicts(Vec<ConflictProposalWire>),
    /// `ResolveConflict` result — whether a terminal marker was newly applied, and its id when it was.
    /// `applied == false` is the idempotent no-op / roll-forward success (never an error to the agent).
    ResolveConflict { applied: bool, marker_event_id: Option<String> },
```

   (c) `Role::allows` (`:71`) — add both ops to the `MemoryClient` `matches!`:

```rust
            Role::MemoryClient => matches!(
                req,
                Request::Recall { .. }
                    | Request::Remember { .. }
                    | Request::CaptureNotify { .. }
                    | Request::Snapshot { .. }
                    // I8 RELAXATION (owner decision 2026-07-17, design §0/§4): for a locally-installed
                    // AIR Agent, local Claude Code IS the owner. The resolve ops are guest-reachable;
                    // safety rests on reversible retire + the visibility digest + the signed log, NOT a
                    // rate cap (the per-connection limiter is a no-op vs a reconnecting MCP client).
                    | Request::ListConflicts { .. }
                    | Request::ResolveConflict { .. }
            ),
```

   Update `memory_client_allows_exactly_four_ops` (`:844`) — move the two ops into the `yes` set (and rename
   the test to `memory_client_allows_exactly_six_ops`, updating its doc-comment; the `no` set stays as-is,
   still refusing RetireMemory/Unretire/etc.).

4. Run → PASS:
   `cargo test -p bossclawd-proto conflict_ref_and_proposal_wire_roundtrip_both_ways resolve_action_wire_maps_to_core_both_ways`
   `cargo test -p bossclawd-proto memory_client_allows_the_two_resolution_ops resolution_ops_round_trip_serde memory_client_allows_exactly_six_ops`
   `cargo test -p bossclawd-proto proto_version_still_one` (must stay 1 — additive variants only).

5. Commit: `feat(rung3-p3): proto ListConflicts/ResolveConflict ops + wire mirrors + I8 Role::allows grant`

---

## Task 13 — Daemon: engine wrappers, dispatch arms, guest passthrough, daemon-side sanitize

Design §2.3, §2.4 (MINOR-1). Adds `EngineHandle::list_conflicts`/`resolve_conflict`; the two dispatch arms
(sanitizing the `ListConflicts` response daemon-side via `sanitize_injected`); the `override_onboarding_for_guest`
passthrough arms; and a test that the two ops are NOT rate-limited.

**Files**
- Modify: `crates/bossclawd/src/engine/mod.rs` — `list_conflicts` / `resolve_conflict` wrappers (beside
  `retire_memory` `:816` / the conflict methods `:1060`).
- Modify: `crates/bossclawd/src/server.rs` — two dispatch arms (in `dispatch` `:243`, beside the retire arms
  `:450`); `override_onboarding_for_guest` passthrough (`:210`); import `ConflictProposalWire`/`ResolveActionWire`.
- Test: `crates/bossclawd/src/server.rs` `mod tests` (`is_rate_limited_op` + guest override) + a dispatch test.

**Steps**

1. Write the failing tests (in `server.rs` `mod tests`):

```rust
    #[test]
    fn resolution_ops_are_not_rate_limited() {
        // Per §0 the per-connection limiter cannot bound a reconnecting MCP client, so the resolve ops are
        // deliberately NOT rate-limited — reversibility + the visibility digest are the controls.
        assert!(!is_rate_limited_op(&Request::ListConflicts { onboarded: true }));
        assert!(!is_rate_limited_op(&Request::ResolveConflict {
            onboarded: true, proposal_id: "P".into(), action: bossclawd_proto::types::ResolveActionWire::KeepBoth,
        }));
    }

    #[test]
    fn guest_onboarding_override_passes_through_the_two_resolution_ops() {
        // I8 relaxation: the two resolve ops are guest ops, so they MUST get an explicit passthrough arm
        // (a missing arm → None → refused). The client's `onboarded` is overwritten with the daemon's truth.
        match override_onboarding_for_guest(Request::ListConflicts { onboarded: false }, true) {
            Some(Request::ListConflicts { onboarded }) => assert!(onboarded),
            other => panic!("expected Some(ListConflicts), got {other:?}"),
        }
        match override_onboarding_for_guest(
            Request::ResolveConflict { onboarded: false, proposal_id: "P".into(), action: bossclawd_proto::types::ResolveActionWire::Dismiss },
            true,
        ) {
            Some(Request::ResolveConflict { onboarded, proposal_id, .. }) => {
                assert!(onboarded, "daemon truth overwrites the client flag");
                assert_eq!(proposal_id, "P");
            }
            other => panic!("expected Some(ResolveConflict), got {other:?}"),
        }
    }
```

2. Run → FAIL: `cargo test -p bossclawd resolution_ops_are_not_rate_limited guest_onboarding_override_passes_through_the_two_resolution_ops`
   Expected: `no variant ListConflicts` (proto) — already added in T12; then the override arms are missing →
   the `guest_...` test fails (`None`).

3. Implement.
   (a) Engine wrappers (`engine/mod.rs`, beside the conflict methods):

```rust
    /// Rung-3 Phase-3: the pending conflict proposals (already coexist/dismissed-filtered). Onboarding is
    /// the daemon's OWN verdict (guest-reachable, but the daemon computes onboarding). Read-only.
    pub async fn list_conflicts(
        &self,
        onboarded: bool,
    ) -> Result<Vec<bossclaw_core::ConflictProposalRow>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || log.pending_conflict_proposals().map_err(|e| EngineOpError::Core(e.to_string())))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Rung-3 Phase-3: resolve one conflict proposal. Deterministic, no LLM, no egress. An unknown or
    /// already-resolved-by-a-different-action proposal folds to the typed `Rejected` (core `InvalidInput`);
    /// any other core failure folds to `Core`. Onboarding is the daemon's OWN verdict.
    pub async fn resolve_conflict(
        &self,
        onboarded: bool,
        proposal_id: String,
        action: bossclaw_core::ResolveAction,
    ) -> Result<bossclaw_core::ResolveOutcome, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.resolve_conflict(&proposal_id, action).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }
```

   (b) Dispatch arms (`server.rs`, beside the retire arms `:450`):

```rust
        // ── Rung 3 §Phase-3: conflict resolution (guest-reachable — the I8 relaxation). ──
        Request::ListConflicts { onboarded } => {
            op_result(engine.list_conflicts(onboarded).await, |rows| {
                // Sanitize DAEMON-SIDE (MINOR-1: air-memory-mcp has bossclawd only as a dev-dep and cannot
                // call `sanitize_injected` in prod). Belt-and-suspenders even though the fields are ids +
                // the content-free `templated_why` today: a future change that puts model text in `why`
                // can never regress into an unfenced injection.
                Response::ListConflicts(rows.into_iter().map(sanitize_conflict_row).collect())
            })
        }
        Request::ResolveConflict { onboarded, proposal_id, action } => {
            op_result(engine.resolve_conflict(onboarded, proposal_id, action.into()).await, |outcome| {
                let (applied, marker_event_id) = match outcome {
                    bossclaw_core::ResolveOutcome::Applied(id) => (true, Some(id)),
                    bossclaw_core::ResolveOutcome::NoOp => (false, None),
                };
                Response::ResolveConflict { applied, marker_event_id }
            })
        }
```

   Add the sanitizer (free fn in `server.rs`, near the other response builders):

```rust
/// Build a sanitized `ConflictProposalWire` from a core row (MINOR-1). The refs are ids (structurally safe);
/// the free-ish string fields are passed through `sanitize_injected` (single-line, ≤ SNAPSHOT_FIELD_MAX) so
/// no memory-derived text can ever cross the wire un-neutralized.
fn sanitize_conflict_row(row: bossclaw_core::ConflictProposalRow) -> bossclawd_proto::types::ConflictProposalWire {
    use crate::capture::snapshot::sanitize_injected;
    bossclawd_proto::types::ConflictProposalWire {
        id: row.id,
        a_ref: row.a_ref.into(),
        b_ref: row.b_ref.into(),
        winner_hint: sanitize_injected(&row.winner_hint),
        confidence_band: sanitize_injected(&row.confidence_band),
        why: sanitize_injected(&row.why),
        detected_at: row.detected_at,
    }
}
```

   (c) `override_onboarding_for_guest` (`:210`) — add explicit passthrough arms BEFORE the `_ => None`:

```rust
        // Rung-3 Phase-3 (I8 relaxation): the two resolve ops are guest ops (`Role::allows` admits them), so
        // they need explicit passthrough arms — a missing arm → None → refused. Rewrite `onboarded` to the
        // daemon's OWN truth like every other guest op.
        Request::ListConflicts { .. } => Some(Request::ListConflicts { onboarded }),
        Request::ResolveConflict { proposal_id, action, .. } => {
            Some(Request::ResolveConflict { onboarded, proposal_id, action })
        }
```

   Import `ConflictProposalWire`/`ResolveActionWire` where needed (the sanitizer references the wire type;
   dispatch uses `action.into()`).

4. Run → PASS:
   `cargo test -p bossclawd resolution_ops_are_not_rate_limited guest_onboarding_override_passes_through_the_two_resolution_ops`
   Existing authz suite still green: `cargo test -p bossclawd --test authz`

5. Commit: `feat(rung3-p3): daemon list_conflicts/resolve_conflict dispatch + guest passthrough + daemon-side sanitize`

---

## Task 14 — Snapshot digest render (never-truncated `render_fence` preamble)

Design §2.4, §3.4, Open-Q10. Two daemon-authored lines (integer counts only — NO memory content, so no
sanitize needed) rendered in the `render_fence` PREAMBLE right after `FENCE_OPEN`, BEFORE the droppable
`entries` — so they survive a max-overflow snapshot. The digest cursor advances on serve.

**Files**
- Modify: `crates/bossclawd/src/capture/snapshot.rs` — `render_fence` (`:444`) + `assemble_fence` (`:428`)
  gain a `preamble: &[String]`; `build` (`:207`) prepends `engine.conflict_digest_lines(...)`.
- Modify: `crates/bossclawd/src/engine/mod.rs` — `conflict_digest_lines(source)` (infallible; advances the digest cursor only when `source == "startup"`).
- Test: `crates/bossclawd/src/capture/snapshot.rs` `mod tests` (extend `assemble_fence_over_budget_...`).

**Steps**

1. Write the failing test (in `snapshot.rs` `mod tests`, extending the over-budget test):

```rust
    #[test]
    fn digest_preamble_survives_a_max_overflow_snapshot() {
        // Many oversized entries would blow the budget; the preamble (digest lines) must NEVER be dropped.
        let preamble = vec![
            "3 memory conflict(s) pending — ask me to review.".to_string(),
            "Since last session: 2 retired, 1 dismissed, 0 kept-both via conflict resolution.".to_string(),
        ];
        let entries: Vec<String> =
            (0..60).map(|_| sanitize_injected(&"x".repeat(SNAPSHOT_FIELD_MAX + 100))).collect();
        let text = assemble_fence(&preamble, &entries);
        assert!(text.len() <= SNAPSHOT_MAX_BYTES, "budget honored: {} bytes", text.len());
        assert!(text.contains("conflict(s) pending"), "pending digest line survives truncation");
        assert!(text.contains("Since last session"), "activity digest line survives truncation");
        assert!(text.contains(FENCE_CLOSE), "close marker survives");
    }

    #[test]
    fn empty_preamble_and_entries_is_still_the_minimal_affordance() {
        assert_eq!(assemble_fence(&[], &[]), AFFORDANCE);
        // A preamble with no entries still renders a fence (the digest is worth a fence on its own).
        let only_digest = assemble_fence(&["1 memory conflict(s) pending — ask me to review.".to_string()], &[]);
        assert!(only_digest.contains("conflict(s) pending"));
        assert!(only_digest.contains(FENCE_CLOSE));
    }
```

2. Run → FAIL: `cargo test -p bossclawd digest_preamble_survives_a_max_overflow_snapshot`
   Expected: `assemble_fence` takes 1 argument, not 2.

3. Implement.
   (a) `render_fence` (`:444`) — render the preamble after `FENCE_OPEN`, before the numbered entries:

```rust
/// Render the fence: `FENCE_OPEN`, the never-dropped `preamble` lines, the numbered `entries`, `FENCE_CLOSE`,
/// affordance. The preamble (the Rung-3 digest) is daemon-authored integer-count text, so it needs no
/// sanitize and is never trimmed — only trailing `entries` are shed under budget (`assemble_fence`).
fn render_fence(preamble: &[String], entries: &[&String]) -> String {
    let mut s = String::with_capacity(SNAPSHOT_MAX_BYTES.min(512));
    s.push_str(FENCE_OPEN);
    s.push('\n');
    for line in preamble {
        s.push_str(line);
        s.push('\n');
    }
    for (i, e) in entries.iter().enumerate() {
        s.push_str(&format!("{}. {}\n", i + 1, e));
    }
    s.push_str(FENCE_CLOSE);
    s.push('\n');
    s.push_str(AFFORDANCE);
    s
}
```

   (b) `assemble_fence` (`:428`) — take the preamble; the minimal-affordance early return only when BOTH are
   empty; trim only `entries`:

```rust
fn assemble_fence(preamble: &[String], entries: &[String]) -> String {
    let live: Vec<&String> = entries.iter().filter(|e| !e.is_empty()).collect();
    if preamble.is_empty() && live.is_empty() {
        return AFFORDANCE.to_string();
    }
    let mut n = live.len();
    loop {
        let text = render_fence(preamble, &live[..n]);
        if text.len() <= SNAPSHOT_MAX_BYTES || n == 0 {
            return text;
        }
        n -= 1;
    }
}
```

   (c) `build` (`:207`) — prepend the digest lines, passing the snapshot `source`:

```rust
    let preamble = engine.conflict_digest_lines(source).await; // infallible: empty vec on error / detection-off
    // ...existing compact/project entry selection...
    assemble_fence(&preamble, &entries)
```

   (d) `engine/mod.rs` — `conflict_digest_lines` (infallible; onboarding = the daemon's own truth; advances
   the digest cursor ONLY on a fresh startup):

```rust
    /// The two Rung-3 visibility-digest lines for the SessionStart snapshot preamble (§2.4). INFALLIBLE —
    /// returns an empty Vec on ANY error (not onboarded, open failure) or when there is no conflict
    /// activity, so the snapshot builder never breaks (I1). Integer counts only — no memory content.
    /// Advances `conflict_digest_cursor` to the store's current max seq ONLY when `source == "startup"`, so
    /// the window is honestly "since the last SESSION START" — `snapshot::build` also runs for
    /// `source == "compact"` (and resume), and advancing there would let a mid-session compact CONSUME the
    /// "Since last session:" activity before the real next start ever shows it. Non-startup serves still
    /// RENDER the (unconsumed) lines — honest, just not window-advancing.
    pub async fn conflict_digest_lines(&self, source: &str) -> Vec<String> {
        let onboarded = self.is_onboarded_local();
        let Ok(log) = self.get_or_open(onboarded).await else {
            return Vec::new();
        };
        let advance = source == "startup";
        spawn_blocking(move || {
            let pending = log.pending_conflict_proposals().map(|v| v.len()).unwrap_or(0);
            let since = log.conflict_digest_cursor().unwrap_or(0);
            let d = log.conflict_digest_counts(since).unwrap_or_default();
            // Advance the "since last session" window ONLY on a fresh startup (best-effort; a failed advance
            // just re-counts next time). A compact/resume serve renders the lines but leaves the window open.
            if advance {
                let _ = log.set_conflict_digest_cursor(d.max_seq);
            }
            let mut lines = Vec::new();
            if pending > 0 {
                lines.push(format!("{pending} memory conflict(s) pending — ask me to review."));
            }
            if d.retired + d.dismissed + d.kept > 0 {
                lines.push(format!(
                    "Since last session: {} retired, {} dismissed, {} kept-both via conflict resolution.",
                    d.retired, d.dismissed, d.kept
                ));
            }
            lines
        })
        .await
        .unwrap_or_default()
    }
```

   Update every OTHER caller of `assemble_fence`/`render_fence` in `snapshot.rs` (and its existing tests) to
   pass the new preamble arg — pass `&[]` where there is no digest (e.g. the existing
   `assemble_fence_empty_is_the_minimal_affordance` / `assemble_fence_over_budget_keeps_preamble_markers_and_fits`
   tests → `assemble_fence(&[], &entries)`).

4. Run → PASS: `cargo test -p bossclawd digest_preamble_survives_a_max_overflow_snapshot empty_preamble_and_entries_is_still_the_minimal_affordance`
   Existing snapshot suite still green: `cargo test -p bossclawd --lib capture::snapshot`

5. Commit: `feat(rung3-p3): snapshot conflict digest in the never-truncated render_fence preamble`

---

## Task 15 — MCP tools: `list_conflicts` + `resolve_conflict`

Design §2.4. Two tools beside `recall`/`remember`, and their thin socket-client methods (reconnect-per-call,
`MemoryClient` handshake — the established `daemon.rs` pattern).

**Files**
- Modify: `crates/air-memory-mcp/src/mcp.rs` — `TOOL_LIST_CONFLICTS`/`TOOL_RESOLVE_CONFLICT` consts (`:22`);
  `tools_list_result` entries (`:82`); `tools/call` routing (`:64`); `run_list_conflicts`/`run_resolve_conflict`
  arg parsers.
- Modify: `crates/air-memory-mcp/src/daemon.rs` — `tool_list_conflicts`/`tool_resolve_conflict` (beside
  `tool_recall` `:151`).
- Test: `crates/air-memory-mcp/src/mcp.rs` `mod tests` (tools/list surface + arg validation) — match the
  crate's existing message-handler test style.

**Steps**

1. Write the failing test (in `mcp.rs` `mod tests`):

```rust
    #[tokio::test]
    async fn tools_list_now_advertises_four_tools_including_resolution() {
        let v: Value = serde_json::from_str(
            &handle_message(std::path::Path::new("/nonexistent.sock"),
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).await.unwrap(),
        ).unwrap();
        let names: Vec<&str> = v.pointer("/result/tools").unwrap().as_array().unwrap()
            .iter().map(|t| t.get("name").unwrap().as_str().unwrap()).collect();
        assert!(names.contains(&TOOL_RECALL) && names.contains(&TOOL_REMEMBER));
        assert!(names.contains(&TOOL_LIST_CONFLICTS), "list_conflicts advertised");
        assert!(names.contains(&TOOL_RESOLVE_CONFLICT), "resolve_conflict advertised");
    }

    #[tokio::test]
    async fn resolve_conflict_rejects_a_bad_action_argument() {
        // A bad `action` is a model-correctable argument error → an isError tool result, not a panic (I4).
        let out = handle_message(std::path::Path::new("/nonexistent.sock"),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"resolve_conflict","arguments":{"proposal_id":"P","action":"nope"}}}"#,
        ).await.unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v.pointer("/result/isError").and_then(Value::as_bool), Some(true));
    }
```

2. Run → FAIL: `cargo test -p air-memory-mcp tools_list_now_advertises_four_tools_including_resolution`
   Expected: `cannot find value TOOL_LIST_CONFLICTS`.

3. Implement.
   (a) `mcp.rs` consts (`:22`):

```rust
/// Rung-3 Phase-3 tool names.
pub const TOOL_LIST_CONFLICTS: &str = "list_conflicts";
pub const TOOL_RESOLVE_CONFLICT: &str = "resolve_conflict";
```

   (b) `tools_list_result` (`:82`). FIRST fix the fn doc-comment (`:81`) — it says "exactly `recall` +
   `remember`"; now there are four tools:

```rust
/// The `tools/list` result: `recall`, `remember`, and the Rung-3 `list_conflicts` + `resolve_conflict`,
/// each with a JSON-Schema `inputSchema`.
fn tools_list_result() -> Value {
```

   Then append the two entries INSIDE the existing `"tools": [ ... ]` array. The current array holds the
   `recall` and `remember` objects; add a comma AFTER the `remember` closing brace, then the two new objects
   (so the array stays well-formed — no leading/trailing stray comma). The two objects:

```rust
            {
                "name": TOOL_LIST_CONFLICTS,
                "description": "List the user's pending AIR memory conflicts (pairs of memories that appear \
                                to contradict). Returns each conflict's id and both sides. Ask the user which \
                                to keep before calling resolve_conflict.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": TOOL_RESOLVE_CONFLICT,
                "description": "Resolve one AIR memory conflict by id. `action` is one of: retire_older, \
                                retire_newer (reversibly hide one side), keep_both, dismiss. Retires are \
                                reversible and reported to the user; never resolve without the user's intent.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "proposal_id": { "type": "string", "description": "The conflict id from list_conflicts." },
                        "action": {
                            "type": "string",
                            "enum": ["retire_older", "retire_newer", "keep_both", "dismiss"],
                            "description": "How to resolve the conflict."
                        }
                    },
                    "required": ["proposal_id", "action"]
                }
            }
```

   (c) `tools/call` routing (`:67`) — two arms:

```rust
                TOOL_LIST_CONFLICTS => Some(tool_result_line(id, run_list_conflicts(sock).await)),
                TOOL_RESOLVE_CONFLICT => Some(tool_result_line(id, run_resolve_conflict(sock, &args).await)),
```

   (d) Arg parsers (`mcp.rs`, beside `run_recall`):

```rust
/// Parse+run `list_conflicts` (no args).
async fn run_list_conflicts(sock: &Path) -> Result<String, DaemonError> {
    daemon::tool_list_conflicts(sock).await
}

/// Parse+run `resolve_conflict`: require a `proposal_id` string and a known `action`.
async fn run_resolve_conflict(sock: &Path, args: &Value) -> Result<String, DaemonError> {
    let proposal_id = args
        .get("proposal_id")
        .and_then(Value::as_str)
        .ok_or_else(|| DaemonError::InvalidArgs("`proposal_id` (string) is required".to_string()))?;
    let action = match args.get("action").and_then(Value::as_str) {
        Some("retire_older") => bossclawd_proto::types::ResolveActionWire::RetireOlder,
        Some("retire_newer") => bossclawd_proto::types::ResolveActionWire::RetireNewer,
        Some("keep_both") => bossclawd_proto::types::ResolveActionWire::KeepBoth,
        Some("dismiss") => bossclawd_proto::types::ResolveActionWire::Dismiss,
        _ => {
            return Err(DaemonError::InvalidArgs(
                "`action` must be one of: retire_older, retire_newer, keep_both, dismiss".to_string(),
            ))
        }
    };
    daemon::tool_resolve_conflict(sock, proposal_id, action).await
}
```

   (e) `daemon.rs` client methods (beside `tool_recall` `:151`):

```rust
/// The `list_conflicts` tool: send `Request::ListConflicts`, render the pending conflicts as text.
pub async fn tool_list_conflicts(sock: &Path) -> Result<String, DaemonError> {
    match call_daemon(sock, Request::ListConflicts { onboarded: true }).await? {
        Response::ListConflicts(rows) => Ok(render_conflicts(&rows)),
        other => Err(map_error_response(other)),
    }
}

/// The `resolve_conflict` tool: send `Request::ResolveConflict`, confirm the outcome.
pub async fn tool_resolve_conflict(
    sock: &Path,
    proposal_id: &str,
    action: bossclawd_proto::types::ResolveActionWire,
) -> Result<String, DaemonError> {
    let req = Request::ResolveConflict { onboarded: true, proposal_id: proposal_id.to_string(), action };
    match call_daemon(sock, req).await? {
        Response::ResolveConflict { applied, marker_event_id } => Ok(if applied {
            format!("Resolved conflict {proposal_id}. (marker {})", marker_event_id.as_deref().unwrap_or("-"))
        } else {
            format!("Conflict {proposal_id} was already resolved (no change).")
        }),
        other => Err(map_error_response(other)),
    }
}

/// Render pending conflicts as a compact, agent-readable block.
fn render_conflicts(rows: &[bossclawd_proto::types::ConflictProposalWire]) -> String {
    if rows.is_empty() {
        return "No pending memory conflicts.".to_string();
    }
    let mut out = format!("{} pending memory conflict(s):\n", rows.len());
    for (i, r) in rows.iter().enumerate() {
        out.push_str(&format!(
            "{}. id={} [{}] {} vs {}\n",
            i + 1, r.id, r.confidence_band, describe_ref(&r.a_ref), describe_ref(&r.b_ref)
        ));
    }
    out.push_str("Use resolve_conflict with the id and an action (retire_older/retire_newer/keep_both/dismiss).");
    out
}

/// One-line, id-only description of a wire ref (no memory content — the daemon carries only ids + band).
fn describe_ref(r: &bossclawd_proto::types::ConflictRefWire) -> String {
    match r {
        bossclawd_proto::types::ConflictRefWire::Note { event_id } => format!("note:{event_id}"),
        bossclawd_proto::types::ConflictRefWire::Passage { session_id, passage_id } => {
            format!("passage:{session_id}#{passage_id}")
        }
    }
}
```

   (Confirm the trailing-comma / array-join in `tools_list_result`'s `json!` macro is well-formed after adding
   the two entries — the leading `,` in the (b) snippet assumes it is appended after the `remember` object; adjust
   to keep valid JSON.)

4. Run → PASS: `cargo test -p air-memory-mcp tools_list_now_advertises_four_tools_including_resolution resolve_conflict_rejects_a_bad_action_argument`

5. Commit: `feat(rung3-p3): air-memory-mcp list_conflicts + resolve_conflict tools`

---

## Task 16 — Final wire-in verification + exit gate (full-workspace)

Design §5 (exit gate), §I3 (dormancy). Phase 3 adds NO new background loop (the resolve path is reactive, called
via the wire op) and NO new `prime_switches` config event — so a fresh brain is byte-for-byte the same at boot.
This task adds the end-to-end dormancy assertion and runs the full-workspace gate.

**Files**
- Modify (if needed): `crates/bossclawd/tests/memory_client_loop.rs` OR `crates/bossclawd/tests/authz.rs` — an
  integration test that a `MemoryClient` connection can call `ListConflicts`/`ResolveConflict` and STILL be
  refused `RetireMemory`/`Unretire`/`Teardown` (positive + negative allowlist over the socket).
- No change: `crates/bossclawd/tests/roundtrip.rs:173` — the fresh-brain config-event count MUST remain `== 5`
  (Phase 3 adds no boot config event). This is the dormancy trip-wire; do not modify the assertion.

**Steps**

1. Write the failing integration test (in `crates/bossclawd/tests/authz.rs`, mirroring the existing
   over-the-socket allowlist tests):

```rust
#[tokio::test]
async fn memory_client_can_resolve_conflicts_but_still_cannot_retire_directly() {
    // Spin an in-process daemon + a MemoryClient connection (reuse the crate's existing test harness).
    let h = test_daemon().await; // existing helper in this test module
    // ListConflicts is permitted for the guest (empty on a fresh brain, but NOT NotPermitted).
    let resp = h.roundtrip(Role::MemoryClient, Request::ListConflicts { onboarded: true }).await;
    assert!(!matches!(resp, Response::Err { kind: OpErrorKindWire::NotPermitted, .. }), "guest may ListConflicts");
    // ResolveConflict on an unknown id is a clean Rejected (permitted op, bad arg) — NOT NotPermitted.
    let resp = h.roundtrip(Role::MemoryClient, Request::ResolveConflict {
        onboarded: true, proposal_id: "NOPE".into(), action: ResolveActionWire::Dismiss,
    }).await;
    assert!(matches!(resp, Response::Err { kind: OpErrorKindWire::Rejected, .. }), "guest may call resolve; unknown id rejects");
    // But direct retire stays App-only: the guest is refused.
    let resp = h.roundtrip(Role::MemoryClient, Request::RetireMemory {
        onboarded: true, target: RetireTarget::Note { event_id: "e".into() },
    }).await;
    assert!(matches!(resp, Response::Err { kind: OpErrorKindWire::NotPermitted, .. }), "guest still cannot RetireMemory");
}
```

   (Adapt to the exact harness helper names in `authz.rs` — `test_daemon`/`roundtrip` are placeholders for the
   module's real seam.)

2. Run → FAIL (until the harness call compiles): `cargo test -p bossclawd --test authz memory_client_can_resolve_conflicts_but_still_cannot_retire_directly`

3. Implement: nothing new in product code — this test exercises the T12/T13 surface end-to-end. If the harness
   lacks a `roundtrip(role, req)` seam, add a minimal one mirroring the existing `memory_client_loop.rs` client.

4. Run → PASS, then the FULL-WORKSPACE EXIT GATE (all foreground, all must be green):

```
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p bossclaw-core
cargo test -p bossclawd-proto
cargo test -p bossclawd
cargo test -p memharness
cargo test -p air-memory-mcp
cargo test -p air_agent_desktop
```

   Exit-gate cross-checks (design §5):
   - **Dormancy (I3):** `cargo test -p bossclawd --test roundtrip` — the fresh-brain `event_count == 5`
     assertion (`roundtrip.rs:173`) MUST stay green (Phase 3 adds no boot config event). The desktop crate's
     socket-parity test asserts the same fresh-brain config count — hence `-p air_agent_desktop` is IN the gate.
   - **Idempotency/terminal/roll-forward (§5.2):** Task 6.
   - **Stop-nagging both sides (§5.3):** Tasks 7 + 8.
   - **Re-open rules (§5.4):** Task 3.
   - **Cursor rewind (§5.5):** Task 9.
   - **Poison budget (§5.6):** Task 10.
   - **Visibility (§5.7):** Tasks 11 + 14 (max-overflow survival + via=conflict scoping + torn-write count).
   - **Guest reachability + dormancy (§5.8):** Tasks 12 + 13 + this task's socket allowlist test.

5. Commit: `test(rung3-p3): end-to-end guest allowlist + full-workspace exit gate green`

---

## Appendix — invariant → task cross-reference (design §4)

| Invariant | Where upheld |
| --- | --- |
| **I1** (no retirement without an explicit `resolve_conflict`; every retire reversible + signed) | T2 (reversible tagged retire), T6 (explicit call), T9 (unretire rewind) |
| **I2** (no LLM / no egress in the resolve path) | T6 (pure engine + append) |
| **I3** (off-by-default; merge ships dormant) | T16 (no new boot config event; roundtrip `== 5`) |
| **I5** (append-only; stale coexist/dismissed linger, accepted) | T3/T5 (fold-derived; no GC — documented) |
| **I6** (fail-safe; idempotent; poison bounded; torn writes benign) | T6 (idempotency + roll-forward), T9 (torn-write-benign rewind), T10 (poison budget) |
| **I7** (hostile-output discipline; content-free `why`; sanitized listing) | Phase-2 `templated_why` reused; T13 daemon-side sanitize |
| **I8** (RELAXED — resolve reachable from `MemoryClient`; compensating controls) | T12 (`Role::allows` grant), T13 (guest passthrough) |
| **I9** (strict-quiet — suppress re-proposal AND drop from read surface) | T7 (finder union) + T8 (reader filter) |
| **I-gc** (a retire auto-withdraws other open proposals on the same ref) | Phase-2 `open_conflict_proposals` currency-GC (unchanged) |
