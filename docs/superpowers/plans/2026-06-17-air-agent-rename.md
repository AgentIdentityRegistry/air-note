# AIR Agent Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the desktop agent app from **BossClaw** to **AIR Agent** — user-facing labels and internal identifiers — while carrying existing secrets/identity across via a one-time migration, and leaving the `bossclaw-core` engine, `air-rs`, AIR Note, the repo slug, and the `bossclaw.ai` DID domain untouched.

**Architecture:** Pure rename in 7 committable tasks ordered so the tree builds green at every commit: (1) npm/workspace identity → (2) TS/JSON wire-ids → (3) TS UI copy → (4) Tauri display + Rust crate identity + cosmetic Rust strings → (5) data-location rename + one-time keychain/file migration (TDD) → (6) docs → (7) final verification + documented-exceptions grep gate. The migration mirrors the existing idempotent read-old→write-new→delete-old pattern in `vault.rs::ensure_loaded_blob`.

**Tech Stack:** Rust (Tauri 2, `keyring`), TypeScript (React, Vite, Vitest, ajv), npm workspaces, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-06-17-air-agent-rename-design.md`

**Branch:** `air-agent-rename` (already created).

---

## Naming reference (single source of truth)

- Display name → `AIR Agent`
- Dotted namespaced ids (`*.plan.v1`, `*.skill.*`, `*.agent.*`) → kebab `air-agent`
- Single snake_case identifiers (Rust crate, blob key) → `air_agent`
- npm scope → `@air-agent`
- Bundle id → `ai.air-agent.desktop` (was `ai.bossclaw.desktop`)
- **Kept on purpose (do NOT change):** the `bossclaw-core` crate + its `docs/superpowers/` specs; the DID domain `bossclaw.ai` in `onboarding.tsx`, `air/tests.rs`, `commands/a2a.rs`, `tests/a2a_command_test.rs`; root `Cargo.toml` member `crates/bossclaw-core`.

---

## Task 1: npm / workspace identity

**Files:**
- Modify: `packages/shared/package.json:2`
- Modify: `apps/desktop/package.json:2,18`
- Modify: `package.json:3,15,16,20` (root)
- Modify: `.github/workflows/build.yml:55,73`

> No TypeScript source imports the `@bossclaw/*` scope (verified by grep), so only the manifests + CI change.

- [ ] **Step 1: Rename the shared package**

In `packages/shared/package.json`: `"name": "@bossclaw/shared"` → `"name": "@air-agent/shared"`.

- [ ] **Step 2: Rename the desktop package + its dep**

In `apps/desktop/package.json`:
- `"name": "@bossclaw/desktop"` → `"name": "@air-agent/desktop"`
- `"@bossclaw/shared": "file:../../packages/shared"` → `"@air-agent/shared": "file:../../packages/shared"`

- [ ] **Step 3: Update root scripts + description**

In `package.json` (root):
- description `...plus the BossClaw reference desktop app.` → `...plus the AIR Agent reference desktop app.`
- `"dev": "npm run dev --workspace @bossclaw/desktop"` → `@air-agent/desktop`
- `"dev:desktop": "npm run dev --workspace @bossclaw/desktop"` → `@air-agent/desktop`
- `"smoke": "npm run typecheck --workspace @bossclaw/desktop"` → `@air-agent/desktop`

- [ ] **Step 4: Update CI workspace references**

In `.github/workflows/build.yml`:
- line ~55 `run: npm run typecheck --workspace @bossclaw/desktop` → `@air-agent/desktop`
- line ~73 `run: npm run build --workspace @bossclaw/desktop` → `@air-agent/desktop`

- [ ] **Step 5: Reinstall + typecheck**

Run:
```bash
cd ~/air-note && npm install && npm run typecheck --workspace @air-agent/desktop
```
Expected: install completes; typecheck exits 0 (no errors).

- [ ] **Step 6: Confirm no stray scope references remain**

Run: `cd ~/air-note && grep -rn "@bossclaw/" --include="*.json" --include="*.mjs" --include="*.ts" --include="*.tsx" --include="*.yml" . | grep -v node_modules`
Expected: no output.

- [ ] **Step 7: Commit**

```bash
cd ~/air-note && git add packages/shared/package.json apps/desktop/package.json package.json .github/workflows/build.yml package-lock.json
git commit -m "refactor(air-agent): rename npm scope @bossclaw -> @air-agent" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: TS/JSON wire-ids (lockstep)

**Files:**
- Modify: `apps/desktop/src/engine/schema/plan.v1.schema.json:3,9`
- Modify: `apps/desktop/src/engine/validatePlan.ts:44,45,95,101,114`
- Modify: `apps/desktop/src/skills/schema/manifest.v1.schema.json:3,4,13,23,46`
- Modify: `apps/desktop/src/skills/validateManifest.ts:13`
- Modify: `apps/desktop/src/metering.ts:23`
- Modify: `skills/verified/registry.json:5,10,15`
- Modify: `skills/verified/{research_assistant,document_converter_markitdown,daily_briefing_framework}/manifest.json:2,6,7`

> These change together because the planner output id, the JSON-Schema that validates it, the TS types, and the bundled skill manifests form one internal contract. There are no external consumers (pre-1.0, packages are `private`), so renaming is safe.

- [ ] **Step 1: Plan schema id**

In `apps/desktop/src/engine/schema/plan.v1.schema.json`: replace both `bossclaw.plan.v1` → `air-agent.plan.v1` (the `$id` line and the `const` line).

- [ ] **Step 2: Plan validator**

In `apps/desktop/src/engine/validatePlan.ts`:
- Rename the type `BossClawPlanV1` → `AirAgentPlanV1` (definition + all usages — lines ~44, ~95, ~101, ~114).
- The schema string literal `schema: "bossclaw.plan.v1"` → `"air-agent.plan.v1"` (line ~45).

- [ ] **Step 3: Manifest schema**

In `apps/desktop/src/skills/schema/manifest.v1.schema.json`:
- `"$id": "https://bossclaw.ai/schemas/manifest.v1.schema.json"` → `"https://agentidentityregistry.org/schemas/manifest.v1.schema.json"`
- `"title": "BossClaw Skill Manifest v1"` → `"AIR Agent Skill Manifest v1"`
- in the `required` array and `properties`: `"minBossClawVersion"` → `"minAirAgentVersion"`
- pattern `"^bossclaw\\.skill\\.[A-Za-z0-9._-]+$"` → `"^air-agent\\.skill\\.[A-Za-z0-9._-]+$"`

- [ ] **Step 4: Manifest validator type**

In `apps/desktop/src/skills/validateManifest.ts`: `minBossClawVersion: string;` → `minAirAgentVersion: string;` (and any reference to that property).

- [ ] **Step 5: Metering key**

In `apps/desktop/src/metering.ts`: `"bossclaw:default"` → `"air-agent:default"`.

- [ ] **Step 6: Verified skills**

In `skills/verified/registry.json` and each `skills/verified/*/manifest.json`:
- `id` values `bossclaw.skill.<name>` → `air-agent.skill.<name>` (names unchanged: `research_assistant`, `document_converter_markitdown`, `daily_briefing_framework`).
- `"author": "BossClaw"` → `"author": "AIR Agent"`.
- `"minBossClawVersion": "0.1.0"` → `"minAirAgentVersion": "0.1.0"`.

- [ ] **Step 7: Build + test (proves the lockstep held)**

Run:
```bash
cd ~/air-note && npm run build --workspace @air-agent/desktop && npm test --workspace @air-agent/desktop
```
Expected: build exits 0; Vitest passes (plan + manifest validation tests accept the new ids and the new `^air-agent\.skill\.` pattern). If a test fails on an old literal, an edit was missed above — fix and re-run.

- [ ] **Step 8: Commit**

```bash
cd ~/air-note && git add apps/desktop/src skills/verified
git commit -m "refactor(air-agent): rename wire ids bossclaw.* -> air-agent.* (lockstep)" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: TS UI copy

