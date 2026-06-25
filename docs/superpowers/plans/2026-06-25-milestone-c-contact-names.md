# Milestone C — Contact Names in AIR Note Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace shortened-DID labels in the AIR Note (inbox) UI with human display names + published `@handles`, make conversations searchable by name/handle, and give the new-message composer a contacts dropdown.

**Architecture:** The data already lives in `~/.air-msg/contacts.json` (per-contact `alias` + registry `name`). Milestone G's published `@handle` is returned by the registry agent-GET but never captured — so the data pipeline grows by one field end-to-end: `agent-bridge-mcp` captures `username` → `air-rs` reads it → a new `inbox_contacts` Tauri command exposes the contact book → React builds a `did → ContactView` map and resolves display names through one shared, unit-tested resolver (`displayName.ts`). No SQL changes; `contacts.json` stays read-only from the desktop. Precedence: **user `alias` → registry `name` → `short(did)`**; `@handle` shows as a secondary line and as the composer's address label.

**Tech Stack:** Node ESM (`agent-bridge-mcp`, `node:test`), Rust (`air-rs` lib + `air_agent_desktop` Tauri commands, `cargo test`), React + TypeScript (`@air-agent/desktop`, `vitest` + `tsc`).

**Source spec:** `docs/superpowers/specs/2026-06-25-air-agent-review-fixes-design.md` § "Milestone C". Scope decision (2026-06-25): **also surface peer `@handle`** (Peter), so Task 1 (agent-bridge-mcp capture) is in scope — the spec's "agent-bridge-mcp needs zero changes" no longer applies.

**Cross-layer data contract (defined once, referenced by every task):**

| Layer | Type / field | Shape |
|---|---|---|
| `contacts.json` record | `username` | `string \| null` — the peer's published `@handle` (lowercase, no `@`) |
| air-rs `Contact` (read) | `name`, `username`, `verified_at_pin` | `Option<String>`, `Option<String>`, `bool` |
| air-rs `ContactView` (emit) | `did, alias, name, username, verified_at_pin` | `String`, `Option<String>×3`, `bool` |
| TS `ContactView` | `did, alias, name, username, verified_at_pin` | `string`, `string\|null ×3`, `boolean` (snake_case — Tauri emits serde field names as-is, like `ConversationSummary`) |

---

## File Structure

**Created:**
- `agent-bridge-mcp/test/contacts.test.mjs` — unit tests for `@handle` capture in `addContact`/`resolveAgent`.
- `apps/desktop/src/inbox/displayName.ts` — the single source of truth for name/handle/label resolution (replaces two duplicated `short()` defs).
- `apps/desktop/src/inbox/displayName.test.ts` — vitest for the resolver.

**Modified:**
- `agent-bridge-mcp/src/contacts.mjs` — capture `rec.username` in `resolveAgent`; persist `username` in `addContact`.
- `crates/air-rs/src/inbox/stores.rs` — extend `Contact`; add `ContactView` + `list_contacts`.
- `crates/air-rs/tests/inbox_stores.rs` — tests for `list_contacts`.
- `apps/desktop/src-tauri/src/commands/inbox.rs` — new `inbox_contacts` command.
- `apps/desktop/src-tauri/src/main.rs` — register `inbox_contacts`.
- `apps/desktop/src/api/inbox.ts` — `ContactView` type + `inboxContacts()` wrapper.
- `apps/desktop/src/state/inbox.tsx` — load contacts; expose `contacts: Map<string, ContactView>`.
- `apps/desktop/src/inbox/ConversationList.tsx` — resolve names; drop local `short`.
- `apps/desktop/src/inbox/InboxPanel.tsx` — pass contacts down; resolve thread-head name; drop local `short`.
- `apps/desktop/src/inbox/Composer.tsx` — recipient dropdown + free-text fallback.
- `apps/desktop/src/search/filterConversations.ts` (+ `.test.ts`) — match on name/handle; title = display name.
- `apps/desktop/src/search/globalSearch.ts` — thread `contacts` through `GlobalSearchDeps`.
- `apps/desktop/src/search/CommandPalette.tsx` — read `contacts` from `useInbox`, pass via a ref (infinite-loop guard).

**Rooms stay rooms:** every resolver is gated on `kind === "peer"`; rooms keep their `room_id` + the 👥 prefix (a room is not a person).

---

## Task 1: Capture peer `@handle` into the contact book (agent-bridge-mcp)

**Files:**
- Modify: `agent-bridge-mcp/src/contacts.mjs` (`resolveAgent` ~lines 78-102; `addContact` ~lines 119-131)
- Test: `agent-bridge-mcp/test/contacts.test.mjs` (create)

**Why this matters:** the registry returns `username` on `GET /api/v1/agents/{id}` (`agent-identity-registry api/src/index.js:814`), but `resolveAgent` only reads `name`/`verified`/`trust_score`. One field, captured at pin time. On re-pin we preserve a known handle if the registry momentarily returns null (`resolved.username ?? prior?.username ?? null`) so a transient metadata hiccup never wipes a good handle.

- [ ] **Step 1: Write the failing test**

Create `agent-bridge-mcp/test/contacts.test.mjs`:

