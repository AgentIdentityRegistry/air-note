#!/usr/bin/env bash
# install-bossclawd.sh — install / uninstall / status for the bossclawd memory daemon
# as a per-USER auto-start service (macOS launchd LaunchAgent, Linux systemd --user).
# (AIR Agent Memory Hub, M1a Task 8.)
#
# ============================================================================
#  THE ONE THING THAT MUST BE RIGHT: the daemon must be CO-SIGNED with the app.
# ============================================================================
# The engine's secrets (service `ai.air-agent.desktop`, including the DEK
# `air-agent.engine.dek`) live in the user's login Keychain / Secret Service. A
# macOS Keychain item's ACL trusts a (code-signing identifier + certificate leaf)
# pair — the app's designated requirement is
#     identifier "ai.air-agent.desktop" and certificate leaf = <AIR Agent Dev>
# For the shipped `bossclawd` to read the DEK with NO password prompt it MUST be
# signed with `--identifier ai.air-agent.desktop` AND the SAME signing identity as
# the app. The 2026-07-02 keychain spike proved: same cert but a DIFFERENT
# identifier (e.g. the per-binary `bossclawd` identifier Tauri's externalBin signing
# assigns) -> PROMPTS. So a plain externalBin bundle is NOT enough; the daemon needs
# an explicit identifier override at signing time.
#
# This script does NOT sign anything (installing is not a signing step). The re-sign
# lives at BUILD time:
#   - dev builds: scripts/dev-build-signed.sh re-signs Contents/MacOS/bossclawd with
#       codesign --force --identifier ai.air-agent.desktop --sign "<identity>"
#   - release CI: MUST run the identical re-sign on the bundled daemon after Tauri
#       bundling, with the release Developer ID identity.
# See scripts/codesign-run.sh for the canonical incantation (it does the same for
# every locally-built binary). A future maintainer must NOT "simplify" the daemon
# down to a plain externalBin with no identifier override — that reintroduces the
# password prompt for every user.
# ----------------------------------------------------------------------------
#
# PATHS: launchd/systemd give a service no useful PATH, so every path must be
# absolute. This script resolves them on the target machine at install time and
# fills the committed templates (resources/bossclawd.plist.in /
# resources/bossclawd.service.in) — no dev path is ever hard-coded/committed.
#
# The socket + data dir are left at the daemon's DEFAULTS on purpose: both the
# daemon (crates/bossclawd/src/main.rs: SOCKET_FILE="bossclawd.sock", default
# <data_dir>/bossclawd.sock) and the app (apps/desktop/src-tauri/src/engine/
# daemon.rs: same default) already agree, so no BOSSCLAWD_SOCKET override is needed.
# The unit sets NO env either: with pull-based model resolution (rung 2, I1) the
# daemon resolves its own model (signed record -> staged English default
# <data_dir>/models/potion-base-8M). This script STAGES the bundled English model
# into that default path instead of pinning BOSSCLAWD_MODEL_DIR — an env pin would
# be highest-priority and would block an opt-in multilingual language pack.
#
# ============================================================================
#  MANUAL SMOKE TEST (Task 8 Step 3) — run on a signed build; Peter-gated.
# ============================================================================
# Prereq: a signed .app built via scripts/dev-build-signed.sh (which stages +
# co-signs the daemon). Copy it to /Applications so autodetect finds it, or pass
# --bin explicitly.
#
#   # 1. install -> the daemon starts at login and now
#   scripts/install-bossclawd.sh install
#   scripts/install-bossclawd.sh status            # loaded: yes, a pid is shown
#
#   # 2. app connects: launch AIR Agent; its daemon.rs probe finds the live
#   #    socket at ~/Library/Application Support/ai.air-agent.desktop/bossclawd.sock
#   #    and does NOT spawn a second one. Memory features work with no keychain
#   #    prompt (proves the co-sign).
#
#   # 3. crash -> relaunch (KeepAlive / Restart=always):
#   pkill -f 'MacOS/bossclawd'                      # or: kill <pid from status>
#   sleep 2
#   scripts/install-bossclawd.sh status            # a NEW pid -> it came back
#   #    the app reconnects on its next engine call (Unavailable -> healthy).
#
#   # 4. uninstall leaves nothing running:
#   scripts/install-bossclawd.sh uninstall
#   scripts/install-bossclawd.sh status            # loaded: no
#   pgrep -f 'MacOS/bossclawd' || echo 'clean'     # no lingering process
#
# Linux is identical with `systemctl --user` semantics (status shows active:
# yes/no; step 3 uses `pkill -f bossclawd`).
set -euo pipefail