**Files:**
- Modify: `apps/desktop/src/onboarding/Welcome.tsx:9,14`
- Modify: `apps/desktop/src/onboarding/Done.tsx:46`
- Modify: `apps/desktop/src/onboarding/NameAgent.tsx:22`
- Modify: `apps/desktop/src/settings/AirSettings.tsx:20`

- [ ] **Step 1: Onboarding copy**

- `Welcome.tsx`: `Welcome to BossClaw` → `Welcome to AIR Agent`; `BossClaw is an open-source AI agent that acts on your behalf.` → `AIR Agent is an open-source AI agent that acts on your behalf.`
- `Done.tsx`: `Open BossClaw` → `Open AIR Agent`.
- `NameAgent.tsx`: placeholder `e.g. Peter's BossClaw` → `e.g. Peter's AIR Agent`.

- [ ] **Step 2: Settings env-var label**

In `AirSettings.tsx`: the `<code>BOSSCLAW_USE_REAL_AIR</code>` reference → `<code>AIR_AGENT_USE_REAL_AIR</code>`. (The Rust side that reads this env var is renamed in Task 4.)

- [ ] **Step 3: Typecheck + build + test**

Run:
```bash
cd ~/air-note && npm run typecheck --workspace @air-agent/desktop && npm run build --workspace @air-agent/desktop && npm test --workspace @air-agent/desktop
```
Expected: all exit 0.

