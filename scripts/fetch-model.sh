#!/usr/bin/env bash
# Fetch the potion-base-8M model2vec model into the Tauri resource dir, verifying
# committed sha256 pins. Run before `tauri dev`/`tauri build`. Fails closed on any
# mismatch. The model is MIT-licensed (minishlab) and never committed to git.
# Bash 3.2-compatible (macOS system bash) — no associative arrays.
set -euo pipefail

DEST="apps/desktop/src-tauri/resources/models/potion-base-8M"
BASE="https://huggingface.co/minishlab/potion-base-8M/resolve/main"

# Pinned sha256 per file. Fill in on first run (Task 10), then commit — DO NOT
# read these from the same response they verify. One source of truth, no bashism.
pinned_sha() {
  case "$1" in
    model.safetensors) echo "REPLACE_WITH_PINNED_SHA256" ;;
    tokenizer.json)    echo "REPLACE_WITH_PINNED_SHA256" ;;
    config.json)       echo "REPLACE_WITH_PINNED_SHA256" ;;
    *) echo "UNKNOWN_FILE" ;;
  esac
}

mkdir -p "$DEST"
for f in model.safetensors tokenizer.json config.json; do
  echo "fetching $f"
  curl -fsSL "$BASE/$f" -o "$DEST/$f"
  got=$(shasum -a 256 "$DEST/$f" | awk '{print $1}')
  want=$(pinned_sha "$f")
  if [ "$want" != "REPLACE_WITH_PINNED_SHA256" ] && [ "$got" != "$want" ]; then
    echo "ERROR: sha256 mismatch for $f (got $got, want $want)" >&2
    rm -f "$DEST/$f"
    exit 1
  fi
  echo "$f: $got"
done
echo "Model ready at $DEST"