# --- stable identifiers -----------------------------------------------------
readonly LABEL="ai.air-agent.bossclawd"          # launchd Label / reverse-DNS id
readonly SYSTEMD_UNIT="bossclawd.service"         # systemd --user unit name
readonly BIN_NAME="bossclawd"                     # matches daemon.rs BIN_NAME
readonly APP_DIR_NAME="ai.air-agent.desktop"      # matches main.rs APP_DIR_NAME
# Model subpath INSIDE the macOS .app bundle (confirmed against the built bundle:
# Contents/Resources/resources/models/potion-base-8M) and the tauri.conf.json
# bundle.resources glob ("resources/models/potion-base-8M/*").
readonly MODEL_SUBPATH="resources/models/potion-base-8M"

# Template locations (relative to this script's repo).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
readonly RESOURCES_DIR="${SCRIPT_DIR}/../apps/desktop/src-tauri/resources"
readonly PLIST_TEMPLATE="${RESOURCES_DIR}/bossclawd.plist.in"
readonly SYSTEMD_TEMPLATE="${RESOURCES_DIR}/bossclawd.service.in"

die() { echo "install-bossclawd: error: $*" >&2; exit 1; }
info() { echo "install-bossclawd: $*"; }

usage() {
  cat >&2 <<EOF
Usage: $(basename "$0") <install|uninstall|status> [--bin PATH] [--model-dir PATH]

  install     Write the auto-start service (launchd/systemd), then load+start it.
              Re-running is safe (reloads with the current paths).
  uninstall   Stop + unload the service and remove its unit file. Safe if absent.
  status      Show whether the service is loaded/running and where its files are.

Options (override auto-detection):
  --bin PATH        Absolute path to the co-signed bossclawd binary. Default:
                    <exe-sibling in the installed AIR Agent app> (auto-detected).
  --model-dir PATH  Absolute path to the bundled English model SOURCE (potion-base-8M),
                    staged into the daemon's default <data_dir>/models/potion-base-8M.
                    Default: the model inside the detected app bundle.

Auto-start is per-USER (launchd LaunchAgent / systemd --user), never root, because
the daemon needs the user's login keychain. See the header for the co-signing rule.
EOF
  exit 2
}

# --- platform ---------------------------------------------------------------
detect_os() {
  case "$(uname -s)" in
    Darwin) echo macos ;;
    Linux)  echo linux ;;
    *)      die "unsupported OS $(uname -s) (macOS launchd / Linux systemd only)" ;;
  esac
}

# --- app data dir (mirrors main.rs app_data_dir / daemon.rs) -----------------
# macOS: ~/Library/Application Support/ai.air-agent.desktop
# Linux: $XDG_DATA_HOME|~/.local/share/ai.air-agent.desktop
app_data_dir() {
  local os="$1"
  if [[ "${os}" == "macos" ]]; then
    echo "${HOME}/Library/Application Support/${APP_DIR_NAME}"
  elif [[ -n "${XDG_DATA_HOME:-}" ]]; then
    echo "${XDG_DATA_HOME}/${APP_DIR_NAME}"
  else
    echo "${HOME}/.local/share/${APP_DIR_NAME}"
  fi
}

# --- log dir ----------------------------------------------------------------
# macOS: ~/Library/Logs/AIR Agent (Console.app shows it). Linux: journald handles
# stdout for systemd, so no file is needed there.
log_dir_macos() { echo "${HOME}/Library/Logs/AIR Agent"; }

