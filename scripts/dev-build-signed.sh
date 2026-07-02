#!/usr/bin/env bash
# Build a code-signed *debug* .app for local QA so the macOS Keychain stops
# re-prompting on every launch. An unsigned / ad-hoc build gets a fresh code
# signature on each rebuild, which the Keychain never learns to trust → a
# password prompt for every stored secret, every launch. A stable signing
# identity fixes that (one "Always Allow" round, then silence).
#
# One-time setup — create a free self-signed code-signing cert:
#   Keychain Access ▸ Certificate Assistant ▸ Create a Certificate…
#     Name: <your choice>   Identity Type: Self Signed Root   Type: Code Signing
#
# Then build with:
#   APPLE_SIGNING_IDENTITY="<that name>" scripts/dev-build-signed.sh
#
# LOCAL DEV CONVENIENCE ONLY. CI and release signing are configured separately
# and do NOT run this script (so the machine-specific cert name stays out of CI).
#
# ============================================================================
#  M1a Task 8: this script ALSO bundles + CO-SIGNS the bossclawd daemon.
# ============================================================================
# The daemon ships as a Tauri externalBin sidecar which Tauri copies to the app's
# Contents/MacOS/bossclawd — exactly the sibling-of-exe path daemon.rs resolves.
#
# externalBin is BUNDLE-ONLY, kept in a SEPARATE config (tauri.bundle.conf.json),
# NOT the base tauri.conf.json. Why: tauri_build runs the externalBin existence
# check on EVERY compile (plain `cargo build`/`test`/`clippy`, not just bundling),
# so a base-config externalBin would fail every non-bundle build on a fresh
# checkout / in CI (the sidecar is a gitignored build artifact). Merging it only at
# bundle time via `--config tauri.bundle.conf.json` keeps plain cargo green while
# still bundling the sidecar. RELEASE CI MUST pass the same `--config` at bundle.
#
# BUT Tauri signs sidecars with only `codesign -s <identity>` (no --identifier
# override; confirmed in tauri-macos-sign), so the sidecar would get the default
# per-binary identifier `bossclawd`. The Task 0.5 keychain spike proved that a
# DIFFERENT identifier -> a Keychain PASSWORD PROMPT on the first DEK read, even
# with the same cert. So after the app is signed we RE-SIGN the bundled daemon
# with `--identifier ai.air-agent.desktop` (the app's identifier) so its
# designated requirement matches the engine-secret ACL and it reads the DEK
# silently. Release CI MUST perform this identical re-sign with the release
# Developer ID identity — do not "simplify" it away. (Same incantation as
# scripts/codesign-run.sh, which does this for every locally-built binary.)
set -euo pipefail

: "${APPLE_SIGNING_IDENTITY:?Set APPLE_SIGNING_IDENTITY to your code-signing cert name (e.g. \"AIR Agent Dev\")}"

# The app's identifier — MUST match tauri.conf.json `identifier` and the Keychain
# ACL on the engine secrets. The daemon is re-signed with THIS identifier.
readonly APP_IDENTIFIER="ai.air-agent.desktop"

cd "$(dirname "$0")/.."

# --- 1. Build + stage the bossclawd sidecar for Tauri externalBin -----------
# Tauri's externalBin requires the binary named with the current target triple
# (e.g. bossclawd-aarch64-apple-darwin). We build the daemon and copy it to the
# staging dir the externalBin path (in tauri.bundle.conf.json) points at
# (apps/desktop/src-tauri/binaries).
TARGET_TRIPLE="$(rustc -vV | awk '/^host:/ {print $2}')"
readonly TARGET_TRIPLE
readonly SIDECAR_DIR="apps/desktop/src-tauri/binaries"
readonly SIDECAR_PATH="${SIDECAR_DIR}/bossclawd-${TARGET_TRIPLE}"

echo "Building bossclawd (debug) for ${TARGET_TRIPLE}…"
cargo build -p bossclawd
mkdir -p "${SIDECAR_DIR}"
cp -f "target/debug/bossclawd" "${SIDECAR_PATH}"
echo "Staged sidecar: ${SIDECAR_PATH}"

# --- 2. Build the .app, merging the bundle-only externalBin config -----------
# `--config tauri.bundle.conf.json` (path relative to src-tauri, the tauri CLI's
# cwd) merges externalBin at bundle time only, so Tauri bundles the sidecar into
# Contents/MacOS/bossclawd. RELEASE CI must pass the same --config.
npm run build --workspace @air-agent/desktop -- --debug --bundles app --config tauri.bundle.conf.json

APP="target/debug/bundle/macos/AIR Agent.app"

# --- 3. Sign the whole app (this also (ad-hoc-ally re-)signs the sidecar) ----
codesign --force --deep --sign "$APPLE_SIGNING_IDENTITY" "$APP"

# --- 4. RE-SIGN the bundled daemon with the APP identifier (the crux) --------
# --deep above signed the sidecar with its default identifier; override it now so
# the daemon's designated requirement matches the engine-secret Keychain ACL.
DAEMON_BIN="${APP}/Contents/MacOS/bossclawd"
[ -x "${DAEMON_BIN}" ] || { echo "ERROR: bundled daemon missing at ${DAEMON_BIN} (externalBin not bundled?)" >&2; exit 1; }
codesign --force --identifier "${APP_IDENTIFIER}" --sign "$APPLE_SIGNING_IDENTITY" "${DAEMON_BIN}"

# --- 5. Verify -------------------------------------------------------------
codesign --verify --verbose=2 "$APP"
codesign --verify --verbose=2 "${DAEMON_BIN}"
# Confirm the daemon's designated requirement carries the app identifier (the
# thing that makes the silent DEK read work). Fail loudly if it does not.
if ! codesign -d -r- "${DAEMON_BIN}" 2>&1 | grep -qF "identifier \"${APP_IDENTIFIER}\""; then
  echo "ERROR: ${DAEMON_BIN} is NOT signed with identifier \"${APP_IDENTIFIER}\";" >&2
  echo "       it will hit a Keychain password prompt on first DEK read." >&2
  exit 1
fi

echo "Signed $APP with identity: $APPLE_SIGNING_IDENTITY"
echo "Bundled daemon re-signed with identifier: ${APP_IDENTIFIER} (silent DEK read)."
echo "First launch prompts once per secret — click \"Always Allow\", then it stays quiet."
echo
echo "To auto-start the daemon at login (launchd), run:"
echo "  scripts/install-bossclawd.sh install"
