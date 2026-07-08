# Memory Hub Phase 2 (M1b Code Loop) — SP1: The Safe Read+Write Loop (Design)

**Status:** Draft 1 — awaiting Peter's spec review → then `writing-plans`.

**Program:** Phase 2 of the memory-hub program (★ North Star `air/memory-strategy-2026-07-03-beat-the-stack`: "install AIR Agent → your Claude Code / Codex just never forgets"). Phase 1 retrieval floor is complete (rungs 0–1 + multilingual shipped, main `54dfefa`). Phase 2 = the Code loop, decomposed into three shippable sub-projects:

- **SP1 (this doc)** — the safe read+write loop *backend*: a `remember` write op, per-op authorization, and a Rust **MCP adapter** exposing `recall` + `remember` to any coding agent (wired by hand for now).
- **SP2** — one-click integration (the desktop Integrations panel that auto-writes the MCP + hook config for Claude Code / Codex).
- **SP3** — automatic behaviors (session-start snapshot, auto-capture, recall-miss instrumentation).

Each sub-project is independently shippable + testable and gets its own spec → plan → build cycle. SP1 depends on nothing new; SP2 and SP3 depend on SP1.

**Goal:** Let a coding agent, through a small Rust MCP adapter, **recall** AIR memories and **remember** new ones — safely (a scoped role the daemon enforces), backed by the existing `bossclawd` daemon — proven end-to-end without any UI.

---

## 1. Why (the North Star, this half)

The product promise is "your coding agent never forgets." That requires the agent to both **read** existing memory (recall) and **write** new memory (remember) over a standard interface (MCP — what Claude Code and Codex speak). Today the daemon exposes `recall` over its socket but has (a) **no first-class write op** other than file ingestion, and (b) **no per-op authorization** — every same-uid socket client can invoke all 29 ops, including destructive/egress ones (`Teardown`, `EnableCloudReasoner`). Exposing memory to an *external* coding agent is unsafe until a scoped role exists. SP1 delivers the write op + the scoped role + the MCP adapter — the complete, safe loop — as the foundation SP2/SP3 build on.

## 2. Product decisions (locked with Peter, 2026-07-08)

1. **Full read+write loop in SP1** (not read-only first): `recall` + `remember` both ship.
2. **Authorization = "Simple" (scope cooperative clients):** the MCP adapter connects with a limited **MemoryClient** role (recall + remember only); the app keeps full access. The daemon **enforces** the role's op-allowlist (a guest-pass client is refused every other op, even if it asks). This does NOT defend against a *malicious* same-uid process forging a full-access role — that process can already connect today, so SP1 does not make it worse. Cryptographic role-proof (capability tokens) is an explicitly deferred future hardening ("Strict"). This is documented as a known limitation, not a gap.
3. **The MCP adapter is a Rust workspace binary**, not Node — so it reuses `bossclawd-proto` + the socket client verbatim rather than reimplementing the security-sensitive wire protocol in JavaScript (drift risk).
4. **Manual wiring in SP1:** point Claude Code at the adapter by hand (a documented `.mcp.json` snippet); SP2 automates it.
5. **Claude Code is the reference target**; Codex is an SP2 config variant. SP1's adapter is agent-agnostic (any MCP client can drive it).

## 3. Verified current-state reality (anchors — re-verify at plan time on this branch's base `54dfefa`)