```js
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { addContact, getContactByDid } from "../src/contacts.mjs";
import { generateIdentity, pubKeyMultibase } from "../src/crypto.mjs";

const PEER_SEED = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6f9"; // RFC 8032
const PEER_DID = "did:wba:agentidentityregistry.org:agents:AIR-PEER";
const AIR_URL = "http://air.test";

let dir, realFetch, peer;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "air-msg-contacts-"));
  process.env.AGENT_BRIDGE_HOME = dir;
  realFetch = global.fetch;
  peer = generateIdentity(PEER_SEED);
});
afterEach(() => {
  global.fetch = realFetch;
  rmSync(dir, { recursive: true, force: true });
  delete process.env.AGENT_BRIDGE_HOME;
});

/** Stub the two GETs resolveAgent makes: the DID document (key) + the agent record (metadata). */
function stubFetch({ username }) {
  global.fetch = async (url) => {
    const u = String(url);
    if (u.includes("/did-document")) {
      return { ok: true, json: async () => ({ id: PEER_DID, verificationMethod: [{ publicKeyMultibase: pubKeyMultibase(peer.rawPublicKey) }] }) };
    }
    if (u.endsWith("/agents/AIR-PEER")) {
      return { ok: true, json: async () => ({ name: "Kenny", username, verification_status: { verified: false } }) };
    }
    throw new Error(`unexpected fetch: ${u}`);
  };
}

test("addContact captures the peer's published @handle (username)", async () => {
  stubFetch({ username: "kenny" });
  await addContact(AIR_URL, { to: PEER_DID });
  const c = getContactByDid(PEER_DID);
  assert.equal(c.username, "kenny");
  assert.equal(c.name, "Kenny");
});

test("a missing handle is stored as null", async () => {
  stubFetch({ username: undefined });
  await addContact(AIR_URL, { to: PEER_DID });
  assert.equal(getContactByDid(PEER_DID).username, null);
});

test("re-pin preserves a known handle when the registry returns null", async () => {
  stubFetch({ username: "kenny" });
  await addContact(AIR_URL, { to: PEER_DID });
  stubFetch({ username: null });            // metadata hiccup on re-pin
  await addContact(AIR_URL, { to: PEER_DID });
  assert.equal(getContactByDid(PEER_DID).username, "kenny"); // preserved, not wiped
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ~/air-note/agent-bridge-mcp && node --test test/contacts.test.mjs`
Expected: FAIL — `c.username` is `undefined` (the field is not yet captured/persisted).

- [ ] **Step 3: Write minimal implementation**

In `agent-bridge-mcp/src/contacts.mjs`, `resolveAgent` — add the `username` local + capture + return field. The metadata block becomes:

```js
  // Metadata via agent record (best-effort — key resolution already succeeded).
  let name = null;
  let verified = false;
  let trust_score = null;
  let username = null; // the peer's published @handle (Milestone G), if claimed
  try {
    const recResp = await fetch(`${airUrl}/api/v1/agents/${airId}`);
    if (recResp.ok) {
      const rec = await recResp.json();
      name = rec.name ?? null;
      verified = rec.verification_status?.verified ?? rec.verified ?? false;
      trust_score = rec.trust_score ?? null;
      username = rec.username ?? null;
    }
  } catch {
    /* metadata optional */
  }

  return {
    air_id: airId,
    did,
    rawPublicKey,
    publicKeyMultibase: vm.publicKeyMultibase,
    fingerprint: fingerprintOf(rawPublicKey),
    name,
    verified,
    trust_score,
    username,
  };
```

In `addContact`, persist it in the stored record (add the line after `name: resolved.name,`):

```js
    name: resolved.name,
    username: resolved.username ?? prior?.username ?? null,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ~/air-note/agent-bridge-mcp && node --test test/contacts.test.mjs`
Expected: PASS (3/3).

- [ ] **Step 5: Run the full agent-bridge-mcp suite (no regressions)**

Run: `cd ~/air-note/agent-bridge-mcp && node --test`
Expected: all existing tests still pass (the suite requires temp homes — already handled per-test). Note: `node --test test/` is broken on Node 25; bare `node --test` is correct.

- [ ] **Step 6: Commit**

```bash
cd ~/air-note
git add agent-bridge-mcp/src/contacts.mjs agent-bridge-mcp/test/contacts.test.mjs
git commit -m "feat(contacts): capture peer @handle (username) at pin time (Milestone C)"
```

---

## Task 2: Read contacts as a list in air-rs + expose via `inbox_contacts` (Rust)

**Files:**
- Modify: `crates/air-rs/src/inbox/stores.rs:3` (imports), `:38-49` (`Contact`), add `ContactView` + `list_contacts`
- Test: `crates/air-rs/tests/inbox_stores.rs` (glob-imports `stores::*` — no import edit needed)
- Modify: `apps/desktop/src-tauri/src/commands/inbox.rs` (add `inbox_contacts` after `inbox_conversations`, ~line 126)
- Modify: `apps/desktop/src-tauri/src/main.rs` (register in `generate_handler!`, near `inbox_conversations`)

- [ ] **Step 1: Write the failing test**

Append to `crates/air-rs/tests/inbox_stores.rs`:

