# AIR Agent Rename — Design Spec

- **Date:** 2026-06-17
- **Status:** Approved (design) — awaiting spec review before implementation plan
- **Branch:** `air-agent-rename`
- **Scope owner:** Peter

## 1. Motivation

The desktop agent currently named **BossClaw** (`apps/desktop/`) is being rebranded to
**AIR Agent** — positioned as an *Agent Identity Registered* agent: it ships AIR-verified
(identity issued/verified through the [AIR registry](https://agentidentityregistry.org)) and
will carry built-in **AIR Note** messaging for agent-to-agent (A2A) communication.

This is a **product rename of the desktop app only**. It is deliberately *not* a rename of the
whole monorepo, the `bossclaw-core` memory engine, the `air-rs` protocol crate, the `air-note`
repo, or the `AIR Note` messaging product — those names stay.

## 2. Decisions (locked with Peter, 2026-06-17)

| # | Decision | Choice |
|---|----------|--------|
| D1 | Rename target | **The desktop app only** (`apps/desktop`). Leave AIR Note, `air-rs`, `bossclaw-core`, repo slug. |
| D2 | Rename depth | **Full** — user-facing labels **and** internal identifiers (npm/crate/bundle-id/schema-ids). |
| D3 | Keychain handling | **Rename + migrate** — rename storage keys, add a one-time idempotent copy-over so no stored secret/identity is lost. |
| D4 | DID domain `bossclaw.ai` | **Keep for now** — it is identity *infrastructure* (must host DID docs), decoupled from this cosmetic rename; revisit when desktop messaging is wired. |

## 3. Name mapping

| Old | New |
|-----|-----|
| Product name + window title `BossClaw` | `AIR Agent` |
| npm package `@bossclaw/desktop` | `@air-agent/desktop` |
| npm package `@bossclaw/shared` | `@air-agent/shared` |
| Rust crate `bossclaw_desktop` | `air_agent_desktop` |
| Tauri bundle id `ai.bossclaw.desktop` | `ai.air-agent.desktop` |
| Keychain service `BossClaw` (blob vault) | `AIR Agent` |
| Keychain service `ai.bossclaw.desktop` (SecretsVault) | `ai.air-agent.desktop` |
| Keychain keys `bossclaw.agent.signing_key`, `bossclaw.agent.air_secret` | `air-agent.agent.signing_key`, `air-agent.agent.air_secret` |
| Keychain blob key `bossclaw_vault_blob` | `air_agent_vault_blob` |
| Test vault service `ai.bossclaw.test` | `ai.air-agent.test` |
| Schema id `bossclaw.plan.v1` | `air-agent.plan.v1` |
| Skill id pattern `bossclaw.skill.*` | `air-agent.skill.*` |
| Manifest field `minBossClawVersion` | `minAirAgentVersion` |
| TS type `BossClawPlanV1` | `AirAgentPlanV1` |
| Metering key `bossclaw:default` | `air-agent:default` |
| Env var `BOSSCLAW_USE_REAL_AIR` | `AIR_AGENT_USE_REAL_AIR` |
| HTTP User-Agent `BossClawDesktop/1.0` | `AIRAgentDesktop/1.0` |
| Crate `repository` URL (→ archived `ahnkwangwook-oss/bossclaw`) | `https://github.com/AgentIdentityRegistry/air-note` (stale-link fix) |

**Convention:**
- Display name → `AIR Agent`.
- Dotted namespaced ids (`*.plan.v1`, `*.skill.*`, `*.agent.*`) → kebab token `air-agent`
  (e.g. `air-agent.plan.v1`, `air-agent.skill.research_assistant`, `air-agent.agent.signing_key`).
- Single snake_case identifiers (Rust crate, blob key) → `air_agent`
  (e.g. `air_agent_desktop`, `air_agent_vault_blob`).
- npm scope → `@air-agent` (npm requires kebab).

## 4. Edit list by file

### 4.1 Packages / workspace / CI (mechanical — must move together)
- `apps/desktop/package.json` — `name`, dep `@bossclaw/shared`.
- `packages/shared/package.json` — `name`.
- `package.json` (root) — `description`; scripts `dev`, `dev:desktop`, `smoke` (`--workspace` target).
- `apps/desktop/src-tauri/Cargo.toml` — `name`, `description`, `repository`.
- `.github/workflows/build.yml` — lines ~55, ~73 (`--workspace @bossclaw/desktop`).

### 4.2 Tauri config
- `apps/desktop/src-tauri/tauri.conf.json` — `productName`, `identifier`, `app.windows[0].title`.

### 4.3 Keychain / vault + migration
- `apps/desktop/src-tauri/src/vault.rs` — `SERVICE_NAME`, `VAULT_SERVICE_NAME`, `VAULT_BLOB_KEY`; **add migration**.
- `apps/desktop/src-tauri/src/air/identity.rs` — `SIGNING_KEY`, `AIR_SECRET`, storage-layout doc comment.
- `apps/desktop/src-tauri/src/secrets/tests.rs` — test service `ai.bossclaw.test`.
- `apps/desktop/src-tauri/src/main.rs` — call migration once at startup; rename `BOSSCLAW_USE_REAL_AIR`.

### 4.4 Schema / wire ids (lockstep — runtime validation depends on these)
- `apps/desktop/src/skills/schema/manifest.v1.schema.json` — `$id`, `title`, `minBossClawVersion`, the `^bossclaw\.skill\.` pattern.
- `apps/desktop/src/engine/schema/plan.v1.schema.json` — `$id`, the `const`.
- `apps/desktop/src/engine/validatePlan.ts` — `BossClawPlanV1` type, `schema` string literal(s).
- `apps/desktop/src/skills/validateManifest.ts` — `minBossClawVersion` field.
- `apps/desktop/src/metering.ts` — `bossclaw:default` key.
- `apps/desktop/src-tauri/src/llm_stream.rs` — planner-prompt schema-id refs (`bossclaw.plan.v1`) **and** persona strings ("BossClaw assistant", "BossClaw planning engine / BossClaw Desktop").
- `skills/verified/registry.json` — skill `id`s.
- `skills/verified/{research_assistant,document_converter_markitdown,daily_briefing_framework}/manifest.json` — `id`, `author`, `minBossClawVersion`.

### 4.5 UI copy / prompts (safe)
- `apps/desktop/src/onboarding/Welcome.tsx` — "Welcome to BossClaw", body.
- `apps/desktop/src/onboarding/Done.tsx` — "Open BossClaw".
- `apps/desktop/src/onboarding/NameAgent.tsx` — placeholder.
- `apps/desktop/src/settings/AirSettings.tsx` — env-var reference text.
- `apps/desktop/src-tauri/src/web_access.rs` — User-Agent strings (lines ~14, ~285).
- `apps/desktop/src-tauri/src/commands/identity.rs` — identity description string.
- `apps/desktop/src-tauri/src/commands/a2a.rs` — demo `item_id` (the `did:wba:bossclaw.ai:*` strings stay — see §6).

### 4.6 Docs
- `README.md` — lines ~17, ~18, ~68, ~70, ~75 (app rows/section + workspace example).
- `crates/air-rs/README.md` — lines ~14, ~18, ~27 (**prose only** — references to the app by name; the crate itself is untouched).

## 5. Migration design (D3)

Mirror the existing idempotent pattern in `vault.rs::ensure_loaded_blob` (read-old → write-new →
best-effort-delete-old; "if new exists, skip"). One startup function (`migrate_legacy_bossclaw_*`)
invoked once from `main.rs` setup, covering three stores:

1. **API-key blob** — old service `BossClaw` / key `bossclaw_vault_blob` → new `AIR Agent` / `air_agent_vault_blob`.
2. **Agent identity** — old service `ai.bossclaw.desktop` / keys `bossclaw.agent.{signing_key,air_secret}` → new `ai.air-agent.desktop` / `air-agent.agent.*`.
3. **`identity.json` metadata** — the Tauri app-data dir is derived from the bundle id, so it changes when the id changes. Copy `identity.json` (the only current app-data file, per `identity.rs`; re-verify none were added during implementation) from the **old** id's data dir to the **new** one if the new is absent.

**Invariants:**
- Idempotent: a second launch finds the new keys present and does nothing.
- New-wins: never overwrite an existing new value with an old one.
- Best-effort delete of old entries after a verified successful copy; failure to delete is non-fatal.
- No secret value is ever logged.

**Test (new):** seed old-named keychain entries (using the test vault) + a fake old `identity.json`,
run the migration, assert new keys populated + old removed + second run is a no-op.

## 6. Deliberately NOT changed (intentional residual "bossclaw")

- The entire `bossclaw-core` crate and its specs under `docs/superpowers/` (the memory engine — D1).
- The DID domain `bossclaw.ai` (D4): `apps/desktop/src/state/onboarding.tsx`, `src/air/tests.rs`,
  `src/commands/a2a.rs`, `tests/a2a_command_test.rs`. These read `did:wba:bossclaw.ai:*` and stay.
- Root `Cargo.toml` workspace member `crates/bossclaw-core`.

These are documented exceptions, not misses. A post-rename grep will still show `bossclaw` hits in
exactly these locations and nowhere else in the app.

## 7. Verification plan

- `cargo build -p air_agent_desktop` + `cargo test -p air_agent_desktop` (rename compiles; tests, incl. new migration test, pass).
- `cargo build` (whole workspace — confirm `air-rs` + `bossclaw-core` unaffected).
- `npm install` (root) then `npm run typecheck --workspace @air-agent/desktop` + `npm run build --workspace @air-agent/desktop` + `npm test --workspace @air-agent/desktop` (schema-id lockstep holds; plan + skill validation still passes).
- `npm run lint` in the app.
- CI: `.github/workflows/build.yml` references resolve.
- **Manual launch** (`npm run dev:desktop`): window titled "AIR Agent"; existing API keys + agent identity survive the migration; onboarding copy reads "AIR Agent".
- Final grep for `bossclaw`/`BossClaw` in the app tree returns only the §6 documented exceptions.

## 8. Out of scope / future

- Renaming the `bossclaw-core` memory engine (kept by D1).
- Changing the DID domain (deferred by D4).
- Renaming the `air-note` repo, the `AIR Note` product, or `air-rs`.
- App icon / brand artwork redesign (icons under `apps/desktop/src-tauri/icons/` keep their files; only names/text change).
- Publishing the renamed npm packages (they are `private: true`; no registry action).

## 9. Risks

- **Schema-id lockstep:** missing one of the `air-agent.plan.v1` / `air-agent.skill.*` /
  `minAirAgentVersion` edits breaks validation at runtime, not compile time → mitigated by the app's
  vitest + cargo tests and the final grep.
- **Keychain migration correctness:** mitigated by the new migration unit test + manual launch check.
- **Tauri identifier change resets app-data dir:** handled by the §5.3 file copy; if no real desktop
  identity exists yet, the copy is a harmless no-op.
