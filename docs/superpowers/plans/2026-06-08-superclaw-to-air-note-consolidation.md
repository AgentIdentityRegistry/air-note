# SuperClaw → air-note Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the BossClaw desktop app (and its shared lib + skills) out of the legacy `~/SuperClaw` monorepo into `~/air-note`, making air-note the single source of truth; drop SuperClaw's stale `agent-bridge-mcp` + duplicate `crates/air-rs`; archive the old repo.

**Architecture:** Clean file import (one logical import, no history graft) into a feature branch `feat/import-bossclaw-desktop` in air-note. air-note becomes a polyglot monorepo: a Cargo workspace (`crates/air-rs` + `apps/desktop/src-tauri`) and an npm workspace (`apps/*` + `packages/*`), with `agent-bridge-mcp` left standalone and untouched. The desktop app's `air-rs` path-dep (`../../../crates/air-rs`) resolves unchanged because the directory layout is preserved. The public push (PR) is the LAST step, gated by a secrets/file-list review.

**Tech Stack:** Tauri 2 + React/Vite (desktop app), Rust (crate + app backend), Node ≥22 (messaging), npm workspaces, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-06-08-superclaw-to-air-note-consolidation-design.md`

**Key names (use exactly):**
- Desktop npm package: `@bossclaw/desktop` · shared lib: `@bossclaw/shared`
- Desktop Rust crate: `bossclaw_desktop` · SDK crate: `air-rs`
- Source repo: `~/SuperClaw` (`ahnkwangwook-oss/bossclaw`) · Target: `~/air-note` (`AgentIdentityRegistry/air-note`)
- Branch (already created): `feat/import-bossclaw-desktop`

---

### Task 1: Import the app source (tracked files only)

Copies ONLY git-tracked files (no `node_modules`/`target`/`dist` junk) via `git archive`, then drops the nested Cargo.lock so the workspace root lock is the single authority.

**Files:**
- Create (copied in): `apps/desktop/**`, `packages/shared/**`, `skills/verified/**`, `tsconfig.base.json`, `eslint.config.mjs`, `.prettierrc`, `.prettierignore`
- Delete after copy: `apps/desktop/src-tauri/Cargo.lock`

- [ ] **Step 1: Copy tracked files from SuperClaw into air-note**

```bash
cd ~/air-note
git -C ~/SuperClaw archive HEAD \
  apps/desktop packages/shared skills/verified \
  tsconfig.base.json eslint.config.mjs .prettierrc .prettierignore \
  | tar -x -C ~/air-note
```

- [ ] **Step 2: Drop the nested Cargo.lock (root workspace lock wins)**

```bash
rm -f ~/air-note/apps/desktop/src-tauri/Cargo.lock
```

- [ ] **Step 3: Verify the import landed and no junk came along**

```bash
cd ~/air-note
test -f apps/desktop/package.json && test -f apps/desktop/src-tauri/Cargo.toml && \
  test -f packages/shared/package.json && test -f skills/verified/registry.json && \
  echo "OK core files present"
test ! -d apps/desktop/node_modules && test ! -d apps/desktop/src-tauri/target && \
  test ! -f apps/desktop/src-tauri/Cargo.lock && echo "OK no junk / nested lock removed"
git status -s | sed 's/^/  /' | head -20
```
Expected: both `OK …` lines print; `git status` shows new untracked `apps/`, `packages/`, `skills/`, and the 4 config files.

- [ ] **Step 4: Fence the carried formatter off air-note's existing code**

Append to `.prettierignore` so `prettier --write .` never rewrites the messaging stack or crate:

```
# air-note code owned outside the BossClaw workspace — do not reformat
agent-bridge-mcp/
crates/
docs/
target/
```

- [ ] **Step 5: Commit**

```bash
cd ~/air-note
git add apps packages skills tsconfig.base.json eslint.config.mjs .prettierrc .prettierignore
git commit -m "feat: import BossClaw desktop app, shared lib, skills, lint config from SuperClaw"
```

---

### Task 2: Add the root npm workspace + extend .gitignore

**Files:**
- Create: `package.json` (repo root)
- Modify: `.gitignore`

- [ ] **Step 1: Create the root `package.json`**

Write `~/air-note/package.json` exactly:

```json
{
  "name": "air-note",
  "description": "AIR Note — cryptographically-signed E2E messaging for AI agents, plus the BossClaw reference desktop app. Built on AIR (Agent Identity Registry).",
  "license": "Apache-2.0",
  "repository": {
    "type": "git",
    "url": "https://github.com/AgentIdentityRegistry/air-note.git"
  },
  "private": true,
  "workspaces": [
    "apps/*",
    "packages/*"
  ],
  "scripts": {
    "dev": "npm run dev --workspace @bossclaw/desktop",
    "dev:desktop": "npm run dev --workspace @bossclaw/desktop",
    "build": "npm run build --workspaces --if-present",
    "lint": "npm run lint --workspaces --if-present",
    "typecheck": "npm run typecheck --workspaces --if-present",
    "smoke": "npm run typecheck --workspace @bossclaw/desktop",
    "format": "prettier --write .",
    "format:check": "prettier --check ."
  },
  "devDependencies": {
    "@eslint/js": "^9.10.0",
    "eslint": "^9.10.0",
    "eslint-plugin-react": "^7.36.1",
    "eslint-plugin-react-hooks": "^5.1.0",
    "globals": "^15.9.0",
    "prettier": "^3.3.3",
    "typescript": "^5.6.2",
    "typescript-eslint": "^8.5.0"
  }
}
```

Note: workspaces globs are `apps/*` + `packages/*` only — `agent-bridge-mcp` is deliberately excluded so the messaging package stays standalone and untouched.

- [ ] **Step 2: Extend `.gitignore`**

Append to `~/air-note/.gitignore`:

```
# BossClaw desktop app build output
dist
build
*.tsbuildinfo

# Env files — NEVER commit (only .env.example is tracked)
.env
.env.*
.env.local

# Local agent tooling state (machine-specific; GBrain is canonical for handoffs)
.claude/
.omc/
.gstack/
NEXT-SESSION.md
```

- [ ] **Step 3: Verify JSON parses + workspaces present**

```bash
cd ~/air-note
node -e "const p=require('./package.json'); if(!p.workspaces.includes('apps/*')) throw new Error('missing workspace'); console.log('OK package.json valid, workspaces:', p.workspaces.join(', '))"
```
Expected: `OK package.json valid, workspaces: apps/*, packages/*`

- [ ] **Step 4: Commit**

```bash
cd ~/air-note
git add package.json .gitignore
git commit -m "build: add root npm workspace + ignore desktop build output and env files"
```

---

### Task 3: Wire the Cargo workspace

**Files:**
- Modify: `Cargo.toml` (repo root)

- [ ] **Step 1: Add the desktop backend as a workspace member**

Replace the `members` line in `~/air-note/Cargo.toml` so the file reads:

```toml
[workspace]
resolver = "2"
members = ["crates/air-rs", "apps/desktop/src-tauri"]
```

- [ ] **Step 2: Verify the workspace resolves both crates**

```bash
cd ~/air-note
cargo metadata --no-deps --format-version 1 \
  | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const m=JSON.parse(s).packages.map(p=>p.name).sort();const have=['air-rs','bossclaw_desktop'].every(n=>m.includes(n));console.log(have?'OK members:':'MISSING:',m.join(', '))})"
```
Expected: `OK members: air-rs, bossclaw_desktop` (order may vary; both present).

- [ ] **Step 3: Verify the crate still compiles in the new workspace**

```bash
cd ~/air-note
cargo check -p air-rs
```
Expected: `Finished` with no errors.

- [ ] **Step 4: Commit (Cargo.toml + any minimal Cargo.lock touch)**

```bash
cd ~/air-note
git add Cargo.toml Cargo.lock
git commit -m "build: add apps/desktop/src-tauri to the cargo workspace"
```

---

### Task 4: Install the npm workspace + typecheck the desktop app

**Files:**
- Create: `package-lock.json` (repo root)

- [ ] **Step 1: Install the workspace (links @bossclaw/shared into the app)**

```bash
cd ~/air-note
npm install
```
Expected: installs without error; creates root `package-lock.json` and a hoisted `node_modules/` with `@bossclaw/shared` symlinked into the workspace.

- [ ] **Step 2: Verify the shared lib is wired into the desktop app**

```bash
cd ~/air-note
test -L node_modules/@bossclaw/shared || test -d node_modules/@bossclaw/shared && echo "OK @bossclaw/shared linked"
```
Expected: `OK @bossclaw/shared linked`

- [ ] **Step 3: Typecheck the desktop app through the workspace**

```bash
cd ~/air-note
npm run typecheck --workspace @bossclaw/desktop
```
Expected: TypeScript completes with no errors (exit 0).

- [ ] **Step 4: Commit the lockfile**

```bash
cd ~/air-note
git add package-lock.json
git commit -m "build: lockfile for the BossClaw npm workspace install"
```

---

### Task 5: Compile-check the desktop Rust backend + prove messaging is untouched

The full Tauri build is heavy; `cargo check` is enough to prove the backend compiles against the workspace `air-rs`. `tauri::generate_context!` needs a frontend `dist/` to exist.

**Files:** none committed (the regenerated root `Cargo.lock` is committed).

- [ ] **Step 1: Create the frontend dist placeholder (required by tauri build macro)**

```bash
mkdir -p ~/air-note/apps/desktop/dist
```

- [ ] **Step 2: Compile-check the desktop backend (heavy first build — run in background)**

```bash
cd ~/air-note
cargo check -p bossclaw_desktop
```
Expected: downloads deps then `Finished`. If it fails on a missing system lib (e.g. webkit on Linux), that's an environment issue, not a migration error — note it and continue; macOS (Peter's machine) has the Tauri toolchain already.

- [ ] **Step 3: Regression — air-note messaging tests still green (proves nothing was disturbed)**

```bash
cd ~/air-note/agent-bridge-mcp
node --test
```
Expected: the existing suite passes (per protocol baseline ~235 tests, 0 fail). Note the exact pass count.

- [ ] **Step 4: Commit the fully-resolved root Cargo.lock**

```bash
cd ~/air-note
git add Cargo.lock
git commit -m "build: resolve workspace Cargo.lock with the desktop backend deps" --allow-empty
```

---

### Task 6: Carry + fix the CI workflows

The workflows were NOT in Task 1's archive — they need path fixes for the air-note workspace. `build.yml`'s cache pointed at the member-level `target/` (wrong in a workspace) and `conformance.yml` still used the pre-rename `a2a-rs` / `specs/a2a` names.

**Files:**
- Create: `.github/workflows/build.yml`, `.github/workflows/conformance.yml`

- [ ] **Step 1: Copy the two workflows in**

```bash
cd ~/air-note
mkdir -p .github/workflows
git -C ~/SuperClaw show HEAD:.github/workflows/build.yml       > .github/workflows/build.yml
git -C ~/SuperClaw show HEAD:.github/workflows/conformance.yml > .github/workflows/conformance.yml
```

- [ ] **Step 2: Fix `build.yml` — workspace target path + lockfile cache key**

In `.github/workflows/build.yml`:
- Change the cache `path:` block line `apps/desktop/src-tauri/target` → `target`
- Change the cache `key:` `hashFiles('apps/desktop/src-tauri/Cargo.lock')` → `hashFiles('Cargo.lock')`

(Leave the `working-directory: apps/desktop/src-tauri` cargo steps as-is — cargo resolves the workspace from a member dir correctly.)

- [ ] **Step 3: Replace `conformance.yml` with the air-note-correct version**

Write `.github/workflows/conformance.yml` exactly:

```yaml
name: AIR Conformance — Rust

on:
  pull_request:
    paths:
      - "specs/air/draft-1/**"
      - "crates/air-rs/**"
      - ".github/workflows/conformance.yml"
  push:
    branches:
      - "main"

jobs:
  rust-conformance:
    name: Rust cross-language conformance vectors
    runs-on: ubuntu-latest

    steps:
      - name: Checkout air-note
        uses: actions/checkout@v4
        with:
          path: air-note

      - name: Checkout air-site (vectors source)
        uses: actions/checkout@v4
        with:
          repository: ahnkwangwook-oss/air-site
          path: air-site

      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable

      - name: Cache Cargo registry
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            air-note/target
          key: ${{ runner.os }}-cargo-conformance-${{ hashFiles('air-note/Cargo.lock') }}
          restore-keys: ${{ runner.os }}-cargo-conformance-

      - name: Run Rust conformance tests
        working-directory: air-note
        env:
          VECTORS_PATH: ${{ github.workspace }}/air-site/specs/air/draft-1/test-vectors.json
        run: cargo test --features conformance -p air-rs -- --nocapture
```

- [ ] **Step 4: Verify no stale `a2a` references remain**

```bash
cd ~/air-note
grep -rn "a2a-rs\|specs/a2a\|src-tauri/target\|src-tauri/Cargo.lock" .github/workflows/ && echo "STALE REFS FOUND — fix above" || echo "OK no stale refs"
```
Expected: `OK no stale refs`

- [ ] **Step 5: Commit**

```bash
cd ~/air-note
git add .github/workflows/build.yml .github/workflows/conformance.yml
git commit -m "ci: bring over build + conformance workflows, fix paths for the air-note workspace"
```

---

### Task 7: Update the README to cover both products

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Extend the "What's in this repo" table**

In `~/air-note/README.md`, add these rows to the existing table (after the `crates/air-rs/` row):

```markdown
| [`apps/desktop/`](apps/desktop/) | **BossClaw** — the reference desktop agent (Tauri + React): AIR identity onboarding, multi-provider LLM streaming, OS-keychain secrets. Consumes `crates/air-rs`. |
| [`packages/shared/`](packages/shared/) | `@bossclaw/shared` — shared TypeScript used by the desktop app. |
| [`skills/verified/`](skills/verified/) | Bundled, manifest-described agent skills (daily briefing, document converter, research assistant). |
```

- [ ] **Step 2: Add a BossClaw section before `## License`**

```markdown
## BossClaw desktop app

`apps/desktop/` is **BossClaw** — the open-source reference desktop agent built on AIR identity, now living in this monorepo alongside the messaging stack it uses.

```bash
npm install                 # from repo root — installs the workspace
npm run dev:desktop         # run the Tauri app in dev mode
npm run typecheck --workspace @bossclaw/desktop
```

The Rust backend (`apps/desktop/src-tauri/`) is a member of the same Cargo workspace as `crates/air-rs`, which it depends on directly.
```

- [ ] **Step 3: Verify + commit**

```bash
cd ~/air-note
grep -q "BossClaw desktop app" README.md && grep -q "apps/desktop/" README.md && echo "OK readme updated"
git add README.md
git commit -m "docs: README covers the BossClaw desktop app alongside AIR Note messaging"
```

---

### Task 8: Final local verification + publish-safety gate (STOP for Peter)

No network actions. This is the gate before the one-way public push.

- [ ] **Step 1: Full local green check**

```bash
cd ~/air-note
echo "== cargo ==" && cargo check -p air-rs && cargo metadata --no-deps --format-version 1 >/dev/null && echo "cargo OK"
echo "== typecheck ==" && npm run typecheck --workspace @bossclaw/desktop && echo "ts OK"
echo "== messaging regression ==" && (cd agent-bridge-mcp && node --test 2>&1 | tail -3)
```
Expected: `cargo OK`, `ts OK`, messaging suite passing.

- [ ] **Step 2: Re-run the secrets scan on the final imported tree**

```bash
cd ~/air-note
git ls-files apps packages skills | grep -iE '\.env$|secret|credential|\.pem$|\.key$|id_rsa|\.p12$|keystore' | grep -v '\.env\.example' && echo "REVIEW THESE" || echo "OK no secret-ish filenames"
git grep -nIE 'sk-[A-Za-z0-9]{20}|xoxb-|ghp_|AKIA[0-9A-Z]{16}|BEGIN (RSA|EC|OPENSSH|PRIVATE) PRIVATE KEY' -- apps packages skills | head || echo "OK no hardcoded secret values"
```
Expected: `OK no secret-ish filenames` and `OK no hardcoded secret values`.

- [ ] **Step 3: Produce the exact "what goes public" list for Peter**

```bash
cd ~/air-note
echo "=== commits on this branch ===" && git log main..HEAD --oneline
echo "=== file change summary vs main ===" && git diff --stat main...HEAD | tail -5
echo "=== new top-level paths ===" && git diff --name-only main...HEAD | awk -F/ '{print $1"/"$2}' | sort -u
```

- [ ] **Step 4: STOP — present the above to Peter and get explicit approval to push.** Do not proceed to Task 9 without it.

---

### Task 9: Push + open the PR

This pushes the branch to the PUBLIC air-note remote — the irreversible step. Only after Task 8 approval.

- [ ] **Step 1: Push the branch**

```bash
cd ~/air-note
git push -u origin feat/import-bossclaw-desktop
```

- [ ] **Step 2: Open the PR**

```bash
cd ~/air-note
gh pr create --repo AgentIdentityRegistry/air-note --base main \
  --title "feat: consolidate BossClaw desktop app into air-note (retire SuperClaw)" \
  --body "$(cat <<'EOF'
Imports the BossClaw desktop app + shared lib + skills from the legacy ~/SuperClaw monorepo into air-note, making this repo the single source of truth. Drops SuperClaw's stale agent-bridge-mcp and duplicate crates/air-rs (air-note's are canonical).

- apps/desktop (Tauri + React), packages/shared, skills/verified imported
- cargo workspace gains apps/desktop/src-tauri; npm workspace added (apps/*, packages/*)
- CI workflows carried over + path-fixed (a2a→air, workspace target)
- agent-bridge-mcp untouched; its test suite verified green

Design: docs/superpowers/specs/2026-06-08-superclaw-to-air-note-consolidation-design.md
Plan: docs/superpowers/plans/2026-06-08-superclaw-to-air-note-consolidation.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Report the PR URL to Peter for review/merge.**

---

### Task 10: After merge — archive the bossclaw repo

Run only AFTER the PR is merged to air-note `main`.

- [ ] **Step 1: Add a redirect README on bossclaw's default branch**

```bash
cd ~/SuperClaw
git switch main
git pull --ff-only
```
Replace `~/SuperClaw/README.md` with:

```markdown
# BossClaw — moved

This project has moved into the **air-note** monorepo:

➡️ https://github.com/AgentIdentityRegistry/air-note (see `apps/desktop/`)

The AIR Note messaging stack, the `air-rs` Rust SDK, and the BossClaw desktop app
now live there as one build. This repository is archived (read-only) for history.
```

```bash
git add README.md
git commit -m "docs: archive notice — BossClaw moved to AgentIdentityRegistry/air-note"
git push origin main
```

- [ ] **Step 2: Archive the GitHub repo (read-only, reversible)**

```bash
gh repo archive ahnkwangwook-oss/bossclaw --yes
```
Expected: repo marked archived. Confirm with `gh repo view ahnkwangwook-oss/bossclaw --json isArchived`.

- [ ] **Step 3: Leave `~/SuperClaw` local folder in place.** (Per decision — not deleted this session.)

---

### Task 11: GBrain handoff + lessons (AIR session-end protocol)

- [ ] **Step 1: Re-run the PRIME DIRECTIVE audit** across all AIR repos; confirm everything pushed.

- [ ] **Step 2: Write the handoff** `mcp__gbrain__put_page` slug `air/session-handoff-2026-06-08-superclaw-consolidation` (type handoff, `supersedes: air/session-handoff-2026-06-05-daemon-kenny-pagination`): what moved, the new air-note monorepo layout, bossclaw archived.

- [ ] **Step 3: Update `air/session-start-protocol`** — point the BossClaw-messaging track at the new handoff; update the "Repos under PRIME DIRECTIVE" list (SuperClaw now archived; air-note holds the desktop app).

- [ ] **Step 4: Append earned lessons** to `air/lessons-learned-canonical` (e.g. "two copies of a crate drift cosmetically via half-done renames — diff on disk before assuming real divergence"; "carried CI workflows need path-fixing for a workspace: root `target/`, post-rename crate names").

---

## Self-Review

**Spec coverage:** Every spec section maps to a task — import (T1), npm workspace (T2), cargo workspace (T3), install+typecheck (T4), backend check + messaging regression (T5), CI carry+fix (T6), README (T7), publish-safety gate (T8), push/PR (T9), archive (T10), GBrain handoff (T11). ✅

**Placeholder scan:** No TBD/TODO; every created/edited file shows exact content; every verify step has an exact command + expected output. ✅

**Type/name consistency:** `@bossclaw/desktop`, `@bossclaw/shared`, `bossclaw_desktop`, `air-rs` used consistently; workspace globs `apps/*`+`packages/*` consistent across T2/T4; `air-rs` path-dep noted unchanged. ✅

**Known heavy step:** T5 Step 2 (`cargo check -p bossclaw_desktop`) downloads the full Tauri dep tree — run in background; an environment-only failure (missing system lib) is distinguished from a migration failure.
