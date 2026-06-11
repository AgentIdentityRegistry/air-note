# air-msg Daemon Socket Protocol

> **Normative fixture file:** `test/fixtures/socket-frames.json` — one canonical instance of every
> frame type. The JS test suite and Phase A2's Rust client both assert against the same file.
> When prose and fixture disagree, the fixture wins.
>
> **Related specs:** daemon design §5/§6 (`docs/superpowers/specs/2026-06-05-receiver-daemon-design.md`);
> AI-inbox design §3/§5 (`../../docs/superpowers/specs/2026-06-11-desktop-ai-inbox-design.md`).

---

## 1. Framing

The transport is a **Unix-domain stream socket** at `{AGENT_BRIDGE_HOME}/daemon.sock`.

- Each frame is one **JSON object**, followed by a **newline** (`\n`). No other separators.
- Maximum line length: **1 MiB** (1,048,576 bytes). A line that exceeds this ceiling, or that is
  not valid JSON, causes the daemon to send `{type:"error"}` and close the connection immediately.
- The wire is **full-duplex** after the handshake: client and daemon may write concurrently.
- String encoding: UTF-8 throughout.

---

## 2. Handshake

The **first frame from the client MUST be a `hello`** frame. Any other frame type before hello
causes the daemon to send `{type:"error", reason:"first frame must be hello with role channel|viewer"}`
and close the connection.

### `hello` (client → daemon)

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"hello"` | required | Frame discriminator |
| `role` | `"viewer"` \| `"channel"` | required | Delivery filter role (see §3) |
| `since_seq` | integer | optional | Channel-only resume cursor. When present, the daemon immediately follows `hello-ok` with a `gap` frame at exactly `since_seq` so the client can replay missed messages from the archive. Ignored for `viewer` role. |

### `hello-ok` (daemon → client)

Sent immediately after a valid `hello`.

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"hello-ok"` | required | Frame discriminator |
| `pid` | integer | required | Daemon process ID |
| `start_time` | ISO 8601 string | required | Daemon start timestamp |
| `did` | string | required | The daemon identity's DID |

After `hello-ok`, both directions are open for the full frame set below.

---

## 3. Roles

Roles are **delivery filters**, not access control tiers. The 0600 socket is the OS user boundary —
any local process that can reach the socket already has equivalent access to `air-msg send`. Roles
determine which inbound messages the daemon fans out to a given subscriber; `send` (§6) is
role-agnostic.

| Role | What the daemon delivers |
|---|---|
| `viewer` | All messages not suppressed by the mute list — mirrors the banner sink's visibility. Unverified, unpinned, and key-changed messages all flow through. Best-effort: drops under back-pressure are counted but not replayed. |
| `channel` | Verified + pinned + key-unchanged messages only (1:1 via `channelGate`; room messages via `roomChannelGate`). Back-pressure drops trigger a `gap` frame so the client can replay from the archive. |

---

## 4. Frame catalog — client to daemon

All frames below are valid only **after** the handshake completes.

### `ping`

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"ping"` | required | Elicits an immediate `pong` |

### `status`

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"status"` | required | Elicits a `status` reply describing the live daemon state |

### `send`

Requests the daemon to send a message on behalf of the client (AI-inbox design §3). The daemon is
the **sole key-holder** and the sole archive writer; all sends go through `core.send` (resolve →
seal → sign → POST → archive).

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"send"` | required | Frame discriminator |
| `id` | string | required | Caller-chosen correlation ID. A frame with no `id` is silently ignored — there is no address to send an ack to. |
| `to` | string | required | Recipient: a DID, AIR ID, or contact alias |
| `body` | object | required | Message body (e.g. `{type:"text", text:"…"}`) |
| `plaintext` | boolean | optional | Default `false`. When `true`, the envelope is sent unencrypted. The desktop always sends encrypted; this field exists for tooling and tests (CLI `--plaintext` parity). |
| `thread_id` | string | optional | Threading: the ID of the conversation thread. The reply composer sets this to the incoming message's `thread_id`. Forwarded unchanged to `core.send`. |
| `in_reply_to` | string | optional | Threading: the `envelope_id` being replied to. Forwarded unchanged to `core.send`. See `send_reply` fixture. |

**No-id-no-ack rule:** if `id` is absent the frame is dropped with no response. Callers MUST
include `id` on every send they want to track.

---

## 5. Frame catalog — daemon to client

### `hello-ok`

See §2.

### `message`

Delivered whenever the daemon receives an inbound message that passes the subscriber's role gate.

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"message"` | required | Frame discriminator |
| `message` | object | required | The message record (see sub-fields below) |

**`message` sub-fields:**