- **Daemon RPC** — `crates/bossclawd-proto/src/lib.rs`: the `Request` enum (~L81–152, 29 ops) incl. `Recall { onboarded, query, k }` (~L98), `RunIngest` (~L96), `EvolveOnce`/`EvolveStatus` (~L99–102), and the destructive/egress ops `Teardown`, `EnableCloudReasoner`, grant/mandate/model ops. `Response::Recall(Vec<HitWire>)` (~L188); `HitWire` (~L273–279) = event_id, score, sources, kind, hydrated `text`. Handshake `Hello`/`HelloOk`, `PROTO_VERSION = 1` (~L43–67).
- **Dispatch** — `crates/bossclawd/src/server.rs` `dispatch()` (~L134–268), one arm per op (exhaustive; no wildcard — a new op forces a decision). Handshake at ~L58–92.
- **Socket authn** — `crates/bossclawd/src/server.rs` `run_accept_loop` (~L478–515): `peer_cred()` same-uid check (rejects other-uid, fail-closed on unreadable creds); socket is `0600`. Doc comment (~L465–473): *"Within the boundary any same-uid process can invoke every wire op — per-op authorization is deferred to M1b."* **This is the seam SP1 fills. There is NO per-op authz today.**
- **Recall (core)** — `crates/bossclaw-core/src/log.rs:1471` `recall(&self, embedder, query, k, opts) -> Result<Vec<Hit>>`. `Hit` (`recall.rs:34–46`) = event_id, score, sources, kind. Engine wrapper `crates/bossclawd/src/engine/mod.rs:558` `recall(...) -> Vec<HitWithText>` (hydrates snippet from `event.content["text"]`).
- **The write chokepoint** — `crates/bossclaw-core/src/log.rs:788` `append` / `803` `append_pair` take a fully-built signed `Event`. There is **no** production `memory`-write op; every prod `event_type` is `file_ingested`/`page`/`entity`/`link`/`config` (all `memory`-type appends are `#[cfg(test)]`). `memory` IS in `EMBEDDABLE_EVENT_TYPES` (`log.rs:320`) → a written memory is recallable.
- **Taint / trust model (reuse verbatim)** — `is_external(&Event)` exported at `lib.rs:84` (checks `content["origin"] == EXTERNAL_ORIGIN`); `EXTERNAL_ORIGIN = "external"` single-sourced in `graph.rs:60–64`; `file_ingested` stamps `"origin": EXTERNAL_ORIGIN` (`graph.rs:825–826`). `Taint { Clean, Untrusted }` (`actuator.rs:55–61`): any write proposal citing an external source becomes `Untrusted` → loud modal, monotonic (`actuator.rs:126–138`). So a `remember`-written memory stamped `origin=external` is **recallable but never auto-trusted** — the exact fail-safe the strategy wants ("Code memories = external/tainted").
- **Existing MCP** — `agent-bridge-mcp/` is messaging-only (`src/index.mjs`), touches neither `bossclaw-core` recall nor the daemon socket. No memory MCP exists. A prior unbuilt Claude *Desktop* recall-shim design (`docs/superpowers/specs/2026-06-25-air-agent-review-fixes-design.md:88–90`) is read-only, pre-daemon, no write op — reference only.
- **Socket client to reuse** — `apps/desktop/src-tauri/src/engine/client.rs` (`recall` → `request(Request::Recall{...})`), `engine/transport.rs` (frame codec), `engine/daemon.rs` `resolve_socket_path`. The adapter reuses these patterns (ideally the `bossclawd-proto` types + a shared client path).

## 4. Design principles / invariants

- **I1 — The guest pass is enforced at the daemon, not just the adapter.** A `MemoryClient`-role connection is refused every op outside its allowlist at the `dispatch` layer (fail-closed: an op not explicitly allowed for the role is refused). The adapter *also* exposes only two tools (defense in depth), but the daemon is the real boundary.
- **I2 — Remembered memories are external-tainted.** A `remember` op stamps `content["origin"] = EXTERNAL_ORIGIN` so `is_external` returns true; downstream write proposals citing them stay `Untrusted` (no new trust path). Recallable, never auto-actioned.
- **I3 — The app is unchanged.** The role defaults to `App` (full access) for any client that doesn't request a downgrade, so the existing app connects exactly as today. Only a client that *opts into* `MemoryClient` is restricted. (This is the "Simple" bar: least-privilege-by-default + capability tokens is the deferred "Strict" upgrade.)
- **I4 — Fail-safe on daemon-down.** The adapter's tools return a clean MCP error (not a crash) when the daemon socket is unavailable.
- **I5 — Single-source the wire protocol.** The adapter links `bossclawd-proto` (and, where practical, a shared socket-client path) — it never reimplements the frame codec or handshake.

## 5. Architecture

A coding agent (Claude Code) launches the **Rust MCP adapter** as an MCP stdio server. The adapter opens the daemon's Unix socket, performs the handshake **requesting the `MemoryClient` role**, and exposes exactly two MCP tools. Every tool call maps to a daemon op the role is allowed to invoke.

```
Claude Code ──stdio(MCP/JSON-RPC)──▶ air-memory-mcp (Rust adapter, role=MemoryClient)
                                          │ bossclawd-proto over the 0600 Unix socket
                                          ▼
                                     bossclawd daemon ──▶ bossclaw-core (recall / remember)
                                     (dispatch enforces the MemoryClient op-allowlist)
```

**Roles (SP1):**
- `App` — all 29 ops (the default; the app is unchanged).
- `MemoryClient` — allowlist = `{ Recall, Remember }` (plus the handshake). Every other op — especially `Teardown`, `EnableCloudReasoner`, grant/mandate/model ops — is **refused** with a typed "operation not permitted for this role" error.

## 6. Units (each: purpose · interface · dependencies)

- **U1 — `remember` write op** (`bossclaw-core` + `bossclawd` engine + `bossclawd-proto`). *Purpose:* append a signed `memory`-type event stamped `origin=external`, recallable immediately. *Interface:* core `remember(text, …) -> event_id` on the append chokepoint; `Request::Remember { onboarded, text }` → `Response` carrying the new event id. *Depends on:* `EventLog::append`, the taint stamp (`graph.rs`), `EMBEDDABLE_EVENT_TYPES`.
- **U2 — Per-op authorization / the guest-pass gate** (`bossclawd-proto` handshake + `bossclawd/src/server.rs` dispatch). *Purpose:* establish a connection `Role` at handshake and enforce a per-role op-allowlist in `dispatch` (I1, fail-closed). *Interface:* `Hello` gains an optional `role` (default `App`); `dispatch` checks `role.allows(&request)` before executing, else returns a typed `NotPermitted` error. *Depends on:* the existing handshake + dispatch.
- **U3 — The Rust MCP adapter** (new workspace crate, e.g. `crates/air-memory-mcp`, a `bin`). *Purpose:* an MCP stdio server exposing `recall` + `remember`, backed by the daemon as a `MemoryClient`. *Interface:* MCP tools `recall(query, k?) -> hits+snippets` and `remember(text) -> confirmation/event_id`; internally a socket client (reusing `bossclawd-proto` + the client/transport pattern). *Depends on:* U1, U2, `bossclawd-proto`, a Rust MCP-over-stdio implementation (SDK `rmcp` or a minimal hand-rolled JSON-RPC — decided at plan time).
- **U4 — End-to-end proof + manual wiring** (integration test + a documented `.mcp.json` snippet). *Purpose:* prove the loop and the boundary. *Interface:* a test that drives the adapter (or the daemon as a `MemoryClient`) through recall, remember-then-recall, and a refused destructive op; a README/docs snippet to point Claude Code at the adapter by hand. *Depends on:* U1–U3.