- [ ] **Step 4: Commit**

```bash
cd ~/air-note && git add apps/desktop/src
git commit -m "refactor(air-agent): rename desktop UI copy BossClaw -> AIR Agent" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Tauri display + Rust crate identity + cosmetic Rust strings

**Files:**
- Modify: `apps/desktop/src-tauri/tauri.conf.json:3,15` (productName + window title — NOT `identifier`, that is Task 5)
- Modify: `apps/desktop/src-tauri/Cargo.toml:2,6,7`
- Modify: `apps/desktop/src-tauri/src/llm_stream.rs:467,485,966,967,968,971`
- Modify: `apps/desktop/src-tauri/src/web_access.rs:14,285`
- Modify: `apps/desktop/src-tauri/src/commands/identity.rs:56`
- Modify: `apps/desktop/src-tauri/src/commands/a2a.rs:37` (item_id only — keep the `did:wba:bossclaw.ai` lines 30,31)
- Modify: `apps/desktop/src-tauri/src/main.rs:49,52`

- [ ] **Step 1: Tauri display name + window title**

In `tauri.conf.json`: `"productName": "BossClaw"` → `"AIR Agent"`; `"title": "BossClaw"` → `"AIR Agent"`. Leave `"identifier"` for Task 5.

- [ ] **Step 2: Crate identity**

In `apps/desktop/src-tauri/Cargo.toml`:
- `name = "bossclaw_desktop"` → `name = "air_agent_desktop"`
- `description = "BossClaw desktop agent"` → `description = "AIR Agent desktop agent"`
- `repository = "https://github.com/ahnkwangwook-oss/bossclaw"` → `repository = "https://github.com/AgentIdentityRegistry/air-note"`

- [ ] **Step 3: Assistant/planner prompts + plan-id in `llm_stream.rs`**

- line ~467: `"You are BossClaw assistant. Be concise and practical."` → `"You are AIR Agent assistant. Be concise and practical."`
- line ~485: `"You are the BossClaw desktop assistant for this agent. Agent purpose: {}"` → `"You are the AIR Agent desktop assistant for this agent. Agent purpose: {}"`
- lines ~966-967 (planner system prompt): `You are the BossClaw planning engine running inside BossClaw Desktop.\nBossClaw Desktop has a built-in mission scheduler and can create recurring missions directly.` → replace each `BossClaw` with `AIR Agent` (`...AIR Agent planning engine running inside AIR Agent Desktop.\nAIR Agent Desktop has a built-in...`).
- line ~968: `Output exactly one JSON object that validates against schema id "bossclaw.plan.v1".` → `"air-agent.plan.v1"`.
- line ~971: `- Use "schema": "bossclaw.plan.v1".` → `"air-agent.plan.v1"`.

- [ ] **Step 4: HTTP User-Agent in `web_access.rs`**

- line ~14: `"BossClawDesktop/1.0 (+web.extract)"` → `"AIRAgentDesktop/1.0 (+web.extract)"`
- line ~285: `'BossClawDesktop/1.0 (+web.extract interactive)'` → `'AIRAgentDesktop/1.0 (+web.extract interactive)'`

- [ ] **Step 5: Identity description + demo item id**

- `commands/identity.rs` line ~56: `"BossClaw agent owned by user (v1)"` → `"AIR Agent owned by user (v1)"`.
- `commands/a2a.rs` line ~37: `item_id: "bossclaw-demo-item-001"` → `item_id: "air-agent-demo-item-001"`. **Do not touch lines 30-31** (`did:wba:bossclaw.ai:*` — kept by spec D4).

- [ ] **Step 6: Env var in `main.rs`**

- line ~49 comment: `BOSSCLAW_USE_REAL_AIR` → `AIR_AGENT_USE_REAL_AIR`.
- line ~52: `std::env::var("BOSSCLAW_USE_REAL_AIR")` → `std::env::var("AIR_AGENT_USE_REAL_AIR")`.

- [ ] **Step 7: Build the web frontend (Tauri build reads `dist/`), then build + test the crate**

Run:
```bash
cd ~/air-note && npm run build:web --workspace @air-agent/desktop && cargo build -p air_agent_desktop && cargo test -p air_agent_desktop
```
Expected: web build emits `apps/desktop/dist/`; `cargo build`/`cargo test` for `air_agent_desktop` succeed. If cargo reports an unresolved `bossclaw_desktop` import in `tests/`, change that import to `air_agent_desktop` and re-run.

- [ ] **Step 8: Commit**

```bash
cd ~/air-note && git add apps/desktop/src-tauri/tauri.conf.json apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/src Cargo.lock
git commit -m "refactor(air-agent): rename crate + Tauri display + Rust strings to AIR Agent" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Data-location rename + one-time migration (TDD)

