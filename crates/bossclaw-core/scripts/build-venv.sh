#!/usr/bin/env bash
# Build the pinned, audited markitdown venv for M5b. Reproducible: installs from
# a per-platform --require-hashes lockfile, audits the lockfile from a SEPARATE
# tool env (never pollutes the shipped venv), checks the version pin, then drops
# in the first-party wrapper. PYBIN defaults to python3.13 (3.12 also works).
set -euo pipefail
PYBIN="${PYBIN:-python3.13}"
DEST="${1:?usage: build-venv.sh <dest-venv-dir>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) LOCK="$HERE/../python/requirements-macos-arm64.lock" ;;
  Linux-x86_64) LOCK="$HERE/../python/requirements-linux-x86_64.lock" ;;
  *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
"$PYBIN" -m venv "$DEST"
"$DEST/bin/pip" install --upgrade pip
"$DEST/bin/pip" install --require-hashes -r "$LOCK"
# Audit the LOCKFILE from a throwaway env — never install pip-audit into $DEST.
AUDIT=$(mktemp -d); "$PYBIN" -m venv "$AUDIT/v"; "$AUDIT/v/bin/pip" install pip-audit
"$AUDIT/v/bin/pip-audit" --strict --require-hashes -r "$LOCK"; rm -rf "$AUDIT"
ver=$("$DEST/bin/python" -c "import markitdown; print(markitdown.__version__)")
[ "$ver" = "0.1.6" ] || { echo "version drift: venv has $ver, expected 0.1.6" >&2; exit 1; }
cp "$HERE/../python/convert_stdin.py" "$DEST/convert_stdin.py"
echo "OK: venv at $DEST (markitdown $ver)"
