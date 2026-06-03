# Autonomous Agent Group Chat (Rooms v1) — Design

**Date:** 2026-06-03
**Status:** Draft **v2.1** — review-hardened + re-verified by both reviewers (clear-to-plan); pending user approval, then plan.
**Repo:** `~/air-note` (canonical). Code under `agent-bridge-mcp/`; Rust reference under `crates/air-rs/`.
**Tracks:** AIR Note messaging issue #34 (group chat). Folds in the first concrete slice of the **Mandate** primitive (scoped, revocable delegation).

---

## 0. Changelog (v1 → v2) — from the two-reviewer second opinion

v2 closes 5 load-bearing holes the architecture + security reviews found (the foundation — crypto reuse, dumb-relay fan-out, module split, scope — was confirmed sound and unchanged):

- **Fork-free merge now uses a founder `op_seq` counter, not wall-clock** (was: timestamp order, which can tie/run backward). §6.
- **Eclipse / digest spoofing addressed**: recipient-list + digest are now identical bytes across a fan-out and cross-checked; per-sender message counter makes drops detectable; the doc now states completeness is *unenforceable* (only eventually detectable). §6.3, §7, §8, §9, §11.
- **Raise-your-hand trigger hardened**: `in_reply_to`-self must be a *provably self-authored* message; `mentions` is a gated hint with a per-human-turn reply budget; agents never auto-reply to another agent's auto-reply. §10.
- **Persistent anti-replay guard** added (was: in-memory only, reset on restart). §9, §12, §14.
- **Honesty + plumbing**: kicked-member silencing is *eventually* effective with **no hard bound**; a real **request channel** (`room/req-ops`/`room/req-snapshot`) is defined; new members **auto-pin the founder**; admin keys are vouched by the founder-signed grant. §6, §9, §11, §14.

**⚑ Three fixes change a v1 rule — called out inline with `⚑ DECISION` for your veto:**
- **(a)** Revoking an admin now **voids that admin's adds** (was "past adds stand"). §6.2.
- **(b)** We **openly state perfect delivery is not guaranteed**; eclipse is only eventually detectable. §11.
- **(c)** Reply rules tightened: **≤1 auto-reply per human turn**, anchored to founder/human-`kind` members; never auto-reply to a bot. §10.

**v2.1 (re-verification fold-in):** both reviewers confirmed all v1 blockers CLOSED and v2 *clear-to-plan*. Folded their three follow-ups: `kind:"human"` is now **founder-only** (admin-adds ⇒ `kind:"agent"`) so a compromised admin can't forge a reply-budget turn-anchor; `room/req-*` is **rate+size-capped** (anti-DoS); `op_id` is defined; plus notes on revoke-rescue batching, missed-turn under-reply, and `sender_seq` poisoning. §6.1, §6.2, §6.4, §7, §11.10, §14, §17.

---

## 1. Goal

Let a human and several **AI agents** (and/or people) share **one sealed conversation thread** — a *room* — in which **agent members read and reply on their own**, while the human stays in control. v1 proves the end-to-end magic trick with the smallest surface and **zero new cryptography**.

Target scene on one machine:
> Founder + two agent identities are in a room. An **admin agent** adds a fourth member. The founder **kicks** someone. The founder **@asks one agent** a question → that agent replies *to the whole room*; the other agent **stays quiet** (not addressed). The founder posts **`/halt`** → every agent freezes the room until **`/resume`**.

This is the autonomous heir to the delegate/Mandate vision: an agent acting inside a room under a **revocable, founder-issued leash**.

---

## 2. Existing seams this builds on

Crypto + transport are reused **unchanged**; group chat is fan-out + roster bookkeeping on top. The trust/safety logic in `channel.mjs`/`core.mjs receive` is **extended with new, security-load-bearing code** (the roster gate, halt gate, replay guard, raise-your-hand logic do **not** exist today — see §15 for their tests).

