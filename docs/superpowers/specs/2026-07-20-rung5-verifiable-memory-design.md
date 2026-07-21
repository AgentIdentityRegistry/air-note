# Rung 5 — Verifiable Memory (signed, exportable, counterparty-verifiable bundles) — Design

**Status:** Rev 3 — Rev 1 owner-approved conversationally 2026-07-20. Review round 1 (2026-07-21):
architect **SOUND-WITH-CHANGES** (6 Major/3 Minor) + critic **REWORK** (1 Blocker/6 Major/3 Minor) →
Rev 2 folded all 19. Re-verification round (SAME reviewers, contexts intact, 2026-07-21): **all 19
confirmed resolved in substance — the critic re-attacked the Blocker fix and it HOLDS** — with a shrunken
second wave (architect N1 Major stamped-metadata disclosure + N2 copy pin; critic NEW-1 Major
owner-adversary label scoping + NEW-2/3/4 Minors) → Rev 3 folds all of those (changelog §11). Final
verdicts: architect **SOUND-WITH-CHANGES**, critic **APPROVE-WITH-CHANGES** — both explicitly plan-ready
with these folds; findings shrank across rounds (the house STOP signal). The §2.2 privacy-over-proof
default (session/ingest items export content-only; stamps would disclose local paths/project/session
metadata) was **owner-RATIFIED 2026-07-21**. Awaiting file-level owner review → planning. Build gated
behind the R4-A dogfood verdict (after Sun 2026-07-27).
**North Star anchor:** `air/memory-strategy-2026-07-03-beat-the-stack` Phase 5 — "M4 verifiable memory
(after the targeted niche-emptiness check). The moat: signed, exportable, counterparty-verifiable bundles —
the secured sharing/sending/receiving Peter named." Beat-the-stack criteria #7 (portable/exportable —
survives tool death) and #8 (verifiable ownership — signed, exportable, counterparty-verifiable).
**Niche re-check (pre-registered open question #2) — RUN 2026-07-20, verdict CONTESTED (was EMPTY):**
`air/competitive-intel-verifiable-memory-2026-07-20`. Four shipped products now cover the bare primitive
(signed hash-chained exportable agent memory/audit): PiQrypt/AISS, ai-memory (AlphaOneDev), Attested
Intelligence AGA, Signet; plus Portable Agent Memory (arXiv 2605.11032, Apache SDK). NONE binds authorship
to a resolvable REGISTRY of agent identities — the surviving wedge is exactly AIR's fused asset (registry +
memory). Window judged months → ship fast, lead the pitch with registry-anchored counterparty verification,
keep the format standards-shaped (C2PA/CAWG VC / SCITT-liftable) rather than standards-built.
**Prior art in-tree (source-verified by the architect review):** every event is Ed25519-signed over its
32-byte hash (`bossclaw-core/src/sign.rs:13-37` `sign_hash`/`verify_hash`, multibase base58btc),
hash-chained via `compute_hash = SHA256(prev_hash ‖ canonical_bytes(event))` (`event.rs:55-81`; JCS
RFC-8785 via `serde_jcs` + NFC via `unicode-normalization`), and DID-attributed (`event.rs:46`,
`log.rs:5356` `signer_did()`); whole-chain verification exists (`EventLog::verify_chain` `log.rs:1259`,
`verify_chain_since` `log.rs:1286`; `verify_rows` checks every row against the SINGLE current brain key,
`log.rs:1361`). External-origin taint is stamped INSIDE the signed content bytes before hashing
(`log.rs:1201-1206`, `ingest.rs:698-718` `is_external`, `graph.rs:75` `EXTERNAL_ORIGIN`) — and note:
`remember()` notes, captured sessions, AND file ingests are ALL stamped external (see §3-H2 taxonomy).
The brain signing key is minted/loaded daemon-side (`crates/bossclawd/src/engine/keystore.rs:17-65`,
single key, no rotation mechanism); the AIR did:wba identity key lives app-side
(`apps/desktop/src-tauri/src/air/did_wba.rs:6-42`) — two genuinely separate keys. The role gate is a
positive allowlist, deny-by-default (`bossclawd-proto/src/lib.rs:74-91`), with the documented same-uid
caveat (`lib.rs:52-54`). Rung 5 adds NO new cryptographic primitive — packaging, binding, verification.