```rust
#[test]
fn list_contacts_returns_all_with_did_keys_and_registry_fields() {
    let h = home();
    fs::write(
        h.path().join("contacts.json"),
        r#"{"version":1,"contacts":{
            "did:wba:x:agents:AIR-1":{"alias":"kenny","air_id":"AIR-1","name":"Kenny","username":"kenny","public_key_multibase":"zA","verified_at_pin":true},
            "did:wba:x:agents:AIR-2":{"air_id":"AIR-2","public_key_multibase":"zB"}
        }}"#,
    )
    .unwrap();
    let got = list_contacts(h.path()); // sorted by did for determinism
    assert_eq!(got.len(), 2);

    assert_eq!(got[0].did, "did:wba:x:agents:AIR-1");
    assert_eq!(got[0].alias.as_deref(), Some("kenny"));
    assert_eq!(got[0].name.as_deref(), Some("Kenny"));
    assert_eq!(got[0].username.as_deref(), Some("kenny"));
    assert!(got[0].verified_at_pin);

    // minimal record: registry fields absent → None; verified defaults to false
    assert_eq!(got[1].did, "did:wba:x:agents:AIR-2");
    assert_eq!(got[1].alias, None);
    assert_eq!(got[1].name, None);
    assert_eq!(got[1].username, None);
    assert!(!got[1].verified_at_pin);
}

#[test]
fn list_contacts_is_empty_on_missing_or_corrupt_file() {
    let h = home();
    assert!(list_contacts(h.path()).is_empty()); // missing
    fs::write(h.path().join("contacts.json"), "{ not json").unwrap();
    assert!(list_contacts(h.path()).is_empty()); // corrupt → fail-soft
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ~/air-note && cargo test -p air-rs --test inbox_stores list_contacts`
Expected: FAIL to COMPILE — `list_contacts`/`ContactView` not found.

- [ ] **Step 3: Write minimal implementation**

In `crates/air-rs/src/inbox/stores.rs`, change the import on line 3:

```rust
use serde::{Deserialize, Serialize};
```

Extend `Contact` (lines 38-49) — add three `#[serde(default)]` fields:

```rust
/// A pinned contact (only the fields A2 consumes; serde ignores the rest).
#[derive(Debug, Clone, Deserialize)]
pub struct Contact {
    /// Display alias for the contact; truthy presence means "pinned".
    #[serde(default)]
    pub alias: Option<String>,
    /// The contact's AIR id, if recorded.
    #[serde(default)]
    pub air_id: Option<String>,
    /// The contact's pinned public key (multibase), if recorded.
    #[serde(default)]
    pub public_key_multibase: Option<String>,
    /// The contact's registry display name (spoofable; the pin is the real guarantee).
    #[serde(default)]
    pub name: Option<String>,
    /// The contact's published, unique @handle (Milestone G), if claimed.
    #[serde(default)]
    pub username: Option<String>,
    /// Whether the registry reported the contact verified at pin time.
    #[serde(default)]
    pub verified_at_pin: bool,
}
```

Add `ContactView` + `list_contacts` immediately after `get_contact_by_did` (after line 62):

```rust
/// A contact paired with its DID key, for the desktop's did→display-name join.
/// Serialized to the frontend; field names cross as-is (snake_case), like `ConversationSummary`.
#[derive(Debug, Clone, Serialize)]
pub struct ContactView {
    /// The contact's DID (the `contacts.json` map key) — the join key the desktop needs.
    pub did: String,
    pub alias: Option<String>,
    pub name: Option<String>,
    pub username: Option<String>,
    pub verified_at_pin: bool,
}

/// All pinned contacts with their DID keys (ports `listContacts`, but KEEPS the map key,
/// which `Object.values` drops). Sorted by DID for a deterministic UI + tests. Returns an
/// empty Vec on any read/parse error — fail-soft, like `get_contact_by_did`.
pub fn list_contacts(home: &Path) -> Vec<ContactView> {
    (|| -> Option<Vec<ContactView>> {
        let raw = std::fs::read_to_string(home.join("contacts.json")).ok()?;
        let file: ContactsFile = serde_json::from_str(&raw).ok()?;
        let mut out: Vec<ContactView> = file
            .contacts
            .into_iter()
            .map(|(did, c)| ContactView {
                did,
                alias: c.alias,
                name: c.name,
                username: c.username,
                verified_at_pin: c.verified_at_pin,
            })
            .collect();
        out.sort_by(|a, b| a.did.cmp(&b.did));
        Some(out)
    })()
    .unwrap_or_default()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ~/air-note && cargo test -p air-rs --test inbox_stores`
Expected: PASS (existing + 2 new).

- [ ] **Step 5: Add the `inbox_contacts` Tauri command**

In `apps/desktop/src-tauri/src/commands/inbox.rs`, add after `inbox_conversations` (after line 126). All names used (`bridge_home`, `json`, `Value`, `spawn_blocking`) are already imported; `air_rs::inbox::...` is reachable fully-qualified:

```rust
/// All pinned contacts with their DID keys, for the §6 did→display-name join (Milestone C).
/// Mirrors `inbox_conversations`: no args, guards on file existence, reads on a blocking task.
#[tauri::command]
pub async fn inbox_contacts() -> Result<Value, String> {
    let home = bridge_home();
    if !home.join("contacts.json").exists() {
        return Ok(json!([]));
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let contacts = air_rs::inbox::stores::list_contacts(&home);
        serde_json::to_value(contacts).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
```

- [ ] **Step 6: Register the command**

In `apps/desktop/src-tauri/src/main.rs`, find the `tauri::generate_handler![` list and add this line directly after the `commands::inbox::inbox_conversations,` entry:

```rust
            commands::inbox::inbox_contacts,
```