| Seam | File | Role in rooms | Touched? |
|---|---|---|---|
| Envelope build + sign | `core.mjs:buildOutboundEnvelope`, `signEnvelope` | One envelope **per member**, shared `thread_id` | reused |
| Per-recipient seal | `crypto.mjs:sealBody`/`openBody` (`x25519-hkdf-sha256-chacha20poly1305`, fresh ephemeral+nonce per seal) | One sealed copy per member key. **No forward secrecy** (unchanged) | reused |
| AAD binding | `crypto.mjs:buildAad` (`{id,from,to,thread_id}`) | Per-envelope binding stops cross-member splice (verified) | reused |
| Send → single inbox | `core.mjs:send` → `POST /inbox/<DID>` | Group send = **N sends**. Relay stays a dumb 1:1 post office | reused |
| Receive (verify+pin+decrypt+archive+cursor) | `core.mjs:receive`, `checkPin`, `archiveMessage` | **Changed**: + roster gate, + `room_id` tag, + persistent replay/skew guard | **changed** |
| Channel-push wake | `channel.mjs`, `channel-server.mjs` | **Changed**: room-aware gate (roster+halt), room-context content (fenced), raise-your-hand + reply budget — all NEW code | **changed** |
| Contacts + pinning | `contacts.mjs` (`~/.air-msg/contacts.json`) | Members are pinned contacts; **founder auto-pinned at join** | reused + |
| Local archive | `archive.mjs` (`~/.air-msg/archive.db`) | + `room_id` column; replay guard reads `envelope_id` PK before push | **changed** |
| Reply tool | `index.mjs:agent_send` | Agents reply via new room-aware fan-out send | + |

**The relay is never taught about rooms.** All room semantics live client-side.

---

## 3. Locked decisions (brainstorm 2026-06-03)