---

## §0 Goal + posture

Let the owner **show a memory to someone else in a way the receiver can verify** — specifically: verify
the **provenance and registered identity of the recorder**, not the truth of the content (invariant H5).
Rung 5 builds the *envelope* (a portable signed bundle of selected memories), the *ID card* (a signed,
seal-covered binding from the brain's signing key to the owner's registered AIR identity), and the
*magnifying glass* (one verifier implementation: Rust library, CLI, and in-browser on the registry site).

Posture mirrors the house rules: exporting is an explicit owner act that never mutates the brain and never
touches the network; the ONLY network-publishing act (the Story-D public pin) sits behind consent (§6).
Verification is fail-closed: one bad byte anywhere = headline ❌.

**Differentiator honesty:** "we sign memory" is table-stakes as of mid-2026 (see niche re-check). The
product claim this design leads with: *the green checkmark resolves to a registered identity a stranger
can look up* — `agentidentityregistry.org` renders WHO recorded these bytes, not just "a key" — while the
copy is explicit about what ✅ does NOT prove (H5).

## §1 Scope — what this spec covers (owner decision 2026-07-20)

One shared **Core** plus two thin story layers; the third story falls out free; the fourth is deferred:

- **Core** — the `bossclaw-canon` + `bossclaw-bundle` crates (§2.1), the export op, the identity-binding
  attestation + its transport op, the verifier (library + CLI + wasm).
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

Sub-project split for building (each: own reviewed plan → subagent build → PR): **SP-V1** = the export
side (extract `bossclaw-canon` FIRST [finding A1], then `bossclaw-bundle` + `SetBinding` + `ExportBundle`
+ CLI verifier + app export UI — Stories A-export and C complete, L1/offline verification usable);
**SP-V2** = the counterparty + public side (air-site verify page + registry-mediated identity lookup +
anchor endpoint + `PublishClaim` op + pin consent UI — Story A-verify and D). The pin op ships in SP-V2
because it is unusable before the registry endpoint exists. SP-V1 merges before SP-V2 starts.

## §2 Architecture

### §2.1 The crates — `bossclaw-canon` (extracted leaf) + `bossclaw-bundle` (new)

**Finding A1 (architect) resolved a structural contradiction in Rev 1:** the verifier must reproduce the
engine's exact canonical bytes, but that code lives in `bossclaw-core`, whose deps (rusqlite/sqlcipher,
hnsw_rs, model2vec-rs, tokenizers, notify, rustix) cannot build for `wasm32-unknown-unknown`. "No engine
dependency" and "reuse the in-tree canonical path" are only simultaneously true if the canonical path
moves to a leaf crate:

