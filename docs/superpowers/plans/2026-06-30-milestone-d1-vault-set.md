# Milestone D — Phase 1: `vault_set` (provider API-key entry) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Wire the missing webview-reachable commands to **save / delete / check** a provider API key in the OS keychain, so the app can store the key the cloud reasoner (Phase 2) and chat both read. Minimal, security-first surface.

**Architecture:** The storage layer already exists — `secret_set_cached` / `secret_get_cached` / `secret_delete_cached` (`apps/desktop/src-tauri/src/vault.rs`) front a single OS-keychain blob. What's missing is any `#[tauri::command]` to reach them from the webview (the frontend `vault.ts` calls five commands that don't exist). This phase adds exactly three thin, **allow-listed** commands (`vault_set`, `vault_has`, `vault_delete`) modeled on the existing `web_auth_set`/`web_auth_has` (`web_access.rs:277-291`), and reshapes `vault.ts` to that minimal surface.

**Tech Stack:** Rust (`air_agent_desktop` Tauri crate, `cargo test`), TypeScript (`@air-agent/desktop`, `tsc`).

**Source decision (Peter, 2026-06-30):** Milestone D split — `vault_set` ships first as its own PR. **Minimal surface: set + delete + a has-key boolean only — NO webview `vault_get`** (the UI never displays a saved key; reading secrets back into JS is an XSS→exfil surface we decline). Providers in scope for the cloud reasoner: Anthropic + OpenAI-compat (so those keys matter most), but the command allow-lists all six provider keys for symmetry with chat.

---

## Design + security rationale

