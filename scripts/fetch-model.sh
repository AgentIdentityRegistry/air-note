#!/usr/bin/env bash
# Fetch the potion-base-8M model2vec model into the Tauri resource dir, verifying
# committed sha256 pins. Run before `tauri dev`/`tauri build`. Fails closed on any
# mismatch. The model is MIT-licensed (minishlab) and never committed to git.
# Bash 3.2-compatible (macOS system bash) — no associative arrays.
set -euo pipefail

DEST="apps/desktop/src-tauri/resources/models/potion-base-8M"
BASE="https://huggingface.co/minishlab/potion-base-8M/resolve/main"

# Pinned sha256 per file (filled 2026-06-23). Provenance:
#   model.safetensors — cross-verified against HuggingFace's published LFS sha256
#     (the metadata-API "oid", a source independent of the downloaded bytes). This
#     is the real attack surface (the weights), so it is anchored, not trust-on-first-use.
#   tokenizer.json / config.json — NOT LFS on HF, so HF publishes no sha256 for them;
#     these are trust-on-first-use pins. Both are tiny and human-auditable.
# The verify below fails closed on ANY drift from these values.
pinned_sha() {
  case "$1" in
    model.safetensors) echo "f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2" ;;
    tokenizer.json)    echo "e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50" ;;
    config.json)       echo "2a6ac0e9aaa356a68a5688070db78fc3a464fefe85d2f06a1905ce3718687553" ;;
    *) echo "UNKNOWN_FILE" ;;
  esac
}

mkdir -p "$DEST"
for f in model.safetensors tokenizer.json config.json; do
  echo "fetching $f"
  curl -fsSL "$BASE/$f" -o "$DEST/$f"
  got=$(shasum -a 256 "$DEST/$f" | awk '{print $1}')
  want=$(pinned_sha "$f")
  if [ "$got" != "$want" ]; then
    echo "ERROR: sha256 mismatch for $f (got $got, want $want)" >&2
    rm -f "$DEST/$f"
    exit 1
  fi
  echo "$f: $got"
done
echo "Model ready at $DEST"
