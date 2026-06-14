# BossClaw Desktop — AI Inbox Phase A3: React Inbox UI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the React Inbox UI for the BossClaw desktop — a live conversation list (seeded from the archive), message thread, composer that can start *new* conversations, and a per-contact autonomy dial — over the already-merged Phase A2 command surface, so the desktop becomes a usable GUI client of the AIR Note daemon.

**Architecture:** Frontend feature + one tiny backend param. A new `InboxProvider` React context owns the live viewer connection and **seeds the conversation sidebar from the archive at launch** (`inbox_conversations` + a recent bulk `inbox_history`), then layers live messages + optimistic sends on top. All bug-prone logic (grouping, dedupe, send-state, sidebar merge, unread, body/badge rendering) lives in **pure, unit-tested TypeScript modules**; the React components are thin and verified by typecheck + manual QA. The lone Rust change adds an optional `room` filter to the existing `inbox_history` command (the reader already supports it) so room threads can load history.

**Tech Stack:** React 18 + TypeScript + Vite + Tauri 2 (`@tauri-apps/api`). Token-based inline styling (matching the existing panel idiom — `Card`/`Button`/`Input`/`StatusBadge`/`ToggleSwitch`). **New:** `vitest` (dev-only) for the pure-logic units (Decision D1).

**Status:**
- **v2.1 (2026-06-14) — re-review APPROVE-WITH-CHANGES → fixes folded.** I1: the adoption notice (D9) was wired to always-null (`inboxIdentity()` called with no prior DID), so the design-§4 dormant-agent notice was dead code — now forwards `getIdentity()?.did`. Minors: deep-load effect depends on a derived `selectedIsRoom` boolean instead of whole arrays + checks `live` for brand-new rooms (m1/m2); Composer is `key`ed so its recipient field can't go stale (m3); `mergeSidebar` tie-breaks equal timestamps on `convKey` (m4); QA step added for the dormant notice. Re-review confirmed C1/M1/M2/recipient truly fixed, no new functional regressions.
- **v2 (2026-06-14) — critic-reworked.** Opus critic verdict on v1 was REWORK. Amendments folded in: **C1** cold-start archive load (sidebar was empty on launch — now seeded from `inbox_conversations` + bulk history; new pure `sidebar.ts`); **M1** room history (new optional `room` param on `inbox_history` + room-aware select); **recipient field** in the composer (start new conversations); **M2** hardened listener lifecycle + corrected the false "mirrors identity.tsx" claim + extracted `unread.ts` as a tested pure module; needs-daemon/adoption gate (design §4) via `inbox_status`/`inbox_identity`; archive-read-failure warning (design §8); minors (bodyText wording parity with the CLI, dead-code removal in `makeOptimistic`, retry guard). Tauri casing (D5) was CONFIRMED correct by the critic — unchanged.
- **v1 (2026-06-14)** — initial draft.