- [ ] **Step 7: Verify the desktop backend compiles + clippy clean**

Run: `cd ~/air-note && cargo test -p air-rs && cargo check -p air_agent_desktop && cargo clippy -p air-rs -p air_agent_desktop --all-targets -- -D warnings`
Expected: PASS, no warnings. (`air_agent_desktop` needs `apps/desktop/dist/` to exist for `generate_context!`; if `cargo check` errors on a missing dist, run `npm run build --workspace @air-agent/desktop` once, or rely on the existing `dist/`.)

- [ ] **Step 8: Commit**

```bash
cd ~/air-note
git add crates/air-rs/src/inbox/stores.rs crates/air-rs/tests/inbox_stores.rs \
        apps/desktop/src-tauri/src/commands/inbox.rs apps/desktop/src-tauri/src/main.rs
git commit -m "feat(inbox): air-rs list_contacts + inbox_contacts command (Milestone C)"
```

---

## Task 3: Frontend foundation — types, wrapper, resolver, state map (TS)

**Files:**
- Create: `apps/desktop/src/inbox/displayName.ts`
- Create: `apps/desktop/src/inbox/displayName.test.ts`
- Modify: `apps/desktop/src/api/inbox.ts` (add type + wrapper after `ConversationSummary`/`inboxConversations`)
- Modify: `apps/desktop/src/state/inbox.tsx` (load contacts, expose on context)

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/src/inbox/displayName.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { shortDid, displayName, handleOf, contactsByDid, conversationLabel } from "./displayName";
import type { ContactView } from "../api/inbox";

const DID = "did:wba:agentidentityregistry.org:agents:AIR-3C33-M64E-KQKJ";
const c = (over: Partial<ContactView> = {}): ContactView => ({
  did: DID, alias: null, name: null, username: null, verified_at_pin: false, ...over,
});

describe("shortDid", () => {
  it("strips the did:wba prefix down to the AIR id", () => {
    expect(shortDid(DID)).toBe("AIR-3C33-M64E-KQKJ");
  });
  it("passes through a value with no prefix", () => {
    expect(shortDid("room-123")).toBe("room-123");
  });
});

describe("displayName precedence: alias → name → short(did)", () => {
  it("prefers the alias", () => {
    expect(displayName(DID, c({ alias: "kenny", name: "Kenny" }))).toBe("kenny");
  });
  it("falls back to the registry name", () => {
    expect(displayName(DID, c({ name: "Kenny" }))).toBe("Kenny");
  });
  it("falls back to short(did) with no contact", () => {
    expect(displayName(DID, undefined)).toBe("AIR-3C33-M64E-KQKJ");
  });
  it("ignores whitespace-only alias/name", () => {
    expect(displayName(DID, c({ alias: "   ", name: "  " }))).toBe("AIR-3C33-M64E-KQKJ");
  });
});

describe("handleOf", () => {
  it("prefixes a claimed handle with @", () => {
    expect(handleOf(c({ username: "kenny" }))).toBe("@kenny");
  });
  it("returns null when unclaimed", () => {
    expect(handleOf(c())).toBeNull();
    expect(handleOf(undefined)).toBeNull();
  });
});

describe("contactsByDid", () => {
  it("indexes by did", () => {
    const m = contactsByDid([c({ alias: "kenny" })]);
    expect(m.get(DID)?.alias).toBe("kenny");
  });
});