1. **Shape:** "agent team room" and "small private circle" are the *same primitive* — a small set of verified DIDs sharing one sealed thread.
2. **Autonomy:** agent members **read and reply on their own**.
3. **Turn-taking = "raise-your-hand" ("walk" tier):** an agent auto-replies **only when @addressed**; otherwise silent. Backed by reply-budget + self-limit + quiet-timer + halt.
4. **Approach = "Thin Room":** owner-signed roster, seal-per-member fan-out, `room_id` tag. Built so a facilitator role and a first-class group-DID layer on later without rewrite.
5. **Ownership = founder + admin Mandates; _admins add, founder kicks_:** the **founder** is the bedrock root. Founder issues **revocable admin Mandates** ("DID X may add members to room R"). **Any admin can ADD; only the founder can REMOVE members / change the admin set / halt.**
6. **Size:** small rooms (target ≤ ~15). O(N) seal-per-message is fine; large-room group keys (MLS) are out of scope (#35/#36).

---

## 4. Architecture overview

```
        ┌──────────────────────── Founder (you) ────────────────────────┐
        │  owns rooms.json · signs room/create · founder op_seq counter  │
        │  grants/revokes admin Mandates · signs member REMOVES · halt   │
        └────────────────────────────────────────────────────────────────┘
                 │ admin-mandate (founder-signed, carries admin pubkey)
                 ▼
   ┌──── Admin member (person or agent) ────┐
   │  holds Mandate · signs member ADD slips │
   └──────────────────────────────────────────┘
                 │ add slip (admin-signed, cites mandate_id)
                 ▼
   Membership = deterministic merge of signed ops
     (founder ops totally ordered by op_seq · admin ADD valid iff its mandate is
      currently non-revoked · founder REMOVE sticky · founder-add can rescue)
                 │
                 ▼
   SEND ── fan-out ──► N sealed 1:1 envelopes (shared thread_id, identical body
                       carrying recipients[] + roster_digest + sender_seq) ──► inboxes
                 │
                 ▼
   RECEIVE ── verify sig + pin + **roster gate** + **replay/skew guard** + decrypt
              ── cross-check (self ∈ recipients, digest matches) ──► archive(room_id)
                 │
                 ▼  (agent members only)
   channel-push ── room-aware gate (roster + not-halted + not-muted) ──► untrusted-fenced
                   content (room name, sender, mentions ALL inside the fence)
                 │
                 ▼
   raise-your-hand: auto-reply iff @mentioned OR provably-self in_reply_to
                    · ≤1 reply per human/founder turn · never reply to a bot
                    · self-limit K · quiet-timer · halt freezes
```

---

## 5. Components & files (`~/air-note/agent-bridge-mcp/src`)

| File | New/changed | One job |
|---|---|---|
| `rooms.mjs` | **new** | Pure room-state store: `rooms.json` load/save; founder `op_seq`; create; grant/revoke Mandate; record add/remove slips; **derive `{members, admins, halted}`** from ops (the merge); compute versioned `roster_digest`; build `room/snapshot`. |
| `room-ops.mjs` | **new** | Pure builders/validators for `room/*` ops: build + sign (`op_sig`) + verify against the issuer key (founder pinned at join; admin key from the founder-signed grant). No state. |
| `core.mjs` | changed | `sendRoom(...)` fan-out (reuses build/seal/sign); `receive` adds roster gate + `room_id` tag + persistent replay/skew guard + surfaces `in_reply_to`/`to` on the pushed object. |
| `channel.mjs` | changed | Room-aware `channelGate` (verified+pinned+roster+not-halted+not-muted); `buildChannelContent` with **fenced** room context; raise-your-hand decision + per-human-turn reply budget + self-limit. |
| `archive.mjs` | changed | `room_id` column+index (migration); replay guard via `envelope_id` existence check; `history({room})`. |
| `index.mjs` | changed | MCP tools: `agent_room_create/invite/kick/grant_admin/revoke_admin/send/list/history/halt/resume` + `agent_room_request` (pull missing ops/snapshot). |
| `cli.mjs` | changed | `air-msg room {create,invite,kick,grant-admin,revoke-admin,send,list,history,halt,resume,sync}`. |
| `crates/air-rs/` | later (§16) | Rust parity for new op types — deferred; also fixes the stale `crates/a2a-rs` comment at `crypto.mjs:4`. |

---

## 6. Membership model — fork-free merge of signed ops

**Membership is not a versioned list.** It is the deterministic merge of a *set* of signed operations; the same op-set yields the same `{members, admins, halted}` for everyone, independent of arrival order.

### 6.1 Operation types (each a signed `room/*` control message, §7)

All ops carry an in-body `op_sig` over their canonical bytes. **Founder ops** additionally carry `founder_seq` — a founder-local **monotonic integer, encoded as a decimal string** (avoids the float64 canonicalization hazard, §15/L1). Wall-clock fields are advisory only and **never** used for ordering.

- `room/create` — `{room_id, name, thread_id, founder_did, founder_pubkey, founder_seq:"0"}`; founder-signed. Root of trust; carries the founder key so members can pin it.
- `room/admin-grant` — `{room_id, mandate_id, holder_did, holder_pubkey, scope:"member:add", founder_seq, expires_at?}`; founder-signed. **This is an admin Mandate.** Carries the holder's pubkey so members verify the admin's op_sigs without a separate pin.
- `room/admin-revoke` — `{room_id, mandate_id, founder_seq}`; founder-signed.
- `room/add` — `{room_id, member_did, member_pubkey, kind:"human"|"agent", mandate_id?}`; founder-signed, **or** admin-signed citing `mandate_id`. **`kind:"human"` is founder-only** — an **admin-signed add is forced to `kind:"agent"`** (only the founder confers human-turn-anchor status, §10), so a compromised admin can't forge a turn-anchor to reopen the reply budget (§11.4/§11.10).
- `room/remove` — `{room_id, member_did, founder_seq}`; **founder-only.**
- `room/snapshot` — `{room_id, founder_seq, members[], admins[], halted}`; founder-signed bootstrap/heal.
- `room/halt` / `room/resume` — `{room_id, founder_seq}`; founder-only (control; §10).
- `room/req-ops` — `{room_id, have:[op_id,…]}` / `room/req-snapshot` — `{room_id}`; any member → founder/any member (the request channel, §6.4).

### 6.2 Derivation rule (deterministic — the heart)

Founder ops share one signer and a strictly increasing `founder_seq`, so they are **totally ordered** (tiebreak on op hash only if seqs somehow collide). From an op-set:

1. **Admins** = each `holder_did` whose **latest founder op (by `founder_seq`) about its `mandate_id` is a `grant`** (not a `revoke`) and not past `expires_at`.
2. **A `room/add` for `member_did` counts** iff it is founder-signed, **or** it is admin-signed and **its cited `mandate_id` is a currently-valid admin Mandate** per (1). _No reliance on any admin-asserted timestamp._
   > **⚑ DECISION (a) — changed from v1:** because validity is evaluated against *current* mandate status, **revoking an admin retroactively drops the members that admin added.** This kills the backdated-add exploit (security H2). A founder who wants to keep a good member after revoking a bad admin re-adds them with a **founder** `room/add` (which never depends on a mandate). Safer default; founder can always rescue. **Operational note:** a revoke *without* rescue is a **mass-removal** of that admin's adds — batch the revoke and any founder-add rescues at adjacent `founder_seq` so honest members merge them together and avoid mid-conversation churn (the transient is self-healing via §9.5).
3. **`member_did` is a MEMBER** iff (a) ≥1 counting `room/add` exists, **and** (b) the **latest founder op about `member_did` is not a `room/remove`**. Founder `room/remove` is **sticky** (overrides admin adds) until a later founder `room/add`.
4. **`halted`** = the latest founder `room/halt`/`room/resume` by `founder_seq` is a `halt`.

Convergence: every clause depends only on the op-set and the totally-ordered founder branch ⇒ honest members converge.

### 6.3 Roster digest (versioned, covers members **and** admins)

`roster_digest = sha256(canonical({digest_v:1, members:[sorted dids], admins:[sorted holder_dids], halted}))`. Carried in every `room/msg` (§7) and cross-checked on receive (§9). Covering admins makes admin-set divergence (the thing §6 most guards) detectable, not just member divergence.

### 6.4 Request channel + snapshots (bootstrap + heal)

A member that detects drift (digest mismatch) or is newly added pulls state via the **request channel**:
- `room/req-ops {have:[op_ids]}` → recipient forwards every op the requester lacks (each with its original `op_sig`), via the normal 1:1 send path.
- `room/req-snapshot` → founder (or any member holding a founder snapshot) returns a `room/snapshot`.
Requests are ordinary sealed+signed 1:1 messages (relay stays dumb). **Anti-DoS (concrete):** a responder answers at most **R=3** `room/req-*` per requester per minute and caps any single response at **C** ops (paginate larger diffs via `have`); an empty `have:[]` is allowed but still C-capped; only **current room members** are answered. New members **auto-pin `founder_did`** from the (founder-signed, key-bearing) invite/snapshot before trusting further ops.

---

## 7. Wire format

**No envelope-level change.** A room message is an ordinary signed+sealed envelope; the decrypted `body` carries room context. **For one fan-out, the body bytes are identical across all N envelopes** (only `id`/`to`/seal differ) — this is what lets recipients cross-check each other.

```jsonc
// normal room message body — identical across the whole fan-out
{
  "type": "room/msg",
  "room_id": "uuid",
  "sender_seq": "42",                  // per-(sender,room) monotonic; gaps ⇒ a drop (eclipse detect)
  "recipients": ["did:wba:…", "…"],     // sorted; the set the sender claims to be mailing
  "roster_digest": "sha256-hex",        // §6.3
  "mentions": ["AIR-CODX-…"],           // who is asked to respond (raise-your-hand)
  "text": "…"
}
```

Control ops use the same sealed+signed envelopes with `body.type = "room/<op>"` + §6.1 fields + an in-body **`op_sig`**.

**Trust rule (N2):** for control ops, membership trust is decided by **`op_sig`** verified against the issuer key — `founder_pubkey` (pinned at join) for founder ops, `holder_pubkey` (from the founder-signed grant) for admin ops. The **envelope** signature only proves the *last forwarding hop*; it is **not** used for control-op authority. This lets any member forward an op to a member who's missing it (§6.4) without being able to forge it.

**Op identity:** `op_id = sha256(canonical(op-body including op_sig))` — the stable id that `room/req-ops {have:[op_id,…]}` diffs against (§6.4).

---

## 8. Data flow — SEND (fan-out)

`sendRoom({ room_id, body_text, in_reply_to, mentions })`:
1. Load room; **re-derive** members *now* (fresh op-set, §6.2); drop self. If `halted` ⇒ refuse to send (honest source-side halt, §10).
2. Build the **single** `room/msg` body: `recipients` = sorted members, `roster_digest` (§6.3), `sender_seq` = next per-room counter, `mentions`, `text`.
3. **For each member:** `buildOutboundEnvelope` (own `id`, `to`=member, shared `thread_id`, same body) → `sealBody` for that member's pinned key → `signEnvelope` → `POST /inbox/<member>`.
4. Archive own copy tagged `room_id`.
5. Return a per-member delivery report (ok / failed inbox); retry failed inboxes with backoff, then surface persistent failures ("Dana didn't receive it").

O(N) seals + N POSTs. No relay change.

---

## 9. Data flow — RECEIVE (verify + pin + roster gate + replay guard + cross-check)

Reuses `receive()`, then for any envelope whose decrypted `body.type` starts `room/`:
1. **Replay/skew guard (H3):** if `envelope_id` already in archive ⇒ **skip push + skip re-archive**. Reject envelopes whose `timestamp` is older than a skew horizon (e.g. 48h). Persistent (survives restart) — not the in-memory `seen` Set alone.
2. **Verify + pin** the *forwarding* sender (existing path); for control ops, also **verify `op_sig`** against the issuer key (§7).
3. **Roster gate:** `room/msg` ⇒ sender ∈ derived members; founder op ⇒ `op_sig` = founder; `room/add` ⇒ `op_sig` = founder or a currently-valid admin. Else **drop**. (Re-derive from the freshest op-set at receive time, H1.)
4. **Control op:** append to op-set, re-derive. An op whose prerequisite is missing (e.g. add citing an unseen grant) is **held pending** + triggers a `room/req-ops` (§6.4); never honored from an unknown mandate.
5. **`room/msg` cross-check (eclipse, C1):** verify `self ∈ recipients`, `recipients == my_derived_members∖{sender}`, `roster_digest == my_digest`, and `sender_seq` has no gap. Any mismatch ⇒ flag drift + `room/req-ops`/`room/req-snapshot`; a `sender_seq` gap ⇒ "I was dropped from ≥1 message."
6. Archive tagged `room_id`. Cursor advances as today.

---

## 10. Autonomous reply loop ("raise-your-hand") + safety brakes

Agent members run `air-msg-channel`. On a gated room `room/msg`:

1. **channelGate (room-aware, NEW):** push only if verified + pinned + `!key_changed` + sender ∈ room + **room not halted** + not muted.
2. **buildChannelContent (room-aware, NEW):** untrusted-fenced. **Every attacker-influenced string — body text, room name, sender alias, and each `mentions` entry — is inside the `⟦untrusted…⟧` fence and `⟦⟧`-stripped (M3).** Only the verified `from` DID and fixed labels live outside.
3. **Raise-your-hand decision:** auto-reply **iff** the agent's handle ∈ `mentions`, **or** the message's `in_reply_to` cites a room message **the agent can prove it authored** (exists in its archive with `from == self`, same `room_id`) (C2/B3). A forged `in_reply_to` ⇒ ignored.
4. **Reply** = `sendRoom(...)`, `in_reply_to` the trigger.

**Brakes (all local; never trust a peer's counter):**
> **⚑ DECISION (c) — tightened from v1:**
- **One reply per human turn:** an agent auto-replies **at most once per founder/`kind:"human"` message**. A new human/founder room message resets the budget. This anchors the conversation to humans and **prevents agent↔agent ping-pong** without trusting peer state.
- **Never reply to a bot's auto-reply:** an agent does not auto-initiate from another agent's message; only a human/founder message (or an explicit human relay) opens a turn.
- **Multi-mention / re-trigger → confirm, don't auto-answer:** a message mentioning >1 agent, or re-triggering within the quiet window, is **queued for human confirmation** rather than auto-answered (kills the @everyone stampede).
- **Self-limit K:** ≤ **K=3** auto-replies total without *any* intervening human/founder message (defense in depth); peer-triggered chains trip at **K=1**.
- **Quiet-timer:** local per-room idle window (default 60s) ends a round.
- **Halt:** founder `room/halt` ⇒ agents stop auto-replying; honest senders also refuse to fan-out (§8). **Honest limitation (M2):** halt is effective for a member only *after* it receives the signed op — eventually-consistent, not instant. Immediate brake = local `air-msg room mute <id>`. Halt/resume are **founder-signed ops**, so free-text "halt" in a message body has **no** effect.

---

## 11. Trust & threat model (v2 — corrected)

1. **Relay is dumb & untrusted.** All trust recipient-side: verify sig + pin + roster gate + replay guard.
2. **Stranger who learns `thread_id`:** dropped (not pinned, not in roster).
3. **Kicked member — _eventually_ silenced, NO hard bound (H1, honest):** a `room/remove` silences the member only after each recipient merges it; an offline member can keep talking until it syncs, and a malicious member can withhold forwarding its own remove. Mitigations: re-derive roster at **send and receive**; a sender with a *pending* remove for a peer **quarantines** that peer's messages. Note (N4): the kicked member retains its key, so a lagging sender could still seal a future message to it until re-derivation. A relay-surfaced revocation tombstone is the future hard-bound fix (out of scope v1).
4. **Compromised admin:** can ADD (sockpuppets) but **cannot remove/kick/self-promote/halt** (founder-only). Founder `room/admin-revoke` **drops all that admin's adds** (⚑ DECISION (a), §6.2) — revoke now contains the blast, not just future adds.
5. **Backdated admin add:** **closed.** Validity keys off *current* mandate status (§6.2 step 2), not the admin's self-signed timestamp.
6. **Eclipse / silent omission (C1) — partially mitigated; honestly bounded:**
   > **⚑ DECISION (b):** In a 1:1-relay model, **delivery completeness is unenforceable** — a sender can omit a recipient from the actual POSTs. v2 makes it **eventually detectable**, not prevented: the per-fan-out `recipients[]`+`roster_digest` are identical bytes and cross-checked (catches "lie to A vs B" and roster-omission), and `sender_seq` gaps reveal a missed message. We **document this limitation** rather than claim a delivery guarantee we don't have.
7. **Prompt injection into autonomous agents:** body **and** room-context strings are fenced (M3); the autonomy trigger is hardened (forged `in_reply_to` ignored; mentions gated; one-reply-per-human-turn; no bot→bot) (C2/M4); control ops are **signed**, never inferred from text.
8. **Replay:** persistent dedup on `envelope_id` + skew horizon (H3) — a replayed envelope no longer re-fires the agent.
9. **Founder key = single root (L2, honest):** loss/compromise = **total room loss with no remediation**; **no founder-key rotation or room recovery in v1**. (#19 hardware custody lowers likelihood, not blast radius.) No forward secrecy (unchanged; MLS #35/#36).
10. **v2 follow-up surfaces (now bounded):** `room/req-*` is rate+size-capped per requester (§6.4) to stop in-room request flooding; `kind:"human"` is **founder-only** so a compromised admin (§11.4) can't forge a reply-budget turn-anchor; `sender_seq` is tied to the sender's pinned **key-epoch** so a peer can't poison the counter (§17.5).

---

## 12. Storage

**`~/.air-msg/rooms.json`** (0600), per room:
```jsonc
{
  "version": 1,
  "rooms": {
    "<room_id>": {
      "name":"…", "thread_id":"uuid", "founder_did":"did:wba:…",
      "ops": [ /* signed room/* ops — the shoebox; founder ops carry founder_seq */ ],
      "founder_seq_next":"1",          // only on the founder's own machine
      "send_seq_next":"43",            // this identity's per-room sender counter
      "muted": false,                  // local immediate brake
      "reply_budget": { "turn_anchor":"<envelope_id>", "used":0 },  // per-human-turn (agent)
      "self_reply_streak": 0,          // self-limit K (agent)
      "joined_via":"create|snapshot|invite"
    }
  }
}
```
Derived `{members, admins, halted, roster_digest}` are computed on read (single source of truth = `ops`).

**`archive.db`** migration (idempotent, additive): `room_id TEXT` column + `CREATE INDEX idx_messages_room ON messages(room_id, timestamp)`. Replay guard relies on the existing `envelope_id` PK — `receive` checks existence **before** pushing to the channel. Existing rows ⇒ `room_id = NULL` (1:1 unchanged).

---

## 13. Single-consumer lock

`consumer-lock.mjs` is unchanged and **per identity**: one puller (channel server **or** `air-msg watch`) demuxes 1:1 *and* all rooms that identity belongs to via `thread_id → room_id`. No new lock.

---

## 14. Error handling / failure modes

- **Partial fan-out:** failed inbox POSTs don't block others; retry w/ backoff; per-member report; no third-party store-and-forward (E2E: only the sender re-seals).
- **Decrypt-fail demux (C5):** a control op that fails to decrypt (wrong/rotated key) has **no `room_id`** to route by ⇒ log as **dead-letter** + surface for snapshot heal; never crash the consumer.
- **Op before prerequisite:** hold pending + emit `room/req-ops` (§6.4).
- **Drift:** digest/recipients/seq mismatch ⇒ request ops/snapshot before relying on completeness.
- **Halt race:** in-flight replies may land; agents stop *initiating* on receipt; immediate brake = local mute.
- **Replay:** duplicate `envelope_id` ⇒ skip push+archive; stale `timestamp` ⇒ reject.
- **Migration:** additive + abort-safe.
- **Missed human turn:** an agent that never received a human/founder message (eclipse/lag, §11.6) won't open a reply turn ⇒ may stay silent even when @mentioned; recovers on the next human turn or a manual `room sync`.
- **Request flood:** `room/req-*` beyond the per-requester budget (§6.4) ⇒ dropped (in-room DoS guard).

---

## 15. Testing strategy (whole JS suite stays green via `node --test`)

**Pure-logic (`rooms.mjs`, `room-ops.mjs`, `channel.mjs`):**
- Op sign/verify per type; tamper ⇒ reject. **No raw numeric fields in signed bodies** (L1) — `founder_seq`/`sender_seq` are strings.
- **Merge convergence:** shuffle one op-set into many orders ⇒ identical `{members, admins, halted, roster_digest}`. *(Acceptance gate per the architect's note.)*
- **Founder-seq ordering:** same-millisecond / backward-clock founder ops resolve deterministically by `founder_seq` (B1).
- **Founder-kick stickiness** + **founder-add rescue**.
- **Admin scope:** admin `remove`/`grant`/`halt` ⇒ rejected. **Revoke drops that admin's adds** (⚑a / H2).
- **Eclipse cross-check (C1):** recipients/digest mismatch or `sender_seq` gap ⇒ drift flagged; per-recipient digest divergence caught.
- **Raise-your-hand:** mentioned ⇒ reply; forged `in_reply_to` (not self-authored) ⇒ ignored; not addressed ⇒ silent.
- **Reply budget (⚑c):** ≤1 per human turn; bot→bot ⇒ no auto-reply; @>1 agent ⇒ queued; self-limit K; peer-chain trips at K=1.
- **Halt:** signed `room/halt` ⇒ gate closed *and* source-side send refused; free-text "halt" ⇒ no effect.
- **Roster gate + replay:** non-roster sender dropped; duplicate `envelope_id` ⇒ no second push; stale timestamp ⇒ rejected.
- **Fence (M3):** room name / alias / mention with `⟦⟧` or injection text ⇒ stripped + fenced.

**One end-to-end** (the §1 scene): founder + 2 agents; admin agent adds a member; founder kicks one; founder @asks agent A ⇒ A replies to all, B silent; `/halt` ⇒ frozen. Mirrors the #29 live-proof rhythm.

Target: current 88 JS tests stay green + the above added.

---

## 16. Out of scope (v1) / future

- **Rust parity** (`crates/air-rs`) for new op types — JS-first; mirror after the shape stabilizes; also fix the stale `crates/a2a-rs` comment at `crypto.mjs:4` (N3).
- **Facilitator role (Approach B)**; **room as a first-class AIR group-DID (Approach C)**.
- **Group keys / forward secrecy (MLS #35/#36)**; **relay-surfaced kick tombstone** (hard-bounds H1); **founder-key rotation / room recovery** (L2).
- **Admins kick / equal co-owners** — founder-reserved in v1.
- **Cross-app bridge × rooms.**

---

## 17. Open questions (resolve during planning)

1. **Mention syntax:** `@AIR-id`, `@alias`, or both; who fills `mentions` for an agent's reply (model vs parser)? Lean: parse `@alias`/`@AIR-id` **and** allow an explicit arg.
2. **Quiet-timer / K defaults:** 60s / K=3 hard-coded for v1, exposed later? Lean: hard-code v1.
3. **`kind` (human/agent) source — RESOLVED (v2.1):** `kind:"human"` is **founder-only**; admin-added members are forced `kind:"agent"` (§6.1), so the reply-budget turn-anchor (§10 ⚑c) can't be forged by a compromised admin (§11.4/§11.10).
4. **Snapshot trigger:** on join + on drift-request only (lazy)? Lean: yes.
5. **`sender_seq` persistence + reset semantics** across re-join/key-rotate. Lean: persist per (identity, room), **keyed to the sender's pinned key-epoch** and never lowered *within* an epoch — so a malicious sender setting `sender_seq` artificially high can't poison a later honest (re-keyed) message into looking like a gap/replay (§11.10).

---

## 18. Mapping to the Mandate crawl-walk-run

`room/admin-grant` is the **first real Mandate**: scoped (`member:add`, one room), revocable (`room/admin-revoke`, now blast-containing), auditable (signed, in the op-set) — exactly "bounded delegation, not power of attorney." Turn-taking sits at **"walk"** (bounded auto-act). Future tiers reuse the primitive: wider scopes (RSVP, scheduling) = "walk" elsewhere; in-room agent-to-agent negotiation = "run."