# --- locate the installed, co-signed daemon binary --------------------------
# Prefer an explicit --bin. Otherwise probe the standard installed app locations
# for the sibling-of-exe path daemon.rs resolves (Contents/MacOS/bossclawd on
# macOS). We deliberately do NOT fall back to a dev target/ path here: a service
# must point at the installed, co-signed binary, not an ad-hoc dev build.
autodetect_bin() {
  local os="$1"
  if [[ "${os}" == "macos" ]]; then
    local candidates=(
      "/Applications/AIR Agent.app/Contents/MacOS/${BIN_NAME}"
      "${HOME}/Applications/AIR Agent.app/Contents/MacOS/${BIN_NAME}"
    )
    local c
    for c in "${candidates[@]}"; do
      [[ -x "${c}" ]] && { echo "${c}"; return 0; }
    done
    return 1
  fi
  # Linux: common install prefixes for the sibling-of-exe layout.
  local candidates=(
    "/usr/bin/${BIN_NAME}"
    "/usr/local/bin/${BIN_NAME}"
    "/opt/AIR Agent/${BIN_NAME}"
  )
  local c
  for c in "${candidates[@]}"; do
    [[ -x "${c}" ]] && { echo "${c}"; return 0; }
  done
  return 1
}

# --- derive the model dir from a resolved binary path -----------------------
# From an .app's Contents/MacOS/bossclawd, the bundled model is at
# ../Resources/resources/models/potion-base-8M. On Linux there is no bundle
# convention, so --model-dir is required if the default data-dir copy is absent.
autodetect_model_dir() {
  local os="$1" bin="$2"
  if [[ "${os}" == "macos" ]]; then
    # .../Contents/MacOS/bossclawd -> .../Contents/Resources/<MODEL_SUBPATH>
    local contents; contents="$(cd "$(dirname "${bin}")/.." && pwd)"
    echo "${contents}/Resources/${MODEL_SUBPATH}"
    return 0
  fi
  # Linux: the daemon's own default (<data_dir>/models/potion-base-8M). If the app
  # stages the model there this just works; otherwise the caller passes --model-dir.
  echo "$(app_data_dir "${os}")/models/potion-base-8M"
}

# --- fill a template: replace @TOKEN@ occurrences ---------------------------
# Uses bash string ops (no sed) so paths with slashes/spaces need no escaping.
render_template() {
  local template="$1"; shift
  [[ -f "${template}" ]] || die "template not found: ${template}"
  local content; content="$(cat "${template}")"
  # Args are TOKEN=VALUE pairs.
  local pair token value
  for pair in "$@"; do
    token="${pair%%=*}"
    value="${pair#*=}"
    content="${content//@${token}@/${value}}"
  done
  # Fail loudly if any @TOKEN@ went unfilled (guards a renamed/misspelled token).
  # Templates carry no other @WORD@ text, so an AT-delimited all-caps run here can
  # only be an unfilled placeholder.
  local leftovers; leftovers="$(printf '%s' "${content}" | grep -oE '@[A-Z_]+@' | sort -u | tr '\n' ' ')"
  [[ -z "${leftovers}" ]] || die "unfilled placeholder(s) in rendered ${template}: ${leftovers}"
  printf '%s' "${content}"
}

# ============================================================================
#  macOS: launchd LaunchAgent
# ============================================================================
plist_path() { echo "${HOME}/Library/LaunchAgents/${LABEL}.plist"; }

install_macos() {
  local bin="$1"
  local plist; plist="$(plist_path)"
  local logdir; logdir="$(log_dir_macos)"
  local out_log="${logdir}/bossclawd.out.log"
  local err_log="${logdir}/bossclawd.err.log"

  # launchd opens StandardOutPath/StandardErrorPath BEFORE spawning and does NOT
  # create missing parent dirs — mkdir them first (air-msg installer note).
  mkdir -p "${logdir}"
  mkdir -p "$(dirname "${plist}")"

  # Warn (do not block) if the binary is not co-signed with the app identifier —
  # this is the #1 cause of a mystery password prompt at first read.
  warn_if_not_cosigned "${bin}"

  local rendered
  rendered="$(render_template "${PLIST_TEMPLATE}" \
    "LABEL=${LABEL}" \
    "BOSSCLAWD_BIN=${bin}" \
    "STDOUT_LOG=${out_log}" \
    "STDERR_LOG=${err_log}")"
  printf '%s' "${rendered}" > "${plist}"
  chmod 0644 "${plist}"
  # Validate what we just wrote before asking launchd to load it (plutil is a macOS
  # base tool; guard it for consistency with warn_if_not_cosigned's codesign check).
  if command -v plutil >/dev/null 2>&1; then
    plutil -lint "${plist}" >/dev/null || die "generated plist failed plutil -lint: ${plist}"
  fi

  # Idempotent (re)load: bootout any existing instance, then bootstrap fresh.
  # `bootout` on an absent label is a no-op-ish error we swallow; `|| true` keeps
  # re-install safe. `enable` clears any prior disabled state.
  local domain="gui/$(id -u)"
  launchctl bootout "${domain}/${LABEL}" 2>/dev/null || true
  launchctl enable "${domain}/${LABEL}" 2>/dev/null || true
  launchctl bootstrap "${domain}" "${plist}" \
    || die "launchctl bootstrap failed for ${plist} (check: ${err_log})"
  # kickstart -k ensures it is running now even if it was already loaded.
  launchctl kickstart -k "${domain}/${LABEL}" 2>/dev/null || true

  info "installed launchd agent ${LABEL}"
  info "  plist:   ${plist}"
  info "  binary:  ${bin}"
  info "  logs:    ${out_log}"
  info "           ${err_log}"
  info "the daemon now starts at login and relaunches on crash (KeepAlive)."
}