- **`bossclaw-canon`** (extracted from `bossclaw-core`, zero behavior change): `Event`/`ModelMeta`
  (`event.rs:11-50`), `canonical_bytes`, `compute_hash`, `sign_hash`/`verify_hash` (`sign.rs`),
  `EXTERNAL_ORIGIN` (`graph.rs:75`), `is_external` (`ingest.rs:716`). Deps — ALL wasm32-clean,
  verify-side needs no RNG: `serde`, `serde_json`, **`serde_jcs`**, **`unicode-normalization`**, `sha2`,
  **`hex`**, `ed25519-dalek`, `multibase` (Rev 1's dep list omitted the three bolded — insufficient).
  `bossclaw-core` re-exports it; a byte-identity test pins that extraction changed nothing.
  **Consensus-critical version pinning (C-NEW-3):** `serde_jcs` + `unicode-normalization` (and the
  `ed25519-dalek`/`multibase` encoding surface) are pinned to the exact versions the log's historical
  events were signed under — a later dependency bump could re-canonicalize history to different bytes,
  silently breaking `verify_chain` and every stamp; §8 carries a cross-version regression vector.
- **`bossclaw-bundle`**: build + verify of `.airmem`, depending on `bossclaw-canon` only (never the
  engine). Canonical-JSON discipline throughout (JCS; no floats in signed material; NFC). Standards-shaped
  on purpose: manifest/receipt shapes liftable into C2PA/VC/SCITT envelopes later without re-signing
  semantics.
- **One verifier on Earth**: native and wasm builds are the same crate; a byte-parity test pins that
  verdicts and error codes can never drift (§8).

### §2.2 The bundle format — `.airmem`

A single canonical-JSON document:

- **`manifest`** (the seal's message): `format_version` (semver; verifier refuses newer-major),
  `created_at`, exporter **`did`** (THE authoritative identity claim, §2.3), brain verifying key
  (multibase), selection description (free text + counts), `merkle_root`, **`binding_hash`** — the hash
  of the canonical `binding` block, so the master seal atomically covers the ID card (critic Blocker C1:
  without this, a counterparty could re-attribute a sealed bundle under a fresh binding of their own).
- **`items[]`** — two verification classes (convergent A4/C5, extended by re-verification A-N1):
  - **Stamped items — external NOTES only** (`remember()` notes; lean canonical bytes: note text +
    origin stamp — the plan verifies the lean-bytes claim against `external_note_event`,
    `log.rs:5396-5412`): the disclosed content IS the original event's canonical byte payload, plus the
    original signature (the write-time wax stamp — verifiable stand-alone by recomputing `compute_hash`
    from the disclosed bytes). Every item stamp is verified against **`manifest.brain_verifying_key`** —
    never a per-item `signed_by_did` resolution (A6: otherwise foreign brains' events could be laundered
    into a green bundle). Mismatch → `ItemStampInvalid{i}`. A stamped item whose recomputed origin is
    NOT external and whose kind is not derived renders **"origin unattested"** — never a brain-authored
    appearance (C-NEW-1b: no "authored" label exists, so an owner-minted origin-less note must not dress
    up as one).
  - **Seal-vouched items — captured-session passages, ingested-file extracts, and derived dossiers**:
    content + display metadata ONLY — no raw event bytes, no standalone write-time stamp; covered by
    Merkle leaf + master seal ("brain-vouched at export"; H1: export-time vouching). Rationale, per
    class: derived-event bytes leak `model_meta.source_event_ids` ULIDs (count + creation timestamps of
    unshared memories) + `prompt_hash` (A4/C5); session/ingest event bytes leak the owner's LOCAL
    provenance metadata — absolute filesystem paths, `project`, `session_id`, `grant_root`, content
    hashes (`log.rs:9955-9968`, `ingest.rs:698-711`; re-verification A-N1). **Owner-RATIFIED default
    (2026-07-21):** trading write-time proof for zero metadata leakage on these classes is a product
    choice, decided for privacy;
    the recorded alternative (keep stamps + disclose the metadata plainly in the §6 review sheet) and
    the salted-commitment upgrade (§9, restores write-time proof leak-free) both remain open.
  - Every item carries its Merkle leaf hash and `kind`; origin labels follow §3-H2 (verifier-derived for
    stamped items; exporter-asserted and rendered weaker for seal-vouched items).
- **`binding`** — the ID card (§2.3), hash-committed by `manifest.binding_hash`.
- **`seal`** — one master Ed25519 signature by the brain key over the canonical manifest (which commits,
  via `merkle_root` + `binding_hash`, to every item and the ID card).

**Merkle construction (frozen in SP-V1, finding A7):** leaf = `H(0x00 ‖ canonical_item_without_leaf_field)`;
internal = `H(0x01 ‖ left ‖ right)` (domain separation defeats leaf/internal second-preimage confusion);
odd node promoted unpaired (no duplicate-last); leaf order = item order. These rules ship in the
cross-repo conformance vectors.

**Analyzed disclosure (corrected twice, now class-complete):** a stamped NOTE's event bytes include its
`prev_hash` — a 32-byte opaque hash of an unshared neighbor: reveals only that *a predecessor exists*
(inherent to an append-only diary), no content beyond the note the owner chose to share. Seal-vouched
items disclose NO event bytes at all — no lineage ULIDs, no local paths/project/session metadata, no
`prompt_hash` (Rev 1's "nothing beyond a predecessor exists" claim was false for derived items; Rev 2's
correction was still false for session/ingest items; Rev 3 makes it true by construction — §8 pins it
structurally). The Merkle tree is the disclosure boundary between items; future single-item sub-proofs
additionally reveal ≈log₂(N) tree depth (noted for the deferred sub-share UX, §9).

### §2.3 The ID card — identity-binding attestation

Two keys exist today and are NOT the same: the daemon's brain signing key (`keystore.rs`) and the app's
AIR did:wba identity key (`did_wba.rs`). The binding attestation closes that gap:

- **Payload** (canonical JSON): `{brain_verifying_key, identity_verifying_key, did,
  purpose:"memory-signing", epoch, created_at}` + `identity_signature` (multibase base58btc — pinned to
  the `sign.rs` encoding discipline; `did_wba.rs:33` raw-64-byte output is wrapped at mint). Finding
  A3/C3: Rev 1 omitted `identity_verifying_key`, making offline card-checking impossible. `epoch` is a
  monotonic integer reserved for future rotation semantics (findings A8/C9 — rotation itself is OUT of
  scope: the engine has no brain-key rotation mechanism, and `verify_chain` is single-key, so "re-mint on
  rotation" is not a current path).
- **Minted app-side** (the ONLY holder of the identity key), once, before first export. **Transport
  (finding A2):** a new App-only op **`SetBinding{attestation}`** — the app signs; the daemon VALIDATES
  before storing (C-NEW-4): the `identity_signature` verifies over the payload against the embedded
  `identity_verifying_key`, AND `binding.brain_verifying_key` equals the daemon's ACTUAL brain verifying
  key (else a buggy app stores a binding that fails `BindingKeyMismatch` on every future export — the
  app must round-trip the key from the daemon and this check closes the loop), AND epoch idempotency is
  first-write-wins (a repeated epoch is rejected, never overwritten); then stores it as a signed config
  event (house pattern — durable, tamper-evident). The daemon only *stores and embeds* the binding; it
  can never mint one. `ExportBundle` is thereby pure-read (§2.4) and refuses with `BindingUnavailable`
  if no binding is stored. "Included verbatim in every bundle" = the latest stored binding (highest
  epoch).
- **What binds the card to the bundle** (critic Blocker C1): the seal covers `binding_hash`, AND the
  verifier REQUIRES `binding.did == manifest.did` and `binding.brain_verifying_key ==
  manifest.brain_verifying_key` (`BindingDidMismatch` / `BindingKeyMismatch`). Identity resolution always
  starts from the **sealed** `manifest.did`, never from the binding's self-declared fields.

### §2.4 Wire ops (daemon) — all App-only

Additive ops, refused for `MemoryClient` by the existing positive-allowlist role gate:

- `BrainVerifyingKey → key` — read-only: the app fetches the daemon's ACTUAL brain verifying key to
  construct the binding payload (§2.3 round-trip; plan-stage addition, ratified 2026-07-21).
- `SetBinding{attestation}` — one-time (idempotent per epoch) binding storage (§2.3).
- `ExportBundle{selection} → Bundle` — pure-read; runs `verify_chain` FIRST and refuses to export from a
  log that does not verify (§7).
- `PublishClaim{merkle_root} → anchor receipt` — Story D; SP-V2; consent-gated (§6); idempotent by root;
  receipt stored as a signed config event.

### §2.5 The magnifying glasses + verification levels

- **Library**: `bossclaw_bundle::verify(bundle) → Verdict` (specific error enum, §7).
- **CLI**: `air-verify bundle.airmem [--offline]`.
- **Web**: `agentidentityregistry.org/verify` (SP-V2) — wasm build; **the file never uploads** (verified
  in-browser; the only network call is the registry lookup); renders verdict + per-item derived origin
  labels + the resolved identity card + the date-vouching level + the H5 disclaimer line.

Three explicit vouching levels, always displayed, never conflated:

- **L1 — self-consistent (offline)**: seal, item stamps, Merkle-root recompute, **`binding_hash`
  recompute** (hash the canonical binding block, compare to the sealed value — C-NEW-2), and binding
  internal consistency (signature against the EMBEDDED `identity_verifying_key` + both §2.3 equality
  checks) all verify.
  **L1 provides ZERO identity assurance** (finding C3: the embedded key is attacker-choosable) — the CLI
  and page render identity as **"unverified (offline)"**, never a green card.
- **L2 — registry-resolved**: `manifest.did` resolves **via agentidentityregistry.org ONLY** —
  registry-mediated lookup keyed by did, NEVER a fetch of the did's own domain (finding C7: standard
  did:wba domain-resolution would turn a crafted bundle into a tracking beacon aimed at an
  attacker-controlled host; non-registry dids → `IdentityUnresolved`, L2 fails cleanly, L1 verdict still
  reported). The registry's published identity key must equal `binding.identity_verifying_key`. L2 copy
  states plainly: **no key-revocation checking exists** (finding C9).
- **L3 — publicly pinned**: the `merkle_root` is found in the public anchor stream → independent
  proof-of-existence-by-time.

**Rendering safety (finding C6 — REQUIRED, SP-V2):** bundle content and registry DID-document fields are
attacker-influenceable text rendered on the registry's own origin. The verify page MUST use
context-appropriate output encoding (DOM `textContent`, never `innerHTML`), ship a strict CSP, and render
HTML/JS/bidi-control payloads inert — pinned by §8 test rows, not left to implementer taste.

## §3 Honesty invariants (the part competitors cut corners on)

- **H1 — Dates say who vouches.** A write-time stamp proves the *brain* signed-and-dated a memory with its
  own clock (L1/L2). Seal-vouched items (sessions/ingests/dossiers) carry export-time vouching only
  (§2.2). Only an L3 pin adds
  third-party time evidence. Verifier copy states which level applies; no overclaiming, ever.
- **H2 — Provenance labels: derived where derivable, honestly scoped everywhere (A5, C4; re-verification
  C-NEW-1, A-N2).** For STAMPED items the verifier recomputes origin by running `is_external` over the
  disclosed, stamp-covered event bytes; any carried display label is cross-checked (`OriginMismatch{i}`).
  For SEAL-VOUCHED items the label is **exporter-asserted** — rendered with explicitly weaker wording
  and visual weight ("labeled machine-derived by the exporter; not independently verified"), never with
  stamped-item confidence. **H2's scope, stated plainly (C-NEW-1):** labels are trustworthy under an
  honest recording daemon and against any third-party tampering; against a MALICIOUS brain-key holder —
  who can mint events with any origin field, or place arbitrary text in seal-vouched items — only H5
  survives, which is exactly why H5, not H2, is the invariant printed on every surface. Taxonomy honesty
  (C4): `remember()` notes, captured sessions, AND file ingests are ALL `origin=external`; the pinned
  external-label copy is **"this brain recorded these bytes; provenance of the underlying text is not
  asserted"** (A-N2 — never affirmatively "from an outside source," which would misread an owner-typed
  note as a third-party document). `is_external=false ∧ kind≠dossier` renders "origin unattested"
  (§2.2). There is deliberately NO "authored" label in this rung: the substrate has no untainted
  owner-authored write class (the taint model working as designed); inventing one is new machinery,
  deferred as an explicit owner decision (§9).
- **H3 — Fail-closed verdicts.** Any single mismatch = headline ❌ (per-item detail listed for diagnosis;
  no partial green).
- **H4 — One verifier.** Native and wasm are the same crate; parity-tested. The webpage can never
  quietly disagree with the CLI.
- **H5 — Provenance is not truth (finding C2).** ✅ proves *which registered identity's brain recorded
  these exact bytes, dated by its own clock* — NOT that the content is true. A brain can sign
  contradictory claims; two contradictory bundles from the same brain both verify green, by design. This
  sentence (owner-approved copy) renders on every verifier surface, not just in this spec.

## §4 Security invariants (house numbering)

- **S1** Export mutates nothing and performs zero network I/O (`SetBinding` is the one-time write, its
  own op). Sharing the produced file is the owner's out-of-band act.
- **S2** The only network-publishing act is `PublishClaim`, behind the one-time consent modal
  (cloud-reasoner family) **plus a lightweight per-pin confirm** showing the exact `merkle_root` and the
  words "permanent, public" (finding C8 — a pin is irreversible; one-time family consent alone gave every
  later click silent publish power).
- **S3** Guest scoping, stated honestly (finding A9): the App-only gate scopes the cooperative
  `MemoryClient` (coding-agent) surface so guests don't *accidentally* export or publish; per the
  in-tree posture (`bossclawd-proto/src/lib.rs:52-54`) it is NOT a defense against a hostile same-uid
  process — that hardening is the deferred "Strict" capability-token work.
- **S4** Export refuses on a non-verifying chain (never seal an envelope from a sick brain — loud error).
- **S5** Bundles disclose plaintext content to their receiver BY DESIGN; the export UI says so in plain
  language before writing the file.
- **S6** The verify page uploads nothing; identity resolution is registry-mediated ONLY (§2.5 L2 — never
  a fetch of the did's own domain), so the registry learns "someone resolved identity X" and no
  third-party host learns anything (finding C7).
- **S7** Verifier rendering safety: output encoding + strict CSP + inert hostile payloads, test-pinned
  (finding C6; details §2.5).

## §5 Data flow summary

Bind (once): App mints attestation (identity key) → `SetBinding` → daemon validates + stores config
event. Export: App UI selection → `ExportBundle` → daemon: `verify_chain` → gather items (stamped:
original event bytes + signatures; derived: content only) → build Merkle tree (§2.2 rules) → embed latest
binding + `binding_hash` → master-seal manifest → canonical `.airmem` → app writes file (temp + rename,
all-or-nothing). Verify (web): drop file → wasm L1 (incl. derived-origin labels + binding equality
checks) → registry-mediated L2 → anchor lookup L3 → render (encoded, CSP). Pin: App UI on an exported
bundle → one-time consent + per-pin confirm → `PublishClaim` → registry anchor stream → receipt config
event.

## §6 Consent + UX surfaces (App)

- Library tab: multi-select (search/filter or select-all = Story C) → "Export signed bundle" → review
  sheet listing exactly what leaves — the ACTUAL payload per class (a stamped note ships its full event
  bytes and the sheet says so; A-N1: the sheet must never show less than what ships), the origin label +
  H1 vouching class per item — plus the S5 plain-language line → save dialog.
- First export: if no binding is stored, the app mints + `SetBinding` transparently and the review sheet
  says so (it publishes nothing; no separate modal).
- "Publish proof of existence" on an exported bundle → S2 one-time consent modal, then per-pin confirm
  (root + "permanent, public") → pin receipt shown with the anchor timestamp.

## §7 Error handling (specific, never vague)

Export refusals: `ChainInvalid` (S4), `EmptySelection`, `BindingUnavailable` (no stored binding —
app-side mint required first). Verify errors (enum, one per failure class): `SealInvalid`,
`ItemStampInvalid{i}` (bad stamp OR stamp not by `manifest.brain_verifying_key`), `ItemHashMismatch{i}`,
`TreeMismatch`, `BindingInvalid` (internal signature check), `BindingKeyMismatch`,
**`BindingDidMismatch`** (C1), **`BindingHashMismatch`** (C-NEW-2: recompute `H(binding)` vs the sealed
`manifest.binding_hash` — a distinct L1 step, exactly like `merkle_root`→`TreeMismatch`),
**`OriginMismatch{i}`** (A5; stamped items only — seal-vouched items carry no recomputable origin),
`IdentityUnresolved` (L2 only — L1 verdict still reported), `FormatTooNew`, `Malformed{detail}`. Pin: idempotent by root; network failure = clean
retry; no half-published state.

## §8 Testing

- **Tamper matrix (the heart):** flip one byte per field class — item content, `ts`, original signature,
  event bytes, Merkle node, seal, binding payload fields, `binding_hash`, manifest fields — each flip
  MUST produce its specific §7 error. No generic-failure passes.
- **Re-attribution forgery (C1):** take a valid sealed bundle, replace `binding` with a fresh attestation
  by a DIFFERENT identity over the same brain key → `BindingDidMismatch` (with `binding_hash` also
  failing the seal — both layers tested independently).
- **Exporter-lied-origin (A5):** hand-craft a bundle whose carried display label disagrees with the
  signed event bytes → `OriginMismatch{i}`.
- **Foreign-event laundering (A6):** item signed by a different brain key, valid in isolation →
  `ItemStampInvalid{i}`.
- Forgery: binding signed by a foreign identity key; swapped binding from another identity's bundle;
  brain-key/manifest-key mismatch — all fail with their `Binding*` errors.
- Round-trip: export → verify green (native), byte-identical verdict + error codes in wasm (H4 parity).
- Merkle vectors: domain-separation (leaf-as-internal second-preimage attempt fails), odd-node rule,
  ordering — in the cross-repo conformance vectors.
- Seal-vouched disclosure: structural test that a bundle containing session/ingest/dossier items
  contains NO `source_event_ids` / `prompt_hash` / absolute paths / `session_id` / `grant_root` / raw
  event bytes for those classes anywhere in the file (A-N1 + A4/C5).
- Canonicalization stability: cross-version regression vectors pin that `bossclaw-canon` reproduces
  byte-identical `canonical_bytes`/`compute_hash` for historical fixture events across dependency
  upgrades (C-NEW-3 — consensus-critical pins on `serde_jcs`/`unicode-normalization`).
- Unattested-origin rendering: a stamped item with `is_external=false ∧ kind≠dossier` renders "origin
  unattested," never a brain-authored appearance (C-NEW-1b, DOM-level assertion on the page).
- **Rendering safety (C6/S7, SP-V2):** memory content and DID-document fields containing
  `<script>`/HTML/bidi controls render inert on the verify page (DOM-level assertion), under CSP.
- Selective disclosure: single-item Merkle proof verifies against the root; structural test that no
  unshared content beyond §2.2's analyzed disclosure is derivable from a bundle.
- Story C sanity: whole-brain export on a large fixture (size + duration bounds).
- Clean-machine e2e: verify a fixture bundle via CLI on a HOME with no brain, no keys, `--offline`
  (L1, identity rendered "unverified") and mocked registry (L2).
- Recall-neutrality: memharness untouched; recall paths byte-untouched by construction (export is a
  reader) — pinned by the existing suites staying green.
- Cross-repo conformance: committed `.airmem` test vectors (incl. Merkle + tamper + re-attribution
  cases) consumed by the air-site verify page's CI.

## §9 Non-goals / deferred

- **Story B** (agent-to-agent verified import) — own spec after A ships.
- Bundle **encryption** (sealing proves provenance, not confidentiality; receivers read plaintext — S5).
- Single-item **sub-share UX** (format supports Merkle sub-proofs from day one; a sub-proof is a NEW
  artifact shape — item + audit path + signed manifest — and additionally reveals ≈log₂(N) tree depth;
  both noted for that design).
- An **"authored" (owner-attested, untainted) write class** — explicit owner decision deferred (H2
  taxonomy honesty): today every user-entered memory is external by design; a first-party authored class
  is new machinery with its own trust story.
- A **redaction-preserving stamp for seal-vouched items** (salted commitments over the withheld
  metadata) so sessions, ingests, AND dossiers could regain write-time vouching without disclosing
  paths or lineage.
- **Display-name look-alike (homoglyph) confusability** of the resolved identity rendered as "WHO" —
  a registry/display-policy concern (S7 makes bidi/HTML inert but cannot make a visually confusable
  registered NAME safe); one-line copy note planned for SP-V2.
- Adopting C2PA/VC/SCITT envelopes (format is standards-*shaped*; adoption is a later, additive layer).
- Brain-key **rotation/revocation lifecycle**: no rotation mechanism exists in-tree (single-key
  `verify_chain`); the binding's `epoch` field reserves the semantics; L2 copy discloses "no revocation
  checking" (C9). Registry-side revocation deserves its own design with the registry hat on.
- Verifier trust of **dossier derivation**: `kind=dossier` is labeled machine-derived; verifying the
  derivation itself is out of scope.

## §10 Open questions → plan stage

- `bossclaw-canon` extraction mechanics (module moves, re-export surface, byte-identity pin test) — SP-V1
  task 1.
- wasm toolchain plumbing (workspace target config, CI job, size budget) — SP-V2 planning input.
- Registry-side anchor endpoint shape (`/claims` vs extending the audit-anchors repo cron) AND the
  registry-mediated did-lookup endpoint (existing did:wba document serving may already suffice — verify
  with the air-site hat on) — SP-V2 planning.
- Binding canonical form: exact field encoding (multibase for both keys; signature multibase base58btc,
  wrapping `did_wba.rs:33`'s raw output) — plan-stage with reviewer input.
- Whether `ExportBundle` streams for Story-C-sized brains or buffers (measure on the fixture first).

## §11 Changelog

- **Rev 3 (2026-07-21):** folded the re-verification round — all 19 round-1 findings confirmed resolved
  in substance (critic re-attacked Blocker C1: HOLDS; no re-attribution path). New folds: A-N1 stamped
  session/ingest metadata disclosure (local paths/`project`/`session_id`/`grant_root`/hashes) → those
  classes moved to the seal-vouched class, stamped = external notes only (owner-ratifiable default;
  keep-stamps-and-disclose alternative + salted-commitment upgrade recorded); A-N2 pinned external-label
  copy ("provenance of the underlying text is not asserted"); C-NEW-1 H2 scoped honestly (trustworthy vs
  third-party tampering under an honest daemon; against a malicious brain-key holder only H5 survives),
  exporter-asserted labels rendered weaker, "origin unattested" rendering defined; C-NEW-2
  `BindingHashMismatch` as an explicit L1 recompute; C-NEW-3 consensus-critical canon version pins +
  cross-version regression vectors; C-NEW-4 `SetBinding` validation (identity-sig verify + brain-key
  match + epoch first-write-wins); homoglyph display-name note (§9). Final verdicts: architect
  SOUND-WITH-CHANGES, critic APPROVE-WITH-CHANGES — both plan-ready with these folds.
- **Rev 2 (2026-07-21):** folded independent review round 1 — architect SOUND-WITH-CHANGES (A1 canon-crate
  extraction + corrected dep list; A2 `SetBinding` transport; A3 identity key in binding; A4 derived-item
  stamp dropped [convergent w/ C5]; A5 verifier-derived origin + `OriginMismatch`; A6 stamps pinned to
  manifest brain key; A7 Merkle rules frozen; A8 rotation softened + epoch; A9 S3 honesty) and critic
  REWORK (C1 **Blocker**: seal covers `binding_hash` + `BindingDidMismatch` + resolution from sealed
  manifest.did + re-attribution test; C2 H5 provenance≠truth + §0 reword; C3 L1 zero-identity-assurance
  rendering [convergent w/ A3]; C4 honest origin taxonomy — no "authored" label, owner decision deferred;
  C5 derived-item ULID/lineage leak closed [convergent w/ A4]; C6 S7 rendering safety + tests; C7
  registry-mediated resolution only; C8 per-pin confirm; C9 revocation disclosure + epoch; C10 encoding/
  latest-binding/displayed-did pins).
- **Rev 1 (2026-07-20):** initial design from the owner brainstorm (scope, approach, honesty rules,
  flows, tamper-matrix testing); self-review fixed the SP-V1/V2 pin-op seam and flagged canonical-bytes
  reuse.
