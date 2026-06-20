# M6a — "Safe Hands": the gated file-write actuator (Design)

- **Date:** 2026-06-20
- **Status:** **Rev 2 — independent critic + security review folded** (both **SHIP-WITH-FIXES**; convergence + resolutions in §14). Next: plan → subagent-driven build → PR.
- **Milestone:** **M6a — brick 1 of the M6 actuator program.** The gated write *mechanism*, with proposals from an **explicit caller** (no autonomous proposer — that is M6b/M6c).
- **Parent program:** `docs/superpowers/specs/2026-06-20-m6-actuator-program-design.md` (§5 "M6a — Safe hands").
- **Master design:** `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` — §5.11 file actuator, §5.12 grant manager, §8.3 file-permission model, §8.4 injection defense, §8.5 write safety, §12 milestone 6 (the **v1 cut-line**), §13 deferred (silent autonomous writes).
- **Consumes:** the `is_external` O(1) taint signal from `docs/superpowers/specs/2026-06-20-bossclaw-core-extraction-from-files-design.md` (and its **D8** lesson — taint anchors to engine lineage, never model/caller assertions).
- **Crate:** `crates/bossclaw-core` — `#![forbid(unsafe_code)]`, all syscalls via `rustix::fs::*` (never raw libc).
- **Seam map:** verified against real code 2026-06-20 (§6) — and **independently re-verified by both reviewers** (all 10 load-bearing claims accurate). The M4b/extraction lesson governs: **every claim about existing code here is verified, not assumed.**

---

## 1. Goal & framing

M6a is the first time `bossclaw-core` mutates anything outside its own encrypted store: it **writes to the user's files**. It builds the *mechanism* — the gated pipeline — and proves it safe with **scripted proposals from an explicit caller**. No autonomous proposer yet (M6b reconciliation, M6c mandates).

The danger is not direct command injection (untrusted file content is fenced as data — the extraction-from-files Pass-A/compose fences, now breakout-hardened). The danger is the **confused deputy**: a booby-trapped file that *socially-engineers a benign-looking write proposal* steering the brain to write an attacker's payload to a sensitive path (master §8.4). No single control stops this, so M6a is **defense-in-depth: every control required, none load-bearing alone** (§4).

**What M6a delivers (engine side only — L4):**
- A **write-grant** model, structurally separate from M5a's read-grants.
- `propose_write(...) → GatedProposal` — the pure §4 gate (provenance + taint verdict + target eligibility + diff-guard + the concurrency base hash **+ identity**). No FS mutation.
- `execute_write(confirmed) → file_written` — execute-time re-check **inside the rename critical section** (closes TOCTOU) + atomic temp-write/rename + a signed `file_written` event + N-deep undo capture.
- `undo_write(id) → file_written` — N-deep recoverable undo that **re-gates** against current state.
- The **verdict** surface the desktop app renders (the app owns the human confirm modal — L4).

---

## 2. Scope decisions (locked)