uninstall_macos() {
  local plist; plist="$(plist_path)"
  local domain="gui/$(id -u)"
  launchctl bootout "${domain}/${LABEL}" 2>/dev/null || true
  if [[ -f "${plist}" ]]; then
    rm -f "${plist}"
    info "removed launchd agent ${LABEL} (${plist})"
  else
    info "nothing installed at ${plist}"
  fi
}

status_macos() {
  local plist; plist="$(plist_path)"
  local domain="gui/$(id -u)"
  echo "label:      ${LABEL}"
  echo "plist:      ${plist} ($( [[ -f "${plist}" ]] && echo present || echo absent ))"
  if launchctl print "${domain}/${LABEL}" >/dev/null 2>&1; then
    echo "loaded:     yes"
    # `print` output is large; surface the state + pid lines only.
    launchctl print "${domain}/${LABEL}" 2>/dev/null \
      | grep -E '^\s*(state|pid) =' | sed 's/^[[:space:]]*/  /' || true
  else
    echo "loaded:     no"
  fi
}

# ============================================================================
#  Linux: systemd --user
# ============================================================================
unit_path() { echo "${HOME}/.config/systemd/user/${SYSTEMD_UNIT}"; }

install_linux() {
  local bin="$1"
  local unit; unit="$(unit_path)"
  mkdir -p "$(dirname "${unit}")"

  local rendered
  rendered="$(render_template "${SYSTEMD_TEMPLATE}" \
    "BOSSCLAWD_BIN=${bin}")"
  printf '%s' "${rendered}" > "${unit}"
  chmod 0644 "${unit}"

  systemctl --user daemon-reload
  systemctl --user enable --now "${SYSTEMD_UNIT}" \
    || die "systemctl --user enable --now failed for ${SYSTEMD_UNIT} (check: journalctl --user -u ${SYSTEMD_UNIT})"
  # If it was already enabled, ensure the new unit is (re)started.
  systemctl --user restart "${SYSTEMD_UNIT}" 2>/dev/null || true

  info "installed systemd --user unit ${SYSTEMD_UNIT}"
  info "  unit:   ${unit}"
  info "  binary: ${bin}"
  info "  logs:   journalctl --user -u ${SYSTEMD_UNIT}"
  info "the daemon now starts at login and relaunches on crash (Restart=always)."
}

uninstall_linux() {
  local unit; unit="$(unit_path)"
  systemctl --user disable --now "${SYSTEMD_UNIT}" 2>/dev/null || true
  if [[ -f "${unit}" ]]; then
    rm -f "${unit}"
    systemctl --user daemon-reload 2>/dev/null || true
    info "removed systemd --user unit ${SYSTEMD_UNIT} (${unit})"
  else
    info "nothing installed at ${unit}"
  fi
}

status_linux() {
  local unit; unit="$(unit_path)"
  echo "unit:       ${SYSTEMD_UNIT}"
  echo "unit file:  ${unit} ($( [[ -f "${unit}" ]] && echo present || echo absent ))"
  if systemctl --user is-active "${SYSTEMD_UNIT}" >/dev/null 2>&1; then
    echo "active:     yes"
  else
    echo "active:     no"
  fi
  systemctl --user status "${SYSTEMD_UNIT}" --no-pager 2>/dev/null \
    | grep -E 'Active:|Main PID:' | sed 's/^[[:space:]]*/  /' || true
}