## 7. Data flow (the happy loop)
1. Claude Code launches `air-memory-mcp` (stdio) per its `.mcp.json`.
2. Adapter opens the daemon socket, handshakes as `MemoryClient`.
3. Agent calls the `recall` tool → adapter sends `Request::Recall` → daemon (role allows) → hits+snippets back → returned as MCP tool output.
4. Agent calls the `remember` tool with a note → adapter sends `Request::Remember` → daemon appends a signed external-tainted `memory` event → event id back → confirmation to the agent.
5. A later `recall` surfaces the remembered note (it's embeddable).

## 8. Error handling
| Case | Behavior |
|---|---|
| Daemon socket down / not started | Adapter tool returns a clean MCP error ("memory service unavailable"); no crash (I4). |
| `MemoryClient` calls a non-allowlisted op | Daemon returns typed `NotPermitted`; adapter surfaces it (should never happen via the 2-tool surface — defense in depth). |
| `remember` with empty/blank text | Rejected with a clear error (no empty memory events). |
| Not onboarded (no identity yet) | Same `onboarded=false` handling as existing ops (the app must be set up first). |
| Malformed MCP request | Adapter returns a JSON-RPC error per MCP spec. |

## 9. Testing strategy
- **Unit (core/daemon):** `remember` appends a signed `memory` event with `origin=external`, `is_external` true, and it is recallable (a recall after a remember returns it). The authz allowlist: a `MemoryClient` role is refused each destructive/egress op (`Teardown`, `EnableCloudReasoner`, a grant/mandate/model op) and allowed `Recall`/`Remember`; `App` is allowed all. Fail-closed: an op not in the allowlist defaults to refused.
- **Integration (the real boundary):** drive the daemon over the socket as a `MemoryClient` (the `roundtrip.rs`/`test_engine` harness pattern): recall works, remember→recall round-trips, and a destructive op is refused loudly. Then an adapter-level test: the two MCP tools map to the right ops + handle daemon-down.
- **Gates:** `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test` for the touched crates, all green. The adapter has zero keychain access (the daemon holds the keystore), so no signing/keychain test hazard.

## 10. Out of scope (SP1) / deferred
- The desktop Integrations panel + one-click config writing (**SP2**).
- Session-start snapshot, auto-capture, recall-miss instrumentation (**SP3**).
- Codex-specific config (**SP2**; the adapter itself is agent-agnostic).
- Cryptographic role-proof / capability tokens ("**Strict**" hardening — a future SP1.x; SP1 ships the "Simple" bar with a documented limitation).
- Any change to the app's own daemon connection (it stays `App`/full).
- Distribution/packaging of the adapter binary (SP2 handles where it lives + how Claude Code finds it; SP1 proves it via a local build + manual path).

## 11. Open questions to resolve during planning
1. Rust MCP-over-stdio: the `rmcp` official SDK vs a minimal hand-rolled JSON-RPC-2.0 stdio loop (the MCP surface is tiny — 2 tools). Pick the lower-risk, lower-dependency option.
2. The `remember` op's exact input: text only, or text + optional light metadata (e.g. a `source`/`tag`)? Default to text-only (YAGNI) unless a field is clearly needed for SP3.
3. Where the socket-client code for the adapter lives: reuse `apps/desktop/src-tauri/src/engine/{client,transport}.rs` by extracting a small shared crate, vs a thin adapter-local client over `bossclawd-proto`. Decide by how much is truly shared.
4. The `Role` representation on the wire (an enum in the `Hello`) + how the app opts into `App` (explicit vs default) without a migration hazard for the existing app.
5. Exact typed error variant for `NotPermitted` (extends the existing `OpErrorKindWire`).

## 12. Sequencing / branch
Build on branch `feat-memory-hub-sp1-code-loop` (off `main` `54dfefa`). TDD per unit (RED test before impl), subagent-driven execution, per-task review on the security-critical units (U2 authz + U1 write op), a consolidated review on the adapter + wiring. **U2 (authz) is the highest-risk unit** — a hole there re-exposes destructive ops to an external agent — so it gets a dedicated adversarial + security review before the PR.