| # | Decision | Choice | Source |
|---|----------|--------|--------|
| **L1** | Delete mechanism | **Hard-delete** (engine), with pre-delete bytes retained in the N-deep undo store (in-engine recovery). **OS Trash is NOT taken by the engine** — Finder-level recovery, if wanted, is an **app-layer** feature later (Tauri already links Cocoa). | **Peter 2026-06-20**, reversing the Rev 1 "OS Trash" choice after the security ruling (W5/§14): trash would leak secret bytes into a world-readable `~/.Trash` (contra master §8.1), add Cocoa FFI to the `forbid(unsafe)` engine, and is redundant with N-deep undo. |
| **L2** | Proposal payload | **Whole new file bytes** — the exact bytes to be written are gated, hashed, confirmed. Diff is computed for **display only**. Patch/diff proposals deferred. | Peter 2026-06-20. |
| **L3** | Undo depth | **N-deep** undo stack per target (configurable `N`, default 16), GC older. | Peter 2026-06-20. |
| **L4** | Engine/app boundary | Engine owns gate + execute + signed events + undo. App owns the preview/confirm modal **and** the per-write anti-fatigue enforcement (W11). This spec is **engine-only**. | Program D3. |
| **L5** | OS | `#[cfg(unix)]` for the write mechanism, **platform-split** (Linux vs macOS) where primitives differ (W2); pure verdict/proposal types un-gated. | Program D4 + seam map §5 + critic C1. |
| **L6** | Action surface | **Files only** — create / edit / delete. No shell, process, or network actions. | Program D5. |
| **L7** | Taint posture | Tainted-origin writes are **allowed but loud** (a forced, un-bundleable confirm with provenance), never silently blocked. Engine computes the *verdict*; app renders the modal. | Program D6 + master §5.11. |
| **L8** | Write-grant storage | A **parallel `write_grants` table + `write_grant`/`write_revoke` event types** — NOT a `mode` column on the M5a read-grant table. | §6/§7. Rationale §7.1. |
| **L9** | `file_written` tier | **Always Tier-B, by construction (W6):** `execute_write`/`record` is the SOLE constructor and unconditionally sets `model_meta: Some{..}` with a validated non-empty source list. A Tier-A write would silently bypass both `reject_empty_tier_b` *and* taint stamping (seam-map flag #5) — closed by type/ownership, not convention. | Seam map §2/§4 + security I2. |
| **L10** | Taint gate fail-closed | The gate re-implements "**any** unresolvable/missing cited source ⇒ the whole proposal is tainted" **over the set** (W7); bare `is_external` is O(1) but **not self-fail-closed** (seam-map flag #6). | Seam map §3/§6 + security I3. |
| **L11** | **D8-for-writes** (NEW) | The taint verdict **anchors to engine-known target provenance**, never the caller's citations alone: if the target is a currently-tracked ingested (external) file, the write is tainted **by construction**, and `execute_write` augments the recorded `source_event_ids` with that engine-known provenance so the persisted stamp ≡ the verdict. | **Security C1** (the extraction D8 hole, re-opened at the actuator). §4/§8. |
| **L12** | Execute-time identity guard (NEW) | The base-state guard is **content-hash AND `(st_dev, st_ino)` identity** (reusing `FileIdentity::DevIno`, `ingest.rs:372`), captured at propose, re-asserted at execute on the **fd written through** — closing the same-content/different-inode swap. | **Critic I2 + Security I1** (R3). §9. |
| **L13** | Atomic-write primitives (NEW) | Portable, platform-split: a named `O_CREAT\|O_EXCL\|O_NOFOLLOW` temp in the target dir + `fsync` + per-OS finalize — Linux `renameat2(RENAME_NOREPLACE)` for create no-clobber; macOS fd-relative existence pre-check + `renameat` (residual documented, NOT claimed `O_EXCL`-atomic). `O_TMPFILE`/`renameat2` are **Linux-only** and absent from pinned `rustix` 0.38.44. | **Critic C1**. §9. |

---

## 3. The pipeline (every write)

```
propose  →  GATE  →  human confirm  →  execute  →  record  →  undo
(engine)   (engine)   (app, L4)        (engine)    (engine)   (engine, re-gated)
            │                          │
            │                          └─ INSIDE the rename critical section:
            │                             re-canonicalize target · re-check target ⊆ write-grant ·
            │                             base guard (content-hash AND (dev,ino) == verdict; create ⇒ no-clobber) ·
            │                             durably capture pre-bytes → undo store (before mutating) ·
            │                             temp-write + fsync + finalize-rename (fd-relative, symlink-safe) ·
            │                             append signed file_written (source augmented w/ engine provenance)
            └─ engine-anchored taint (target provenance ∪ fail-closed cited-source check) ·
               provenance display · target ⊆ active write-grant ·
               secret/value-shaped diff-guard (advisory) · base content-hash + identity (the anchor) ·
               requires_loud_modal verdict (monotonic: taint OR delete always force it)
```

- **`propose_write`** is **pure + non-mutating**. It canonicalizes the target, reads the current file (for edit/delete) via a no-symlink open to capture the **base content-hash + `(dev,ino)` identity** and the display diff, computes the **engine-anchored** taint verdict + provenance + diff-guard flags + eligibility, and returns `GatedProposal { proposal, verdict }`. It never writes.
- **human confirm** is the **app's** job (L4). Calling `execute_write` **is** the confirmation signal — the engine never enforces "did the human click yes" (it can't see the UI); it enforces the *re-checks + atomicity + audit*. (The app, not the engine, delivers anti-fatigue — W11.)
- **`execute_write`** re-derives the **FS-mutable** security facts inside the critical section (path canonicalization, grant membership, content-hash + identity). The taint verdict cannot change (source ids fixed; the log is append-only, so `is_external` can never un-set; the target-provenance anchor is monotone). Anything diverged ⇒ **fail-closed reject** (the app must re-propose).
- **`record`** appends a signed Tier-B `file_written` event (the SOLE constructor — L9/W6) to the append-only, hash-chained log (M1 substrate), with `source_event_ids` = the caller's cites **∪** the engine-known target provenance (L11), so the chokepoint's taint stamp matches the verdict.
- **`undo`** restores prior state from the N-deep undo store via the *same* gated atomic-write path, **re-gating** the target against current grants + identity and verifying restored bytes hash to the recorded `prev_content_hash` (W9); it is itself recorded as a `file_written` (`undo_of: <id>`).

---

## 4. Security model — the confused deputy is the whole point

Master §8.4 is normative: the fence "raises the bar against direct injection; does **not** by itself stop confused-deputy proposals." So **all controls are required**:

| Control | Stops | Where in M6a |
|---|---|---|
| **Write-grants** (separate from read-grants; L8) | writing outside explicitly write-allowed folders | `write_grants` table + `is_write_allowed` (§8) |
| **Execute-time target re-check** (canonicalized, fd-relative, in the rename critical section) | TOCTOU path-swap between gate and execute | `execute_write` critical section (§9) |
| **Base guard — content-hash AND `(dev,ino)` identity** (create ⇒ no-clobber) | clobbering a file that changed since propose; a **same-content different-inode** swap (L12); a create racing a new file into place | `execute_write` (§9) |
| **Engine-anchored taint (L11/D8-for-writes)** — target-provenance floor **∪** `any(external OR unresolvable)` over cited sources, fail-closed over the set | a confused-deputy caller **citing around** the tainted inducing event; trusting self-reported provenance; a missing source reading "clean" | `propose_write` gate (§8) |
| **Provenance display** ("this edit came from `~/x/README.md`, ingested 2026-06-14") | invisible influence the user can't trace | `WriteVerdict.provenance` (§8) |
| **Secret/value-shaped diff guard** (advisory, NOT a boundary; can only *escalate* the modal, never downgrade it — monotonic) | escalates the loud confirm for diffs touching money / keys / URLs / `curl\|sh` / crontab / shell-rc. A denylist misses obfuscation — load-bearing controls are target-restriction + taint + human confirm; this only raises the prompt | `WriteVerdict.diff_flags` (§8) |
| **Anti-fatigue** — single-target engine surface (no batch ⇒ different files structurally can't be bundled); `requires_loud_modal` always set for deletes/taint. **The property is realized by the app rendering one un-bundleable modal per write (W11); M6a supplies the per-target verdict, not the enforcement** | rubber-stamping a batch that hides one bad write | API shape + verdict (§8) + app (L4) |
| **Atomic write + N-deep undo** (durable pre-bytes before mutation — W8) | a partial / unrecoverable write | `execute_write` + undo store (§7/§9) |

**Taint completeness (the W3 resolution — there is NO recursive lineage walk):** completeness rests on three verified facts, not a walk:
1. **Eager-stamp transitivity** (extraction D2, verified `log.rs:400-408`): any *cited* tainted ancestor already carries `content.origin="external"` on the cited event itself, so an O(1) `is_external` on each direct cited source catches it — a deep walk buys nothing.
2. **Engine-anchored target provenance (L11):** the attacker's escape — *omitting* the tainted citation — is closed for the primary case (editing a tracked ingested file) because the engine derives taint from the target itself, not the citation list.
3. **Fail-closed over the set (L10):** any unresolvable/missing cited id taints the whole proposal.

**Honest scope boundary:** for M6a's *explicit caller*, controls (2)+(3) are the floor; a write to a **non-tracked** target *influenced by* tainted content still relies on the caller citing it honestly. Full proposer-side honesty (the proposer cannot omit its true sources) is enforced where the **autonomous** proposer is built (M6b/M6c) — there the engine **is** the caller and stamps the *true* gathered lineage (exactly как extraction's `emit_page` uses `FactSet.source_ids`, not model cites — D8). M6a builds the gate so that honesty is already non-bypassable for the target-provenance case.

**Key invariant (master §5.11 + extraction):** every Tier-B event carries a **mandatory non-empty `source_event_ids`** (rejected at append by `reject_empty_tier_b`, verified `log.rs:355`). `file_written` is Tier-B by construction (L9). The gate never trusts an event's self-reported origin; it consumes the signed stamp + the engine-known target provenance.

---

## 5. Open questions — RESOLVED

The program design (§5 M6a) left four. All resolved:

- **L1 — Delete → hard-delete + N-deep undo (Peter, reversing OS-Trash after the security ruling W5).** In-engine recovery via the undo store; out-of-app/Finder recovery deferred to the app layer.
- **L2 — Payload → whole new file (Peter).** Execute writes exactly those bytes; the base guard (content-hash + identity, §9) makes "the file changed since propose" a fail-closed reject. The diff is derived for display.
- **L3 — Undo depth → N-deep (Peter).** Per-target stack, default `N = 16`, GC older (§7.3).
- **(4th program question) — `file_written` shape + undo-as-event → RESOLVED:** `file_written` is Tier-B by construction (L9); content = `{ target, op, content_hash, prev_content_hash?, byte_size, undo_of? }`; `op ∈ {Create, Edit, Delete}` (closed set; `undo_of` is the undo discriminator, not a 4th op); producer → `model_meta.model_id`; `prompt_hash = String::new()` (no prompt — explicit caller; matches `link_machine`/`entity`, `log.rs:1571/1637` — W10). **Undo IS a signed event** (`undo_of` + `source_event_ids=[original_id]`) so every restore is audited + lineage-bound.

---

## 6. Seam reality — verified against code (2026-06-20; re-verified by both reviewers)

**Exists and reused as-is:**
- The **sole append chokepoint** `append_event_in_tx` (`log.rs:390`) stamps `content.origin="external"` on any Tier-B event whose lineage touches an external source, **before** hash+sign (`log.rs:403-408`). `file_written` rides this unchanged. (Note M1: the stamp only applies `if content.as_object_mut()` is `Some` — `file_written` content MUST be a JSON object; §10 tests this.)
- `reject_empty_tier_b` (`log.rs:355`) — non-empty `source_event_ids` on every Tier-B append (**verified**; fires only when `model_meta.is_some()` — hence L9/W6).
- `append_pair` (`log.rs:340`) — atomic 2-event commit pattern.
- `is_external` (`ingest.rs:633`, re-exported `lib.rs:61`) — O(1) content lookup; **not self-fail-closed** (the fail-closed rule is only in the private in-tx `source_is_external_in_tx`, `log.rs:370`) → L10.
- `event_by_id` (`log.rs:432`, "Public read for tests + M6's walk").
- Files projection: `FileRecord` + `fold_files` (`graph.rs:404/431`); `current_files_active` (`log.rs:1898`) — the engine's knowledge of "is this path a tracked ingested file" that L11 anchors to. (A point-lookup-by-canonical-path helper is **new** — built on this projection.)
- `FileIdentity::DevIno(st_dev, st_ino)` (`ingest.rs:372`) — the inode-identity primitive L12 reuses.
- `Event`/`ModelMeta` (`event.rs:23`/`11` — `prompt_hash` is **non-optional**, so W10 sets it empty); JCS → SHA-256 → Ed25519 (`event.rs:55/67`, `sign.rs:13`); frozen-vector recipe (`tests/vectors.rs`).
- Careful-open: `careful_open_file` (`ingest.rs:310`) — **RDONLY + `pub(crate)`**, so M6a builds a **new writable-dir-fd careful-open** (it cannot reuse this for writing — seam-map M1); the `openat2 BENEATH|NO_SYMLINKS` w/ `openat`+`NOFOLLOW` fallback discipline (`ingest.rs:325-354`) is the model to mirror.
- Hermetic harness (`tests/extraction.rs:23-94`): `open_log`, `seed_memory`, `ingest_file`, `MockEmbedder`, fixed DEK + key (no shared `TempHome` — preamble copied per test binary).

**Greenfield — M6a builds from zero (the program design WRONGLY assumed these existed — seam-map flags, all confirmed by both reviewers):**
1. **No `is_allowed(path, mode)` / authorization predicate exists.** M6a builds `is_write_allowed(path)` (canonicalize → path-segment descent). ⚠️ It is a **string-segment** check — weaker than the read side's fd-walk against intermediate-symlink swaps; it is **advisory**, and the **fd-relative execute-time open (§9) is the real boundary** (documented; §8/§9).
2. **Grants are MODE-LESS:** `Grant { canonical_root, granted_at, revoked }` (`graph.rs:361`), table `grants(canonical_root PK, granted_at, revoked)` (`log.rs:244`), content `{canonical_root}` (`log.rs:1825`). M6a adds write-grants as a **parallel** table + event types (L8).
3. **No atomic-write / temp-rename helper** beyond `FileHighWater::save` (`highwater.rs:58` — hard-wired one path, no `fsync`, no fd-relative rename). No FS-write critical section. M6a builds both (L13/§9).
4. **No undo state mechanism.** Greenfield (§7.3).
5. **`producer` is not an `Event` field** — it is `model_meta.model_id` (W10).
6. **`is_external` is O(1) but not self-fail-closed** — the gate re-implements "unresolvable ⇒ tainted" over the set (L10).

---

## 7. Data model (additive)

All additive; no existing table/struct is modified (preserves M5a read-grant byte-identity).

### 7.1 Write-grants (parallel to read-grants — L8)
- New event types `write_grant` / `write_revoke` (consts in `graph.rs`), content `{ "canonical_root": <realpath> }`.
- New projection table `write_grants(canonical_root PK, granted_at, revoked INTEGER DEFAULT 0)` (in `open`, `log.rs:150+`), folded by `fold_write_grants` (mirror `fold_grants` `graph.rs:380`, last-writer-wins).
- **Rationale (parallel-not-mode):** (a) M5a read-grant semantics + the proven ingest path stay **byte-identical**; (b) write is a **deliberate, separate signed act** — never a side-effect/silent widening of a read grant; (c) structural guarantee for the security review: *write capability is opt-in via its own event type, never derivable from a read grant* (R5). Verified: no shared fold or projection lets a read grant authorize a write (§10 tests it).

### 7.2 `file_written` event (Tier-B by construction — L9/W6)
- Const `FILE_WRITTEN_EVENT_TYPE = "file_written"` (`graph.rs`). **NOT** in `EMBEDDABLE_EVENT_TYPES` (`log.rs:119`) — a write record is not a recallable memory.
- `content` (always a JSON object — §6 M1) = `{ target: <canonical>, op: "create"|"edit"|"delete", content_hash, prev_content_hash?: <edit/delete/undo>, byte_size, undo_of?: <event id> }`.
- `model_meta = Some { model_id: <producer>, prompt_hash: "", source_event_ids: <caller cites ∪ engine target-provenance, non-empty> }` (L11). Taint stamped automatically by the chokepoint.
- **Sole constructor:** only `execute_write`/`record` build a `file_written`; it validates non-empty sources + sets `model_meta` itself (W6) — no public path can emit a Tier-A `file_written` (§10 asserts this).
- Frozen vector `file_written_canonicalization_is_frozen` in `tests/vectors.rs` (mirror `tainted_page_canonicalization_is_frozen` `vectors.rs:147`), with `prompt_hash=""`.

### 7.3 Undo store (N-deep — L3)
- New table `undo_state(file_written_id PK, canonical_target, op, pre_bytes BLOB NULL, base_content_hash, created_at)` **inside the existing SQLCipher store** (`Store`) → encrypted at rest automatically (no sidecar; seam-map M2). `pre_bytes` = bytes to restore (edit ⇒ old content; delete ⇒ deleted content; create ⇒ `NULL`, undo = remove).
- **Retention:** keep the last `N` rows per `canonical_target`; GC older on each write. `N` default 16.
- **Crash ordering (W8):** the `file_written` ULID is minted **up front**; the undo row is written + **committed durably keyed by that id BEFORE the FS mutation is observable** (§9). So a crash after the rename never leaves a write with no recoverable pre-bytes.
- **Not authoritative / not signed.** Undo is a recovery convenience: tampering can lose undo ability but cannot forge/unsign a write (writes are signed; `prev_content_hash` is recorded). On `undo_write`, restored bytes are verified against the recorded `prev_content_hash` before writing (W9). (Consequence: a signed log export — master §14 q#6 — carries `file_written` events but not undo bytes; **not a blocker** for M6a.)

---

## 8. API surface

**Pure, un-gated types (cross-platform):**
```rust
pub enum WriteOp { Create, Edit, Delete }

pub struct WriteProposal {
    pub target: PathBuf,                 // as-proposed (un-canonicalized)
    pub new_content: Vec<u8>,            // whole-file bytes (L2); ignored for Delete
    pub op: WriteOp,
    pub source_event_ids: Vec<String>,   // NON-EMPTY: the caller's inducing events (L9)
    pub rationale: String,
}

pub enum Taint { Clean, Untrusted }      // Untrusted iff target-provenance OR any cited source external/unresolvable (L10/L11)

pub struct FileId { pub dev: u64, pub ino: u64, pub size: u64 }   // L12 identity (from fstat)

pub struct Provenance { pub event_id: String, pub kind: String,
    pub origin_path: Option<String>, pub ingested_at: Option<String>, pub is_external: bool }

pub struct DiffFlags { pub touches_secret_shaped: bool, pub touches_value_shaped: bool }  // advisory only

pub struct WriteVerdict {
    pub target_canonical: Option<PathBuf>,
    pub allowed: bool,                   // target ⊆ an active WRITE-grant
    pub taint: Taint,
    pub provenance: Vec<Provenance>,     // incl. the engine-anchored target provenance, if any
    pub diff_flags: DiffFlags,
    pub base_content_hash: Option<String>, // current file hash at propose (None for create)
    pub base_identity: Option<FileId>,     // current (dev,ino,size) at propose (None for create) — L12
    pub requires_loud_modal: bool,       // MONOTONIC: taint==Untrusted || op==Delete || diff_flags.any()
    pub reject_reason: Option<String>,   // Some ⇒ cannot proceed (unresolvable target, empty sources, op×existence mismatch)
}

pub struct GatedProposal { pub proposal: WriteProposal, pub verdict: WriteVerdict }
```

**Engine methods (`impl EventLog`, `#[cfg(unix)]` for the mutating ones):**
```rust
pub fn propose_write(&self, p: WriteProposal) -> Result<GatedProposal, BossclawError>; // PURE gate
pub fn execute_write(&self, confirmed: GatedProposal) -> Result<String, BossclawError>; // → file_written id
pub fn undo_write(&self, file_written_id: &str) -> Result<String, BossclawError>;        // re-gated (W9)
pub fn add_write_grant(&self, path: &Path) -> Result<String, BossclawError>;
pub fn revoke_write_grant(&self, path: &Path) -> Result<String, BossclawError>;
pub fn write_grants(&self) -> Result<Vec<WriteGrant>, BossclawError>;
pub fn is_write_allowed(&self, path: &Path) -> Result<bool, BossclawError>;   // advisory (§6 #1); fd-relative execute is the boundary
```

**Gate logic (`propose_write`):**
1. **Sources non-empty** else `reject_reason`. For each cited `src`: `event_by_id(src)` → **unresolvable/error ⇒ the whole proposal is tainted** (fail-closed over the set — L10/W7); else collect `Provenance` + `is_external`.
2. **Canonicalize** target. For **Create** canonicalize the **parent** (the target is absent — seam-map M4) and require parent ⊆ a write-grant; for Edit/Delete canonicalize the target. Unresolvable ⇒ `reject_reason`. `allowed = is_write_allowed(...)`.
3. **op × existence matrix** (gate-level rejects, not deferred to execute): Create-of-existing ⇒ reject; Edit/Delete-of-nonexistent ⇒ reject; the final component being an existing **symlink** ⇒ reject.
4. **Engine-anchored taint (L11):** via the files projection, resolve whether `target_canonical` is a currently-tracked ingested file; if so its `file_ingested` event is external by construction ⇒ `Untrusted` (and that event id is carried into `execute_write`'s recorded sources). Union with step 1's cited-source result.
5. **Base hash + identity + diff:** for Edit/Delete open the current file via the new no-symlink careful-open → `base_content_hash` + `base_identity` (`fstat` dev/ino/size) + display diff. Create ⇒ both `None`.
6. **Diff-guard** advisory flags. `requires_loud_modal = taint==Untrusted || op==Delete || diff_flags.any()` (monotonic — diff-guard can only escalate; §10 locks it).

`undo_write` rebuilds a `WriteProposal` from the `undo_state` row (restore pre-bytes, or delete for an undone create) and runs it through the **full** `propose_write`→critical-section path against **current** grants + identity, additionally asserting restored bytes hash to the recorded `prev_content_hash` (W9). Undo of a write whose target identity has since diverged ⇒ fail-closed.

---

## 9. Execute-time critical section (the TOCTOU close)

`execute_write` holds a dedicated **actuator rename mutex** (new; distinct from the SQLite `inner` mutex) across the entire re-check→mutate→append window, so a second write cannot interleave its base read against a half-applied first write.

Inside the critical section:
1. **Re-canonicalize** `proposal.target` → `real`; **re-check** `is_write_allowed(real)` (a grant revoked since propose ⇒ fail-closed; §10 tests it).
2. **Open the target dir fd** with the new **writable careful-open** (`openat2 BENEATH|NO_SYMLINKS` on Linux; `openat`+`NOFOLLOW` on macOS — mirroring `careful_open_file`, which is RDONLY/`pub(crate)` and cannot be reused). All subsequent steps are **fd-relative**, never re-resolving the path string.
3. **Base guard (L12):**
   - *Edit/Delete:* `fstat` the target fd → require `(dev,ino,size)` == `verdict.base_identity` **AND** re-hash bytes == `verdict.base_content_hash`. Either diverged ⇒ fail-closed (closes both the content-change race and the same-content/different-inode swap).
   - *Create:* no-clobber finalize (step 5); the final component must not exist.
4. **Durably capture pre-bytes → undo store** (edit/delete), committed **before** any mutation (W8); GC to last `N`.
5. **Mutate atomically (L13, platform-split):**
   - *Create/Edit:* create a named `O_CREAT|O_EXCL|O_NOFOLLOW` temp in the target dir (same FS ⇒ atomic rename) → write `new_content` → `fsync` → finalize: **Linux** `renameat2(RENAME_NOREPLACE)` for create / plain `renameat` for edit; **macOS** fd-relative existence pre-check (inside the mutex) + `renameat` (the residual — a foreign process winning a microsecond race on create — is documented, NOT claimed `O_EXCL`-atomic). Failure leaves the FS unchanged (master §10).
   - *Delete:* **hard-delete** via fd-relative `unlinkat` (L1); pre-bytes already durably captured (step 4). No OS Trash (W5).
6. **Append** the signed Tier-B `file_written` (the sole constructor — L9), `source_event_ids` = caller cites **∪** engine target-provenance (L11), minted-id == the undo row key (W8).

All syscalls via `rustix::fs::*` (`openat`/`openat2`/`renameat`/`renameat2`/`unlinkat`/`fstat`/`fsync`) — `forbid(unsafe_code)` preserved; **no new FFI dependency** (W5).

---

## 10. Test plan (hermetic; the §6 harness)

Integration tests in `tests/` (own binary — **no cross-binary imports**). In-crate unit tests for pure gate logic.

- **Gate happy path** + **op×existence matrix**: Create-of-existing / Edit-or-Delete-of-nonexistent / final-component-symlink ⇒ `reject_reason`.
- **Out-of-grant reject**; **read-grant ≠ write-grant** (a path with only a read grant ⇒ write rejected — proves L8/R5).
- **Grant revoked between propose and execute** ⇒ execute fail-closed (W9-adjacent; mirrors ingest's per-append grant re-check).
- **TOCTOU path-swap:** benign target passes the gate; swap to a symlink/out-of-grant path before execute ⇒ rejected.
- **Same-content/different-inode swap (L12/W4):** swap the target for a different inode with identical bytes between propose and execute ⇒ fail-closed (revert-sensitive: must FAIL if the guard drops the `(dev,ino)` check).
- **Base content-change race** ⇒ fail-closed (no clobber). **Create no-clobber** ⇒ reject.
- **Taint — engine-anchored (L11/W1, revert-sensitive):** a **malicious caller that cites only clean ids while editing a tracked ingested (external) file** ⇒ STILL `Untrusted` + `requires_loud_modal`, and the recorded `file_written` is stamped external (its augmented sources include the `file_ingested` id). Must FAIL if the anchor is removed (gate degrades to caller-cites-only).
- **Taint — fail-closed over the set (L10/W7, revert-sensitive):** an unresolvable cited id ⇒ whole proposal tainted. Must FAIL if changed to `filter_map(resolvable).any(external)`.
- **No Tier-A `file_written` (L9/W6):** assert the public API cannot produce a `file_written` with `model_meta: None`.
- **Monotonic loud-modal:** taint or delete forces `requires_loud_modal` regardless of diff flags.
- **Atomicity / no partial write:** dir-scan (mirror `tests/no_plaintext.rs`): no leftover `*.tmp`; target fully new or fully old.
- **Undo N-deep:** N+1 edits ⇒ oldest GC'd, last N restore; undo records `file_written(undo_of=..)`; undo of create removes the file; undo of delete recreates it; **undo re-gates** (revoked grant / diverged identity / `prev_content_hash` mismatch ⇒ fail-closed — W9).
- **Frozen vector** `file_written_canonicalization_is_frozen` (independent JCS-hash reproduction; `prompt_hash=""`).
- `forbid(unsafe_code)` intact; `clippy -D warnings` clean (default + `ollama`); `cargo-deny` / `cargo-audit` green (**no new dependency** added — W5).

Deterministic + scripted ⇒ **no live-model gate** (M6b/M6c territory).

---

## 11. Non-goals

- **Autonomous proposers** (M6b reconciliation, M6c mandates) — explicit caller only.
- **The desktop confirm UI + anti-fatigue enforcement + any OS-Trash/Finder recovery** (L4/W5 — app's job).
- **The confidentiality / "leak" egress bouncer** (Vault-Brain brick 3).
- **Shell / process / network actions** (L6); **Windows** write path (L5).
- **Patch/diff proposals** (L2); **signed-export format** (master §14 q#6 — separate, not a blocker, §7.3).

---

## 12. Build sequence (preview — the plan details tasks)

Each task TDD red-first, two-stage review (spec-compliance → code-quality), then whole-impl Opus SHIP.
1. Write-grant table + `write_grant`/`write_revoke` events + `fold_write_grants` + `add/revoke/write_grants` + `is_write_allowed` (canonicalize-parent-for-create; read/write separation tests).
2. Pure gate types + `propose_write` — engine-anchored taint (L11), fail-closed-over-set (L10), op×existence, diff-guard, base hash+identity capture; in-crate unit tests incl. the cite-around + fail-closed revert-sensitive tests.
3. The new writable careful-open + atomic-write helper (rustix temp+fsync+per-OS finalize, L13) + the actuator rename mutex.
4. `execute_write` — critical-section re-check + base guard (hash+identity) + atomic mutate + hard-delete + sole-constructor `file_written` + source augmentation + frozen vector.
5. N-deep undo store (durable-before-mutation ordering) + `undo_write` (re-gated) + GC.
6. Security consolidation: TOCTOU + same-inode swap, atomicity dir-scan, taint revert-sensitivity (both), no-Tier-A, monotonic-modal, grant-revoked-mid-flight.

---

## 13. Risks & flags (Rev 2 status)

- **R1 — OS-Trash dependency:** ✅ **RESOLVED — dropped** (W5/L1; Peter confirmed). Engine hard-deletes; no new FFI; Finder recovery → app layer.
- **R2 — fail-closed taint:** ✅ resolved — over-the-set (L10/W7) + revert-sensitive test; the dead "recursive walk" claim removed (W3).
- **R3 — same-hash swap:** ✅ resolved — content-hash **AND** `(dev,ino)` identity (L12/W4).
- **R4 — undo taint inheritance:** ✅ confirmed fine by the critic (chokepoint re-stamps via `source=[original]`); plus undo now re-gates + verifies `prev_content_hash` (W9).
- **R5 — write-grant vs read-grant:** ✅ structural separation (L8) + a dedicated test.
- **R6 (NEW) — confused-deputy cite-around (security C1):** ✅ resolved — D8-for-writes engine anchor (L11). The honest residual (non-tracked target + lying caller) is documented and closed at the autonomous-proposer milestone (M6b/M6c).

---

## 14. Review log

**Rev 1 → independent critic + security review (both Opus, 2026-06-20). Both SHIP-WITH-FIXES.** The critic independently re-verified all 10 §6 seam claims against source — all accurate.

- **Critic:** 1 Critical (the Create-atomicity mechanism named `O_TMPFILE`/`renameat2` — Linux-only, absent from pinned rustix, unbuildable on macOS) + 5 Important (dead recursive-walk language; same-hash swap; undo crash-ordering; missing `prompt_hash`; over-claimed anti-fatigue) + minors.
- **Security:** 2 Critical (**C1 cite-around taint laundering** — the gate trusted caller `source_event_ids`, re-opening the extraction D8 hole; **C2** the claimed recursive-walk backstop doesn't exist / isn't wired) + 5 Important (same-hash→inode identity; Tier-A-by-convention; forged/empty sources; **drop the engine OS-Trash dep**; undo must re-gate) + minors.
- **Convergence (the high-signal pattern):** both landed on (i) **taint-input trust** → fixed by **D8-for-writes** (L11) + removing the dead walk (W3); (ii) **same-content inode swap** → `(dev,ino)` identity (L12); (iii) **engine should not take the OS-Trash FFI** → dropped (W5, Peter-confirmed).

**Rev 2 resolutions:** W1 (L11 D8-for-writes) · W2 (L13 portable atomic-create) · W3 (walk language removed, §4) · W4 (L12 identity) · W5 (L1 hard-delete) · W6 (L9 sole constructor) · W7 (L10 fail-closed over set) · W8 (§7.3/§9 crash ordering) · W9 (undo re-gates) · W10 (`prompt_hash=""`) · W11 (anti-fatigue = app, §4). Minors folded: op×existence matrix, symlink-on-create, `is_write_allowed` advisory + canonicalize-parent-for-create, grant-revoked-mid-flight test, RDONLY careful-open ⇒ new write-side open, undo table in SQLCipher, closed op set, content-is-object test, monotonic modal.

_(Build-time validation: every revert-sensitive security test must be shown to FAIL when its fix is reverted — the extraction D8 lesson.)_

---

## 15. Cross-links

`docs/superpowers/specs/2026-06-20-m6-actuator-program-design.md` (parent; M6a = brick 1) · `docs/superpowers/specs/2026-06-15-bossclaw-core-design.md` (§5.11/§5.12/§8.4/§12) · `docs/superpowers/specs/2026-06-20-bossclaw-core-extraction-from-files-design.md` (the `is_external` signal + **D8** anchor-to-engine-lineage lesson that L11 re-applies) · [[air/vault-brain-architecture]] (M6 = brick 2) · [[air/forever-companion-architecture]] (the "acts on your behalf" vision M6c realizes) · [[air/lessons-learned-canonical]] (prove-a-security-test-is-revert-sensitive; verify-claims-about-existing-code; D8 anchor-to-engine-lineage).
