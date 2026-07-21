# Rung 5 — Verifiable Memory (signed, exportable, counterparty-verifiable bundles) — Design

**Status:** Rev 1 — owner-approved conversationally 2026-07-20 (scope, approach, both design chunks).
Awaiting independent review (architect + critic) and file-level owner review before planning. Build is
gated behind the R4-A dogfood verdict (after Sun 2026-07-27); design/plan work proceeds during the gate week.
**North Star anchor:** `air/memory-strategy-2026-07-03-beat-the-stack` Phase 5 — "M4 verifiable memory
(after the targeted niche-emptiness check). The moat: signed, exportable, counterparty-verifiable bundles —
the secured sharing/sending/receiving Peter named." Beat-the-stack criteria #7 (portable/exportable —
survives tool death) and #8 (verifiable ownership — signed, exportable, counterparty-verifiable).
**Niche re-check (pre-registered open question #2) — RUN 2026-07-20, verdict CONTESTED (was EMPTY):**
`air/competitive-intel-verifiable-memory-2026-07-20`. Four shipped products now cover the bare primitive
(signed hash-chained exportable agent memory/audit): PiQrypt/AISS, ai-memory (AlphaOneDev), Attested
Intelligence AGA, Signet; plus Portable Agent Memory (arXiv 2605.11032, Apache SDK). NONE binds authorship
to a resolvable REGISTRY of agent identities — every entrant stops at a bare operator keypair. The surviving
wedge is exactly AIR's fused asset (registry + memory). Window judged months, not years → ship fast, lead
the pitch with registry-anchored counterparty verification, keep the format standards-shaped (C2PA/CAWG
VC / SCITT-liftable) rather than standards-built.
**Prior art in-tree (the substrate is already built):** every event is Ed25519-signed over its 32-byte
hash (`bossclaw-core/src/sign.rs` `sign_hash`/`verify_hash`, multibase base58btc), hash-chained
(`prev_hash`/`hash`), and DID-attributed (`signed_by_did`, `log.rs` `signer_did()`); whole-chain
verification exists (`EventLog::verify_chain` `log.rs:1259`, `verify_chain_since` `log.rs:1286`). The brain
signing key is minted/loaded daemon-side (`crates/bossclawd/src/engine/keystore.rs`); the AIR did:wba
identity key lives app-side (`apps/desktop/src-tauri/src/air/did_wba.rs`). External-origin taint is a
first-class, transitive event property (`origin=external`, SP1/SP3/M5 taint model). Rung 5 adds NO new
cryptographic primitive — it adds packaging, binding, and verification surfaces.

---

## §0 Goal + posture

Let the owner **show a memory to someone else in a way the receiver can trust without trusting the
owner**. Rung 5 builds the *envelope* (a portable signed bundle of selected memories), the *ID card* (a
signed binding from the brain's signing key to the owner's registered AIR identity), and the *magnifying
glass* (one verifier implementation, usable as a Rust library, a CLI, and in-browser on the registry site).

Posture mirrors the house rules: exporting is an explicit owner act that never mutates the brain and never
touches the network; the ONLY network-publishing act (the Story-D public pin) sits behind a blunt one-time
consent modal in the same family as the cloud-reasoner consent. Verification is fail-closed: one bad byte
anywhere = headline ❌.

**Differentiator honesty:** "we sign memory" is table-stakes as of mid-2026 (see niche re-check). The
product claim this design leads with is: *the green checkmark resolves to a registered identity a stranger
can look up* — `agentidentityregistry.org` renders WHO, not just "a key."

## §1 Scope — what this spec covers (owner decision 2026-07-20)

One shared **Core** plus two thin story layers; the third story falls out free; the fourth is deferred:

- **Core** — the `bossclaw-bundle` envelope crate, the export op, the identity-binding attestation, the
  verifier (library + CLI + wasm).
- **Story A — human-verifiable receipts** (the first demo): selective export in the app; drag-and-drop
  verify page at `agentidentityregistry.org/verify` (cross-repo: page + registry lookup live in
  `~/air-site`; format conformance vectors shared between repos, following the existing AIR conformance-
  vector pattern).
- **Story C — trustable whole-brain backup**: free byproduct — "select everything" export. Named here so
  its tests exist; no extra machinery.
- **Story D — public timestamped claims**: publish ONE hash (the envelope's Merkle root) to the registry's
  public anchor stream (reusing the shipped `audit-anchors` externally-anchored pattern); the verify page
  upgrades date-vouching when it finds a pin.
- **Story B — agent-to-agent verified import: DEFERRED** to its own spec (touches the messaging stack and
  the import-side taint model; design after A ships and is observed).

Sub-project split for building (each: own reviewed plan → subagent build → PR): **SP-V1** = the
export side (envelope crate + `ExportBundle` + binding + CLI verifier + app export UI — Stories A-export
and C complete, L1/offline verification usable); **SP-V2** = the counterparty + public side (air-site
verify page + registry identity lookup + anchor endpoint + `PublishClaim` op + pin consent UI — Story
A-verify and D). The pin op ships in SP-V2 because it is unusable before the registry endpoint exists.
SP-V1 merges before SP-V2 starts.

## §2 Architecture

### §2.1 The envelope crate — `crates/bossclaw-bundle` (new)

The single implementation of build + verify. Constraints:

- **No engine dependency** (portable; consumed by the daemon, the CLI, and compiled to WASM for the
  browser verifier). Dependencies limited to what already ships: `ed25519-dalek`, `sha2`, `serde`/
  `serde_json`, `multibase`. Must compile for `wasm32-unknown-unknown`.
- **Canonical serialization**: JCS-style canonical JSON (RFC 8785 discipline — sorted keys, no floats in
  signed material, UTF-8) so the same bytes hash everywhere. Canonicalization is standards-shaped on
  purpose: the manifest/receipt shapes must be liftable into C2PA/VC/SCITT envelopes later without
  re-signing semantics.
- **One verifier on Earth**: the native and wasm builds are the same crate; a byte-parity test pins that
  the two verdicts and error codes can never drift (§8).

### §2.2 The bundle format — `.airmem`

A single canonical-JSON document:

- **`manifest`**: `format_version` (semver; verifier refuses newer-major), `created_at`, exporter
  `did`, brain verifying key (multibase), selection description (free text + counts), `merkle_root`.
- **`items[]`** — one per exported memory: disclosed content; `kind` (note / captured-session passage /
  ingested-file extract / dossier); `origin` class (**authored vs external — copied verbatim from the
  event log's taint field, §5**); write-time `ts`; original `event_id`; **the original event's canonical
  byte payload and its original signature** (the write-time wax stamp — verifiable stand-alone by
  recomputing the event hash from the disclosed bytes); the item's Merkle leaf hash.
- **`binding`** — the ID card (§2.3).
- **`seal`** — one master Ed25519 signature by the brain key over the canonical manifest (which commits,
  via `merkle_root`, to every item).

**Analyzed disclosure (deliberate):** an item's original event bytes include its `prev_hash` — a 32-byte
opaque hash of an unshared neighbor. This reveals only that *a predecessor exists* (inherent to an
append-only diary) and no content (preimage resistance). Accepted; contrast with the rejected raw-log
Approach 1, which leaked the full internal format and inter-item gap structure. The Merkle tree (not the
chain) is the disclosure boundary: sharing sheet #5 proves membership without revealing #1–#4, and
later single-item sub-proofs (root + path) need no new format.

### §2.3 The ID card — identity-binding attestation

Two keys exist today and are NOT the same: the daemon's brain signing key (`keystore.rs`) and the app's
AIR did:wba identity key (`did_wba.rs`). The binding attestation closes that gap:

- Minted **app-side** (the only holder of the identity key), once, on first export (or re-minted on brain-
  key rotation): the identity key signs canonical `{brain_verifying_key, did, purpose:"memory-signing",
  created_at}`.
- Stored in the event log as a **signed config event** (house pattern — durable, tamper-evident,
  exportable), handed to the daemon over the existing app↔daemon socket; included verbatim in every bundle.
- Counterparty resolution: the verifier resolves `did` → the registry's did:wba document → the identity's
  published key → checks the attestation signature. The registry is the trust anchor; L2 in §2.5.

### §2.4 Wire ops (daemon) — all App-only

Additive ops, refused for `MemoryClient` by the existing positive-allowlist role gate (I8 precedent —
guests can read memories; they cannot exfiltrate signed bundles or publish):

- `ExportBundle{selection} → Bundle` — read-only against the log except the one-time binding-event mint;
  runs `verify_chain` FIRST and refuses to export from a log that does not verify (§7).
- `PublishClaim{merkle_root} → anchor receipt` — Story D; consent-gated (§6); idempotent by root.

### §2.5 The magnifying glasses + verification levels

- **Library**: `bossclaw_bundle::verify(bundle) → Verdict` (specific error enum, §7).
- **CLI**: `air-verify bundle.airmem [--offline]`.
- **Web**: `agentidentityregistry.org/verify` (SP-V2) — wasm build; **the file never uploads** (verified
  in-browser; the only network call is the registry identity lookup); renders verdict + per-item
  origin labels + the resolved identity card + the date-vouching level.

Three explicit vouching levels, always displayed, never conflated:

- **L1 — self-consistent**: stamps, seal, tree, and card all verify internally (offline-checkable).
- **L2 — registry-resolved**: the `did` resolves at the registry and the binding checks against the
  published identity key.
- **L3 — publicly pinned**: the `merkle_root` is found in the public anchor stream → independent
  proof-of-existence-by-time.

## §3 Honesty invariants (the part competitors cut corners on)

- **H1 — Dates say who vouches.** A write-time stamp proves the *brain* signed-and-dated a memory with its
  own clock (L1/L2). Only an L3 pin adds third-party time evidence. Verifier copy states which level
  applies; no overclaiming, ever.
- **H2 — Borrowed knowledge stays labeled.** The log's external-origin taint is copied verbatim into
  `items[].origin`, covered by both the original stamp and the master seal, and rendered by every
  verifier surface ("authored by this brain" vs "recorded by this brain from an outside source").
  Without H2, a green checkmark would launder pasted text into "verified memory" — H2 is what makes ✅
  mean something.
- **H3 — Fail-closed verdicts.** Any single mismatch = headline ❌ (per-item detail listed for diagnosis;
  no partial green).
- **H4 — One verifier.** Native and wasm are the same crate; parity-tested. The webpage can never
  quietly disagree with the CLI.

## §4 Security invariants (house numbering)

- **S1** Export mutates nothing (sole exception: the one-time binding config event) and performs zero
  network I/O. Sharing the produced file is the owner's out-of-band act.
- **S2** The only network-publishing act is `PublishClaim`, behind a blunt one-time consent modal
  (cloud-reasoner-consent family): *one hash goes public, forever; content does not.*
- **S3** Guests cannot export or publish (`Role::allows` — both ops App-only).
- **S4** Export refuses on a non-verifying chain (never seal an envelope from a sick brain — loud error).
- **S5** Bundles disclose plaintext content to their receiver BY DESIGN; the export UI says so in plain
  language before writing the file.
- **S6** The verify page uploads nothing; the registry learns only "someone resolved identity X," never
  bundle contents.

## §5 Data flow summary

Export: App UI selection → `ExportBundle` → daemon: `verify_chain` → gather items (+original event bytes
+ signatures + taint origins) → build Merkle tree → attach binding → master-seal → canonical `.airmem` →
app writes file (temp + rename, all-or-nothing). Verify (web): drop file → wasm L1 → registry lookup L2 →
anchor lookup L3 → render. Pin: App UI on an exported bundle → consent modal (first time) →
`PublishClaim` → registry anchor stream → receipt stored as a config event.

## §6 Consent + UX surfaces (App)

- Library tab: multi-select (search/filter or select-all = Story C) → "Export signed bundle" → review
  sheet listing exactly what leaves (with origin labels) + the S5 plain-language line → save dialog.
- First export: silent one-time binding mint (surfaced in the export review sheet, not a separate modal —
  it publishes nothing).
- "Publish proof of existence" on an exported bundle → S2 consent modal (one-time, forward-only,
  explicit) → pin receipt shown with the anchor timestamp.

## §7 Error handling (specific, never vague)

Export refusals: `ChainInvalid` (S4), `EmptySelection`, `BindingUnavailable` (identity key missing —
app-side mint failed), partial-write impossible (temp+rename). Verify errors (enum, one per failure
class): `SealInvalid`, `ItemStampInvalid{i}`, `ItemHashMismatch{i}`, `TreeMismatch`, `BindingInvalid`,
`BindingKeyMismatch`, `IdentityUnresolved` (L2 only — L1 verdict still reported), `FormatTooNew`,
`Malformed{detail}`. Pin: idempotent by root; network failure = clean retry; no half-published state.

## §8 Testing

- **Tamper matrix (the heart):** flip one byte per field class — item content, `ts`, `origin` label,
  original signature, event bytes, Merkle node, seal, binding, manifest fields — each flip MUST produce
  its specific §7 error. No generic-failure passes.
- Round-trip: export → verify green (native), byte-identical verdict + error codes in wasm (H4 parity).
- Forgery: binding signed by a foreign key; swapped binding from another identity; brain-key/manifest-key
  mismatch — all fail with `Binding*` errors.
- Selective disclosure: single-item Merkle proof verifies against the root; structural test that no
  unshared content or count beyond "a predecessor exists" is derivable from a bundle.
- Story C sanity: whole-brain export on a large fixture (size + duration bounds, streaming build).
- Clean-machine e2e: verify a fixture bundle via CLI on a HOME with no brain, no keys, `--offline` (L1)
  and mocked registry (L2).
- Recall-neutrality: memharness untouched; `vector_index`/recall paths byte-untouched by construction
  (export is a reader) — pinned by the existing suites staying green.
- Cross-repo conformance: committed `.airmem` test vectors consumed by the air-site verify page's CI
  (existing AIR conformance-vector pattern).

## §9 Non-goals / deferred

- **Story B** (agent-to-agent verified import) — own spec after A ships.
- Bundle **encryption** (sealing proves authorship, not confidentiality; receivers read plaintext — S5.
  Encrypted transport is the messaging stack's job, or the owner's out-of-band channel).
- Single-item **sub-share UX** (format supports Merkle sub-proofs from day one; UI later).
- Adopting C2PA/VC/SCITT envelopes (format is standards-*shaped*; adoption is a later, additive layer).
- Brain-key **rotation/revocation lifecycle** beyond re-mint-on-rotation (registry-side revocation
  semantics deserve their own design with the registry hat on).
- Verifier trust of **dossiers as derived content**: a dossier item is labeled `kind=dossier` (machine-
  derived from cited sources); deeper "verify the derivation" is out of scope.

## §10 Open questions → plan stage

- wasm toolchain plumbing (workspace target config, CI job, size budget) — SP-V2 planning input.
- Registry-side anchor endpoint shape (`/claims` vs extending the audit-anchors repo cron) — decide with
  the air-site hat on during SP-V2 planning; SP-V1 only needs the root format frozen.
- Binding event schema fields (exact canonical form) — plan-stage with reviewer input.
- Item write-time evidence MUST reuse the exact in-tree canonical-bytes path the log's `append` hashes
  over (bind to that function; a re-implementation that drifts by one byte silently breaks every
  original-stamp verification) — plan task, not a new design decision.
- Whether `ExportBundle` streams for Story-C-sized brains or buffers (measure on the fixture first).