Builds on the approved design `docs/superpowers/specs/2026-06-11-desktop-ai-inbox-design.md` (§6 = this UI) and the A2 backend (PR #16, `main` `61fc494`).

---

## §0. Decisions locked before tasks (review these first)

- **D1 — Test strategy.** The desktop frontend has **no test runner today**. **Decision: add `vitest` (dev-only) and TDD the *pure logic*** (`bodyText`, `badges`, `model`, `sendState`, `unread`, `sidebar`) — that is where every real bug lives and it needs no DOM. React components + the provider are verified by `npm run typecheck` + `npm run lint` + a live-daemon QA pass (Task 14). **The `InboxProvider` connection/listener lifecycle is NEW code with no in-repo precedent** (the v1 plan wrongly called it "mirrors identity.tsx" — `state/identity.tsx` has no `listen()`); it is the single highest-risk surface and is QA-verified, so as much of its logic as possible is extracted into the tested pure modules to shrink it.
- **D2 — Dial placement.** Per-DID setting lives **in the conversation header** (set autonomy while looking at that contact). Rooms have no dial (AI is excluded from rooms in v1, design §7).
- **D3 — A3 scope = Phase A UI + one backend param.** Inbox view + composer + dial *control*. The AI loop that *consumes* the dial is Phase B and is NOT built here. The one backend change is the `room` filter on `inbox_history` (Task 2). Everything else is frontend.
- **D4 — Send body shape.** Composer sends `body = {type:"text", text}` (`core.mjs:wrapBody` passes objects through). Rendering ports `bodyText()` from `cli.mjs:81-86` **verbatim**, including the room-join wording.
- **D5 — Tauri arg casing.** Tauri v2 maps JS camelCase → Rust snake_case (so `includeSpam`→`include_spam`, etc.). **CONFIRMED by the critic** against the official v2 docs + 3 in-repo precedents (`skills/registry.ts:90`, `llm_stream` validation strings). Holds because the inbox commands use bare `#[tauri::command]` (no `rename_all`).
- **D6 — Unread + spam.** Unread is **session-local UI state** (resets on relaunch; design §6), via the pure `unread.ts`. Spam hidden by default with a toggle → `include_spam` on `inbox_history`.
- **D7 — Cold-start (the C1 fix).** The sidebar is the merge of (a) `inbox_conversations()` — the authoritative, complete, newest-first grouped list — with (b) conversations derived from loaded rows (a recent bulk `inbox_history(undefined, 200)` for previews + any new live conversations). So every historical conversation appears on launch, with previews for recent ones. The viewer connection stays live-only (design §5); the archive is the backfill.
- **D8 — Recipient.** The composer has a **raw-DID recipient field** for starting new conversations (shown when no peer is selected / via "New message"); when a conversation is selected it sends to that peer. The `contacts.json` picker is deferred — it needs a new `inbox_contacts` command (named follow-up), and raw-DID is sufficient for v1 (critic-agreed).
- **D9 — Needs-daemon / adoption gate.** On mount, probe `inbox_status` + `inbox_identity`. If there is no daemon identity (`state:"needs_daemon"` / `identity_exists:false`), render the "install AIR Note's CLI" screen (design §4) instead of the inbox. If a prior desktop-created identity is now dormant, show the one-time adoption notice.

## §1. Data contract (TypeScript mirror of the Rust surface)

Mirrors `crates/air-rs/src/inbox/{frames,archive_reader,identity_adopter}.rs`. Created in Task 3 (`api/inbox.ts`).

| Source (Rust) | TS type | Notes |
|---|---|---|
| `frames::Message` (event `inbox_message`) | `InboxMessage` | `contact`/`key_changed`/`thread_id`/`room_id`/`body` optional (omitted-when-falsy) |
| `archive_reader::ArchiveRow` (`inbox_history`) | `ArchiveRow` | all fields present; `relay_seq`/`room_id` nullable |
| `archive_reader::ConversationSummary` (`inbox_conversations`) | `ConversationSummary` | `conv_key`, `kind:"room"\|"peer"`, `last_timestamp`, `count` |
| `identity_adopter::Adoption` (`inbox_identity`) | `Adoption` | tagged `{state:"adopted",…} \| {state:"needs_daemon"}` |
| `inbox_status` json | `InboxStatus` | `{home, socket_exists, identity_exists, archive_exists}` |

Events from `commands/inbox.rs`: `inbox_attached {pid,did}`, `inbox_detached {}`, `inbox_offline {}`, `inbox_message <InboxMessage>`, `inbox_send_ok {id,envelope_id,encrypted}`, `inbox_send_err {id,retryable,reason}`.

## §2. File structure

**New — pure logic (TDD, `vitest`):**
- `apps/desktop/src/inbox/bodyText.ts` (+ `.test.ts`) — render a body to display text
- `apps/desktop/src/inbox/badges.ts` (+ `.test.ts`) — badges
- `apps/desktop/src/inbox/model.ts` (+ `.test.ts`) — `ThreadItem`, normalizers, `convKey`, `dedupeById`, `groupConversations`
- `apps/desktop/src/inbox/sendState.ts` (+ `.test.ts`) — optimistic-send reducer
- `apps/desktop/src/inbox/unread.ts` (+ `.test.ts`) — session-local unread set ops
- `apps/desktop/src/inbox/sidebar.ts` (+ `.test.ts`) — merge `inbox_conversations` summaries with loaded-row conversations (the C1 fix)

**New — React (typecheck + QA):**
- `apps/desktop/src/api/inbox.ts` — typed wrappers + event helpers + §1 types
- `apps/desktop/src/state/inbox.tsx` — `InboxProvider` + `useInbox`
- `apps/desktop/src/inbox/ConversationList.tsx` · `MessageThread.tsx` · `Composer.tsx` · `DialControl.tsx` · `NeedsDaemon.tsx` · `InboxPanel.tsx`

**New — config:** `apps/desktop/vitest.config.ts`

**Modified:**
- `apps/desktop/src-tauri/src/commands/inbox.rs` — add optional `room` param to `inbox_history` (Task 2)
- `apps/desktop/package.json` — `vitest` devDep + `test`/`test:watch` scripts
- `apps/desktop/src/App.tsx` — `"inbox"` view + nav button (unread badge) + `<InboxProvider>`

---

## Task 1: Test infrastructure + first pure module (`bodyText`)

**Files:** Modify `apps/desktop/package.json`; Create `apps/desktop/vitest.config.ts`, `apps/desktop/src/inbox/bodyText.ts` (+ `.test.ts`).

- [ ] **Step 1: Add to `package.json`** `scripts`: `"test": "vitest run"`, `"test:watch": "vitest"`; `devDependencies`: `"vitest": "^2.1.0"`.
- [ ] **Step 2: Install** — `cd apps/desktop && npm install`. Expected: vitest added, no errors.
- [ ] **Step 3: Create `vitest.config.ts`**

```ts
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
```

- [ ] **Step 4: Failing test** `src/inbox/bodyText.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { bodyText } from "./bodyText";

describe("bodyText", () => {
  it("renders a text body", () => { expect(bodyText({ type: "text", text: "hi" })).toBe("hi"); });
  it("renders a room message", () => { expect(bodyText({ type: "room/msg", text: "yo" })).toBe("yo"); });
  it("matches the CLI room-join wording exactly", () => {
    expect(bodyText({ type: "room/joined", room_name: "ops" })).toBe('📥 You were added to room "ops"');
  });
  it("marks an undecryptable/absent body as locked", () => {
    expect(bodyText(undefined)).toBe("🔒 (encrypted)");
    expect(bodyText({ type: "encrypted" })).toBe("🔒 (encrypted)");
  });
  it("falls back to JSON for unknown shapes", () => {
    expect(bodyText({ type: "offer", item_id: "x" })).toBe('{"type":"offer","item_id":"x"}');
  });
});
```

- [ ] **Step 5: Run — expect FAIL** (`Cannot find module './bodyText'`). Run: `cd apps/desktop && npm test`.
- [ ] **Step 6: Implement `src/inbox/bodyText.ts`** (ports `cli.mjs:81-86` verbatim; adds the encrypted/absent case the GUI must show)

```ts
/** Render a message body to display text. Ports agent-bridge-mcp `bodyText` (cli.mjs:81-86) verbatim,
 *  plus the encrypted/absent case the GUI must show (body is omitted on the wire when undecryptable). */
export function bodyText(body: unknown): string {
  if (body == null) return "🔒 (encrypted)";
  if (typeof body !== "object") return String(body);
  const b = body as Record<string, unknown>;
  if (b.type === "text") return typeof b.text === "string" ? b.text : "";
  if (b.type === "room/msg") return typeof b.text === "string" ? b.text : "";
  if (b.type === "room/joined") return `📥 You were added to room "${(b.room_name as string) ?? ""}"`;
  if (b.type === "encrypted") return "🔒 (encrypted)";
  return JSON.stringify(body);
}
```

- [ ] **Step 7: Run — expect PASS** (5 tests). Run: `cd apps/desktop && npm test`.
- [ ] **Step 8: Commit**

```bash
git add apps/desktop/package.json apps/desktop/package-lock.json apps/desktop/vitest.config.ts apps/desktop/src/inbox/bodyText.ts apps/desktop/src/inbox/bodyText.test.ts
git commit -m "feat(desktop): vitest + bodyText pure renderer (A3 task 1)"
```

---

## Task 2: Backend — add a `room` filter to `inbox_history` (M1 fix)

The reader already supports room filtering; the command just doesn't pass it. This is the only Rust change.

**Files:** Modify `apps/desktop/src-tauri/src/commands/inbox.rs`.

- [ ] **Step 1: Replace the `inbox_history` command** with a `room`-aware version

```rust
/// History for one peer, one room, or recent across peers when both are None.
#[tauri::command]
pub async fn inbox_history(
    peer: Option<String>,
    room: Option<String>,
    limit: Option<i64>,
    include_spam: Option<bool>,
) -> Result<Value, String> {
    let home = bridge_home();
    if !home.join("archive.db").exists() {
        return Ok(json!([]));
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let reader = ArchiveReader::open(&home).map_err(|e| e.to_string())?;
        let rows = reader
            .history(peer.as_deref(), None, room.as_deref(), None, limit.unwrap_or(50), include_spam.unwrap_or(false))
            .map_err(|e| e.to_string())?;
        serde_json::to_value(rows).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
```

- [ ] **Step 2: Verify the command is already registered** — `main.rs:82` lists `commands::inbox::inbox_history` (signature change needs no registration change).
- [ ] **Step 3: Compile + clippy**

Run: `cd ~/air-note && cargo check -p bossclaw_desktop && cargo clippy -p bossclaw_desktop --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/inbox.rs
git commit -m "feat(desktop): inbox_history accepts an optional room filter (A3 task 2, M1)"
```

---

## Task 3: The data contract + Tauri wrappers (`api/inbox.ts`)

**Files:** Create `apps/desktop/src/api/inbox.ts`. Typecheck-verified.

- [ ] **Step 1: Write `api/inbox.ts`**

```ts
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ── Contract types (mirror crates/air-rs/src/inbox/*) ───────────────────────────

export type InboxMessage = {
  seq: number; relay_seq: number; envelope_id: string; from: string;
  verified: boolean; encrypted: boolean; received_at: string;
  contact?: string; key_changed?: boolean; thread_id?: string; room_id?: string; body?: unknown;
};

export type ArchiveRow = {
  envelope_id: string; direction: "received" | "sent"; thread_id: string; peer_did: string;
  from: string; to: string; timestamp: string; body: unknown;
  encrypted: boolean; verified: boolean; key_changed: boolean; spam: boolean;
  relay_seq: number | null; room_id: string | null; archived_at: string;
};

export type ConversationSummary = {
  conv_key: string; kind: "room" | "peer"; last_timestamp: string; count: number;
};

export type Adoption =
  | { state: "adopted"; did: string; air_id: string; name: string | null; dormant_did: string | null }
  | { state: "needs_daemon" };

export type InboxStatus = {
  home: string; socket_exists: boolean; identity_exists: boolean; archive_exists: boolean;
};

export type Autonomy = "off" | "draft" | "auto";

// ── Command wrappers (Tauri v2: JS camelCase → Rust snake_case, D5) ──────────────

export const inboxStatus = () => invoke<InboxStatus>("inbox_status");
export const inboxIdentity = (desktopPriorDid?: string) =>
  invoke<Adoption>("inbox_identity", { desktopPriorDid });
export const inboxStart = () => invoke<void>("inbox_start");
export const inboxStop = () => invoke<void>("inbox_stop");
/** Returns the correlation id; the ack arrives as an `inbox_send_ok`/`inbox_send_err` event. */
export const inboxSend = (to: string, body: unknown, threadId?: string, inReplyTo?: string) =>
  invoke<string>("inbox_send", { to, body, threadId, inReplyTo });
export const inboxConversations = () => invoke<ConversationSummary[]>("inbox_conversations");
/** peer XOR room XOR neither (recent across peers). */
export const inboxHistory = (peer?: string, room?: string, limit?: number, includeSpam?: boolean) =>
  invoke<ArchiveRow[]>("inbox_history", { peer, room, limit, includeSpam });
export const inboxPolicyGet = (did: string) => invoke<Autonomy>("inbox_policy_get", { did });
export const inboxPolicySet = (did: string, value: Autonomy) =>
  invoke<void>("inbox_policy_set", { did, value });

// ── Event payloads + a typed subscribe helper ───────────────────────────────────

export type InboxEvents = {
  inbox_attached: { pid: number; did: string };
  inbox_detached: Record<string, never>;
  inbox_offline: Record<string, never>;
  inbox_message: InboxMessage;
  inbox_send_ok: { id: string; envelope_id: string; encrypted: boolean };
  inbox_send_err: { id: string; retryable: boolean; reason: string };
};

export function onInboxEvent<K extends keyof InboxEvents>(
  name: K, handler: (payload: InboxEvents[K]) => void,
): Promise<UnlistenFn> {
  return listen<InboxEvents[K]>(name, (e) => handler(e.payload));
}
```

- [ ] **Step 2: Typecheck** — `cd apps/desktop && npm run typecheck`. Expected: PASS.
- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/api/inbox.ts
git commit -m "feat(desktop): typed inbox command + event API (A3 task 3)"
```

---

## Task 4: Badges (pure)

**Files:** Create `apps/desktop/src/inbox/badges.ts` (+ `.test.ts`).

- [ ] **Step 1: Failing test** `badges.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { badgesFor } from "./badges";

describe("badgesFor", () => {
  it("lock + verified for a normal encrypted message", () => {
    expect(badgesFor({ encrypted: true, verified: true })).toEqual([
      { label: "🔒", tone: "neutral" }, { label: "✓", tone: "success" },
    ]);
  });
  it("flags unverified", () => {
    expect(badgesFor({ encrypted: false, verified: false })).toEqual([{ label: "unverified", tone: "warning" }]);
  });
  it("flags changed key + spam", () => {
    const out = badgesFor({ encrypted: true, verified: true, key_changed: true, spam: true });
    expect(out).toContainEqual({ label: "⚠ key changed", tone: "error" });
    expect(out).toContainEqual({ label: "spam", tone: "warning" });
  });
});
```

- [ ] **Step 2: Run — expect FAIL.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 3: Implement `badges.ts`**

```ts
export type BadgeTone = "neutral" | "success" | "warning" | "error";
export type Badge = { label: string; tone: BadgeTone };
export type BadgeInput = { encrypted: boolean; verified: boolean; key_changed?: boolean; spam?: boolean };

/** Badge vocabulary mirrors the CLI (🔒 ✓) plus the GUI's key-changed/spam flags (design §6). */
export function badgesFor(m: BadgeInput): Badge[] {
  const out: Badge[] = [];
  if (m.encrypted) out.push({ label: "🔒", tone: "neutral" });
  out.push(m.verified ? { label: "✓", tone: "success" } : { label: "unverified", tone: "warning" });
  if (m.key_changed) out.push({ label: "⚠ key changed", tone: "error" });
  if (m.spam) out.push({ label: "spam", tone: "warning" });
  return out;
}
```

- [ ] **Step 4: Run — expect PASS.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/inbox/badges.ts apps/desktop/src/inbox/badges.test.ts
git commit -m "feat(desktop): message badge derivation (A3 task 4)"
```

---

## Task 5: The conversation model — normalize, dedupe, group (pure)

**Files:** Create `apps/desktop/src/inbox/model.ts` (+ `.test.ts`).

- [ ] **Step 1: Failing test** `model.test.ts`

```ts
import { describe, it, expect } from "vitest";
import {
  fromArchiveRow, fromLiveMessage, makeOptimistic, convKey, dedupeById, groupConversations,
  type ThreadItem,
} from "./model";
import type { ArchiveRow, InboxMessage } from "../api/inbox";

const row = (over: Partial<ArchiveRow>): ArchiveRow => ({
  envelope_id: "e1", direction: "received", thread_id: "t1", peer_did: "did:wba:p1",
  from: "did:wba:p1", to: "did:wba:me", timestamp: "2026-06-14T00:00:00Z",
  body: { type: "text", text: "hi" }, encrypted: true, verified: true,
  key_changed: false, spam: false, relay_seq: 1, room_id: null, archived_at: "x", ...over,
});

describe("convKey", () => {
  it("room_id for rooms, peer_did for 1:1", () => {
    expect(convKey({ room_id: "r1", peer_did: "p" })).toBe("r1");
    expect(convKey({ room_id: null, peer_did: "p" })).toBe("p");
  });
});

describe("normalizers", () => {
  it("live received message: peer = from, direction received", () => {
    const m: InboxMessage = { seq: 1, relay_seq: 1, envelope_id: "e9", from: "did:wba:p2",
      verified: true, encrypted: true, received_at: "2026-06-14T01:00:00Z", body: { type: "text", text: "yo" } };
    const t = fromLiveMessage(m);
    expect(t).toMatchObject({ peer_did: "did:wba:p2", direction: "received", timestamp: "2026-06-14T01:00:00Z" });
  });
  it("optimistic sent row is pending with a correlation id", () => {
    const t = makeOptimistic("corr1", "did:wba:p3", { type: "text", text: "draft" }, "2026-06-14T02:00:00Z");
    expect(t).toMatchObject({ direction: "sent", peer_did: "did:wba:p3", status: "pending", correlationId: "corr1", room_id: null });
  });
});

describe("dedupeById", () => {
  it("keeps the first occurrence (confirmed rows passed first win)", () => {
    const confirmed = fromArchiveRow(row({ envelope_id: "dup" }));
    const optimistic = { ...makeOptimistic("c", "did:wba:p1", {}, "z"), envelope_id: "dup" } as ThreadItem;
    const out = dedupeById([confirmed, optimistic]);
    expect(out).toHaveLength(1);
    expect(out[0].status).toBe("ok");
  });
});

describe("groupConversations", () => {
  it("groups by conv key, newest-first, with preview + unread", () => {
    const items: ThreadItem[] = [
      fromArchiveRow(row({ envelope_id: "a", peer_did: "did:wba:p1", timestamp: "2026-06-14T00:00:00Z" })),
      fromArchiveRow(row({ envelope_id: "b", peer_did: "did:wba:p2", timestamp: "2026-06-14T03:00:00Z" })),
      fromArchiveRow(row({ envelope_id: "c", peer_did: "did:wba:p1", timestamp: "2026-06-14T02:00:00Z" })),
    ];
    const convs = groupConversations(items, new Set(["b"]));
    expect(convs.map((c) => c.convKey)).toEqual(["did:wba:p2", "did:wba:p1"]);
    expect(convs[0].unread).toBe(1);
    expect(convs[1].lastText).toBe("hi");
  });
});
```

- [ ] **Step 2: Run — expect FAIL.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 3: Implement `model.ts`**

```ts
import type { ArchiveRow, InboxMessage } from "../api/inbox";
import { bodyText } from "./bodyText";

/** The one shape the inbox UI renders. `status`/`retryable`/`reason`/`correlationId` set only for optimistic sends. */
export type ThreadItem = {
  envelope_id: string; direction: "received" | "sent"; peer_did: string; room_id: string | null;
  from: string; to: string | null; timestamp: string; body: unknown;
  encrypted: boolean; verified: boolean; key_changed: boolean; spam: boolean;
  status?: "pending" | "ok" | "err"; retryable?: boolean; reason?: string; correlationId?: string;
};

export type Conversation = {
  convKey: string; kind: "room" | "peer"; lastTimestamp: string; lastText: string; unread: number;
};

export const convKey = (x: { room_id: string | null; peer_did: string }): string => x.room_id ?? x.peer_did;

export function fromArchiveRow(r: ArchiveRow): ThreadItem {
  return {
    envelope_id: r.envelope_id, direction: r.direction, peer_did: r.peer_did, room_id: r.room_id,
    from: r.from, to: r.to, timestamp: r.timestamp, body: r.body, encrypted: r.encrypted,
    verified: r.verified, key_changed: r.key_changed, spam: r.spam, status: "ok",
  };
}

/** Live viewer messages are always received; the peer is the sender. */
export function fromLiveMessage(m: InboxMessage): ThreadItem {
  return {
    envelope_id: m.envelope_id, direction: "received", peer_did: m.from, room_id: m.room_id ?? null,
    from: m.from, to: null, timestamp: m.received_at, body: m.body, encrypted: m.encrypted,
    verified: m.verified, key_changed: m.key_changed === true, spam: false, status: "ok",
  };
}

/** An optimistic sent row, shown immediately; resolved by the send-ok/err ack (Task 6). 1:1 only in v1. */
export function makeOptimistic(correlationId: string, to: string, body: unknown, timestamp: string): ThreadItem {
  return {
    envelope_id: `pending:${correlationId}`, direction: "sent", peer_did: to, room_id: null,
    from: "", to, timestamp, body, encrypted: true, verified: true, key_changed: false, spam: false,
    status: "pending", correlationId,
  };
}

/** First-occurrence wins. Callers MUST pass confirmed rows before optimistic ones so a
 *  pending/confirmed clash on the same envelope_id keeps the confirmed row (design §3 cross-stream dedupe). */
export function dedupeById<T extends { envelope_id: string }>(items: T[]): T[] {
  const seen = new Map<string, T>();
  for (const it of items) if (!seen.has(it.envelope_id)) seen.set(it.envelope_id, it);
  return [...seen.values()];
}

/** Group items into conversations, newest-first. `unreadIds` = envelope_ids counted as unread. */
export function groupConversations(items: ThreadItem[], unreadIds: Set<string>): Conversation[] {
  const map = new Map<string, Conversation>();
  for (const it of items) {
    const key = convKey(it);
    const prev = map.get(key);
    const isNewer = !prev || it.timestamp > prev.lastTimestamp;
    map.set(key, {
      convKey: key, kind: it.room_id ? "room" : "peer",
      lastTimestamp: isNewer ? it.timestamp : prev!.lastTimestamp,
      lastText: isNewer ? bodyText(it.body) : prev!.lastText,
      unread: (prev?.unread ?? 0) + (unreadIds.has(it.envelope_id) ? 1 : 0),
    });
  }
  return [...map.values()].sort((a, b) => (a.lastTimestamp < b.lastTimestamp ? 1 : -1));
}
```

- [ ] **Step 4: Run — expect PASS.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/inbox/model.ts apps/desktop/src/inbox/model.test.ts
git commit -m "feat(desktop): ThreadItem model — normalize/dedupe/group (A3 task 5)"
```

---

## Task 6: Optimistic send reducer (pure)

**Files:** Create `apps/desktop/src/inbox/sendState.ts` (+ `.test.ts`).

- [ ] **Step 1: Failing test** `sendState.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { onSendStart, onSendOk, onSendErr, type SendState } from "./sendState";

describe("send reducer", () => {
  it("start → pending", () => { expect(onSendStart({}, "c1")).toEqual({ c1: { status: "pending" } }); });
  it("ok → records envelope_id", () => {
    const s: SendState = onSendStart({}, "c1");
    expect(onSendOk(s, { id: "c1", envelope_id: "e1", encrypted: true })).toEqual({ c1: { status: "ok", envelope_id: "e1" } });
  });
  it("err → carries retryable + reason", () => {
    const s: SendState = onSendStart({}, "c1");
    expect(onSendErr(s, { id: "c1", retryable: false, reason: "unresolvable" }))
      .toEqual({ c1: { status: "err", retryable: false, reason: "unresolvable" } });
  });
  it("ignores acks for unknown ids", () => { expect(onSendOk({}, { id: "ghost", envelope_id: "e", encrypted: true })).toEqual({}); });
});
```

- [ ] **Step 2: Run — expect FAIL.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 3: Implement `sendState.ts`**

```ts
export type SendEntry =
  | { status: "pending" }
  | { status: "ok"; envelope_id: string }
  | { status: "err"; retryable: boolean; reason: string };
export type SendState = Record<string, SendEntry>;

export function onSendStart(s: SendState, id: string): SendState { return { ...s, [id]: { status: "pending" } }; }
export function onSendOk(s: SendState, a: { id: string; envelope_id: string }): SendState {
  if (!(a.id in s)) return s;
  return { ...s, [a.id]: { status: "ok", envelope_id: a.envelope_id } };
}
export function onSendErr(s: SendState, a: { id: string; retryable: boolean; reason: string }): SendState {
  if (!(a.id in s)) return s;
  return { ...s, [a.id]: { status: "err", retryable: a.retryable, reason: a.reason } };
}
```

- [ ] **Step 4: Run — expect PASS.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/inbox/sendState.ts apps/desktop/src/inbox/sendState.test.ts
git commit -m "feat(desktop): optimistic send reducer (A3 task 6)"
```

---

## Task 7: Unread set ops (pure, M2 extraction)

**Files:** Create `apps/desktop/src/inbox/unread.ts` (+ `.test.ts`).

- [ ] **Step 1: Failing test** `unread.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { addUnread, clearConv } from "./unread";
import { fromArchiveRow, type ThreadItem } from "./model";
import type { ArchiveRow } from "../api/inbox";

const item = (id: string, peer: string): ThreadItem =>
  fromArchiveRow({ envelope_id: id, direction: "received", thread_id: "t", peer_did: peer, from: peer,
    to: "me", timestamp: "z", body: {}, encrypted: false, verified: true, key_changed: false,
    spam: false, relay_seq: 1, room_id: null, archived_at: "z" } as ArchiveRow);

describe("unread ops", () => {
  it("addUnread returns a NEW set with the id", () => {
    const a = new Set<string>(); const b = addUnread(a, "e1");
    expect(b.has("e1")).toBe(true); expect(a.has("e1")).toBe(false);
  });
  it("clearConv removes only the conv's loaded ids", () => {
    const set = new Set(["e1", "e2", "e3"]);
    const loaded = [item("e1", "p1"), item("e2", "p2")];
    const out = clearConv(set, loaded, "p1");
    expect(out.has("e1")).toBe(false); expect(out.has("e2")).toBe(true); expect(out.has("e3")).toBe(true);
  });
});
```

- [ ] **Step 2: Run — expect FAIL.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 3: Implement `unread.ts`**

```ts
import { convKey, type ThreadItem } from "./model";

export function addUnread(set: Set<string>, envelopeId: string): Set<string> {
  const next = new Set(set); next.add(envelopeId); return next;
}

/** Clear unread for every loaded item belonging to `convKeyToClear`. */
export function clearConv(set: Set<string>, loaded: ThreadItem[], convKeyToClear: string): Set<string> {
  const next = new Set(set);
  for (const it of loaded) if (convKey(it) === convKeyToClear) next.delete(it.envelope_id);
  return next;
}
```

- [ ] **Step 4: Run — expect PASS.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/inbox/unread.ts apps/desktop/src/inbox/unread.test.ts
git commit -m "feat(desktop): session-local unread set ops (A3 task 7)"
```

---

## Task 8: Sidebar merge (pure, C1 fix)

Merges the authoritative `inbox_conversations` summaries (complete, newest-first, but no preview text) with the conversations derived from loaded rows (have previews + unread). This is what makes the sidebar non-empty on launch.

**Files:** Create `apps/desktop/src/inbox/sidebar.ts` (+ `.test.ts`).

- [ ] **Step 1: Failing test** `sidebar.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { mergeSidebar } from "./sidebar";
import type { Conversation } from "./model";
import type { ConversationSummary } from "../api/inbox";

const summary = (key: string, ts: string): ConversationSummary =>
  ({ conv_key: key, kind: "peer", last_timestamp: ts, count: 1 });
const conv = (key: string, ts: string, text: string, unread = 0): Conversation =>
  ({ convKey: key, kind: "peer", lastTimestamp: ts, lastText: text, unread });

describe("mergeSidebar", () => {
  it("shows every summary; enriches preview/unread from loaded rows", () => {
    const out = mergeSidebar([summary("p1", "2"), summary("p2", "1")], [conv("p1", "2", "hi", 3)]);
    expect(out.map((c) => c.convKey)).toEqual(["p1", "p2"]);
    expect(out[0]).toMatchObject({ lastText: "hi", unread: 3 });
    expect(out[1]).toMatchObject({ lastText: "", unread: 0 }); // p2 not loaded yet → no preview
  });
  it("appends a brand-new live conversation absent from summaries", () => {
    const out = mergeSidebar([summary("p1", "1")], [conv("p1", "1", "a"), conv("pNEW", "9", "new!", 1)]);
    expect(out.map((c) => c.convKey)).toEqual(["pNEW", "p1"]); // newest-first
  });
});
```

- [ ] **Step 2: Run — expect FAIL.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 3: Implement `sidebar.ts`**

```ts
import type { Conversation } from "./model";
import type { ConversationSummary } from "../api/inbox";

/** Authoritative sidebar = every archived conversation (`summaries`), enriched with preview/unread
 *  from `grouped` (conversations built from loaded rows), plus any conv that exists only in loaded
 *  rows (a new live conversation this session). Newest-first. (C1 fix.) */
export function mergeSidebar(summaries: ConversationSummary[], grouped: Conversation[]): Conversation[] {
  const byKey = new Map(grouped.map((g) => [g.convKey, g]));
  const used = new Set<string>();
  const out: Conversation[] = [];
  for (const s of summaries) {
    used.add(s.conv_key);
    const g = byKey.get(s.conv_key);
    out.push(g ?? { convKey: s.conv_key, kind: s.kind, lastTimestamp: s.last_timestamp, lastText: "", unread: 0 });
  }
  for (const g of grouped) if (!used.has(g.convKey)) out.push(g);
  return out.sort((a, b) =>
    a.lastTimestamp === b.lastTimestamp ? (a.convKey < b.convKey ? -1 : 1)
      : a.lastTimestamp < b.lastTimestamp ? 1 : -1); // m4: stable tie-break on convKey
}
```

- [ ] **Step 4: Run — expect PASS.** Run: `cd apps/desktop && npm test`.
- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/inbox/sidebar.ts apps/desktop/src/inbox/sidebar.test.ts
git commit -m "feat(desktop): sidebar merge — archive seeds the conversation list (A3 task 8, C1)"
```

---

## Task 9: The InboxProvider (context + lifecycle)

Owns: needs-daemon gate, connection lifecycle (hardened listener teardown, M2), cold-start archive seed (C1), room-aware thread load (M1), live + optimistic layering, unread, spam, send. NEW event-lifecycle code (D1) — the highest-risk surface; logic is delegated to the tested pure modules.

**Files:** Create `apps/desktop/src/state/inbox.tsx`.

- [ ] **Step 1: Write `state/inbox.tsx`**

```tsx
import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  inboxStart, inboxStop, inboxSend, inboxHistory, inboxConversations, inboxStatus, inboxIdentity,
  onInboxEvent, type InboxMessage, type Adoption, type ConversationSummary,
} from "../api/inbox";
import { getIdentity } from "../api/tauri";
import {
  fromArchiveRow, fromLiveMessage, makeOptimistic, convKey, dedupeById, groupConversations,
  type ThreadItem, type Conversation,
} from "../inbox/model";
import { onSendStart, onSendOk, onSendErr, type SendState } from "../inbox/sendState";
import { addUnread, clearConv } from "../inbox/unread";
import { mergeSidebar } from "../inbox/sidebar";

type InboxCtx = {
  gate: "loading" | "needs_daemon" | "ready";
  adoption: Adoption | null;
  online: boolean;
  archiveError: boolean;
  conversations: Conversation[];
  selected: string | null;
  thread: ThreadItem[];
  includeSpam: boolean;
  totalUnread: number;
  select: (convKey: string) => void;
  setIncludeSpam: (v: boolean) => void;
  send: (to: string, text: string) => Promise<void>;
};

const Ctx = createContext<InboxCtx | null>(null);
const BULK_LIMIT = 200;

export function InboxProvider({ children }: { children: ReactNode }) {
  const [gate, setGate] = useState<"loading" | "needs_daemon" | "ready">("loading");
  const [adoption, setAdoption] = useState<Adoption | null>(null);
  const [online, setOnline] = useState(false);
  const [archiveError, setArchiveError] = useState(false);
  const [summaries, setSummaries] = useState<ConversationSummary[]>([]);
  const [recent, setRecent] = useState<ThreadItem[]>([]);       // bulk cross-peer backfill (previews)
  const [threadRows, setThreadRows] = useState<ThreadItem[]>([]); // deep history for the open conv
  const [live, setLive] = useState<ThreadItem[]>([]);
  const [optimistic, setOptimistic] = useState<ThreadItem[]>([]);
  const [sendState, setSendState] = useState<SendState>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [includeSpam, setIncludeSpam] = useState(false);
  const [unread, setUnread] = useState<Set<string>>(new Set());
  const selectedRef = useRef<string | null>(null);
  selectedRef.current = selected;

  // Probe daemon presence + adopted identity (design §4 gate). Forward the desktop's prior self-created
  // DID (if onboarding ran here before) so the adoption can name the now-dormant agent (I1).
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const prior = await getIdentity().catch(() => null);
        const [status, adopt] = await Promise.all([inboxStatus(), inboxIdentity(prior?.did)]);
        if (!alive) return;
        setAdoption(adopt);
        setGate(adopt.state === "needs_daemon" || !status.identity_exists ? "needs_daemon" : "ready");
      } catch {
        if (alive) setGate("needs_daemon");
      }
    })();
    return () => { alive = false; };
  }, []);

  // Connect + subscribe once we're ready. Listener teardown is race-safe (M2): if the effect is torn
  // down mid-await, unlisten the just-registered handler instead of leaking it.
  useEffect(() => {
    if (gate !== "ready") return;
    let cancelled = false;
    const unlisten: Array<() => void> = [];
    const reg = async (p: Promise<() => void>) => {
      const off = await p;
      if (cancelled) { off(); return; } // torn down mid-await → unlisten instead of leaking (M2)
      unlisten.push(off);
    };
    (async () => {
      await reg(onInboxEvent("inbox_attached", () => setOnline(true)));
      await reg(onInboxEvent("inbox_offline", () => setOnline(false)));
      await reg(onInboxEvent("inbox_detached", () => setOnline(false)));
      await reg(onInboxEvent("inbox_message", (m: InboxMessage) => {
        const it = fromLiveMessage(m);
        setLive((prev) => [...prev, it]);
        if (convKey(it) !== selectedRef.current) setUnread((u) => addUnread(u, it.envelope_id));
      }));
      await reg(onInboxEvent("inbox_send_ok", (a) => setSendState((s) => onSendOk(s, a))));
      await reg(onInboxEvent("inbox_send_err", (a) => setSendState((s) => onSendErr(s, a))));
      if (!cancelled) await inboxStart();
    })();
    return () => { cancelled = true; unlisten.forEach((fn) => fn()); inboxStop().catch(() => {}); };
  }, [gate]);

  // C1: seed the sidebar from the archive (complete list + recent previews). Re-runs on spam toggle.
  useEffect(() => {
    if (gate !== "ready") return;
    let alive = true;
    inboxConversations().then((s) => { if (alive) setSummaries(s); }).catch(() => {});
    inboxHistory(undefined, undefined, BULK_LIMIT, includeSpam)
      .then((rows) => { if (alive) { setRecent(rows.map(fromArchiveRow)); setArchiveError(false); } })
      .catch(() => { if (alive) setArchiveError(true); });
    return () => { alive = false; };
  }, [gate, includeSpam]);

  // Is the selected conversation a room? Derived so the deep-load effect can depend on a stable
  // boolean (not the whole summaries/recent/live arrays) and re-fire only on real changes (m1/m2).
  const selectedIsRoom = useMemo(
    () => summaries.find((s) => s.conv_key === selected)?.kind === "room"
      || recent.some((r) => r.room_id === selected)
      || live.some((r) => r.room_id === selected), // m2: a room first seen live this session
    [selected, summaries, recent, live],
  );

  // M1: deep-load the selected conversation (peer vs room).
  useEffect(() => {
    if (!selected) { setThreadRows([]); return; }
    let alive = true;
    const p = selectedIsRoom
      ? inboxHistory(undefined, selected, BULK_LIMIT, includeSpam)
      : inboxHistory(selected, undefined, BULK_LIMIT, includeSpam);
    p.then((rows) => { if (alive) { setThreadRows(rows.map(fromArchiveRow)); setArchiveError(false); } })
     .catch(() => { if (alive) setArchiveError(true); });
    return () => { alive = false; };
  }, [selected, selectedIsRoom, includeSpam]);

  const resolvedOptimistic = useMemo(
    () => optimistic.map((o) => {
      const st = o.correlationId ? sendState[o.correlationId] : undefined;
      if (!st) return o;
      if (st.status === "ok") return { ...o, status: "ok" as const, envelope_id: st.envelope_id };
      if (st.status === "err") return { ...o, status: "err" as const, retryable: st.retryable, reason: st.reason };
      return o;
    }),
    [optimistic, sendState],
  );

  // confirmed rows (threadRows, recent) BEFORE optimistic so dedupe keeps the confirmed copy.
  const all = useMemo(
    () => dedupeById([...threadRows, ...recent, ...live, ...resolvedOptimistic]),
    [threadRows, recent, live, resolvedOptimistic],
  );
  const grouped = useMemo(() => groupConversations(all, unread), [all, unread]);
  const conversations = useMemo(() => mergeSidebar(summaries, grouped), [summaries, grouped]);
  const thread = useMemo(
    () => all.filter((it) => convKey(it) === selected).sort((a, b) => (a.timestamp < b.timestamp ? -1 : 1)),
    [all, selected],
  );
  const totalUnread = useMemo(() => conversations.reduce((n, c) => n + c.unread, 0), [conversations]);

  const select = (key: string) => {
    setSelected(key);
    setUnread((u) => clearConv(u, all, key));
  };

  const send = async (to: string, text: string) => {
    const body = { type: "text", text };
    const id = await inboxSend(to, body);
    setSendState((s) => onSendStart(s, id));
    setOptimistic((prev) => [...prev, makeOptimistic(id, to, body, new Date().toISOString())]);
  };

  return (
    <Ctx.Provider value={{
      gate, adoption, online, archiveError, conversations, selected, thread, includeSpam, totalUnread,
      select, setIncludeSpam, send,
    }}>
      {children}
    </Ctx.Provider>
  );
}

export function useInbox() {
  const c = useContext(Ctx);
  if (!c) throw new Error("useInbox must be inside InboxProvider");
  return c;
}
```

> **Implementer notes:** (1) `new Date().toISOString()` is the only impurity (optimistic-row timestamp), deliberately kept out of the pure model. (2) This effect/listener lifecycle has no in-repo precedent — verify teardown under React 18 StrictMode double-invoke in QA (Task 14): connect once, no leaked listeners, `inbox_start` idempotent (it is, server-side).

- [ ] **Step 2: Typecheck** — `cd apps/desktop && npm run typecheck`. Expected: PASS.
- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/state/inbox.tsx
git commit -m "feat(desktop): InboxProvider — gate, lifecycle, cold-start seed, room load (A3 task 9)"
```

---

## Task 10: ConversationList + MessageThread (presentational)

**Files:** Create `apps/desktop/src/inbox/ConversationList.tsx`, `MessageThread.tsx`.

- [ ] **Step 1: `ConversationList.tsx`**

```tsx
import { StatusBadge } from "../components/ui/StatusBadge";
import type { Conversation } from "./model";

const short = (did: string) => did.replace(/^did:wba:[^:]+:agents:/, "");

export function ConversationList({
  conversations, selected, onSelect,
}: { conversations: Conversation[]; selected: string | null; onSelect: (k: string) => void }) {
  if (conversations.length === 0) {
    return <div style={{ color: "#666", fontSize: 13, padding: 12 }}>No conversations yet.</div>;
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      {conversations.map((c) => (
        <button key={c.convKey} onClick={() => onSelect(c.convKey)}
          style={{
            textAlign: "left", padding: "8px 10px", borderRadius: 8, cursor: "pointer",
            border: "1px solid " + (c.convKey === selected ? "#2F6BFF" : "#eee"),
            background: c.convKey === selected ? "#EEF3FF" : "white",
          }}>
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
            <span style={{ fontSize: 13, fontWeight: 600 }}>{c.kind === "room" ? "👥 " : ""}{short(c.convKey)}</span>
            {c.unread > 0 ? <StatusBadge tone="primary">{c.unread}</StatusBadge> : null}
          </div>
          <div style={{ fontSize: 12, color: "#666", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {c.lastText}
          </div>
        </button>
      ))}
    </div>
  );
}
```

- [ ] **Step 2: `MessageThread.tsx`**

```tsx
import { StatusBadge } from "../components/ui/StatusBadge";
import { Button } from "../components/Button";
import { bodyText } from "./bodyText";
import { badgesFor } from "./badges";
import type { ThreadItem } from "./model";

export function MessageThread({ items, onRetry }: { items: ThreadItem[]; onRetry: (it: ThreadItem) => void }) {
  if (items.length === 0) return <div style={{ color: "#666", fontSize: 13, padding: 12 }}>No messages.</div>;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
      {items.map((it) => {
        const mine = it.direction === "sent";
        return (
          <div key={it.envelope_id} style={{ alignSelf: mine ? "flex-end" : "flex-start", maxWidth: "80%" }}>
            <div style={{
              padding: "8px 12px", borderRadius: 10, fontSize: 14,
              background: mine ? "#2F6BFF" : "#F3F4F6", color: mine ? "white" : "#0B0F17",
              opacity: it.status === "pending" ? 0.6 : 1,
            }}>{bodyText(it.body)}</div>
            <div style={{ display: "flex", gap: 4, marginTop: 2, justifyContent: mine ? "flex-end" : "flex-start", flexWrap: "wrap" }}>
              {badgesFor(it).map((b, i) => <StatusBadge key={i} tone={b.tone}>{b.label}</StatusBadge>)}
              {it.status === "pending" ? <StatusBadge tone="neutral">sending…</StatusBadge> : null}
              {it.status === "err" ? (
                <>
                  <StatusBadge tone="error">{it.reason ?? "failed"}</StatusBadge>
                  {it.retryable ? <Button variant="secondary" onClick={() => onRetry(it)} style={{ padding: "2px 8px", fontSize: 12 }}>Retry</Button> : null}
                </>
              ) : null}
            </div>
          </div>
        );
      })}
    </div>
  );
}
```

- [ ] **Step 3: Typecheck** — `cd apps/desktop && npm run typecheck`. Expected: PASS.
- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/inbox/ConversationList.tsx apps/desktop/src/inbox/MessageThread.tsx
git commit -m "feat(desktop): conversation list + message thread (A3 task 10)"
```

---

## Task 11: Composer (with recipient) + DialControl

**Files:** Create `apps/desktop/src/inbox/Composer.tsx`, `DialControl.tsx`.

- [ ] **Step 1: `Composer.tsx`** — recipient field shown when no peer is selected (D8: start a new conversation)

```tsx
import { useState } from "react";
import { Input } from "../components/Input";
import { Button } from "../components/Button";

/** When `to` is null the composer shows a recipient (raw DID) field so a NEW conversation can start.
 *  When `to` is set (a conversation is open) it sends to that peer. */
export function Composer({ to, disabled, onSend }: {
  to: string | null; disabled: boolean; onSend: (to: string, text: string) => void;
}) {
  const [recipient, setRecipient] = useState("");
  const [text, setText] = useState("");
  const target = to ?? recipient.trim();
  const canSend = !disabled && !!target && !!text.trim();

  const submit = () => {
    if (!canSend) return;
    onSend(target, text.trim());
    setText("");
    if (!to) setRecipient("");
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 12 }}>
      {!to ? (
        <Input value={recipient} placeholder="Recipient DID (did:wba:…)" disabled={disabled}
          onChange={(e) => setRecipient(e.target.value)} />
      ) : null}
      <div style={{ display: "flex", gap: 8 }}>
        <Input value={text} placeholder={disabled ? "daemon offline" : "Message…"} disabled={disabled}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }} />
        <Button variant="primary" disabled={!canSend} onClick={submit}>Send</Button>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: `DialControl.tsx`** (off/draft/auto; writes via `inbox_policy_set` — D2/D3)

```tsx
import { useEffect, useState } from "react";
import { inboxPolicyGet, inboxPolicySet, type Autonomy } from "../api/inbox";

const OPTIONS: Autonomy[] = ["off", "draft", "auto"];

export function DialControl({ did }: { did: string }) {
  const [value, setValue] = useState<Autonomy>("draft");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    inboxPolicyGet(did).then((v) => { if (alive) setValue(v); }).catch(() => {});
    return () => { alive = false; };
  }, [did]);

  const change = async (v: Autonomy) => {
    setBusy(true);
    try { await inboxPolicySet(did, v); setValue(v); } finally { setBusy(false); }
  };

  return (
    <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
      <span style={{ fontSize: 12, color: "#666" }}>AI:</span>
      <div style={{ display: "inline-flex", border: "1px solid #ccc", borderRadius: 6, overflow: "hidden" }}>
        {OPTIONS.map((o) => (
          <button key={o} disabled={busy} onClick={() => change(o)}
            style={{ padding: "4px 10px", fontSize: 12, border: "none", cursor: "pointer",
              background: o === value ? "#2F6BFF" : "white", color: o === value ? "white" : "#0B0F17" }}>
            {o}
          </button>
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Typecheck** — `cd apps/desktop && npm run typecheck`. Expected: PASS.
- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/inbox/Composer.tsx apps/desktop/src/inbox/DialControl.tsx
git commit -m "feat(desktop): composer with recipient + per-contact AI dial (A3 task 11, D8)"
```

---

## Task 12: NeedsDaemon screen + InboxPanel

**Files:** Create `apps/desktop/src/inbox/NeedsDaemon.tsx`, `InboxPanel.tsx`.

- [ ] **Step 1: `NeedsDaemon.tsx`** (design §4 install screen)

```tsx
import { Card } from "../components/Card";

export function NeedsDaemon() {
  return (
    <Card>
      <h2 style={{ margin: 0 }}>Connect AIR Note</h2>
      <p style={{ marginTop: 12, color: "#666", lineHeight: 1.5 }}>
        No local AIR Note agent found. Install the CLI and start the daemon, then reopen this tab:
      </p>
      <pre style={{ background: "#F3F4F6", padding: 12, borderRadius: 8, fontSize: 12, overflowX: "auto" }}>
        air-msg daemon install
      </pre>
    </Card>
  );
}
```

- [ ] **Step 2: `InboxPanel.tsx`** (gate, offline banner, archive warning, spam toggle, "New message")

```tsx
import { useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { ToggleSwitch } from "../components/ui/ToggleSwitch";
import { useInbox } from "../state/inbox";
import { ConversationList } from "./ConversationList";
import { MessageThread } from "./MessageThread";
import { Composer } from "./Composer";
import { DialControl } from "./DialControl";
import { NeedsDaemon } from "./NeedsDaemon";
import type { ThreadItem } from "./model";

const short = (did: string) => did.replace(/^did:wba:[^:]+:agents:/, "");

export function InboxPanel() {
  const { gate, adoption, online, archiveError, conversations, selected, thread, includeSpam, select, setIncludeSpam, send } = useInbox();
  const [composing, setComposing] = useState(false);

  if (gate === "loading") return <Card>Loading…</Card>;
  if (gate === "needs_daemon") return <NeedsDaemon />;

  const selectedConv = conversations.find((c) => c.convKey === selected);
  const isRoom = selectedConv?.kind === "room";
  const showNew = composing;                    // explicit compose mode (overrides selection)
  const showConv = !composing && !!selected;    // viewing an existing conversation
  const showPane = showNew || showConv;

  const onRetry = (it: ThreadItem) => {
    const b = it.body as { type?: string; text?: string } | null;
    if (selected && b?.type === "text" && typeof b.text === "string") send(selected, b.text);
  };
  // On send, leave compose mode and select the recipient so the optimistic row shows immediately.
  const handleSend = (to: string, text: string) => { send(to, text); setComposing(false); select(to); };

  return (
    <Card>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <h2 style={{ margin: 0 }}>Inbox</h2>
        <div style={{ display: "flex", gap: 12, alignItems: "center" }}>
          <ToggleSwitch checked={includeSpam} onChange={setIncludeSpam} label="Show spam" />
          <Button variant="secondary" onClick={() => setComposing(true)}>New message</Button>
        </div>
      </div>

      {adoption?.state === "adopted" && adoption.dormant_did ? (
        <div style={{ marginTop: 8, fontSize: 12, color: "#666" }}>
          This app previously created {short(adoption.dormant_did)}; it is now dormant. Active agent: {short(adoption.did)}.
        </div>
      ) : null}

      {!online ? (
        <div style={{ marginTop: 12, padding: "8px 12px", borderRadius: 8, background: "#FFF3F3", color: "#A75D61", fontSize: 13 }}>
          daemon offline — reconnecting. History is read-only.
        </div>
      ) : null}
      {archiveError ? (
        <div style={{ marginTop: 8, padding: "8px 12px", borderRadius: 8, background: "#FFF8EC", color: "#A57C42", fontSize: 13 }}>
          Couldn't read the local archive — showing the live feed only.
        </div>
      ) : null}

      <div style={{ display: "grid", gridTemplateColumns: "220px 1fr", gap: 16, marginTop: 16 }}>
        <ConversationList conversations={conversations} selected={showConv ? selected : null} onSelect={(k) => { setComposing(false); select(k); }} />
        <div>
          {showPane ? (
            <>
              {showConv && selected ? (
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
                  <span style={{ fontSize: 13, fontWeight: 600 }}>{isRoom ? "👥 " : ""}{short(selected)}</span>
                  {!isRoom ? <DialControl did={selected} /> : null}
                </div>
              ) : <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 8 }}>New message</div>}
              {showConv ? <MessageThread items={thread} onRetry={onRetry} /> : null}
              <Composer key={showNew ? "new" : selected} to={showNew ? null : selected} disabled={!online} onSend={handleSend} />
            </>
          ) : (
            <div style={{ color: "#666", fontSize: 13, padding: 12 }}>Select a conversation, or start a new message.</div>
          )}
        </div>
      </div>
    </Card>
  );
}
```

> **Note:** `composing` (local) explicitly overrides selection for new-message mode — no sentinel values. `handleSend` exits compose mode and selects the recipient so the optimistic row is visible immediately.

- [ ] **Step 3: Typecheck** — `cd apps/desktop && npm run typecheck`. Expected: PASS.
- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/inbox/NeedsDaemon.tsx apps/desktop/src/inbox/InboxPanel.tsx
git commit -m "feat(desktop): InboxPanel — gate, banners, new-message, dial (A3 task 12)"
```

---

## Task 13: Wire into the App shell

**Files:** Modify `apps/desktop/src/App.tsx`.

- [ ] **Step 1: Replace `App.tsx`**

```tsx
import { useState } from "react";
import { IdentityProvider, useIdentity } from "./state/identity";
import { OnboardingProvider, useOnboarding } from "./state/onboarding";
import { InboxProvider, useInbox } from "./state/inbox";
import { Welcome } from "./onboarding/Welcome";
import { NameAgent } from "./onboarding/NameAgent";
import { GenerateAndRegister } from "./onboarding/GenerateAndRegister";
import { Done } from "./onboarding/Done";
import { IdentityPanel } from "./identity/IdentityPanel";
import { InboxPanel } from "./inbox/InboxPanel";
import { AirSettings } from "./settings/AirSettings";
import { Button } from "./components/Button";

export default function App() {
  return (
    <IdentityProvider>
      <OnboardingProvider>
        <InboxProvider>
          <Shell />
        </InboxProvider>
      </OnboardingProvider>
    </IdentityProvider>
  );
}

type View = "identity" | "inbox" | "settings";

function Shell() {
  const { identity, loading } = useIdentity();
  const [view, setView] = useState<View>("identity");
  const [onboardingDone, setOnboardingDone] = useState(false);

  if (loading) return <div style={{ padding: "2rem" }}>Loading...</div>;
  if (!identity && !onboardingDone) {
    return <div style={{ padding: "2rem", maxWidth: 600 }}><OnboardingFlow onDone={() => setOnboardingDone(true)} /></div>;
  }
  return (
    <div style={{ padding: "2rem", maxWidth: 760, fontFamily: "system-ui" }}>
      <nav style={{ display: "flex", gap: 8, marginBottom: 16 }}>
        <Button variant={view === "identity" ? "primary" : "secondary"} onClick={() => setView("identity")}>Identity</Button>
        <InboxNavButton active={view === "inbox"} onClick={() => setView("inbox")} />
        <Button variant={view === "settings" ? "primary" : "secondary"} onClick={() => setView("settings")}>Settings</Button>
      </nav>
      {view === "identity" ? <IdentityPanel /> : view === "inbox" ? <InboxPanel /> : <AirSettings />}
    </div>
  );
}

function InboxNavButton({ active, onClick }: { active: boolean; onClick: () => void }) {
  const { totalUnread } = useInbox();
  return (
    <Button variant={active ? "primary" : "secondary"} onClick={onClick}>
      Inbox{totalUnread > 0 ? ` (${totalUnread})` : ""}
    </Button>
  );
}

function OnboardingFlow({ onDone }: { onDone: () => void }) {
  const { state } = useOnboarding();
  switch (state.step) {
    case "welcome": return <Welcome />;
    case "name": return <NameAgent />;
    case "generating":
    case "registering": return <GenerateAndRegister />;
    case "done": return <Done onFinish={onDone} />;
  }
}
```

- [ ] **Step 2: Typecheck + lint + full unit suite**

Run: `cd apps/desktop && npm run typecheck && npm run lint && npm test`
Expected: all PASS (6 pure-logic suites green).

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/App.tsx
git commit -m "feat(desktop): mount Inbox tab in the app shell (A3 task 13)"
```

---

## Task 14: Verification — backend intact + live-daemon QA

No automated UI tests (D1); this is the real-app proof. **Run with Peter's consent** — temp daemon home, never the real `~/.air-msg`.

- [ ] **Step 1: Backend compiles + clippy** — `cd ~/air-note && cargo check -p bossclaw_desktop && cargo clippy -p bossclaw_desktop --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 2: Daemon on a throwaway home**

```bash
export AGENT_BRIDGE_HOME=$(mktemp -d)/air-msg
cd ~/air-note/agent-bridge-mcp
node src/cli.mjs register --name a3-qa
node src/cli.mjs daemon start --detach
node src/cli.mjs daemon status
```

- [ ] **Step 3: Launch the desktop against that home**

```bash
cd ~/air-note/apps/desktop && AGENT_BRIDGE_HOME=$AGENT_BRIDGE_HOME npm run dev
```

- [ ] **Step 4: QA checklist**

- [ ] Inbox tab appears; opens without error. With no daemon home → the **Connect AIR Note** screen shows instead (kill the daemon + clear the home to test).
- [ ] **Cold start (C1):** send yourself a couple of messages via the CLI FIRST, then open the desktop — the conversations appear in the sidebar **immediately on launch** (not blank).
- [ ] **New conversation (D8):** click "New message", paste your own full DID, type + Send → optimistic `sending…` row, resolves to `🔒 ✓` on `inbox_send_ok`; the received copy appears and does **not** duplicate (envelope_id dedupe).
- [ ] **Casing (D5):** the send + history + conversations calls succeed; devtools console is clean (a wrong key would throw).
- [ ] Offline: kill the daemon → "daemon offline" banner, composer disables; restart → banner clears (reconnect; no leaked listeners — check `daemon status` clients count is 1, not climbing).
- [ ] Unread badge increments for a non-selected conversation and clears on select; nav shows `Inbox (n)`.
- [ ] Spam toggle re-queries (flip it; no crash).
- [ ] **Room (M1):** if a room thread exists in the archive, selecting it shows its history (not just live).
- [ ] Dial: set a contact to `auto` → `cat $AGENT_BRIDGE_HOME/agent-policy.json` shows it.
- [ ] **Adoption notice (I1):** on a split-brain home (desktop app-data DID ≠ daemon `identity.json` DID), the "previously created … now dormant" line renders.
- [ ] Terminal send error (malformed DID): row shows the reason, **no Retry button** (retryable:false).

- [ ] **Step 5: Tear down**

```bash
cd ~/air-note/agent-bridge-mcp && node src/cli.mjs daemon stop
rm -rf "$(dirname "$AGENT_BRIDGE_HOME")"
```

- [ ] **Step 6: Push + PR**

```bash
git push -u origin feat/ai-inbox-a3-ui
gh pr create --fill --title "Phase A3: React Inbox UI"
```

---

## §3. Self-review (v2, against the design spec)

**Spec coverage (design §6 + handoff A3 scope):**
- ✅ Inbox view in nav (Task 13) · ✅ **conversation list populated from the archive on launch** (Tasks 8/9, C1) · ✅ group 1:1 by `peer_did`, rooms by `room_id` (`convKey`) · ✅ **room history loads** (Task 2 + Task 9, M1) · ✅ badges 🔒 ✓ (Task 4) · ✅ spam hidden + toggle → `include_spam` · ✅ **composer can start NEW conversations** (Task 11, D8) · ✅ optimistic send + per-row ack + retryable-only retry (Tasks 6/10/11) · ✅ per-contact dial (Task 11) · ✅ session-local unread (Task 7) · ✅ daemon-offline banner + disabled composer + read-only history (Task 12) · ✅ **archive-read-failure warning** (Task 9/12, design §8) · ✅ **needs-daemon/adoption gate** (Task 9/12, design §4) · ✅ no second OS notification (in-app badge only).
- **Out of scope, correctly absent:** AI loop / channel connection (Phase B) · GUI identity creation · AI in rooms · persistent outbox · `contacts.json` picker (needs a new command; raw-DID covers v1, D8).

**Critic findings (v1 → v2):** C1 (empty inbox) FIXED via `inbox_conversations` + bulk `inbox_history` seed + `mergeSidebar`. M1 (room history) FIXED via the `room` param. Missing-recipient FIXED via the composer recipient field. M2 (lifecycle) FIXED: race-safe unlisten + honest "new code" framing + `unread.ts` extracted & tested. Minors fixed (bodyText wording parity, `makeOptimistic` dead code removed, retry body-type guard). Tauri casing (D5) CONFIRMED — no change. **Re-review (v2.1):** APPROVE-WITH-CHANGES — I1 (adoption notice wired to always-null) fixed by forwarding `getIdentity()?.did`; minors m1 (effect deps → derived `selectedIsRoom`), m2 (live-room detection), m3 (Composer remount key), m4 (stable sidebar sort) folded in. No new functional regressions found.

**Placeholder scan:** none. **Type consistency:** `ThreadItem`/`Conversation`/`Autonomy`/`SendState`/`Badge`/`ConversationSummary`/`Adoption` and the fn names (`convKey`/`dedupeById`/`groupConversations`/`fromArchiveRow`/`fromLiveMessage`/`makeOptimistic`/`onSendStart|Ok|Err`/`addUnread`/`clearConv`/`mergeSidebar`) match across definition and call sites.

**Remaining soft spots (acceptable for v1; flag in PR):** (1) sidebar previews are blank for conversations older than the 200-row bulk window until selected — acceptable (summaries still list them newest-first). (2) Retry re-sends as a new envelope rather than re-driving the failed one — simplest correct behavior. (3) Live messages carry `spam:false`; spam state is an archive property, so a live message only gains a spam badge after archive + re-read — matches the daemon gating model.

---

## §4. Execution handoff

Two options:
1. **Subagent-Driven (recommended)** — fresh subagent per task, two-stage review between tasks (`superpowers:subagent-driven-development`).
2. **Inline Execution** — batch with checkpoints (`superpowers:executing-plans`).