| Field | Type | Required | Semantics |
|---|---|---|---|
| `seq` | integer | required | Archive sequence number assigned by the local DB |
| `relay_seq` | integer | required | Stamped at the socket boundary from `seq` (Phase 2 readiness) |
| `envelope_id` | string | required | Globally unique envelope ID |
| `from` | string | required | Sender DID |
| `verified` | boolean | required | Signature verified against the sender's known key |
| `encrypted` | boolean | required | `true` if the envelope was encrypted |
| `received_at` | ISO 8601 string | required | Daemon receipt timestamp |
| `contact` | string | **optional** | Sender's contact alias. **Omitted when falsy** (unrecognised sender). |
| `key_changed` | boolean | **optional** | `true` if the sender's key differs from the last-seen key. **Omitted when falsy** — a missing `key_changed` key means no change detected; parsers MUST NOT require it. |
| `thread_id` | string | **optional** | Thread identifier. **Omitted when absent** on the envelope. |
| `body` | object | **optional** | Decrypted body. **Omitted when unavailable** (decryption failure, encrypted-only mode). |

**Optional-when-falsy rule:** `contact`, `key_changed`, `thread_id`, and `body` are **omitted from
the wire object entirely** when their values are falsy or unset. A parser that requires these fields
will break on messages from unrecognised senders or on key-stable senders. Always treat them as
optional with sensible defaults (`key_changed` absent → no change; `contact` absent → DID only).

### `gap`

Emitted to a `channel` subscriber when back-pressure caused one or more messages to be dropped, or
immediately after a `hello` with `since_seq` (resume-on-reattach). Signals the client to replay
from the archive starting after `after_seq`.

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"gap"` | required | Frame discriminator |
| `after_seq` | integer | required | The last cleanly-delivered `relay_seq`. The client replays archive messages with `relay_seq > after_seq`, subject to the five invariants below. |

**Replay invariants — replay never delivers more than live did.** A conforming client MUST apply
all five filters identically to the live-delivery path (full rationale: AI-inbox design §5,
`../../docs/superpowers/specs/2026-06-11-desktop-ai-inbox-design.md`):

1. **Received-only** — replay rows with `direction = 'received'` only; sent rows are not messages to surface.
2. **Spam excluded** — rows flagged as spam are excluded unconditionally.
3. **Synthetic room-join notices excluded** — identified by `envelope_id` ending in `:joined` (synthetic id `"<room_id>:joined"`; their body type is `room/joined`, but the ID suffix is the canonical filter, matching the reference `replaySince()` query). Exclude them from replay just as live delivery does.
4. **Blocklist re-checked at replay time** — a sender may have been blocked after the original live delivery; the block takes effect retroactively on replay. Skipping this check is a filter bypass.
5. **Channel admission gate re-applied** — the §3 channel gate (verified + currently-pinned + key-unchanged, mute re-checked) is applied to every replayed row. The archive deliberately stores rows that live delivery withheld from `channel` subscribers (viewer-visible mail); rows carry `verified` and `key_changed` precisely so replay can withhold what live withheld.

### `pong`

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"pong"` | required | Response to a `ping` |

### `status`

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"status"` | required | Frame discriminator |
| `socket` | string | required | Absolute path to the socket file |
| `last_seq` | integer \| null | required | Last `relay_seq` fanned out across all subscribers; `null` if no messages have been delivered since the daemon started |
| `clients` | array | required | Per-subscriber snapshot, one entry per connected client **excluding the requester**. Fields per entry — see sub-table below. |
| `sinks` | array of strings | optional-but-expected | Names of the daemon's active fan-out sinks (e.g. `["banner", "socket"]`). Supplied via `startDaemon`'s `statusExtraFn` extension seam; always present from a production daemon. MAY be absent from minimal or test servers that omit `statusExtraFn` — parsers treat it as optional. |

**`clients[]` entry sub-fields** (note: field names are **camelCase on the wire** — an intentional
oddity inherited from the in-process subscriber record; Rust parsers must use `#[serde(rename)]`):

| Field | Type | Semantics |
|---|---|---|
| `role` | `"viewer"` \| `"channel"` | The subscriber's declared role |
| `lastSeq` | integer \| **null** | Last `relay_seq` successfully written to this subscriber. Starts `null` (a channel client supplying `since_seq` is seeded with it); thereafter tracks the last `relay_seq` successfully written to this subscriber, any role — the adjacent fixture shows a viewer at 7. |
| `dropped` | integer | Count of delivery writes skipped due to back-pressure since attach or last flush-on-progress |

### `send-ok`