describe("conversationLabel", () => {
  it("resolves a peer to name + handle", () => {
    expect(conversationLabel(DID, "peer", c({ alias: "kenny", username: "kenny" })))
      .toEqual({ label: "kenny", handle: "@kenny" });
  });
  it("passes a room id through unchanged with no handle", () => {
    expect(conversationLabel("room-abc", "room", undefined))
      .toEqual({ label: "room-abc", handle: null });
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ~/air-note/apps/desktop && npx vitest run src/inbox/displayName.test.ts`
Expected: FAIL — cannot resolve `./displayName`.

- [ ] **Step 3: Write the resolver**

Create `apps/desktop/src/inbox/displayName.ts`:

```ts
import type { ContactView } from "../api/inbox";

/** Strip a `did:wba:<domain>:agents:` prefix down to the bare AIR id (the historical `short()`). */
export function shortDid(did: string): string {
  return did.replace(/^did:wba:[^:]+:agents:/, "");
}

/**
 * Resolve a peer DID to a human display name.
 * Precedence: user `alias` → registry `name` → `short(did)`.
 * Rooms are NOT people — callers gate on `kind === "peer"` (see `conversationLabel`).
 */
export function displayName(did: string, contact?: ContactView): string {
  const alias = contact?.alias?.trim();
  const name = contact?.name?.trim();
  return alias || name || shortDid(did);
}

/** The published `@handle` for a contact (with leading `@`), or null when unclaimed. */
export function handleOf(contact?: ContactView): string | null {
  const u = contact?.username?.trim();
  return u ? `@${u}` : null;
}

/** Index a contacts payload by DID for O(1) lookups during rendering/search. */
export function contactsByDid(contacts: ContactView[]): Map<string, ContactView> {
  return new Map(contacts.map((cv) => [cv.did, cv]));
}

/**
 * The label + optional handle for a conversation row / thread head / search title.
 * Peers resolve through `displayName`/`handleOf`; rooms keep their id and carry no handle.
 */
export function conversationLabel(
  convKey: string,
  kind: "room" | "peer",
  contact?: ContactView,
): { label: string; handle: string | null } {
  if (kind === "room") return { label: convKey, handle: null };
  return { label: displayName(convKey, contact), handle: handleOf(contact) };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ~/air-note/apps/desktop && npx vitest run src/inbox/displayName.test.ts`
Expected: PASS.

- [ ] **Step 5: Add the `ContactView` type + `inboxContacts` wrapper**

In `apps/desktop/src/api/inbox.ts`, after the `ConversationSummary` type (line 33) add:

```ts
/** A pinned contact + its DID key (mirrors air-rs `ContactView`; snake_case crosses as-is). */
export type ContactView = {
  did: string;
  alias: string | null;
  name: string | null;
  username: string | null;
  verified_at_pin: boolean;
};
```

After the `inboxConversations` wrapper (line 54) add:

```ts
export const inboxContacts = () => invoke<ContactView[]>("inbox_contacts");
```

- [ ] **Step 6: Load contacts into inbox state**

In `apps/desktop/src/state/inbox.tsx`:

(a) extend the api import (line 3-5) to include the new wrapper + type, and import the indexer:

```ts
import {
  inboxStart, inboxStop, inboxSend, inboxHistory, inboxConversations, inboxContacts, inboxStatus, inboxIdentity,
  onInboxEvent, type InboxMessage, type Adoption, type ConversationSummary, type ContactView,
} from "../api/inbox";
```

Add to the existing model import or a new import line:

```ts
import { contactsByDid } from "../inbox/displayName";
```

(b) add `contacts` to the `InboxCtx` type (after `conversations: Conversation[];`):

```ts
  contacts: Map<string, ContactView>;
```

(c) add state (next to the other `useState`s, ~line 38):

```ts
  const [contacts, setContacts] = useState<Map<string, ContactView>>(new Map());
```

(d) add a load effect after the C1 effect (after line 104). Contacts don't depend on the spam toggle, so load once when ready:

```ts
  // Load the contact book once ready, for did→display-name resolution (Milestone C).
  useEffect(() => {
    if (gate !== "ready") return;
    let alive = true;
    inboxContacts().then((cs) => { if (alive) setContacts(contactsByDid(cs)); }).catch(() => {});
    return () => { alive = false; };
  }, [gate]);
```

(e) add `contacts` to the provider value (the `Ctx.Provider value={{ … }}` object, ~line 165):

```ts
      gate, adoption, online, archiveError, conversations, contacts, selected, thread, includeSpam, totalUnread,
      select, setIncludeSpam, send,
```

- [ ] **Step 7: Typecheck + run the inbox/search vitest**

Run: `cd ~/air-note/apps/desktop && npm run typecheck && npx vitest run src/inbox src/search`
Expected: PASS (typecheck clean; displayName tests green; existing inbox/search tests unaffected).

- [ ] **Step 8: Commit**

```bash
cd ~/air-note
git add apps/desktop/src/inbox/displayName.ts apps/desktop/src/inbox/displayName.test.ts \
        apps/desktop/src/api/inbox.ts apps/desktop/src/state/inbox.tsx
git commit -m "feat(inbox): contact display-name resolver + contacts state (Milestone C)"
```

---

## Task 4: Inbox UI shows names + @handle (ConversationList, InboxPanel, Composer)

**Files:**
- Modify: `apps/desktop/src/inbox/ConversationList.tsx`
- Modify: `apps/desktop/src/inbox/InboxPanel.tsx`
- Modify: `apps/desktop/src/inbox/Composer.tsx`

**Note on testing:** the codebase has **no render tests** for these components (convention: pure logic is extracted + tested — done in Task 3). This task is wiring already-tested resolvers; it is gated by `npm run typecheck` and the manual GUI QA at the end of the milestone. Do not invent a render-test harness.

- [ ] **Step 1: Resolve names in `ConversationList`**

Replace the whole of `apps/desktop/src/inbox/ConversationList.tsx` with (drops the local `short`, adds the `contacts` prop, renders label + handle + a verified tick):

```tsx
import { StatusBadge } from "../components/ui/StatusBadge";
import type { Conversation } from "./model";
import type { ContactView } from "../api/inbox";
import { conversationLabel } from "./displayName";

export function ConversationList({
  conversations, contacts, selected, onSelect,
}: {
  conversations: Conversation[];
  contacts: Map<string, ContactView>;
  selected: string | null;
  onSelect: (k: string) => void;
}) {
  if (conversations.length === 0) {
    return <div style={{ color: "var(--text-secondary)", fontSize: 13, padding: 12 }}>No conversations yet.</div>;
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      {conversations.map((c) => {
        const contact = c.kind === "peer" ? contacts.get(c.convKey) : undefined;
        const { label, handle } = conversationLabel(c.convKey, c.kind, contact);
        return (
          <button key={c.convKey} onClick={() => onSelect(c.convKey)}
            style={{
              textAlign: "left", padding: "8px 10px", borderRadius: 8, cursor: "pointer",
              border: "1px solid " + (c.convKey === selected ? "color-mix(in srgb, var(--primary) 26%, transparent)" : "var(--border-soft)"),
              background: c.convKey === selected ? "color-mix(in srgb, var(--primary) 10%, var(--surface))" : "var(--surface)",
            }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 8 }}>
              <span style={{ fontSize: 13, fontWeight: 600, display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
                <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                  {c.kind === "room" ? "👥 " : ""}{label}
                </span>
                {contact?.verified_at_pin ? (
                  <span title="Verified on AIR" aria-label="Verified" style={{ color: "var(--primary)", fontSize: 11 }}>✓</span>
                ) : null}
                {handle ? (
                  <span style={{ color: "var(--text-secondary)", fontWeight: 400, fontSize: 11 }}>{handle}</span>
                ) : null}
              </span>
              {c.unread > 0 ? <StatusBadge tone="primary">{c.unread}</StatusBadge> : null}
            </div>
            <div style={{ fontSize: 12, color: "var(--text-secondary)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {c.lastText}
            </div>
          </button>
        );
      })}
    </div>
  );
}
```

- [ ] **Step 2: Resolve the thread head + pass contacts down in `InboxPanel`**

In `apps/desktop/src/inbox/InboxPanel.tsx`:

(a) replace the local `short` def (line 14) with shared imports:

```tsx
import { shortDid, conversationLabel } from "./displayName";
import { useMemo } from "react";
```
(Merge `useMemo` into the existing `import { useEffect, useRef, useState } from "react";` line rather than adding a second react import.)

(b) destructure `contacts` from `useInbox()` (line 17) — add it to the existing list:

```tsx
  const { gate, adoption, online, archiveError, conversations, contacts, selected, thread, includeSpam, select, setIncludeSpam, send } = useInbox();
```

(c) the dormant/active banner (lines 53-54) shows the user's OWN DIDs — keep `shortDid` there (NOT a contact):

```tsx
          This app previously created {shortDid(adoption.dormant_did)}; it is now dormant. Active agent: {shortDid(adoption.did)}.
```

(d) pass `contacts` to `ConversationList` (line 71):

```tsx
          <ConversationList conversations={conversations} contacts={contacts} selected={showConv ? selected : null} onSelect={(k) => { setComposing(false); select(k); }} />
```

(e) resolve the thread head (lines 76-80). Compute the label from the selected conversation's kind, then render label + handle:

```tsx
              {showConv && selected ? (
                (() => {
                  const headContact = !isRoom ? contacts.get(selected) : undefined;
                  const { label, handle } = conversationLabel(selected, isRoom ? "room" : "peer", headContact);
                  return (
                    <div className="inbox-thread-head">
                      <span style={{ fontSize: 13, fontWeight: 600, display: "flex", alignItems: "center", gap: 6 }}>
                        <span>{isRoom ? "👥 " : ""}{label}</span>
                        {handle ? <span style={{ color: "var(--text-secondary)", fontWeight: 400, fontSize: 12 }}>{handle}</span> : null}
                      </span>
                      {!isRoom ? <DialControl did={selected} /> : null}
                    </div>
                  );
                })()
              ) : <div className="inbox-thread-head"><span style={{ fontSize: 13, fontWeight: 600 }}>New Message</span></div>}
```

(f) pass the contact list to the `Composer` (line 87). Add a memoized list near the top of the component body (after the `useInbox()` destructure):

```tsx
  const contactList = useMemo(() => [...contacts.values()], [contacts]);
```

and update the Composer element:

```tsx
              <Composer key={showNew ? "new" : selected} to={showNew ? null : selected} contacts={contactList} disabled={!online} onSend={handleSend} />
```

- [ ] **Step 3: Add the recipient dropdown to `Composer`**

Replace the whole of `apps/desktop/src/inbox/Composer.tsx` (adds the `contacts` prop + a native combobox in the `!to` branch; free-text DID stays as the fallback). When a real conversation is open (`to` set) nothing changes:

```tsx
import { useState } from "react";
import { Input } from "../components/Input";
import { Button } from "../components/Button";
import type { ContactView } from "../api/inbox";
import { displayName, handleOf } from "./displayName";

/** When `to` is null the composer shows a recipient picker (contacts dropdown + free-text DID) so a
 *  NEW conversation can start. When `to` is set (a conversation is open) it sends to that peer. */
export function Composer({ to, contacts, disabled, onSend }: {
  to: string | null;
  contacts: ContactView[];
  disabled: boolean;
  onSend: (to: string, text: string) => void;
}) {
  const [recipient, setRecipient] = useState("");
  const [text, setText] = useState("");
  const target = to ?? recipient.trim();
  const canSend = !disabled && !!target && !!text.trim();
  const known = contacts.some((c) => c.did === recipient);

  const submit = () => {
    if (!canSend) return;
    onSend(target, text.trim());
    setText("");
    if (!to) setRecipient("");
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 12 }}>
      {!to ? (
        <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
          {contacts.length > 0 ? (
            <select
              aria-label="Recipient contact"
              value={known ? recipient : ""}
              disabled={disabled}
              onChange={(e) => setRecipient(e.target.value)}
              style={{ padding: "8px 10px", borderRadius: 8, border: "1px solid var(--border-soft)", background: "var(--surface)", color: "var(--text-primary)", fontSize: 13 }}
            >
              <option value="">Choose a contact…  (or type a DID below)</option>
              {contacts.map((c) => {
                const h = handleOf(c);
                return (
                  <option key={c.did} value={c.did}>
                    {displayName(c.did, c)}{h ? ` (${h})` : ""}
                  </option>
                );
              })}
            </select>
          ) : null}
          <Input value={recipient} placeholder="Recipient DID (did:wba:…)" disabled={disabled}
            onChange={(e) => setRecipient(e.target.value)} />
        </div>
      ) : null}
      <div style={{ display: "flex", gap: 8 }}>
        <Input value={text} placeholder={disabled ? "Agent offline" : "Message…"} disabled={disabled}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }} />
        <Button variant="primary" disabled={!canSend} onClick={submit}>Send</Button>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Typecheck + full vitest (no regressions)**

Run: `cd ~/air-note/apps/desktop && npm run typecheck && npx vitest run`
Expected: typecheck clean; all existing tests pass (no component test references the changed props).

- [ ] **Step 5: Commit**

```bash
cd ~/air-note
git add apps/desktop/src/inbox/ConversationList.tsx apps/desktop/src/inbox/InboxPanel.tsx apps/desktop/src/inbox/Composer.tsx
git commit -m "feat(inbox): show contact names + @handle in list, thread head, composer (Milestone C)"
```

---

## Task 5: Search by name + handle (filterConversations, globalSearch, CommandPalette)

**Files:**
- Modify: `apps/desktop/src/search/filterConversations.ts`
- Modify: `apps/desktop/src/search/filterConversations.test.ts`
- Modify: `apps/desktop/src/search/globalSearch.ts`
- Modify: `apps/desktop/src/search/CommandPalette.tsx`

- [ ] **Step 1: Write the failing test**

Replace `apps/desktop/src/search/filterConversations.test.ts` with (keeps the existing cases; adds name/handle matching + a contacts map; updates the cap call to the new arg order):

```ts
import { describe, it, expect } from "vitest";
import { filterConversations } from "./filterConversations";
import type { Conversation } from "../inbox/model";
import type { ContactView } from "../api/inbox";

const conv = (convKey: string, lastText: string, kind: "peer" | "room" = "peer"): Conversation => ({
  convKey, kind, lastTimestamp: "2026-06-24T00:00:00Z", lastText, unread: 0,
});
const cv = (did: string, over: Partial<ContactView> = {}): ContactView => ({
  did, alias: null, name: null, username: null, verified_at_pin: false, ...over,
});

describe("filterConversations", () => {
  const convs = [
    conv("did:key:alice", "lunch tomorrow?"),
    conv("did:key:bob", "shipping the redesign"),
  ];

  it("returns [] for an empty query", () => {
    expect(filterConversations(convs, "  ")).toEqual([]);
  });

  it("matches case-insensitively on convKey and lastText", () => {
    expect(filterConversations(convs, "ALICE").map((r) => r.id)).toEqual(["conv:did:key:alice"]);
    expect(filterConversations(convs, "redesign").map((r) => r.id)).toEqual(["conv:did:key:bob"]);
  });

  it("matches on the resolved contact name and titles the result with it", () => {
    const contacts = new Map([["did:key:bob", cv("did:key:bob", { alias: "Bob Loblaw" })]]);
    const [r] = filterConversations(convs, "loblaw", contacts);
    expect(r).toMatchObject({ id: "conv:did:key:bob", title: "Bob Loblaw" });
  });

  it("matches on the @handle", () => {
    const contacts = new Map([["did:key:alice", cv("did:key:alice", { username: "alice_a" })]]);
    expect(filterConversations(convs, "@alice_a", contacts).map((r) => r.id)).toEqual(["conv:did:key:alice"]);
  });

  it("titles a result with short(did) when no contact is known", () => {
    const [r] = filterConversations(convs, "alice");
    expect(r).toMatchObject({
      kind: "conversation",
      title: "did:key:alice",
      snippet: "lunch tomorrow?",
      target: { view: "inbox", convKey: "did:key:alice" },
    });
  });

  it("caps the number of results", () => {
    const many = Array.from({ length: 10 }, (_, i) => conv(`did:key:p${i}`, "hello"));
    expect(filterConversations(many, "hello", undefined, 3)).toHaveLength(3);
  });
});
```

(Note: `title` for `did:key:alice` is `short(did)` = the value itself, since it has no `did:wba:…:agents:` prefix — unchanged from the original test.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ~/air-note/apps/desktop && npx vitest run src/search/filterConversations.test.ts`
Expected: FAIL — the new `contacts` arg/handle/name matches aren't implemented; `title` still equals `convKey`.

- [ ] **Step 3: Implement name/handle matching**

Replace `apps/desktop/src/search/filterConversations.ts` with (adds an optional `contacts` map as the 3rd param, `cap` becomes 4th; resolves a peer's display name + handle for both the filter predicate and the result title):

```ts
import type { Conversation } from "../inbox/model";
import type { ContactView } from "../api/inbox";
import { conversationLabel } from "../inbox/displayName";
import { type SearchResult, RESULTS_PER_GROUP } from "./types";

/** Pure client-side filter over already-loaded conversation summaries (name/handle + convKey + preview). */
export function filterConversations(
  convs: Conversation[],
  query: string,
  contacts?: Map<string, ContactView>,
  cap = RESULTS_PER_GROUP,
): SearchResult[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  return convs
    .filter((c) => {
      const contact = c.kind === "peer" ? contacts?.get(c.convKey) : undefined;
      const { label, handle } = conversationLabel(c.convKey, c.kind, contact);
      return (
        c.convKey.toLowerCase().includes(q) ||
        c.lastText.toLowerCase().includes(q) ||
        label.toLowerCase().includes(q) ||
        (handle ?? "").toLowerCase().includes(q)
      );
    })
    .slice(0, cap)
    .map((c) => {
      const contact = c.kind === "peer" ? contacts?.get(c.convKey) : undefined;
      const { label } = conversationLabel(c.convKey, c.kind, contact);
      return {
        id: `conv:${c.convKey}`,
        kind: "conversation" as const,
        title: label,
        snippet: c.lastText,
        target: { view: "inbox" as const, convKey: c.convKey },
      };
    });
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ~/air-note/apps/desktop && npx vitest run src/search/filterConversations.test.ts`
Expected: PASS.

- [ ] **Step 5: Thread `contacts` through the search façade**

In `apps/desktop/src/search/globalSearch.ts`:

(a) import the type at the top:

```ts
import type { ContactView } from "../api/inbox";
```

(b) add `contacts` to `GlobalSearchDeps`:

```ts
export type GlobalSearchDeps = {
  recall: (q: string, k: number) => Promise<HitDto[]>;
  listFiles: () => Promise<FileRecordDto[]>;
  conversations: Conversation[];
  contacts: Map<string, ContactView>;
};
```

(c) pass it into the filter (line 27 region):

```ts
    conversations: filterConversations(deps.conversations, q, deps.contacts),
```

(d) extend `defaultSearchDeps`:

```ts
export const defaultSearchDeps = (
  conversations: Conversation[],
  contacts: Map<string, ContactView>,
): GlobalSearchDeps => ({
  recall: recallOp,
  listFiles: listFilesOp,
  conversations,
  contacts,
});
```

- [ ] **Step 6: Supply `contacts` from `CommandPalette` (with the infinite-loop guard)**

In `apps/desktop/src/search/CommandPalette.tsx`:

(a) pull `contacts` from `useInbox` (line 25):

```tsx
  const { conversations, contacts } = useInbox();
```

(b) add a ref next to `conversationsRef` (after line 33) — same reason: a fresh Map identity each render must not be a search-effect dependency (the documented infinite-render trap):

```tsx
  const contactsRef = useRef(contacts);
  contactsRef.current = contacts;
```

(c) pass it at debounce-fire time (line 53):

```tsx
      search(q, defaultSearchDeps(conversationsRef.current, contactsRef.current)).then((results) => dispatch({ type: "setResults", results }));
```

- [ ] **Step 7: Typecheck + full vitest**

Run: `cd ~/air-note/apps/desktop && npm run typecheck && npx vitest run`
Expected: typecheck clean; all tests pass.

- [ ] **Step 8: Commit**

```bash
cd ~/air-note
git add apps/desktop/src/search/filterConversations.ts apps/desktop/src/search/filterConversations.test.ts \
        apps/desktop/src/search/globalSearch.ts apps/desktop/src/search/CommandPalette.tsx
git commit -m "feat(search): match conversations by contact name + @handle (Milestone C)"
```

---

## Final verification (whole-milestone, before PR)

- [ ] **All gates green:**

```bash
cd ~/air-note
cargo test -p air-rs
cargo clippy -p air-rs -p air_agent_desktop --all-targets -- -D warnings
( cd agent-bridge-mcp && node --test )
( cd apps/desktop && npm run typecheck && npx vitest run )
```

- [ ] **Manual GUI QA** (native WKWebView can't attach to chrome-devtools — use the `npm run dev:web` + `mockIPC` web-preview technique from the 2026-06-25 lessons, seeding `inbox_contacts` with the live `contacts.json` fixture: "me" + Kenny). Verify:
  - Conversation list shows **kenny** (alias) not `AIR-3C33-…`; an unknown peer shows `short(did)`.
  - A contact with a claimed `@handle` shows it as a muted secondary label; rooms keep `👥 <room_id>`.
  - Thread head matches the list label; `DialControl` still renders for peers, not rooms.
  - ⌘K search: typing the name or `@handle` finds the conversation; the result title is the name.
  - New Message: the recipient dropdown lists contacts (label = name + `(@handle)`); picking one fills the DID; a free-text DID still works for unknown peers.

- [ ] **Re-pin caveat (note in PR):** existing `contacts.json` records gain `username` only after a re-pin (`agent_add_contact`) post-Task-1; until then the handle is absent and the UI falls back to name/alias — correct + graceful.

---

## Self-Review (run after writing, before execution)

- **Spec coverage:** (6) names → Tasks 2-4; (7) search → Task 5; (8) dropdown → Task 4; @handle (Peter's scope add) → Task 1 + threaded through. Rooms-keep-id gated in `conversationLabel`. Verified/key-changed badge → `verified_at_pin` tick (key-changed-in-list deferred; per-message key-change already handled in the thread).
- **Type consistency:** `ContactView` fields identical across air-rs (`Serialize`), TS type, and all consumers; `list_contacts`/`inboxContacts`/`inbox_contacts` names consistent; `conversationLabel(convKey, kind, contact?)` signature identical at all call sites; `filterConversations(convs, query, contacts?, cap?)` arg order matches every caller (globalSearch passes `contacts`, the test passes `undefined, 3`).
- **No placeholders:** every step has real code/commands/expected output.
- **Deferred (non-blocking, note in handoff):** filter self ("me") from the composer dropdown; key-changed indicator in the list; richer combobox (type-ahead filtering inside the dropdown) — the native `<select>` + free-text input is the v1.