- **Allow-list, not arbitrary keys.** `vault_set`/`vault_has`/`vault_delete` accept ONLY the six provider keys (`openai_compat_api_key`, `openai_api_key`, `anthropic_api_key`, `google_api_key`, `brave_api_key`, `tavily_api_key`). They reject `session_jwt` (auth-flow-managed, not user-entered) and any other/`web_auth::*` key. This stops a compromised webview from overwriting the session token or writing junk into the blob. Validation runs **before** any keychain access.
- **No `vault_get` command.** Internal Rust (`secret_get_cached`) still reads keys for chat/the reasoner; we simply never expose a read-the-secret command to the webview. `vault_has` returns a **bool** (is a non-empty value stored?) — enough to drive UI state ("key saved ✓") without leaking the value. Mirrors `web_auth_has`.
- **Empty-value rejection** before the keychain write (so a blank submit can't clobber a real key with "").
- **Value trimmed** before storage (a pasted trailing newline must not corrupt the `Authorization` header later).
- **Testability falls out of the order:** both rejection paths (bad key, empty value) return before `secret_set_cached`, so they're CI-testable with no keychain. The happy-path write reuses the production-proven `secret_set_cached` path (same one `web_auth_set` uses). We do NOT add a round-trip test that calls `secret_set_cached("anthropic_api_key", …)` — it would write to the user's REAL keychain blob; the existing macOS-gated `blob_migrate_tests` already cover the storage layer with isolated test services.
- **No UI in this phase.** The provider/key entry UI lands in Phase 2 (the Brain-tab Local/Cloud selector), which consumes these commands. This PR is the backend prerequisite + the corrected `vault.ts` bindings.

---

## File Structure

**Modified:**
- `apps/desktop/src-tauri/src/vault.rs` — add the three `#[tauri::command]`s + `validate_provider_key` + `SETTABLE_PROVIDER_KEYS` + unit tests (next to the existing storage fns they wrap).
- `apps/desktop/src-tauri/src/main.rs` — register the three commands in `generate_handler!`.
- `apps/desktop/src/vault.ts` — reshape to the minimal surface (`ProviderVaultKey`, `vaultSet`/`vaultDelete`/`vaultHas`); remove the orphaned `vaultGet`/`vaultLock`/`vaultWarmCache` + `session_jwt` from the settable type.

No new files. No frontend consumers change (vault.ts is confirmed orphaned — zero importers).

---

## Task 1: Allow-listed vault commands + minimal TS bindings

**Files:**
- Modify: `apps/desktop/src-tauri/src/vault.rs`
- Modify: `apps/desktop/src-tauri/src/main.rs` (the `tauri::generate_handler![` list near `web_access::web_auth_set` at `main.rs:116`)
- Modify: `apps/desktop/src/vault.ts`

- [ ] **Step 1: Write the failing test**

Append a `#[cfg(test)] mod tests` at the end of `apps/desktop/src-tauri/src/vault.rs` (these test the pure allow-list + the pre-keychain rejection paths — no keychain touched):

```rust
#[cfg(test)]
mod vault_command_tests {
    use super::*;

    #[test]
    fn allow_list_accepts_the_six_provider_keys() {
        for k in [
            "openai_compat_api_key", "openai_api_key", "anthropic_api_key",
            "google_api_key", "brave_api_key", "tavily_api_key",
        ] {
            assert!(validate_provider_key(k).is_ok(), "{k} should be settable");
        }
    }

    #[test]
    fn allow_list_rejects_session_jwt_and_unknown_keys() {
        assert!(validate_provider_key("session_jwt").is_err());        // auth-managed, not user-set
        assert!(validate_provider_key("web_auth::example.com").is_err()); // goes through web_auth_set
        assert!(validate_provider_key("anything_else").is_err());
        assert!(validate_provider_key("").is_err());
    }

    #[test]
    fn vault_set_rejects_bad_key_before_touching_keychain() {
        // Unknown key → Err from validation, BEFORE secret_set_cached (so no keychain access).
        assert!(vault_set("session_jwt".into(), "x".into()).is_err());
        assert!(vault_set("bogus".into(), "x".into()).is_err());
    }

    #[test]
    fn vault_set_rejects_empty_value_before_touching_keychain() {
        // Valid key but blank value → Err after validation, BEFORE secret_set_cached.
        assert!(vault_set("anthropic_api_key".into(), "   ".into()).is_err());
        assert!(vault_set("anthropic_api_key".into(), "".into()).is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ~/air-note && cargo test -p air_agent_desktop vault_command_tests`
Expected: FAIL TO COMPILE — `validate_provider_key` / `vault_set` not defined.

- [ ] **Step 3: Write the commands**

In `apps/desktop/src-tauri/src/vault.rs`, add after the `secret_delete_cached` fn (after line 183):

```rust
// ---------------------------------------------------------------------------
// Webview-reachable provider-key commands (Milestone D Phase 1).
// Minimal surface: set / delete / has. NO `vault_get` — the webview must never
// be able to read a stored secret back (XSS→exfil). Internal Rust reads still
// use `secret_get_cached`. Every command is allow-listed to provider keys only.
// ---------------------------------------------------------------------------

/// The provider API keys the webview is allowed to write/delete/check. Excludes
/// `session_jwt` (auth-flow managed) and `web_auth::*` (those go via `web_auth_set`).
const SETTABLE_PROVIDER_KEYS: [&str; 6] = [
    "openai_compat_api_key",
    "openai_api_key",
    "anthropic_api_key",
    "google_api_key",
    "brave_api_key",
    "tavily_api_key",
];

/// Reject any key not on the provider allow-list. Runs before any keychain access.
fn validate_provider_key(key: &str) -> Result<(), String> {
    if SETTABLE_PROVIDER_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(format!("'{key}' is not a settable provider key"))
    }
}

/// Save a provider API key into the OS-keychain blob. Trims surrounding whitespace
/// (a pasted trailing newline must not corrupt an auth header). Rejects unknown keys
/// and empty values BEFORE touching the keychain.
#[tauri::command]
pub fn vault_set(key: String, value: String) -> Result<(), String> {
    validate_provider_key(&key)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("API key is empty.".to_string());
    }
    secret_set_cached(&key, trimmed)
}

/// Whether a non-empty value is stored for this provider key. Returns a bool only —
/// NEVER the secret itself (mirrors `web_auth_has`).
#[tauri::command]
pub fn vault_has(key: String) -> Result<bool, String> {
    validate_provider_key(&key)?;
    match secret_get_cached(&key)? {
        Some(value) => Ok(!value.trim().is_empty()),
        None => Ok(false),
    }
}

/// Delete a stored provider API key.
#[tauri::command]
pub fn vault_delete(key: String) -> Result<(), String> {
    validate_provider_key(&key)?;
    secret_delete_cached(&key)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ~/air-note && cargo test -p air_agent_desktop vault_command_tests`
Expected: PASS (4/4).

- [ ] **Step 5: Register the commands**

In `apps/desktop/src-tauri/src/main.rs`, in the `tauri::generate_handler![` list, add directly after the `web_access::web_auth_set,` line (`main.rs:116`):

```rust
            vault::vault_set,
            vault::vault_has,
            vault::vault_delete,
```

(If `web_auth_has` is also registered nearby, place these next to it. Confirm `mod vault;` / `use crate::vault;` already exists — `web_access.rs` imports `crate::vault::secret_set_cached`, so the module is already in the crate. The handler path is `vault::vault_*` because the fns are in `vault.rs`.)

- [ ] **Step 6: Reshape the frontend bindings**

Replace the WHOLE of `apps/desktop/src/vault.ts` with the minimal surface (drops `vaultGet`/`vaultLock`/`vaultWarmCache` + `session_jwt`; `vaultHas` replaces the read):

```ts
import { invoke } from "@tauri-apps/api/core";

/** Provider API keys the user can set from the app (matches the Rust allow-list). */
export type ProviderVaultKey =
  | "openai_compat_api_key"
  | "openai_api_key"
  | "anthropic_api_key"
  | "google_api_key"
  | "brave_api_key"
  | "tavily_api_key";

export const PROVIDER_VAULT_KEYS: ProviderVaultKey[] = [
  "openai_compat_api_key",
  "openai_api_key",
  "anthropic_api_key",
  "google_api_key",
  "brave_api_key",
  "tavily_api_key",
];

/** Save a provider API key into the OS keychain. */
export async function vaultSet(key: ProviderVaultKey, value: string): Promise<void> {
  await invoke("vault_set", { key, value });
}

/** Delete a stored provider API key. */
export async function vaultDelete(key: ProviderVaultKey): Promise<void> {
  await invoke("vault_delete", { key });
}

/**
 * Whether a non-empty value is stored for this key. Returns only a boolean —
 * the secret itself is never read back into the webview.
 */
export async function vaultHas(key: ProviderVaultKey): Promise<boolean> {
  return invoke<boolean>("vault_has", { key });
}
```

- [ ] **Step 7: Gates**

Run: `cd ~/air-note && cargo test -p air_agent_desktop && cargo clippy -p air_agent_desktop --all-targets -- -D warnings && ( cd apps/desktop && npm run typecheck && npx eslint src/vault.ts )`
Expected: tests pass, clippy 0 warnings, typecheck clean, eslint 0 warnings. (`air_agent_desktop` needs `apps/desktop/dist/` for `generate_context!` — it already exists.)

- [ ] **Step 8: Commit**

```bash
cd ~/air-note
git add apps/desktop/src-tauri/src/vault.rs apps/desktop/src-tauri/src/main.rs apps/desktop/src/vault.ts
git commit -m "feat(vault): allow-listed vault_set/has/delete commands; minimal TS bindings (Milestone D Phase 1)"
```

---

## Verification (before PR)

- `cargo test -p air_agent_desktop` (incl. the 4 new validation tests) + `cargo clippy … -D warnings`.
- `npm run typecheck` + `npx eslint src/vault.ts`.
- Confirm the three commands appear exactly once in `main.rs` `generate_handler!`.
- Manual (optional, dev): in the running app, `invoke("vault_has", {key:"anthropic_api_key"})` returns a bool without error.

## Self-review checklist
- Allow-list rejects `session_jwt` + arbitrary keys + `web_auth::*` ✓ (tested).
- No `vault_get` command anywhere; `vault.ts` exports no read-secret function ✓.
- Empty/whitespace value rejected before keychain ✓ (tested).
- `vault.ts` reshape breaks no importer (confirmed zero importers) ✓.

## Deferred to Phase 2 (cloud reasoner)
- The provider/key **entry UI** (Brain-tab Local/Cloud selector) that calls `vaultSet`/`vaultHas`/`vaultDelete`.
- `vault_lock` (cache-clear) + `vault_warm_cache` were intentionally dropped as out-of-minimal-scope; revisit only if a real need appears.
