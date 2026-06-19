# M5b markitdown venv

The bundled, pinned Python environment behind the **sandboxed** rich-document parser
(`SandboxedMarkitdownParser`). markitdown runs as an *untrusted* subprocess inside an
OS jail (macOS Seatbelt / Linux bwrap) that denies network and bounds resources. Full
design: `docs/superpowers/specs/2026-06-19-bossclaw-core-m5b-sandboxed-parser-design.md`.

## Build the venv
```bash
PYBIN=python3.13 crates/bossclaw-core/scripts/build-venv.sh /path/to/venv
export BOSSCLAW_MARKITDOWN_VENV=/path/to/venv   # the engine locates the venv here
```
`build-venv.sh` installs from a per-platform `--require-hashes` lockfile, audits that
lockfile with `pip-audit --strict` (from a throwaway env — never polluting the shipped
venv), and asserts the installed markitdown equals the pinned `0.1.6`.

## Pinning + lockfiles
- Single source pin: `requirements.in` → `markitdown[pdf,docx,pptx,xlsx,xls,outlook]==0.1.6`.
- `requirements-macos-arm64.lock` — committed, fully hash-pinned (regenerate with
  `pip-compile --generate-hashes` on macOS arm64).
- **`requirements-linux-x86_64.lock` is a placeholder.** Linux wheels + hashes differ
  from macOS, so it must be generated + committed on a Linux x86_64 box. CI's Ubuntu leg
  currently regenerates it fresh; a committed, reviewed Linux lock is a **pre-merge
  follow-up** (the dev environment is macOS-only). — *security review #1*
- The Rust `MARKITDOWN_VERSION` const MUST equal the pin; CI's drift guard enforces it.

## On a markitdown version bump
Update `requirements.in` + the Rust `MARKITDOWN_VERSION` const, regenerate **both** locks,
re-run `pip-audit`, and confirm the wrapper's converter-strip still holds — `convert_stdin.py`
exits 9 (loudly) if a markitdown release changed the converter-registry shape.

## Run the gated jail proofs
```bash
BOSSCLAW_MARKITDOWN_VENV=/path/to/venv \
  cargo test -p bossclaw-core --features sandbox-test-hooks -- --ignored
```
The default build (`cargo test -p bossclaw-core`, no feature) ships no subprocess and no
network: rich files skip-with-report, exactly like M5a.