**Files:**
- Modify: `apps/desktop/src-tauri/tauri.conf.json:5` (`identifier`)
- Modify: `apps/desktop/src-tauri/src/vault.rs` (constants, `vault_for_service`, blob migration + test)
- Modify: `apps/desktop/src-tauri/src/air/identity.rs` (key constants, doc comment, `migrate_identity_keys`, `migrate_identity_metadata` + tests)
- Modify: `apps/desktop/src-tauri/src/air/mod.rs` (re-export the two migration fns)
- Modify: `apps/desktop/src-tauri/src/secrets/tests.rs:44` (test service name)
- Modify: `apps/desktop/src-tauri/src/main.rs` (call migrations at top of `.setup()`)

> This is the only task with *new behavior*, so it is test-first. Renaming the keychain service/keys without the migration would orphan the user's API keys + agent identity — they land together in one commit.

- [ ] **Step 1: Write the failing identity-key migration test**

In `apps/desktop/src-tauri/src/air/identity.rs`, append:

```rust
#[cfg(test)]
mod migrate_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MockVault {
        store: Mutex<HashMap<String, String>>,
    }
    impl MockVault {
        fn new() -> Self {
            Self { store: Mutex::new(HashMap::new()) }
        }
    }
    impl SecretsVault for MockVault {
        fn set(&self, k: &str, v: &str) -> Result<(), String> {
            self.store.lock().unwrap().insert(k.to_string(), v.to_string());
            Ok(())
        }
        fn get(&self, k: &str) -> Result<Option<String>, String> {
            Ok(self.store.lock().unwrap().get(k).cloned())
        }
        fn delete(&self, k: &str) -> Result<(), String> {
            self.store.lock().unwrap().remove(k);
            Ok(())
        }
    }

    #[test]
    fn migrates_then_idempotent() {
        let old = MockVault::new();
        let new = MockVault::new();
        old.set("bossclaw.agent.signing_key", "deadbeef").unwrap();
        old.set("bossclaw.agent.air_secret", "s3cr3t").unwrap();

        migrate_identity_keys(&old, &new).unwrap();
        assert_eq!(new.get(IdentityStore::SIGNING_KEY).unwrap(), Some("deadbeef".to_string()));
        assert_eq!(new.get(IdentityStore::AIR_SECRET).unwrap(), Some("s3cr3t".to_string()));
        assert_eq!(old.get("bossclaw.agent.signing_key").unwrap(), None);

        // second run is a no-op
        migrate_identity_keys(&old, &new).unwrap();
        assert_eq!(new.get(IdentityStore::SIGNING_KEY).unwrap(), Some("deadbeef".to_string()));
    }

    #[test]
    fn new_wins_over_old() {
        let old = MockVault::new();
        let new = MockVault::new();
        old.set("bossclaw.agent.signing_key", "OLD").unwrap();
        new.set(IdentityStore::SIGNING_KEY, "NEW").unwrap();
        migrate_identity_keys(&old, &new).unwrap();
        assert_eq!(new.get(IdentityStore::SIGNING_KEY).unwrap(), Some("NEW".to_string()));
    }

    #[test]
    fn copies_identity_json_to_renamed_dir() {
        let base = std::env::temp_dir().join("air_agent_meta_migration_test");
        let _ = std::fs::remove_dir_all(&base); // clean slate
        let old_id = "ai.bossclaw.desktop";
        let old_dir = base.join(old_id);
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("identity.json"), b"{\"did\":\"x\"}").unwrap();
        let new_dir = base.join("ai.air-agent.desktop");

        migrate_identity_metadata(&new_dir, old_id).unwrap();
        assert!(new_dir.join("identity.json").exists());

        // idempotent: does not overwrite an existing new file
        std::fs::write(new_dir.join("identity.json"), b"{\"did\":\"NEW\"}").unwrap();
        migrate_identity_metadata(&new_dir, old_id).unwrap();
        assert_eq!(std::fs::read(new_dir.join("identity.json")).unwrap(), b"{\"did\":\"NEW\"}".to_vec());

        let _ = std::fs::remove_dir_all(&base);
    }
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd ~/air-note && cargo test -p air_agent_desktop migrate_tests 2>&1 | tail -20`
Expected: FAILS to compile — `migrate_identity_keys` / `migrate_identity_metadata` not found, and `IdentityStore::SIGNING_KEY` still equals the old string.

