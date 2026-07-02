#!/usr/bin/env bash
# Cargo `runner` that co-signs every locally-built binary with the app's STABLE
# code-signing identity, then execs it. Wired in via .cargo/config.toml (macOS only).
#
# WHY THIS EXISTS
# --------------
# macOS Keychain items are ACL'd to a code signature: an app reads a stored secret
# without a password prompt only if its signature matches the item's trusted-app
# "designated requirement" (identifier + certificate leaf). An unsigned / ad-hoc
# build (what `cargo test`, `cargo run`, and each rebuild produce) gets a FRESH
# random signature every time, so the Keychain treats every run as a new stranger
# → a password prompt (or, for `cargo test`, an un-clickable prompt that HANGS the
# test forever on a headless/parallel run).
#
# The 2026-07-02 keychain spike proved: a binary signed with identifier
# `ai.air-agent.desktop` + the "AIR Agent Dev" cert reads the engine DEK SILENTLY.
# So we sign every dev/test binary with that same stable identity here — one
# "Always Allow" click, then silence forever, across every rebuild.
#
# SAFETY / SCOPE
# --------------
# - Fail-OPEN: on any machine without the cert (CI, another dev, Linux), it just
#   execs the binary unsigned — never breaks a build or a test run.
# - Local-dev convenience ONLY. Release + CI signing are configured separately and
#   do NOT use this script.
set -euo pipefail

BIN="$1"
shift

# The stable identity. Override the cert name via APPLE_SIGNING_IDENTITY if yours
# differs; the identifier MUST stay ai.air-agent.desktop to match the Keychain ACL.
IDENTITY="${APPLE_SIGNING_IDENTITY:-AIR Agent Dev}"
BUNDLE_ID="ai.air-agent.desktop"

# Only attempt signing on macOS with the cert actually present; otherwise run as-is.
if [[ "$(uname -s)" == "Darwin" ]] \
  && command -v codesign >/dev/null 2>&1 \
  && security find-identity -p codesigning 2>/dev/null | grep -qF "$IDENTITY"; then
  # --force replaces the ad-hoc signature; --identifier pins the ACL-matching id.
  # Idempotent and cheap (~100ms). A signing failure is non-fatal: warn and run.
  if ! codesign --force --identifier "$BUNDLE_ID" --sign "$IDENTITY" "$BIN" 2>/dev/null; then
    echo "codesign-run: warning — could not sign $BIN (running unsigned)" >&2
  fi
fi

exec "$BIN" "$@"
