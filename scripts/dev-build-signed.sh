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
set -euo pipefail

: "${APPLE_SIGNING_IDENTITY:?Set APPLE_SIGNING_IDENTITY to your code-signing cert name (e.g. \"AIR Agent Dev\")}"

cd "$(dirname "$0")/.."

npm run build --workspace @air-agent/desktop -- --debug --bundles app

APP="target/debug/bundle/macos/AIR Agent.app"
codesign --force --deep --sign "$APPLE_SIGNING_IDENTITY" "$APP"
codesign --verify --verbose=2 "$APP"

echo "Signed $APP with identity: $APPLE_SIGNING_IDENTITY"
echo "First launch prompts once per secret — click \"Always Allow\", then it stays quiet."