- [ ] **Step 3: Rename the identity key constants + add the two migration fns**

In `apps/desktop/src-tauri/src/air/identity.rs`:
- Add `use std::path::Path;` to the existing `use std::path::PathBuf;` import (i.e. `use std::path::{Path, PathBuf};`).
- Update the storage-layout doc comment: `bossclaw.agent.signing_key` → `air-agent.agent.signing_key`, `bossclaw.agent.air_secret` → `air-agent.agent.air_secret`.
- `const SIGNING_KEY: &'static str = "bossclaw.agent.signing_key";` → `"air-agent.agent.signing_key"`.
- `const AIR_SECRET: &'static str = "bossclaw.agent.air_secret";` → `"air-agent.agent.air_secret"`.

Then add these free functions at module level (after the `impl IdentityStore` block):

```rust
/// One-time rename migration: copy the agent's identity secrets from the legacy
/// keychain service into the renamed one. Idempotent (new wins); best-effort delete of old.
pub fn migrate_identity_keys(old: &dyn SecretsVault, new: &dyn SecretsVault) -> Result<(), String> {
    const PAIRS: [(&str, &str); 2] = [
        ("bossclaw.agent.signing_key", IdentityStore::SIGNING_KEY),
        ("bossclaw.agent.air_secret", IdentityStore::AIR_SECRET),
    ];
    for (old_key, new_key) in PAIRS {
        if new.get(new_key)?.is_some() {
            continue; // already migrated; never clobber
        }
        if let Some(value) = old.get(old_key)? {
            new.set(new_key, &value)?;
            let _ = old.delete(old_key); // best-effort
        }
    }
    Ok(())
}

/// One-time rename migration: copy `identity.json` from the legacy bundle-id data
/// dir to the renamed one (the dir name changes with the Tauri identifier). Never overwrites.
pub fn migrate_identity_metadata(new_data_dir: &Path, legacy_identifier: &str) -> Result<(), String> {
    let new_file = new_data_dir.join(IdentityStore::METADATA_FILE);
    if new_file.exists() {
        return Ok(());
    }
    let Some(base) = new_data_dir.parent() else {
        return Ok(());
    };
    let old_file = base.join(legacy_identifier).join(IdentityStore::METADATA_FILE);
    if !old_file.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(new_data_dir).map_err(|e| e.to_string())?;
    std::fs::copy(&old_file, &new_file).map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 4: Re-export the migration fns from `air/mod.rs`**

In `apps/desktop/src-tauri/src/air/mod.rs`, add to the existing identity re-export so `crate::air::identity::*` is reachable (mirror how `IdentityStore` is exposed):

```rust
pub use identity::{migrate_identity_keys, migrate_identity_metadata};
```

- [ ] **Step 5: Run the identity tests — expect PASS**

Run: `cd ~/air-note && cargo test -p air_agent_desktop migrate_tests 2>&1 | tail -20`
Expected: the three `migrate_tests` pass.

- [ ] **Step 6: Rename vault constants + add `vault_for_service` + blob migration**

In `apps/desktop/src-tauri/src/vault.rs`:

Replace the service/key constants:
```rust
pub const SERVICE_NAME: &str = "ai.air-agent.desktop";
/// Legacy SecretsVault service (pre-rename); also the legacy Tauri bundle id, so it
/// doubles as the old app-data dir name for `migrate_identity_metadata`.
pub const LEGACY_IDENTITY_SERVICE: &str = "ai.bossclaw.desktop";
```

Refactor `default_vault` to share construction:
```rust
pub fn vault_for_service(service: &str) -> Arc<dyn SecretsVault> {
    #[cfg(target_os = "macos")]
    return Arc::new(crate::secrets::macos::MacosVault::new(service));

    #[cfg(target_os = "windows")]
    return Arc::new(crate::secrets::windows::WindowsVault::new(service));

    #[cfg(target_os = "linux")]
    return Arc::new(crate::secrets::linux::LinuxVault::new(service));

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    compile_error!("Unsupported platform — only macOS, Windows, Linux supported");
}