# --- co-sign warning (macOS) ------------------------------------------------
# Non-fatal: if the daemon's designated requirement does not name the app
# identifier, the first DEK read will PROMPT. Warn early with the exact fix.
warn_if_not_cosigned() {
  local bin="$1"
  command -v codesign >/dev/null 2>&1 || return 0
  local dr
  dr="$(codesign -d -r- "${bin}" 2>&1 || true)"
  if ! printf '%s' "${dr}" | grep -qF "identifier \"${APP_DIR_NAME}\""; then
    echo "install-bossclawd: WARNING — ${bin}" >&2
    echo "  is not signed with identifier \"${APP_DIR_NAME}\"; the daemon will" >&2
    echo "  hit a Keychain PASSWORD PROMPT on first DEK read. Re-sign it with:" >&2
    echo "    codesign --force --identifier ${APP_DIR_NAME} --sign \"<identity>\" \"${bin}\"" >&2
    echo "  (dev: scripts/dev-build-signed.sh does this; release CI must too.)" >&2
  fi
}

# ============================================================================
#  main
# ============================================================================
main() {
  local cmd="${1:-}"; shift || true
  [[ -n "${cmd}" ]] || usage

  local bin_override="" model_override=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --bin)       bin_override="${2:-}"; shift 2 || die "--bin needs a path" ;;
      --model-dir) model_override="${2:-}"; shift 2 || die "--model-dir needs a path" ;;
      -h|--help)   usage ;;
      *)           die "unknown option: $1" ;;
    esac
  done

  local os; os="$(detect_os)"

  case "${cmd}" in
    install)
      # Resolve the binary: explicit override, else auto-detect the installed app.
      local bin="${bin_override}"
      if [[ -z "${bin}" ]]; then
        bin="$(autodetect_bin "${os}")" \
          || die "could not find the installed ${BIN_NAME}; pass --bin PATH (must be the co-signed binary, e.g. /Applications/AIR Agent.app/Contents/MacOS/${BIN_NAME})"
      fi
      [[ -x "${bin}" ]] || die "binary not found or not executable: ${bin}"
      case "${bin}" in /*) ;; *) die "--bin must be an ABSOLUTE path (launchd/systemd have no PATH): ${bin}" ;; esac

      # Resolve the model SOURCE dir: explicit override, else derive from the binary. This is the
      # source we STAGE into the daemon's default resolution path (below) — no longer an env pin.
      local model_dir="${model_override}"
      [[ -z "${model_dir}" ]] && model_dir="$(autodetect_model_dir "${os}" "${bin}")"
      case "${model_dir}" in /*) ;; *) die "--model-dir must be an ABSOLUTE path: ${model_dir}" ;; esac
      [[ -d "${model_dir}" ]] || info "note: model source dir does not exist yet: ${model_dir} (the daemon will error until a model is staged — check --model-dir)"

      # Stage the bundled English model into the daemon's default resolution path so pull-based
      # model resolution (rung 2, I1) works without pinning BOSSCLAWD_MODEL_DIR (which would be
      # highest-priority and block the opt-in multilingual language pack). Idempotent: skip if a
      # model is already staged. On the Linux default the source IS the staging target, so there
      # is nothing to copy (the operator passes --model-dir to point at a real source).
      local staged; staged="$(app_data_dir "${os}")/models/potion-base-8M"
      if [[ -f "${staged}/model.safetensors" ]]; then
        info "english model already staged: ${staged}"
      elif [[ "${model_dir}" == "${staged}" ]]; then
        info "note: model source is the staging target (${staged}); nothing to copy"
      else
        info "staging english model: ${model_dir} -> ${staged}"
        mkdir -p "${staged}"
        cp -R "${model_dir}/." "${staged}/"
      fi

      if [[ "${os}" == "macos" ]]; then install_macos "${bin}"; else install_linux "${bin}"; fi
      ;;
    uninstall)
      if [[ "${os}" == "macos" ]]; then uninstall_macos; else uninstall_linux; fi
      ;;
    status)
      if [[ "${os}" == "macos" ]]; then status_macos; else status_linux; fi
      ;;
    -h|--help) usage ;;
    *) die "unknown command: ${cmd} (expected install|uninstall|status)" ;;
  esac
}

main "$@"