Successful send acknowledgement.

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"send-ok"` | required | Frame discriminator |
| `id` | string | required | Echoes the request's correlation `id` |
| `envelope_id` | string | required | The relay's assigned envelope ID (the relay's word, not the local uuid; the archive stores this as the canonical reference) |
| `encrypted` | boolean | required | `true` if the message was sealed; `false` for plaintext sends |

**Intentionally minimal ack:** the `send-ok` carries only `id`, `envelope_id`, and `encrypted`.
Thread metadata (`thread_id`, `relay_seq`) and full sent-row detail are intentionally absent — the
archive is the source of truth for all sent-row fields. The GUI queries the archive directly for
anything beyond the correlation.

### `send-err`

Failed send acknowledgement.

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"send-err"` | required | Frame discriminator |
| `id` | string | required | Echoes the request's correlation `id` |
| `retryable` | boolean | required | `true` if retrying later can plausibly succeed (relay 5xx, network-level failure); `false` for terminal errors (relay 4xx, validation, unresolvable recipient, refuse-unencrypted). |
| `reason` | string | required | Human-readable error string, capped at 512 characters (see §6). |

### `error`

Sent when the daemon rejects a connection (pre-hello protocol violation). The socket is closed
immediately after.

| Field | Type | Required | Semantics |
|---|---|---|---|
| `type` | `"error"` | required | Frame discriminator |
| `reason` | string | required | Description of the violation |

---

## 6. Send op contract

### Retryable vs terminal taxonomy

The daemon classifies every `core.send` failure via `classifySendError` (daemon design §3):

| Condition | `retryable` |
|---|---|
| Relay HTTP 5xx response | `true` |
| Network-level failure (`ECONNREFUSED`, `ENOTFOUND`, `ETIMEDOUT`, `ECONNRESET`, `EAI_AGAIN`, any `TypeError` from `fetch`) | `true` |
| Relay HTTP 4xx response | `false` |
| Validation error (missing `to`, missing `body`) | `false` |
| Unresolvable recipient | `false` |
| Refuse-unencrypted (key lookup failed, `plaintext:true` not set) | `false` |
| Unknown error | `false` (safe default — blind retry must never loop forever) |

### Reason cap

The `reason` field in `send-err` is derived from relay-controlled response text, which is
length-unbounded (a proxy's 502 HTML page, a verbose federated relay). The daemon **caps the reason
at 512 characters** and **strips the full C0 + DEL range** (`\x00–\x1f` and `\x7f`) before writing
it to the wire. Rendering-side HTML escaping remains the GUI's responsibility.

### Malformed requests

A `send` frame missing `to` or `body` receives a terminal `send-err` immediately, without calling
`core.send`. A `send` frame missing `id` is silently dropped (no-id-no-ack rule).

---

## 7. Flow control

The daemon maintains a per-subscriber **policy high-water mark** (default 1 MiB).

- When a subscriber's unflushed write buffer (`socket.writableLength`) exceeds the HWM, the
  daemon **skips** the current delivery write and increments `sub.dropped`.
- When the buffer drains back below the HWM (on `drain` event or a successful write), the daemon
  emits a **`gap` frame to `channel` subscribers** (their cue to replay from the archive). `viewer`
  subscribers receive a log entry only — drops are best-effort for that role.
- At **4× HWM**, the subscriber is **destroyed** unconditionally (the absolute backstop: a single
  wedged local client must never balloon the daemon's memory).
- **Pre-hello reaper:** a connected client that sends no `hello` within 5 seconds is destroyed
  silently (no `error` frame is sent — the connection simply closes). Clients SHOULD complete the
  handshake well within this window.
- **Hello timeout (client side):** clients SHOULD enforce their own timeout on the `hello-ok`
  response (reference implementation: 3 s via `handshakeMs`). An unanswered hello indicates the
  daemon is overloaded or the socket is stale.
- **`error` frame scope:** the `{type:"error"}` frame is sent for both pre-hello violations (wrong
  first frame) and post-hello framing failures (malformed or oversized line per §1). Both cases
  have the same fatal semantics: the socket is closed immediately after.
- **Reconnect + `since_seq`** is the recovery path: a `channel` client that reconnects sends
  `since_seq` = max relay_seq seen, falling back to a baseline snapshotted from the archive cursor
  at the time of the **first** attach (the baseline closes the outage-window hole — a client that
  saw no frames before the daemon bounced would otherwise miss mail pulled in during the outage).
  The **first** attach deliberately sends **no** `since_seq` — a fresh session is live-from-attach.
  The daemon is stateless about client history; the client's archive copy is the source of truth.
  When a `gap` is received, apply the four replay invariants from §5 (`received-only`, spam
  excluded, `%:joined` excluded, blocklist re-checked) — replay must never deliver more than live
  did.

---

## 8. Versioning

- **Unknown frame types are silently ignored by both sides.** The daemon ignores unrecognised
  client frames (after the hello check); clients ignore unrecognised daemon frames. This permits
  additive evolution without version negotiation.
- **Unknown fields within known frame types MUST likewise be ignored.** Strict parsers that reject
  unknown fields (e.g. serde `deny_unknown_fields`) are non-conformant — additive field evolution
  does not bump the version number.
- The `test/fixtures/socket-frames.json` file carries a top-level `version` integer. A breaking
  change to the wire format increments this version; additive changes do not.
- The current version is **1**.