pub fn default_vault() -> Arc<dyn SecretsVault> {
    vault_for_service(SERVICE_NAME)
}
```

Replace the blob constants:
```rust
const BLOB_SERVICE: &str = "AIR Agent";
const BLOB_KEY: &str = "air_agent_vault_blob";
// Legacy blob + individual-key location (pre-rename) — read-only migration source.
const LEGACY_BLOB_SERVICE: &str = "BossClaw";
const LEGACY_BLOB_KEY: &str = "bossclaw_vault_blob";
```

Point the blob/legacy entry helpers at the right services:
- `blob_entry()` → `Entry::new(BLOB_SERVICE, BLOB_KEY)`
- `legacy_entry(key)` → `Entry::new(LEGACY_BLOB_SERVICE, key)` (the individual legacy keys only ever existed under the old service)
- `#[cfg(debug_assertions)] eprintln!(... VAULT_BLOB_KEY)` → `BLOB_KEY`

Add the blob migration:
```rust
/// Copy a keychain blob from one (service, key) to another if the destination is empty.
/// Idempotent (destination wins); best-effort delete of the source.
fn migrate_blob_between(
    old_service: &str,
    old_key: &str,
    new_service: &str,
    new_key: &str,
) -> Result<(), String> {
    let new_entry = Entry::new(new_service, new_key).map_err(|_| "vault".to_string())?;
    if new_entry.get_password().is_ok() {
        return Ok(()); // destination already populated; never clobber
    }
    let old_entry = Entry::new(old_service, old_key).map_err(|_| "vault".to_string())?;
    match old_entry.get_password() {
        Ok(serialized) => {
            new_entry.set_password(&serialized).map_err(|_| "vault".to_string())?;
            let _ = old_entry.delete_password(); // best-effort
            Ok(())
        }
        Err(KeyringError::NoEntry) => Ok(()),
        Err(_) => Ok(()), // best-effort: a read failure must not block startup
    }
}

/// One-time rename migration: adopt the pre-rename API-key blob into the renamed location.
pub fn migrate_legacy_blob_once() {
    let _ = migrate_blob_between(LEGACY_BLOB_SERVICE, LEGACY_BLOB_KEY, BLOB_SERVICE, BLOB_KEY);
}
```

Append a macOS-gated integration test (uses throwaway test services so it never touches the real `AIR Agent` keychain — mirrors the existing `macos_keychain_round_trip` precedent):
```rust
#[cfg(all(test, target_os = "macos"))]
mod blob_migrate_tests {
    use super::*;

    #[test]
    fn migrates_blob_then_idempotent() {
        let old_s = "ai.air-agent.test.legacy";
        let new_s = "ai.air-agent.test.current";
        let key = "blob";
        let payload = "{\"openai_api_key\":\"sk-x\"}";

        Entry::new(old_s, key).unwrap().set_password(payload).unwrap();
        let _ = Entry::new(new_s, key).unwrap().delete_password();

        migrate_blob_between(old_s, key, new_s, key).unwrap();
        assert_eq!(Entry::new(new_s, key).unwrap().get_password().unwrap(), payload);
        assert!(matches!(
            Entry::new(old_s, key).unwrap().get_password(),
            Err(KeyringError::NoEntry)
        ));

        // second run is a no-op
        migrate_blob_between(old_s, key, new_s, key).unwrap();
        assert_eq!(Entry::new(new_s, key).unwrap().get_password().unwrap(), payload);

        let _ = Entry::new(new_s, key).unwrap().delete_password();
    }
}
```

- [ ] **Step 7: Rename the SecretsVault test service**

In `apps/desktop/src-tauri/src/secrets/tests.rs:44`: `MacosVault::new("ai.bossclaw.test")` → `MacosVault::new("ai.air-agent.test")`.

- [ ] **Step 8: Change the Tauri bundle identifier**

In `apps/desktop/src-tauri/tauri.conf.json:5`: `"identifier": "ai.bossclaw.desktop"` → `"identifier": "ai.air-agent.desktop"`.

- [ ] **Step 9: Wire the migrations into `main.rs` setup**

In `apps/desktop/src-tauri/src/main.rs`, replace the start of the `.setup(|app| { ... })` closure (the `let vault = ...; let data_dir = ...; let identity_store = ...;` lines and the `air_client` env-var read) with:

```rust
        .setup(|app| {
            // One-time rename migration (BossClaw -> AIR Agent). Idempotent + best-effort;
            // safe on every launch. MUST run before the vault/identity store are used below.
            let vault = vault::default_vault();
            let legacy_vault = vault::vault_for_service(vault::LEGACY_IDENTITY_SERVICE);
            let _ = air::migrate_identity_keys(legacy_vault.as_ref(), vault.as_ref());
            vault::migrate_legacy_blob_once();

            let data_dir = app.path().app_data_dir().expect("app data dir");
            let _ = air::migrate_identity_metadata(&data_dir, vault::LEGACY_IDENTITY_SERVICE);

            let identity_store = IdentityStore::new(vault, data_dir);

            // Default to mock for dev; toggle to real AIR via AIR_AGENT_USE_REAL_AIR env var.
            let air_client: Arc<dyn air::AirClient> =
                if std::env::var("AIR_AGENT_USE_REAL_AIR").is_ok() {
                    Arc::new(HttpAirClient::production())
                } else {
                    Arc::new(MockAirClient::new())
                };

            app.manage(AppState {
                air_client,
                identity_store,
                inbox: std::sync::Arc::new(crate::inbox::manager::InboxManager::new()),
            });
            Ok(())
        })
```

- [ ] **Step 10: Full build + test**

Run:
```bash
cd ~/air-note && npm run build:web --workspace @air-agent/desktop && cargo build -p air_agent_desktop && cargo test -p air_agent_desktop 2>&1 | tail -30
```
Expected: builds; all tests pass (incl. `migrate_tests`; on macOS also `blob_migrate_tests`). `cargo clippy -p air_agent_desktop --all-targets` should be clean if the repo gates on it.

- [ ] **Step 11: Commit**

```bash
cd ~/air-note && git add apps/desktop/src-tauri Cargo.lock
git commit -m "feat(air-agent): rename keychain/bundle id + one-time bossclaw->air-agent migration" -m "Migrates the agent identity keys, the API-key blob, and identity.json across the rename; idempotent and best-effort so existing secrets/identity survive." -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Docs

**Files:**
- Modify: `README.md:17,18,68,70,75`
- Modify: `crates/air-rs/README.md:14,18,27` (prose only — the crate itself is untouched)

- [ ] **Step 1: Root README**

In `README.md`:
- line ~17: `| ... | **BossClaw** — the reference desktop agent ...` → `**AIR Agent** — the reference desktop agent ...`
- line ~18: `` `@bossclaw/shared` — shared TypeScript used by the desktop app.`` → `` `@air-agent/shared` — shared TypeScript used by the desktop app.``
- line ~68: heading `## BossClaw desktop app` → `## AIR Agent desktop app`
- line ~70: `` `apps/desktop/` is **BossClaw** — the open-source reference desktop agent ...`` → `` `apps/desktop/` is **AIR Agent** — ...``
- line ~75: `npm run typecheck --workspace @bossclaw/desktop` → `@air-agent/desktop`

- [ ] **Step 2: air-rs README prose**

In `crates/air-rs/README.md`, update prose that names the app (lines ~14, ~18, ~27): replace `BossClaw` (the app/product references) with `AIR Agent`, and the link text `[BossClaw](https://github.com/AgentIdentityRegistry/air-note)` → `[AIR Agent](https://github.com/AgentIdentityRegistry/air-note)`. Leave the crate name `air-rs` and the URL itself unchanged.

- [ ] **Step 3: Commit**

```bash
cd ~/air-note && git add README.md crates/air-rs/README.md
git commit -m "docs(air-agent): update README references BossClaw -> AIR Agent" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Final verification + documented-exceptions grep gate

**Files:** none (verification only).

- [ ] **Step 1: Whole-workspace build + all suites**

Run:
```bash
cd ~/air-note && npm install \
  && npm run build:web --workspace @air-agent/desktop \
  && cargo build \
  && cargo test -p air_agent_desktop \
  && npm run typecheck --workspace @air-agent/desktop \
  && npm run build --workspace @air-agent/desktop \
  && npm test --workspace @air-agent/desktop \
  && npm run lint
```
Expected: every step exits 0. (`cargo build` confirms `air-rs` + `bossclaw-core` are unaffected.)

- [ ] **Step 2: Confirm only the documented exceptions remain**

Run:
```bash
cd ~/air-note && grep -rin "bossclaw" --include="*.json" --include="*.toml" --include="*.rs" --include="*.ts" --include="*.tsx" --include="*.js" --include="*.mjs" --include="*.html" --include="*.md" --include="*.css" --include="*.yml" --include="*.yaml" . \
  | grep -v node_modules | grep -v "/target/" | grep -v "/dist/" | grep -v "package-lock" | grep -v "Cargo.lock" \
  | grep -v "crates/bossclaw-core" | grep -v "docs/superpowers" \
  | grep -v "bossclaw.ai" | grep -v "bossclaw.agent" | grep -v "ai.bossclaw"
```
Expected: **no output.** Every remaining `bossclaw` hit is one of the spec §6 exceptions: the `bossclaw-core` engine, its specs under `docs/superpowers/`, the `bossclaw.ai` DID domain, or the legacy keychain coordinates referenced by the migration (`bossclaw.agent.*`, `ai.bossclaw.desktop`). If anything else prints, it is a missed rename — fix it.

- [ ] **Step 3: Manual launch check**

Run: `cd ~/air-note && npm run dev:desktop`
Verify by eye:
- Window title reads **AIR Agent**.
- Onboarding copy reads **AIR Agent** ("Welcome to AIR Agent").
- If you had previously entered LLM API keys / onboarded an identity in the desktop app: they are still present (the migration carried them across). If the app was never onboarded, it starts fresh — also correct.

- [ ] **Step 4: Push + open PR (only after Peter confirms)**

```bash
cd ~/air-note && git push -u origin air-agent-rename
gh pr create --fill --base main --title "Rename desktop app: BossClaw -> AIR Agent"
```

---

## Self-Review (planner)

**Spec coverage:** §4.1 → Task 1 + Task 4 (Cargo). §4.2 → Tasks 4 (display) + 5 (identifier). §4.3 → Task 5. §4.4 → Task 2 (TS/JSON) + Task 4 (`llm_stream.rs` plan-id). §4.5 → Task 3 (TS) + Task 4 (Rust strings). §4.6 → Task 6. §5 migration (3 parts) → Task 5 steps 1-9. §6 kept-exceptions → enforced by Task 7 step 2. §7 verification → Task 7. No spec requirement is unmapped.

**Placeholder scan:** every code step shows full code; every command shows expected output. The two "if cargo reports… / if a test fails…" notes are concrete contingencies (exact fix named), not open-ended TODOs.

**Type consistency:** `migrate_identity_keys(&dyn SecretsVault, &dyn SecretsVault)`, `migrate_identity_metadata(&Path, &str)`, `vault_for_service(&str)`, `migrate_legacy_blob_once()`, `migrate_blob_between(&str,&str,&str,&str)` — names/signatures identical across the test (Step 1), the impl (Steps 3, 6), the re-export (Step 4), and the wiring (Step 9). `IdentityStore::{SIGNING_KEY,AIR_SECRET,METADATA_FILE}` referenced as private consts from within the same module (`identity.rs`) — valid Rust. New wire-ids (`air-agent.plan.v1`, `air-agent.skill.*`, `minAirAgentVersion`) consistent between Task 2 (schema/validators/skills) and Task 4 (`llm_stream.rs` prompt).
