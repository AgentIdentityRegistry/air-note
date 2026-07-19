// Copied from apps/desktop/src-tauri/src/engine/mod.rs (M1a Task 4); the in-app original is removed in Task 6.
// The ~30 `EngineHandle` methods + `EngineError`/`EngineOpError`/`EngineState`/summary types +
// `EvolveTelemetry` + the free fns (`parse_reasoner_config`, `reseed_reasoner_cell`,
// `key_fingerprint`, `record_tick_into`, `map_err_state`) are copied FAITHFULLY. The `try_lock`→`Busy`
// mutating-op semantics + the signed-consent fail-closed cloud-egress gate in `evolve_once` + the boot
// reseed semantics are preserved VERBATIM. The only cross-crate refs (`crate::vault::secret_get_cached`,
// `crate::secrets::SecretsVault`) resolve to the daemon's own copied `vault`/`secrets` modules.

//! The engine spine (SP1): a single live, encrypted `EventLog`.
//! See docs/superpowers/specs/2026-06-22-desktop-engine-spine-design.md.

pub mod cloud_reasoner;
pub mod embed;
pub mod keystore;
pub mod ollama_probe;
pub mod reason;
pub mod scheduler;

use crate::engine::keystore::EngineKeystore;
use crate::secrets::SecretsVault;
use bossclaw_core::{Embedder, EventLog};
use serde::Serialize;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::spawn_blocking;

/// Errors from opening / accessing the engine. Mapped to `EngineState` for the UI.
#[derive(Debug)]
pub enum EngineError {
    /// No identity yet — the brain is not created before onboarding.
    NotOnboarded,
    /// Exactly one of (brain key, DEK) is present — never re-mint (would orphan the DB).
    KeystoreInconsistent,
    /// The DB could not be opened with the stored DEK (wrong key or unopenable).
    KeystoreDbMismatch(String),
    /// A keychain or other I/O error.
    Vault(String),
    /// A background task failed to join.
    Join(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NotOnboarded => write!(f, "not onboarded"),
            EngineError::KeystoreInconsistent => write!(f, "engine keystore inconsistent"),
            EngineError::KeystoreDbMismatch(e) => write!(f, "engine keystore/DB mismatch: {e}"),
            EngineError::Vault(e) => write!(f, "engine keychain error: {e}"),
            EngineError::Join(e) => write!(f, "engine task error: {e}"),
        }
    }
}

/// Errors from the SP2 operational commands (grant/ingest/list). Wraps the
/// SP1 open/gate path so SP1's `EngineError`/`map_err_state`/`EngineState`
/// stay a status-only concern (untouched).
#[derive(Debug)]
pub enum EngineOpError {
    Open(EngineError),
    Core(String),
    Embedder(String),
    /// Reasoner BUILD failure — part of the `ReasonerProvider` seam's error surface. The
    /// production `ConfigReasonerProvider` builds infallibly today (Local→`OllamaReasoner::new`
    /// and Cloud→`CloudReasoner::new` are both infallible; reachability is verified per-call
    /// inside `complete_json`, surfaced through `evolve_once` as `Core`), so nothing constructs
    /// this variant YET. It is load-bearing for a future fallible provider that drops in behind
    /// the same seam; the `?` on `reasoner()` in `evolve_once` already routes to it.
    #[allow(dead_code)]
    Reasoner(String),
    /// A serialized op is already in flight; the `&'static str` names it ("ingest" | "evolve" |
    /// "conflict" | "reflect").
    Busy(&'static str),
    /// The on-disk file changed since the proposal was drafted; the re-gate at confirm
    /// fails closed. Carries the reason. Nothing is written.
    Stale(String),
    /// The folder's write-grant was revoked between propose and apply; re-gate fails closed.
    Revoked(String),
    /// The FRESH re-gate verdict is loud (secret-/value-shaped or Delete) but the caller did not
    /// pass `acknowledged_loud == true`. The op refuses to write — the UI must show the
    /// "I've reviewed this" confirm and retry with the ack. Carries the reason.
    NeedsLoudConfirm(String),
    /// A mandate grant was refused by an engine grant-time guard (recipe too long, source scope not resolvable,
    /// target not write-granted, or target under a read-grant root). Carries the reason so the
    /// New-mandate form can show *why*. Distinct from `Core` so the UI can style it as a validation
    /// error, not an engine fault.
    Rejected(String),
    Join(String),
}

impl std::fmt::Display for EngineOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineOpError::Open(e) => write!(f, "{e}"),
            EngineOpError::Core(m) => write!(f, "engine error: {m}"),
            EngineOpError::Embedder(m) => write!(f, "memory model unavailable: {m}"),
            EngineOpError::Reasoner(m) => write!(f, "reasoner unavailable: {m}"),
            EngineOpError::Busy(op) => write!(f, "an {op} is already running"),
            EngineOpError::Stale(m) => write!(f, "the file changed since this was suggested: {m}"),
            EngineOpError::Revoked(m) => write!(f, "edits aren't allowed in this folder anymore: {m}"),
            EngineOpError::NeedsLoudConfirm(m) => write!(f, "this change needs an explicit review confirmation: {m}"),
            EngineOpError::Rejected(m) => write!(f, "{m}"),
            EngineOpError::Join(m) => write!(f, "engine task error: {m}"),
        }
    }
}

/// Classify an error string returned by the engine's execute path into an `EngineOpError`.
///
/// The engine's defense-in-depth loud-gate (`execute_write_inner`) refuses a loud write with a
/// known phrase (`bossclaw_core::LOUD_ACK_REQUIRED_MSG`). Surface that as `NeedsLoudConfirm`
/// (not `Core`) so the auto-apply sweep treats it as the benign "risky → leave queued" case rather
/// than an unexpected fault that pollutes the unexpected-error log channel. Any other engine error
/// stays `Core`. The phrase is the discriminator because the engine reports it as the generic
/// `BossclawError::InvalidInput` (shared by every fail-closed reject), so the typed variant alone
/// cannot distinguish the loud-reject — only its message can. The substring is single-sourced from
/// the engine const so the refusal site and this classifier can never drift.
fn execute_error_to_engine_op_error(msg: String) -> EngineOpError {
    if msg.contains(bossclaw_core::LOUD_ACK_REQUIRED_MSG) {
        EngineOpError::NeedsLoudConfirm(msg)
    } else {
        EngineOpError::Core(msg)
    }
}

/// A row in the Review queue, projected from one open `PendingProposal`. The
/// `requires_loud_modal` is lifted out of the propose-time `verdict_summary` for the badge/card.
#[derive(Debug, Clone)]
pub struct ProposalSummary {
    pub id: String,
    pub target: String,
    pub op: String,
    pub new_content_hash: String,
    pub rationale: String,
    pub requires_loud_modal: bool,
    /// The proposer's producer stamp (`"m6b-reconciler"` / `"m6c-mandate-proposer"`), surfaced so
    /// the Review UI can label a mandate-driven rewrite "from mandate".
    pub producer: String,
}

impl ProposalSummary {
    fn from_pending(p: bossclaw_core::PendingProposal) -> Self {
        // Single-sourced fail-loud default (m2): `PendingProposal::requires_loud_modal()` returns
        // true when the verdict is absent/garbled. This is a UI HINT to pre-show the modal; the
        // authoritative loud-confirm gate is the FRESH re-gate inside `apply_proposal` (Task 8).
        let requires_loud_modal = p.requires_loud_modal();
        Self {
            id: p.id,
            target: p.target,
            op: p.op,
            new_content_hash: p.new_content_hash,
            rationale: p.rationale,
            requires_loud_modal,
            producer: p.producer,
        }
    }
}

/// A mandate row for the desktop Mandates list, projected from `bossclaw_core::Mandate` (the six
/// fields map 1:1).
#[derive(Debug, Clone)]
pub struct MandateSummary {
    pub mandate_grant_id: String,
    pub target: String,
    pub source_scope: String,
    pub recipe: String,
    pub granted_at: String,
    pub revoked: bool,
}

/// One Mandate-activity row, projected from `bossclaw_core::MandateWriteRecord`.
#[derive(Debug, Clone)]
pub struct MandateWriteSummary {
    pub file_written_id: String,
    pub target: String,
    pub written_at: String,
    pub undone: bool,
}

impl From<bossclaw_core::MandateWriteRecord> for MandateWriteSummary {
    fn from(r: bossclaw_core::MandateWriteRecord) -> Self {
        Self { file_written_id: r.file_written_id, target: r.target, written_at: r.written_at, undone: r.undone }
    }
}

impl From<bossclaw_core::Mandate> for MandateSummary {
    fn from(m: bossclaw_core::Mandate) -> Self {
        Self {
            mandate_grant_id: m.mandate_grant_id,
            target: m.target,
            source_scope: m.source_scope,
            recipe: m.recipe,
            granted_at: m.granted_at,
            revoked: m.revoked,
        }
    }
}

/// Everything the Review card renders for one proposal: paths, the "Why", op, both text halves,
/// and the propose-time loud-modal/taint flags. `old_text`/`new_text` are lossy-UTF8 (the engine
/// only proposes against UTF-8 targets; non-UTF8 is rejected at synthesis).
#[derive(Debug, Clone)]
pub struct PreviewData {
    pub path: String,
    pub folder: String,
    pub rationale: String,
    pub op: String,
    pub old_text: String,
    pub new_text: String,
    pub requires_loud_modal: bool,
    pub taint: String,
}

/// The coarse engine state surfaced to the UI (distinguishes setup states from faults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    NotOnboarded,
    Ready,
    KeystoreInconsistent,
    KeystoreDbMismatch,
    ChainFailed,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub state: EngineState,
    pub event_count: i64,
    pub chain_ok: bool,
}

/// A recall `Hit` paired with its hydrated snippet text (`recall.rs::Hit` carries no text).
/// The command layer maps this to `HitDto`; the snippet is best-effort (missing → empty).
#[derive(Debug)]
pub struct HitWithText {
    pub hit: bossclaw_core::Hit,
    pub text: String,
}

/// Session-scoped evolve telemetry (the engine's own `EvolveStatus` stubs these fields to
/// `None/0/None`, so SP3 owns the real values). Reset on app restart; persistence is M7.
/// Written by `record_tick` + read by `evolve_status`/`EvolveStatusDto` (SP3 Tasks 7–8).
#[derive(Default, Clone)]
pub struct EvolveTelemetry {
    pub last_tick_ms: Option<u128>,
    pub error_count: usize,
    pub last_error: Option<String>,
    /// File-derived (`is_external`) snippet count sent by the most recent CLOUD tick
    /// (spec R4 egress transparency). `None` until a cloud tick runs this session; set
    /// ONLY on cloud ticks so the banner never reports snippets that stayed on-device.
    pub last_tainted_snippets: Option<usize>,
}

/// Session-scoped conflict-detection telemetry (mirrors [`EvolveTelemetry`]; in-memory, cleared
/// on restart — a durable lifetime count is derivable from the append-only `conflict_proposal`
/// events, so no table is needed). Written by `record_conflict_tick` + read by `conflict_telemetry`.
#[derive(Debug, Default, Clone)]
pub struct ConflictTelemetry {
    /// Wall-clock duration of the most recent cycle, ms.
    pub last_cycle_ms: Option<u128>,
    /// Cumulative proposals emitted this session.
    pub proposed_total: usize,
    /// Cumulative pairs the judge declined this session.
    pub dropped_total: usize,
    /// Cumulative reasoner errors this session.
    pub reasoner_errors_total: usize,
}

/// Session-scoped reflection telemetry (mirrors [`ConflictTelemetry`]; in-memory, cleared on restart — the
/// durable operational totals live in the core `reflect_counters` table). Written by `record_reflect_tick`.
#[derive(Debug, Default, Clone)]
pub struct ReflectTelemetry {
    /// Wall-clock duration of the most recent tick, ms.
    pub last_tick_ms: Option<u128>,
    /// Ticks that returned an engine error this session.
    pub error_count: usize,
    /// The most recent tick error (truncated ~512 bytes — it can embed paths/reasoner output).
    pub last_error: Option<String>,
    /// Cumulative session tallies from the per-tick `ReflectReport` (the scoreboard, §2.4).
    pub dossiers_refreshed_total: usize,
    /// Session total of honest "we never knew this" classifications.
    pub no_material_total: usize,
    /// Session total of misses parked at the attempt budget.
    pub parked_total: usize,
    /// Session total of structurally-unhealable stale pages (§2.3 residual).
    pub unhealable_thin_total: usize,
    /// Session total of per-item MODEL failures (see `ReflectReport::reasoner_errors` granularity note).
    pub reasoner_errors_total: usize,
    /// Session total of per-item ENGINE-I/O failures (kept apart — never blamed on the model).
    pub transient_errors_total: usize,
}

/// The core reads the pure `decide_reflect` gate consumes (spec §2.1).
pub struct ReflectGateInputs {
    pub newest_activity_at: Option<i64>,
    pub open_unparked_misses: usize,
    pub last_completed_run_at: i64,
    pub last_floor_fire_at: i64,
}

/// The single chokepoint for engine access. Holds one lazily-opened `Arc<EventLog>`
/// behind an async mutex; `get_or_open` serializes first-open and gates on onboarding.
pub struct EngineHandle {
    cell: Mutex<Option<Arc<EventLog>>>,
    keystore: EngineKeystore,
    db_path: PathBuf,
    embedder_provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
    /// Read by `evolve_once` (SP3 Task 7) to lazily build the reasoner on first evolve.
    reasoner_provider: Arc<dyn crate::engine::reason::ReasonerProvider>,
    ingest_lock: Mutex<()>,
    /// Serializes manual + scheduled evolve ticks (`try_lock` → `Busy("evolve")`).
    evolve_lock: Mutex<()>,
    /// Serializes manual + scheduled conflict-detection cycles (`try_lock` → `Busy("conflict")`).
    /// Mirrors `evolve_lock`.
    conflict_lock: Mutex<()>,
    /// Serializes `resolve_conflict` wrapper calls against EACH OTHER, per daemon. A double-submit of
    /// the SAME proposal (realistic when a reconnecting MCP client re-fires the op) becomes a
    /// deterministic first-wins outcome: one caller does the real work (`Applied`), the serialized
    /// second observes the terminal marker and returns a clean `NoOp` — never a spurious fail-loud
    /// `Rejected("already retired"/"already resolved")`. DEDICATED, NOT reused `conflict_lock`: a sweep
    /// cycle can hold `conflict_lock` for a long time (LLM judges) and must never block a user's
    /// resolve. resolve-vs-sweep and resolve-vs-App-manual-retire stay UNSERIALIZED by design — core's
    /// retired-set roll-forward gate + fail-loud primitives make those interleavings benign (design
    /// Open-Q9).
    resolve_lock: Mutex<()>,
    /// `true` once the in-memory recall index reflects persisted vectors this session.
    /// Set ONLY after a successful rebuild (a failure stays retryable). See `ensure_indexed`.
    indexed: Mutex<bool>,
    /// The evolve status read path (a `std::sync::Mutex`, poison-recovered on read).
    /// Written by `record_tick` + read by `evolve_status` (SP3 Task 7).
    evolve_tel: std::sync::Mutex<EvolveTelemetry>,
    /// Session conflict-detection telemetry (a `std::sync::Mutex`, poison-recovered on read).
    /// Written by `record_conflict_tick` + read by `conflict_telemetry`. Mirrors `evolve_tel`.
    conflict_tel: std::sync::Mutex<ConflictTelemetry>,
    /// Serializes manual + scheduled reflect ticks (`try_lock` → `Busy("reflect")`). DEDICATED, NOT reused
    /// `evolve_lock`: a long evolve tick must never block a reflect tick and vice-versa (the Rung-3
    /// dedicated-lock lesson).
    reflect_lock: Mutex<()>,
    /// Session reflection telemetry (poison-recovered on read). Mirrors `conflict_tel`.
    reflect_tel: std::sync::Mutex<ReflectTelemetry>,
    /// The shared reasoner-config cell the daemon's `ConfigReasonerProvider` closure reads on
    /// every evolve tick (attached by `main.rs` via [`Self::with_reasoner_cell`]; `None` in unit
    /// tests that don't care). Held HERE so BOTH config-writing ops (`set_reasoner_config`,
    /// `enable_cloud_reasoner`) refresh it write-through-style after a successful persist — the
    /// daemon-side replacement for the pre-M1a app-side write-through, and what makes a mode flip
    /// (including a Cloud→Local REVOCATION) take effect on the next tick without a daemon
    /// restart. Boot additionally reseeds it from the signed log (`reseed_reasoner_cell`).
    /// Living inside the engine (not the dispatch layer) means EVERY client surface — the app
    /// today, Claude Code in M1b — gets the write-through; no transport can persist a config the
    /// running provider won't see.
    reasoner_cell: Option<Arc<std::sync::Mutex<reason::ReasonerConfig>>>,
    /// TEST-ONLY probe-reasoner override for `enable_cloud_reasoner`'s R5 probe. ALWAYS `None` in
    /// production — the only setter is the `#[cfg(test)]` builder below, so no production path
    /// can bypass `build_reasoner`. Exists so the hermetic suite can drive the full
    /// probe→persist→cell-write-through path with a `ScriptedReasoner` (mirroring
    /// `CloudReasoner`'s `#[cfg(test)]` key seam — the reviewed 2a pattern for egress-free tests).
    probe_reasoner_for_test: Option<Arc<dyn bossclaw_core::Reasoner>>,
}

impl EngineHandle {
    pub fn new(
        vault: Arc<dyn SecretsVault>,
        data_dir: PathBuf,
        embedder_provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
        reasoner_provider: Arc<dyn crate::engine::reason::ReasonerProvider>,
    ) -> Self {
        Self {
            cell: Mutex::new(None),
            keystore: EngineKeystore::new(vault),
            db_path: data_dir.join("brain.db"),
            embedder_provider,
            reasoner_provider,
            ingest_lock: Mutex::new(()),
            evolve_lock: Mutex::new(()),
            conflict_lock: Mutex::new(()),
            resolve_lock: Mutex::new(()),
            indexed: Mutex::new(false),
            evolve_tel: std::sync::Mutex::new(EvolveTelemetry::default()),
            conflict_tel: std::sync::Mutex::new(ConflictTelemetry::default()),
            reflect_lock: Mutex::new(()),
            reflect_tel: std::sync::Mutex::new(ReflectTelemetry::default()),
            reasoner_cell: None,
            probe_reasoner_for_test: None,
        }
    }

    /// Attach the shared reasoner-config cell (see the field docs): after this, every successful
    /// `set_reasoner_config`/`enable_cloud_reasoner` persist ALSO refreshes the cell, so the
    /// provider closure picks the change up on the next tick — no restart, no revocation latency.
    /// Builder-style, called once at daemon boot; the write-through is a no-op when unattached.
    pub fn with_reasoner_cell(mut self, cell: Arc<std::sync::Mutex<reason::ReasonerConfig>>) -> Self {
        self.reasoner_cell = Some(cell);
        self
    }

    /// TEST-ONLY: override the one-shot reasoner `enable_cloud_reasoner` probes with (see the
    /// field docs). Compiled out of production and out of the `test-helpers` feature.
    #[cfg(test)]
    pub(crate) fn with_probe_reasoner_for_test(
        mut self,
        reasoner: Arc<dyn bossclaw_core::Reasoner>,
    ) -> Self {
        self.probe_reasoner_for_test = Some(reasoner);
        self
    }

    /// Open-or-get the single `EventLog`. The onboarding gate is enforced HERE (returns
    /// `NotOnboarded` if `!onboarded`) so no caller — including future SP3/SP4 commands —
    /// can bypass it. Holding the async-mutex guard across the open serializes concurrent
    /// first-opens, so exactly one `EventLog` is ever minted/constructed.
    pub async fn get_or_open(&self, onboarded: bool) -> Result<Arc<EventLog>, EngineError> {
        if !onboarded {
            return Err(EngineError::NotOnboarded);
        }
        let mut guard = self.cell.lock().await;
        if let Some(log) = guard.as_ref() {
            return Ok(log.clone());
        }
        let keystore = self.keystore.clone();
        let db_path = self.db_path.clone();
        let log = tokio::task::spawn_blocking(move || {
            let keys = keystore.load_or_mint()?;
            // Open bare, then force the three autonomy switches off. Any open/prime failure
            // maps to the existing open-failure path (KeystoreDbMismatch) — no new variant.
            EventLog::open(&db_path, &keys.dek, keys.signing_key)
                .and_then(|log| {
                    Self::prime_switches(&log)?;
                    Ok(log)
                })
                .map(Arc::new)
                .map_err(|e| EngineError::KeystoreDbMismatch(e.to_string()))
        })
        .await
        .map_err(|e| EngineError::Join(e.to_string()))??;
        // The `guard` held across the spawn_blocking open above is LOAD-BEARING: it
        // serializes first-open so exactly one EventLog is ever constructed. Do NOT
        // release it before storing here.
        *guard = Some(log.clone());
        Ok(log)
    }

    /// The daemon's OWN onboarding check (`<data_dir>/identity.json` parses), used to override a
    /// `MemoryClient`'s self-asserted `onboarded` flag so a guest-pass client can never force a
    /// keystore mint / brain creation. `data_dir` is `db_path`'s parent (`db_path =
    /// data_dir/brain.db`). Fail-safe false if the parent is unresolvable.
    pub fn is_onboarded_local(&self) -> bool {
        self.db_path.parent().map(crate::identity::is_onboarded).unwrap_or(false)
    }

    /// The daemon's data dir — `db_path`'s parent (`db_path = <data_dir>/brain.db`). The SP3
    /// capture dispatch (A10+) needs it to drive `capture::store::store_capture`; deriving it here
    /// keeps it single-sourced with [`Self::is_onboarded_local`] rather than threading `data_dir`
    /// through the shared accept loop. `None` if the parent is unresolvable (fail-safe).
    pub fn data_dir(&self) -> Option<&std::path::Path> {
        self.db_path.parent()
    }

    /// Get-or-open + verify the chain + count, mapped to a never-erroring `EngineStatus`.
    /// Open-failure (wrong DEK / unopenable) maps to `KeystoreDbMismatch`; an opened-but-
    /// tampered log maps to `ChainFailed` — the two are kept distinct (spec failure matrix).
    pub async fn status(&self, onboarded: bool) -> EngineStatus {
        let log = match self.get_or_open(onboarded).await {
            Ok(log) => log,
            Err(e) => return EngineStatus { state: map_err_state(&e), event_count: 0, chain_ok: false },
        };
        let probe = tokio::task::spawn_blocking(move || {
            let chain_ok = log.verify_chain().is_ok();
            let count = log.count().unwrap_or(0);
            (chain_ok, count)
        })
        .await;
        match probe {
            Ok((true, count)) => EngineStatus { state: EngineState::Ready, event_count: count, chain_ok: true },
            Ok((false, count)) => EngineStatus { state: EngineState::ChainFailed, event_count: count, chain_ok: false },
            Err(_) => EngineStatus { state: EngineState::KeystoreDbMismatch, event_count: 0, chain_ok: false },
        }
    }

    /// The loaded-vs-intended model state + live re-index progress (rung 2; U5/U6). A pure read of
    /// the provider's cells; the migration task and the loader guard keep them current.
    pub fn model_state(&self) -> (crate::engine::embed::ModelState, Option<(u64, u64)>) {
        (self.embedder_provider.model_state(), self.embedder_provider.reindex())
    }

    /// Language-pack status for the UI poll (rung 2; U6), the RPC entry point behind
    /// `Request::ModelStatus`. Onboarding-gated only to avoid touching the engine before onboarding;
    /// a not-onboarded daemon reports `Ok`/no-progress/base-id. Otherwise the provider cells for
    /// state + re-index progress, plus the currently-SERVED model id read from the signed record.
    ///
    /// The served id is a pure record read (never force-builds the embedder): a `Complete` record's
    /// id is what `resolve` loads; an absent/`InProgress` record (or a read failure) means the bundled
    /// English base is still serving — so the card reflects what is actually served (the old model
    /// until the migration flips), not merely what was requested.
    pub async fn model_status(
        &self,
        onboarded: bool,
    ) -> (crate::engine::embed::ModelState, Option<(u64, u64)>, String) {
        let base = crate::engine::embed::MODEL_ID.to_string();
        if !onboarded {
            return (crate::engine::embed::ModelState::Ok, None, base);
        }
        let (state, reindex) = self.model_state();
        let active_model_id = match self.get_or_open(onboarded).await {
            Ok(log) => match log.language_pack_record() {
                Ok(Some(r)) if r.migration == bossclaw_core::MigrationState::Complete => r.model_id,
                _ => base,
            },
            Err(_) => base,
        };
        (state, reindex, active_model_id)
    }

    /// Grant read-access to `path` (canonicalized + appended by the engine). Gated.
    pub async fn add_grant(&self, onboarded: bool, path: PathBuf) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.add_grant(&path).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Revoke a previously-granted folder. Gated.
    pub async fn revoke_grant(&self, onboarded: bool, path: PathBuf) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.revoke_grant(&path).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Enable (`on=true` → `add_write_grant`) or disable (`on=false` → `revoke_write_grant`)
    /// edits for `path`. Lock 1 of two. Gated. The engine canonicalizes + fails closed on a
    /// missing path; execute re-checks the grant at write time regardless.
    pub async fn set_folder_writable(&self, onboarded: bool, path: PathBuf, on: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let r = if on { log.add_write_grant(&path) } else { log.revoke_write_grant(&path) };
            r.map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// The canonical roots of every ACTIVE write-grant (revoked ones excluded). The UI uses
    /// this to mark folders + files writable. Gated.
    pub async fn list_writable(&self, onboarded: bool) -> Result<Vec<String>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let grants = log.write_grants().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(grants.into_iter().filter(|g| !g.revoked).map(|g| g.canonical_root).collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Every grant (active + revoked); the UI filters to active. Gated.
    pub async fn list_grants(&self, onboarded: bool) -> Result<Vec<bossclaw_core::Grant>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.grants().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Ingest every active granted folder (native text only), then record the
    /// active model once (so SP3 recall can discover it). Gated; serialized by an
    /// in-flight guard (a concurrent call returns `Busy`).
    pub async fn run_ingest(&self, onboarded: bool) -> Result<bossclaw_core::IngestReport, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let _guard = self.ingest_lock.try_lock().map_err(|_| EngineOpError::Busy("ingest"))?;
        let provider = self.embedder_provider.clone();
        let report = tokio::task::spawn_blocking(move || -> Result<bossclaw_core::IngestReport, EngineOpError> {
            let embedder = provider.embedder_for(&log)?; // resolved (env → signed record → bundled), cached — built BEFORE the walk
            let router = bossclaw_core::ingest::ParserRouter::native_only();
            let report = log.ingest_all(&router, &*embedder).map_err(|e| EngineOpError::Core(e.to_string()))?;
            // Record the active model at vector-birth (idempotent: only when absent or changed).
            let needs = match log.active_model().map_err(|e| EngineOpError::Core(e.to_string()))? {
                Some(m) => m.active_model_id != embedder.model_id(),
                None => true,
            };
            if needs {
                log.set_active_model(embedder.model_id(), embedder.dim() as u32)
                    .map_err(|e| EngineOpError::Core(e.to_string()))?;
            }
            Ok(report)
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        // `ingest_all` rebuilds the in-memory index internally, so the index now reflects
        // persisted vectors — mark it current to skip a redundant first-recall rebuild.
        *self.indexed.lock().await = true;
        Ok(report)
    }

    /// Neutralize the engine's dangerous default-ON autonomy flags at startup, WITHOUT
    /// clobbering a user's explicit choice. `evolve`/`proposals`/`mandates` are forced off ONLY
    /// when the user never explicitly set them (`!explicitly_set`), so an explicit on/off persists
    /// across opens (SP4 change-b for evolve/proposals; SP5 for mandates). Each setter is sticky;
    /// runs inside `get_or_open`'s first-open closure.
    fn prime_switches(log: &EventLog) -> Result<(), bossclaw_core::BossclawError> {
        use bossclaw_core::ConfigFlag;
        if !log.explicitly_set(ConfigFlag::Evolve)? && log.evolve_enabled()? {
            log.set_evolve_enabled(false)?;
        }
        if !log.explicitly_set(ConfigFlag::Proposals)? && log.proposals_enabled()? {
            log.set_proposals_enabled(false)?;
        }
        // SP5 ships mandates: persist an explicit user choice (force off ONLY when never set),
        // exactly like evolve/proposals above. A fresh install still primes off (default-open,
        // never-set ⇒ explicitly_set is false ⇒ force off).
        if !log.explicitly_set(ConfigFlag::Mandates)? && log.mandates_enabled()? {
            log.set_mandates_enabled(false)?;
        }
        // SP3 §6a (critic C1): capture is default-CLOSED — its getter already returns false when
        // unset — so, UNLIKE the flags above, the force-off is NOT gated on the getter being true
        // (that half would never fire). We persist an EXPLICIT OFF the first time it was never set:
        // belt-and-suspenders parity with the mandates precedent and a tamper-evident I10 record
        // ("this brain has capture off"). Idempotent — `explicitly_set` is true afterward, so a
        // re-open writes nothing. The timestamp arg is inert on the disable (off) path.
        if !log.explicitly_set(ConfigFlag::CaptureEnabled)? {
            log.set_capture_enabled(false, false, 0)?;
        }
        // Rung-3 Phase-2 (§3.6, I3): conflict detection is default-CLOSED — its getter already
        // returns false when unset — so, like capture above, we persist an EXPLICIT OFF the first
        // time it was never set (a tamper-evident "this brain has conflict-detect off" record).
        // Idempotent: `explicitly_set` is true afterward.
        if !log.explicitly_set(ConfigFlag::ConflictDetect)? {
            log.set_conflict_detect_enabled(false)?;
        }
        // Rung-4 R4-A (§2.1, I3): reflection is default-CLOSED — its getter already returns false when
        // unset — so, like capture/conflict above, persist an EXPLICIT OFF the first time it was never set
        // (a tamper-evident "this brain has reflection off" record). Idempotent: `explicitly_set` is true
        // afterward, so a re-open writes nothing. This is the SIXTH sticky config event on a fresh brain.
        if !log.explicitly_set(ConfigFlag::Reflect)? {
            log.set_reflect_enabled(false)?;
        }
        Ok(())
    }

    /// Build the in-memory recall index from persisted vectors the first time it's needed.
    /// The flag is set ONLY after a successful rebuild, so a rebuild error leaves it `false`
    /// and the next call retries (no silent-empty-recall trap). The `tokio::Mutex<bool>`
    /// serializes concurrent first-recalls (no double rebuild) and makes "set true only on
    /// success" race-free. Returns the (cached) embedder for the caller.
    async fn ensure_indexed(&self, log: &Arc<EventLog>) -> Result<Arc<dyn Embedder>, EngineOpError> {
        // Resolution-aware (env → signed record → bundled): a signed model whose files are
        // missing/mismatched makes this fail loud here, so recall REFUSES rather than silently
        // serving the wrong/empty model (I3/U5). `log: &Arc<EventLog>` auto-derefs to `&EventLog`.
        let embedder = self.embedder_provider.embedder_for(log)?;
        let mut done = self.indexed.lock().await;
        if !*done {
            let (log2, emb2) = (log.clone(), embedder.clone());
            spawn_blocking(move || -> Result<(), EngineOpError> {
                log2.rebuild_indexes(&*emb2).map_err(|e| EngineOpError::Core(e.to_string()))?;
                log2.rebuild_graph().map_err(|e| EngineOpError::Core(e.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))??;
            *done = true;
        }
        Ok(embedder)
    }

    /// Hybrid recall over persisted memory (semantic + keyword), with best-effort snippet
    /// hydration. Gated → `ensure_indexed` → `spawn_blocking(log.recall)`. A missing snippet
    /// becomes an empty string (never errors): `event_by_id` is best-effort per hit.
    pub async fn recall(
        &self,
        onboarded: bool,
        query: String,
        k: usize,
    ) -> Result<Vec<HitWithText>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let embedder = self.ensure_indexed(&log).await?;
        spawn_blocking(move || -> Result<Vec<HitWithText>, EngineOpError> {
            let hits = log
                .recall(&*embedder, &query, k, &bossclaw_core::RecallOptions::default())
                .map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(hits
                .into_iter()
                .map(|h| {
                    let text = log
                        .event_by_id(&h.event_id)
                        .ok()
                        .flatten()
                        .and_then(|e| e.content.get("text").and_then(|t| t.as_str()).map(str::to_owned))
                        .unwrap_or_default();
                    HitWithText { hit: h, text }
                })
                .collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Append a signed external-tainted `memory` (U1) and return its event id. Resolves the
    /// active embedder (env → signed record → bundled), derives the note's vector on a blocking
    /// thread, then invalidates the recall index so the NEXT `recall` rebuilds and surfaces it
    /// (the same index-invalidation contract as `publish_and_invalidate`). Empty/blank text is
    /// the engine's typed `Rejected`; any other core failure folds to `Core`.
    pub async fn remember(&self, onboarded: bool, text: String) -> Result<String, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let embedder = self.embedder_provider.embedder_for(&log)?;
        let id = spawn_blocking(move || -> Result<String, EngineOpError> {
            log.remember(&*embedder, &text).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        // Force the next recall to rebuild the in-memory index so the new memory is searchable
        // (its vector is persisted; the FTS entry is (re)built by `rebuild_indexes`).
        *self.indexed.lock().await = false;
        Ok(id)
    }

    /// SP3 A7: record a captured coding-agent session as a signed, external-tainted,
    /// embeddable `session_captured` event (the `.md` body file is written separately by the
    /// capture store — this records only the event). Resolves the active embedder (env → signed
    /// record → bundled), derives the title vector on a blocking thread, then invalidates the
    /// recall index so the next `recall` surfaces it (the same index-invalidation contract as
    /// [`Self::remember`]). A tombstoned session (I9) or other reject folds to the typed
    /// `Rejected`; any other core failure folds to `Core`.
    ///
    /// Onboarding is resolved from the daemon's OWN [`Self::is_onboarded_local`] (the capture
    /// path is daemon-internal — sweeper/dispatch — never a client-asserted `onboarded` flag).
    pub async fn capture_session(
        &self,
        meta: bossclaw_core::log::SessionMeta,
    ) -> Result<String, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let embedder = self.embedder_provider.embedder_for(&log)?;
        let id = spawn_blocking(move || -> Result<String, EngineOpError> {
            log.capture_session(&*embedder, &meta).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        *self.indexed.lock().await = false;
        Ok(id)
    }

    /// Rung-3 Phase-1 (§7.1): embed + persist a capture's body passages into the
    /// `session_passage_vectors` table (the conflict index's restart-safe source). The
    /// embedder is resolved INTERNALLY (like [`Self::capture_session`]) so the daemon capture
    /// path never handles one — it passes only the capture's `event_id` + its body `chunks`.
    /// A blank/empty embed folds to `Rejected`; any other core failure folds to `Core`, mirroring
    /// `capture_session`'s mapping.
    ///
    /// This writes a SEPARATE table, NOT the recall index — so (unlike `capture_session`) it must
    /// NOT invalidate `self.indexed`.
    pub async fn store_session_passages(
        &self,
        event_id: String,
        chunks: Vec<String>,
    ) -> Result<(), EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let embedder = self.embedder_provider.embedder_for(&log)?;
        spawn_blocking(move || -> Result<(), EngineOpError> {
            log.store_session_passages(&*embedder, &event_id, &chunks).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Rung-3 Phase-1 (§7.1): true when NO passage rows exist yet for `event_id`. The capture
    /// path gates on this so it persists passages on the FIRST capture and SKIPS re-embedding on a
    /// same-`sha` recapture that already has rows. A pure read; gated + `spawn_blocking` like
    /// [`Self::current_sessions`].
    pub async fn session_passages_absent(&self, event_id: String) -> Result<bool, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.session_passages_absent(&event_id).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// SP3 A7/I7: append the owner-commanded `session_deleted` tombstone so the session leaves
    /// the fold + recall (the `.md` file itself is removed by the capture store). A delete of a
    /// session that is NOT current (already gone / superseded away / never captured) is the
    /// engine's typed `Rejected` — the capture store treats that as an idempotent no-op (the
    /// session is already tombstoned). Invalidates the recall index like [`Self::remember`].
    pub async fn delete_session(&self, session_id: String) -> Result<String, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let id = spawn_blocking(move || -> Result<String, EngineOpError> {
            log.delete_session(&session_id).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        *self.indexed.lock().await = false;
        Ok(id)
    }

    /// SP3 A7: the CURRENT captured sessions (the latest non-superseded, non-tombstoned capture
    /// per `session_id`) — the fold the capture store reconciles against during orphan healing.
    /// A pure read; gated + `spawn_blocking` like the other reads.
    pub async fn current_sessions(
        &self,
    ) -> Result<Vec<bossclaw_core::log::CurrentSession>, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || log.current_sessions().map_err(|e| EngineOpError::Core(e.to_string())))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// SP3 §7/§9: the CURRENT (non-superseded) `remember` notes, newest-first — the read
    /// behind the App-only `ListNotes` op (the Memory-browser notes list). A pure read;
    /// gated + `spawn_blocking` like [`Self::current_sessions`]. Onboarding is the daemon's
    /// OWN verdict (App-only op; the SP3 read family never trusts a client `onboarded` flag).
    pub async fn current_notes(
        &self,
    ) -> Result<Vec<bossclaw_core::log::CurrentNote>, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || log.current_notes().map_err(|e| EngineOpError::Core(e.to_string())))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// SP3 §7: supersede a `remember` note with new text (the App-only `SupersedeNote` edit),
    /// returning the NEW note's event id. Appends the atomic `supersede`+corrected-note pair
    /// then invalidates the recall index so the next `recall` surfaces the NEW text and drops
    /// the old (the same index-invalidation contract as [`Self::remember`]). A blank text, a
    /// non-note / missing target, or an already-superseded target folds to the typed `Rejected`
    /// (the engine reports all four as `InvalidInput`); any other core failure folds to `Core`.
    /// Onboarding is the daemon's OWN verdict (App-only, mirrors [`Self::delete_session`]).
    pub async fn supersede_note(
        &self,
        target_event_id: String,
        text: String,
    ) -> Result<String, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let embedder = self.embedder_provider.embedder_for(&log)?;
        let id = spawn_blocking(move || -> Result<String, EngineOpError> {
            log.supersede_note(&*embedder, &target_event_id, &text).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        *self.indexed.lock().await = false;
        Ok(id)
    }

    /// Rung 3 §7.3: retire a `remember` note (the App-only `RetireMemory` note variant) by appending a
    /// distinct `note_retired` marker, returning the marker's event id. Recall/list exclusion is
    /// fold-time, so — UNLIKE [`Self::supersede_note`] — NO embedder is resolved and the recall index
    /// is NOT invalidated (nothing to re-embed; the next fold simply drops the retired target). A
    /// missing / non-memory, superseded, or already-retired target folds to the typed `Rejected` (core
    /// reports these as `InvalidInput`); any other core failure folds to `Core`. Onboarding is the
    /// daemon's OWN verdict (App-only, mirrors [`Self::current_notes`]).
    pub async fn retire_memory(&self, event_id: String) -> Result<String, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.retire_memory(&event_id, None).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Rung 3 §7.3: reverse a prior note retire (the App-only `Unretire` op) by appending an `unretire`
    /// marker, returning its event id. Like [`Self::retire_memory`] this is a pure marker append: NO
    /// embedder, NO index invalidation (fold-time exclusion). An id that is not CURRENTLY retired folds
    /// to the typed `Rejected` (core reports `InvalidInput`); any other core failure folds to `Core`.
    /// Onboarding is the daemon's OWN verdict (App-only).
    pub async fn unretire(&self, retired_event_id: String) -> Result<String, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.unretire(&retired_event_id).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Rung 3 §7.2: retire a SINGLE session passage (the App-only `RetireMemory{Passage}` op) by
    /// appending a `passage_retired` marker, returning its event id. Like [`Self::retire_memory`] this
    /// is a pure marker append: NO embedder, NO index invalidation (the next conflict-index rebuild
    /// simply drops the retired passage). An unknown session, an out-of-range `passage_id`, or an
    /// already-retired passage folds to the typed `Rejected` (core reports `InvalidInput`); any other
    /// core failure folds to `Core`. Onboarding is the daemon's OWN verdict (App-only). Passage
    /// UNretire is core-only in Phase 1 (no wire op), so there is no matching wrapper here.
    pub async fn retire_passage(
        &self,
        session_id: String,
        passage_id: usize,
    ) -> Result<String, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.retire_passage(&session_id, passage_id, None).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// SP3 A9/I9: the tombstoned (owner-deleted) `session_id`s. The sweeper reads this so a
    /// deleted session's still-present transcript is never re-captured (never resurrected) — the
    /// engine's `capture_session` reject is the backstop, this is the cheap decision-time filter.
    /// A pure read; gated + `spawn_blocking` like [`Self::current_sessions`].
    pub async fn deleted_session_ids(&self) -> Result<Vec<String>, EngineOpError> {
        let onboarded = self.is_onboarded_local();
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.deleted_session_ids().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// SP3 §6a: flip the capture flags (the Connect checkbox → `enabled=true, backfill=true`; the
    /// Integrations toggle → `enabled=true, backfill=false`; off → `enabled=false`). Mirrors
    /// [`Self::set_mandates_enabled`]; the daemon supplies `at` so core stays clock-free. The
    /// sweeper (A9) and the dispatch arms (A10/A13) drive this. Gated.
    pub async fn set_capture_enabled(
        &self,
        onboarded: bool,
        enabled: bool,
        backfill: bool,
        at: i64,
    ) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_capture_enabled(enabled, backfill, at)
                .map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// SP3 §6a: read the sticky ongoing-capture flag (default CLOSED — critic C1). The sweeper (A9)
    /// gates every candidate on this; the `CaptureEnabled` dispatch (A10/A13) surfaces it to the
    /// app. Mirrors [`Self::mandates_enabled`]. Gated.
    pub async fn capture_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.capture_enabled().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// SP3 §6a: read the sticky one-time backfill consent (default CLOSED — critic M4). The sweeper
    /// (A9) reads it to decide the backfill-vs-forward-only window. Gated.
    pub async fn backfill_consented(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.backfill_consented().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// SP3 §6a: read the instant capture last flipped ON (`None` if never), backing the sweeper's
    /// forward-only window (`mtime >= capture_enabled_at`). Gated.
    pub async fn capture_enabled_at(&self, onboarded: bool) -> Result<Option<i64>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.capture_enabled_at().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Run ONE evolve tick (gated, serialized). Gate → `evolve_lock.try_lock()`
    /// (`Busy("evolve")` if a manual + scheduled tick overlap) → `ensure_indexed` (yields
    /// the embedder) → build the reasoner → `spawn_blocking`: `evolve_once` THEN
    /// `rebuild_indexes` THEN `rebuild_graph` (so recall sees the new entities/links/dossiers)
    /// → record telemetry. The post-tick rebuild is why a follow-up `recall` surfaces what
    /// this tick minted.
    pub async fn evolve_once(&self, onboarded: bool) -> Result<bossclaw_core::EvolveReport, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        // Serialize manual + scheduled ticks: a second overlapping tick is Busy, not queued.
        let _guard = self.evolve_lock.try_lock().map_err(|_| EngineOpError::Busy("evolve"))?;
        // Consent chokepoint for BOTH the scheduler AND manual `engine_evolve_now` (R1/R5/R8),
        // shared with the conflict sweep (I2). Placed BEFORE the reasoner is built (and any
        // spawn_blocking/network), so a cloud-not-ready tick constructs no reasoner and egresses
        // nothing.
        if !self.cloud_consent_ok(onboarded).await {
            return Err(EngineOpError::Reasoner(
                "cloud reasoner not ready — signed consent or provider key missing".to_string(),
            ));
        }
        let embedder = self.ensure_indexed(&log).await?;
        let reasoner = self.reasoner_provider.reasoner()?;
        let t0 = std::time::Instant::now();
        let result = spawn_blocking({
            let log = log.clone();
            let emb = embedder.clone();
            move || -> Result<bossclaw_core::EvolveReport, EngineOpError> {
                // Precondition: `evolve_once` resolves mentions via the entity-resolution
                // index, which `entity_search` requires be built (it errors otherwise). It is
                // SEPARATE from the recall index (`ensure_indexed` builds that) and is NOT
                // built by `open`/`ingest_all` — the engine only rebuilds it AT THE END of a
                // tick, so the FIRST tick on a freshly-opened log must build it here. Cheap:
                // it reads persisted entity_vectors (empty on a fresh store → empty index,
                // which is valid — every mention then mints). Mirrors the engine's own test
                // seed lifecycle (`tests/evolve.rs::seed_memory`).
                log.rebuild_entity_index(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
                let report = log
                    .evolve_once(&*emb, &*reasoner)
                    .map_err(|e| EngineOpError::Core(e.to_string()))?;
                // Fold the new vectors + edges so recall reflects this tick's curation.
                log.rebuild_indexes(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
                log.rebuild_graph().map_err(|e| EngineOpError::Core(e.to_string()))?;
                Ok(report)
            }
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?;
        // The spec-R4 backstop needs to know whether this was a CLOUD tick and whether the
        // queue had work, so an Ok-but-0-processed cloud tick (a silent bad/expired key) is
        // recorded as a visible error rather than no-oping forever.
        let cloud_mode = matches!(
            self.reasoner_config_or_default(onboarded).await.mode,
            crate::engine::reason::ReasonerMode::Cloud
        );
        let queue_depth = self.queue_depth_or_zero(onboarded).await;
        self.record_tick(t0.elapsed().as_millis(), &result, cloud_mode, queue_depth);
        result
    }

    /// Run ONE reflect tick (gated, serialized). `reflect_lock.try_lock()` (`Busy("reflect")` on
    /// overlap) → `cloud_consent_ok` (BEFORE the reasoner is built, so a cloud-not-ready tick egresses
    /// nothing, I2) → read the SP3 miss-ring queries (non-destructive) → `ensure_indexed` →
    /// `spawn_blocking`: `rebuild_entity_index` (the query→topic bridge precondition) THEN core
    /// `reflect_once` THEN `rebuild_indexes` + `rebuild_graph` (so a follow-up recall sees the refreshed
    /// dossiers) → record telemetry → stamp the floor's last-completed marker. `now` is the
    /// sweeper-boundary clock (mirrors `detect_conflicts_once(onboarded, now)`).
    pub async fn reflect_once(
        &self,
        onboarded: bool,
        now: i64,
    ) -> Result<bossclaw_core::ReflectReport, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        // Serialize manual + scheduled ticks: a second overlapping tick is Busy, not queued. DEDICATED
        // lock (never `evolve_lock`): a long evolve tick must never block a reflect tick and vice-versa.
        let _guard = self.reflect_lock.try_lock().map_err(|_| EngineOpError::Busy("reflect"))?;
        // Shared cloud-egress consent barrier (I2), placed BEFORE the reasoner is built (and any
        // spawn_blocking/network), so a cloud-not-ready tick constructs no reasoner and egresses
        // nothing — the same chokepoint `evolve_once`/`detect_conflicts_once` use.
        if !self.cloud_consent_ok(onboarded).await {
            return Err(EngineOpError::Reasoner(
                "cloud reasoner not ready — signed consent or provider key missing".to_string(),
            ));
        }
        // Non-destructive read of the SP3 miss ring (queries only). The durable backlog's `seed_miss`
        // is upsert-if-new, so re-reading the same ≤20 queries every tick is idempotent (a terminal
        // miss is never reset); a non-destructive read also PRESERVES the App's `RecallStats` "recent
        // misses" view. A missing/unreadable telemetry store degrades to a zero-miss tick, never an
        // error (I6 fail-safe — telemetry absence must not block reflection).
        let new_misses: Vec<String> = self
            .data_dir()
            .and_then(|d| crate::telemetry::Telemetry::open(d).ok())
            .and_then(|t| t.stats().ok())
            .map(|s| s.recent_misses.into_iter().map(|m| m.query).collect())
            .unwrap_or_default();
        let embedder = self.ensure_indexed(&log).await?;
        let reasoner = self.reasoner_provider.reasoner()?;
        let t0 = std::time::Instant::now();
        let result = spawn_blocking({
            let log = log.clone();
            let emb = embedder.clone();
            move || -> Result<bossclaw_core::ReflectReport, EngineOpError> {
                // Precondition: the miss→topic bridge resolves queries via the entity-resolution
                // index (as `evolve_once` does), which `entity_search` requires be built. It is
                // SEPARATE from the recall index (`ensure_indexed`) and NOT built by `open` — the
                // FIRST tick on a freshly-opened log must build it here.
                log.rebuild_entity_index(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
                let report = log
                    .reflect_once(&*emb, &*reasoner, &new_misses, now)
                    .map_err(|e| EngineOpError::Core(e.to_string()))?;
                // Fold the refreshed dossiers/vectors so a follow-up recall reflects this tick.
                log.rebuild_indexes(&*emb).map_err(|e| EngineOpError::Core(e.to_string()))?;
                log.rebuild_graph().map_err(|e| EngineOpError::Core(e.to_string()))?;
                Ok(report)
            }
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?;
        self.record_reflect_tick(t0.elapsed().as_millis(), &result);
        // On a completed tick, stamp the last-completed-run marker (best-effort; backs the §2.1
        // starvation floor). ONLY on Ok — a failed tick must not advance the floor's clock.
        if result.is_ok() {
            let log2 = log.clone();
            let _ = spawn_blocking(move || log2.set_reflect_last_completed_run(now)).await;
        }
        result
    }

    /// Record one reflect tick's telemetry (thin `&self` wrapper over the pure recorder).
    fn record_reflect_tick(
        &self,
        ms: u128,
        result: &Result<bossclaw_core::ReflectReport, EngineOpError>,
    ) {
        record_reflect_tick_into(&self.reflect_tel, ms, result);
    }

    /// Record one tick's telemetry. Thin wrapper over the pure [`record_tick_into`] so the
    /// recorder (including the spec-R4 cloud-0-item backstop) is unit-testable without a handle.
    fn record_tick(
        &self,
        ms: u128,
        result: &Result<bossclaw_core::EvolveReport, EngineOpError>,
        cloud_mode: bool,
        queue_depth: usize,
    ) {
        record_tick_into(&self.evolve_tel, ms, result, cloud_mode, queue_depth);
    }

    /// Evolve status: the engine's live `{queue_depth, enabled}` plus a clone of the
    /// session telemetry (`{last_tick_ms, error_count, last_error}` — the engine stubs those).
    /// Gated → `spawn_blocking(log.evolve_status())` → poison-recovered telemetry clone.
    pub async fn evolve_status(
        &self,
        onboarded: bool,
    ) -> Result<(bossclaw_core::EvolveStatus, EvolveTelemetry), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let status = spawn_blocking(move || {
            log.evolve_status().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        let tel = self.evolve_tel.lock().unwrap_or_else(|p| p.into_inner()).clone();
        Ok((status, tel))
    }

    /// The evolve off-switch verdict, defaulting to `false` (OFF) on ANY error (not onboarded,
    /// open failure, …). A thin gate-and-default read the scheduler loop uses each tick — it
    /// must never propagate an error (a transient read failure must not trip the loop ON).
    ///
    /// Unlike the sister gates, this reads the composite [`Self::evolve_status`] (which also carries
    /// queue depth + telemetry), NOT a bare flag getter — so it deliberately does NOT share
    /// [`Self::flag_enabled_or_false`]; folding it in would change its read path.
    pub async fn evolve_enabled_or_false(&self, onboarded: bool) -> bool {
        match self.evolve_status(onboarded).await {
            Ok((status, _telemetry)) => status.enabled,
            Err(_) => false,
        }
    }

    /// Shared fail-closed flag read for the sister off-switches: open the log (→ `false` on any open
    /// failure) and read one sticky bool getter inside `spawn_blocking`, defaulting to `false` (OFF)
    /// on ANY error — a transient read failure must never trip a background loop ON. `read` is the
    /// bare getter ([`EventLog::conflict_detect_enabled`] / [`EventLog::reflect_enabled`] /
    /// [`EventLog::mandates_enabled`]). [`Self::evolve_enabled_or_false`] is deliberately NOT a
    /// caller — it reads the composite `evolve_status`, not a bare flag.
    async fn flag_enabled_or_false(
        &self,
        onboarded: bool,
        read: fn(&EventLog) -> Result<bool, bossclaw_core::BossclawError>,
    ) -> bool {
        let Ok(log) = self.get_or_open(onboarded).await else {
            return false;
        };
        spawn_blocking(move || read(&log).unwrap_or(false))
            .await
            .unwrap_or(false)
    }

    /// The conflict-detection off-switch verdict, defaulting to `false` (OFF) on ANY error (not
    /// onboarded, open failure, …). The gate the conflict sweep reads each cycle — it must never
    /// propagate an error (a transient read failure must not trip detection ON). A fail-closed bare
    /// getter read via [`Self::flag_enabled_or_false`], like [`Self::mandates_enabled_or_false`] /
    /// [`Self::reflect_enabled_or_false`].
    pub async fn conflict_detect_enabled_or_false(&self, onboarded: bool) -> bool {
        self.flag_enabled_or_false(onboarded, EventLog::conflict_detect_enabled).await
    }

    /// The reflection off-switch verdict, defaulting to `false` (OFF) on ANY error (not onboarded, open
    /// failure, …). The gate the reflect sweep reads each cycle — it must never propagate an error (a
    /// transient read failure must not trip reflection ON). Mirrors [`Self::conflict_detect_enabled_or_false`].
    pub async fn reflect_enabled_or_false(&self, onboarded: bool) -> bool {
        self.flag_enabled_or_false(onboarded, EventLog::reflect_enabled).await
    }

    /// Rung-4 R4-A: flip the sticky reflection off-switch (the toggle behind the settings panel). The
    /// wire write behind [`Request::SetReflectEnabled`]. Gated. Mirrors [`Self::set_evolve_enabled`].
    pub async fn set_reflect_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_reflect_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Rung-4 R4-A: read the sticky reflection flag (the toggle POSITION; default CLOSED). The
    /// Result-returning wire read behind [`Request::ReflectEnabled`]; DISTINCT from
    /// [`Self::reflect_enabled_or_false`] (the fail-safe bool gate the sweeper reads each cycle).
    /// Mirrors [`Self::capture_enabled`]. Gated.
    pub async fn reflect_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.reflect_enabled().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// The core reads `decide_reflect` needs, in ONE spawn_blocking. `None` on an open failure (→ the
    /// sweeper no-ops that cycle).
    pub async fn reflect_gate_inputs(&self, onboarded: bool) -> Option<ReflectGateInputs> {
        let log = self.get_or_open(onboarded).await.ok()?;
        spawn_blocking(move || {
            let cur = log.reflect_cursor().unwrap_or_default();
            ReflectGateInputs {
                newest_activity_at: log.newest_memory_activity_at().unwrap_or(None),
                open_unparked_misses: log.open_miss_count().unwrap_or(0),
                last_completed_run_at: cur.last_completed_run_at,
                last_floor_fire_at: cur.last_floor_fire_at,
            }
        })
        .await
        .ok()
    }

    /// Reasoner readiness for reflection, mirroring the evolve scheduler's `select_ready` block
    /// (scheduler.rs): cloud mode trusts signed-consent readiness, local mode the Ollama probe; cloud
    /// NEVER silently falls back to local (spec §2.1 / §3.4).
    pub async fn reflect_reasoner_ready(&self, onboarded: bool) -> bool {
        let cfg = self.reasoner_config_or_default(onboarded).await;
        let cloud_mode = matches!(cfg.mode, crate::engine::reason::ReasonerMode::Cloud);
        let ollama_ready = if cloud_mode {
            false
        } else {
            let oll = crate::engine::ollama_probe::probe(crate::engine::reason::REASONER_MODEL_ID).await;
            oll.reachable && oll.model_present
        };
        let cloud_ready = if cloud_mode {
            self.reasoner_ready_or_false(onboarded).await
        } else {
            false
        };
        crate::engine::scheduler::select_ready(cloud_mode, ollama_ready, cloud_ready)
    }

    /// Stamp the last-floor-fire marker (best-effort; the §2.1 re-fire guard).
    pub async fn stamp_reflect_floor_fire(&self, onboarded: bool, now: i64) {
        if let Ok(log) = self.get_or_open(onboarded).await {
            let _ = spawn_blocking(move || log.set_reflect_last_floor_fire(now)).await;
        }
    }

    /// Run ONE conflict-detection cycle (gated, serialized). Gate → `conflict_lock.try_lock()`
    /// (`Busy("conflict")` on overlap) → shared cloud-consent pre-gate (I2) → `ensure_indexed`
    /// (embedder) → build reasoner → `spawn_blocking(log.detect_conflicts_once)` with the daemon
    /// passage-text resolver → record session telemetry. Mirrors [`Self::evolve_once`]'s op pattern.
    ///
    /// Off-by-default: the core flag gate makes the cycle emit nothing, but the scheduler MUST still
    /// gate on [`Self::conflict_detect_enabled_or_false`] before calling (as the evolve loop does) —
    /// calling this on a disabled brain still does lock/consent/index/reasoner-build work and, for a
    /// cloud-configured-but-unconsented brain, returns `Err(Reasoner(..))` rather than a clean
    /// `Ok(skipped_disabled)`.
    pub async fn detect_conflicts_once(
        &self,
        onboarded: bool,
        now: i64,
    ) -> Result<bossclaw_core::ConflictDetectReport, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        // Serialize manual + scheduled cycles: a second overlapping cycle is Busy, not queued.
        let _guard = self.conflict_lock.try_lock().map_err(|_| EngineOpError::Busy("conflict"))?;
        // Shared cloud-egress consent barrier (I2), placed BEFORE the reasoner is built (and any
        // spawn_blocking/network), so a cloud-not-ready cycle constructs no reasoner and egresses
        // nothing — the same chokepoint `evolve_once` uses.
        if !self.cloud_consent_ok(onboarded).await {
            return Err(EngineOpError::Reasoner(
                "cloud reasoner not ready — signed consent or provider key missing".to_string(),
            ));
        }
        // Core holds only passage VECTORS; the daemon supplies passage TEXT by reading the `.md`.
        // Resolve the data dir now (fail-safe) for the resolver closure below.
        let Some(data_dir) = self.data_dir().map(|p| p.to_path_buf()) else {
            return Err(EngineOpError::Core("data dir unresolvable".to_string()));
        };
        let embedder = self.ensure_indexed(&log).await?;
        let reasoner = self.reasoner_provider.reasoner()?;
        let t0 = std::time::Instant::now();
        let result =
            spawn_blocking(move || -> Result<bossclaw_core::ConflictDetectReport, EngineOpError> {
                // The daemon's side of the judge: re-chunk the stored `.md` with the SAME
                // `chunk_text` the capture used, so `passage_id` maps to the identical chunk.
                let passage_text = |session_id: &str, passage_id: usize| -> Option<String> {
                    crate::capture::store::session_passage_text(&data_dir, session_id, passage_id)
                };
                // Phase 2: `resolution_excluded_refs` is EMPTY (Phase 3 fills it).
                log.detect_conflicts_once(
                    &*embedder,
                    &*reasoner,
                    &passage_text,
                    &std::collections::HashSet::new(),
                    now,
                )
                .map_err(|e| EngineOpError::Core(e.to_string()))
            })
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?;
        self.record_conflict_tick(t0.elapsed().as_millis(), &result);
        result
    }

    /// Accumulate one cycle's outcome into the session telemetry (poison-tolerant, like evolve's).
    /// `last_cycle_ms` is the wall-clock of the just-finished cycle; the totals accrue only on `Ok`.
    fn record_conflict_tick(
        &self,
        ms: u128,
        result: &Result<bossclaw_core::ConflictDetectReport, EngineOpError>,
    ) {
        let mut tel = self.conflict_tel.lock().unwrap_or_else(|p| p.into_inner());
        tel.last_cycle_ms = Some(ms);
        if let Ok(r) = result {
            tel.proposed_total += r.proposed;
            tel.dropped_total += r.dropped;
            tel.reasoner_errors_total += r.reasoner_errors;
        }
    }

    /// A clone of the session conflict telemetry (poison-recovered). Mirrors the evolve telemetry read.
    pub fn conflict_telemetry(&self) -> ConflictTelemetry {
        self.conflict_tel.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Flip the sticky conflict-detection off-switch. Gated + `spawn_blocking`. Mirrors
    /// [`Self::set_evolve_enabled`].
    pub async fn set_conflict_detect_enabled(
        &self,
        onboarded: bool,
        enabled: bool,
    ) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_conflict_detect_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Rung-3 Phase-3: the pending conflict proposals (already coexist/dismissed-filtered by core).
    /// Onboarding is the daemon's OWN verdict passed in by the dispatch layer (guest-reachable — the
    /// I8 relaxation — but the daemon computes onboarding, never the client). Read-only: NO embedder,
    /// NO reasoner, NO egress. Mirrors [`Self::deleted_session_ids`]'s pure-read idiom (`Core` on any
    /// core failure).
    pub async fn list_conflicts(
        &self,
        onboarded: bool,
    ) -> Result<Vec<bossclaw_core::ConflictProposalRow>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.pending_conflict_proposals().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Rung-3 Phase-3: resolve one conflict proposal. Deterministic, no LLM, no egress. An unknown or
    /// already-resolved-by-a-different-action proposal folds to the typed `Rejected` (core reports
    /// `InvalidInput`); any other core failure folds to `Core`. Onboarding is the daemon's OWN verdict
    /// passed in by the dispatch layer. Mirrors [`Self::retire_memory`]'s marker-append idiom (same
    /// `InvalidInput` → `Rejected` mapping).
    ///
    /// Holds `resolve_lock` across the WHOLE body (acquired before `get_or_open`, released after the
    /// `spawn_blocking` completes) so concurrent resolves serialize against EACH OTHER per daemon: a
    /// double-submit of one proposal (realistic when a reconnecting MCP client re-fires the op) is a
    /// deterministic first-wins `Applied` + serialized `NoOp`, never a spurious fail-loud `Rejected`.
    /// resolve-vs-sweep and resolve-vs-App-manual-retire stay unserialized by design — core's
    /// retired-set roll-forward gate + fail-loud primitives make those interleavings benign (Open-Q9).
    pub async fn resolve_conflict(
        &self,
        onboarded: bool,
        proposal_id: String,
        action: bossclaw_core::ResolveAction,
    ) -> Result<bossclaw_core::ResolveOutcome, EngineOpError> {
        // Serialize resolves against each other (see the doc + the `resolve_lock` field): held across
        // the whole op — acquired before `get_or_open`, dropped at function return after the
        // `spawn_blocking` await — so a double-submit is first-wins Applied + NoOp, never a raced
        // fail-loud Err. `.lock().await` WAITS (does not `Busy`-reject like `conflict_lock`): the
        // second submit must observe the first's terminal marker, which is what yields the clean NoOp.
        let _resolve_guard = self.resolve_lock.lock().await;
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.resolve_conflict(&proposal_id, action).map_err(|e| match e {
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// SERVE the two Rung-3 visibility-digest lines for the SessionStart snapshot preamble (§2.4) —
    /// "serve" because this read has ONE deliberate side effect: it conditionally advances the digest
    /// cursor. INFALLIBLE — returns an empty Vec on ANY error (not onboarded, open failure, join/core
    /// failure) or when there is no conflict activity, so the snapshot builder never breaks (I1).
    /// Integer counts only — no memory content, so nothing here needs sanitizing. Onboarding is the
    /// daemon's OWN verdict (mirrors [`Self::current_sessions`]); this read holds NO conflict-sweep
    /// lock (a pure read is safe concurrent with a sweep). Advances `conflict_digest_cursor` to the
    /// store's current max seq ONLY when `source == "startup"`, so the window is honestly "since the
    /// last SESSION START": [`crate::capture::snapshot::build`] also runs for `source == "compact"`
    /// (and resume), and advancing there would let a mid-session compact CONSUME the "Since last
    /// session:" activity before the real next start ever shows it; `clear` and unknown sources
    /// likewise deliberately do not advance — only a true fresh startup consumes the window.
    /// Non-startup serves still RENDER the (unconsumed) lines — honest, just not window-advancing.
    /// `source` is CLIENT-supplied, so the advance is reachable by any guest snapshot claiming
    /// `"startup"` — accepted: the digest is a cooperative-channel signal, not a tamper-evident alarm,
    /// and the signed append-only log is the backstop (design §0).
    pub async fn serve_conflict_digest_lines(&self, source: &str) -> Vec<String> {
        let onboarded = self.is_onboarded_local();
        let Ok(log) = self.get_or_open(onboarded).await else {
            return Vec::new();
        };
        let advance = source == "startup";
        spawn_blocking(move || {
            let pending = log.pending_conflict_proposals().map(|v| v.len()).unwrap_or(0);
            let since = log.conflict_digest_cursor().unwrap_or(0);
            let d = log.conflict_digest_counts(since).unwrap_or_default();
            // Advance the "since last session" window ONLY on a fresh startup (best-effort; a failed
            // advance just re-counts next time). A compact/resume serve renders the lines but leaves the
            // window open, so the activity is not consumed before the next real session start.
            if advance {
                let _ = log.set_conflict_digest_cursor(d.max_seq);
            }
            Self::build_digest_lines(pending, &d)
        })
        .await
        .unwrap_or_default()
    }

    /// The PURE tail of [`Self::serve_conflict_digest_lines`]: render the two §2.4 digest lines from
    /// the already-read counts. Split out so the exact `format!` bytes and the two non-zero gating
    /// branches are unit-testable without a brain (the snapshot tests use hand-typed look-alike
    /// strings; this is the real builder). A line is emitted only on a non-zero count: the pending
    /// line when `pending > 0`, the activity line when any of retired/dismissed/kept is non-zero.
    fn build_digest_lines(pending: usize, d: &bossclaw_core::ConflictDigest) -> Vec<String> {
        let mut lines = Vec::new();
        if pending > 0 {
            lines.push(format!("{pending} memory conflict(s) pending — ask me to review."));
        }
        if d.retired + d.dismissed + d.kept > 0 {
            lines.push(format!(
                "Since last session: {} retired, {} dismissed, {} kept-both via conflict resolution.",
                d.retired, d.dismissed, d.kept
            ));
        }
        lines
    }

    /// The PURE reflect digest line (spec §2.4). Renders ONLY when `n + m > 0` (an all-quiet reflect brain
    /// adds nothing). Deliberately NEUTRAL copy — the digest must not present an operational counter as
    /// proven benefit (critic New-Minor-1), and every unit must be TRUE under its label (critic M2/OQ4):
    /// `n` = REAL dossier emits (miss-pipeline `Emitted`s + stale-refresh heals — the `refreshed_total`
    /// counter; never `candidate_repaired`), so the copy carries no topic-attribution clause (the tidy's
    /// share is not miss-driven). `m` (no_material) is the owner's most actionable signal ("your memory
    /// never knew this"). Integer-only.
    fn build_reflect_digest_line(n: u64, m: u64) -> Option<String> {
        if n + m == 0 {
            return None;
        }
        Some(format!("{n} dossier(s) refreshed, {m} unknown-topic gap(s) since last session."))
    }

    /// SERVE the reflect digest line for the SessionStart snapshot preamble (§2.4). INFALLIBLE — empty Vec
    /// on any error / not onboarded / no new activity (I1). Integer counts only (no memory content → no
    /// sanitize). Reads cumulative counters vs the last-served totals; advances the last-served totals ONLY
    /// on `source == "startup"` (mirrors `serve_conflict_digest_lines` — a mid-session compact must not
    /// consume the "since last session" window). Non-startup serves render the (unconsumed) line honestly.
    pub async fn serve_reflect_digest_line(&self, source: &str) -> Vec<String> {
        let onboarded = self.is_onboarded_local();
        let Ok(log) = self.get_or_open(onboarded).await else {
            return Vec::new();
        };
        let advance = source == "startup";
        spawn_blocking(move || {
            let (refreshed_total, no_material_total) = log.reflect_counters().unwrap_or((0, 0));
            let cur = log.reflect_cursor().unwrap_or_default();
            let n = refreshed_total.saturating_sub(cur.last_served_refreshed.max(0) as u64);
            let m = no_material_total.saturating_sub(cur.last_served_no_material.max(0) as u64);
            if advance {
                let _ = log.set_reflect_last_served(refreshed_total as i64, no_material_total as i64);
            }
            Self::build_reflect_digest_line(n, m).into_iter().collect()
        })
        .await
        .unwrap_or_default()
    }

    /// The unprocessed-memory queue depth, defaulting to `0` on ANY error. A thin gate-and-
    /// default read the scheduler loop uses each tick (a `0` makes the tick a no-op, the safe
    /// default — never run a tick we can't size).
    pub async fn queue_depth_or_zero(&self, onboarded: bool) -> usize {
        match self.evolve_status(onboarded).await {
            Ok((status, _telemetry)) => status.queue_depth,
            Err(_) => 0,
        }
    }

    /// Flip the sticky engine evolve off-switch (the toggle behind the Memory tab). Gated.
    pub async fn set_evolve_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_evolve_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Flip the sticky engine proposals off-switch (Lock-1 enablement; turned on under the hood
    /// on first folder-enable). Gated.
    pub async fn set_proposals_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_proposals_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Flip the sticky engine mandates off-switch (SP5; gates the autonomous M6c proposer + the
    /// desktop auto-apply sweep). Off by default; an explicit choice persists across launches via
    /// `prime_switches`. Gated.
    pub async fn set_mandates_enabled(&self, onboarded: bool, enabled: bool) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.set_mandates_enabled(enabled).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Read the sticky mandates on/off flag (SF5 — the UI toggle's mount-time read, so it reflects
    /// the persisted state after relaunch rather than defaulting to OFF until clicked). Gated
    /// `Result` form (a not-onboarded state surfaces via `Open(NotOnboarded)`). The sweep uses the
    /// infallible `mandates_enabled_or_false` (Task 11) instead.
    pub async fn mandates_enabled(&self, onboarded: bool) -> Result<bool, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.mandates_enabled().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Grant a mandate (SP5). On success returns the new mandate's row. A grant-time guard
    /// failure (recipe too long, source scope not resolvable, target not write-granted, target under a read
    /// root) is surfaced as a TYPED `Rejected` error so the form can show *why*. Gated.
    pub async fn add_mandate(&self, onboarded: bool, target: PathBuf, source_scope: PathBuf, recipe: String) -> Result<MandateSummary, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            let id = log.add_mandate(&target, &source_scope, &recipe).map_err(|e| match e {
                // The engine's grant-time guards reject with InvalidInput — show the reason.
                bossclaw_core::BossclawError::InvalidInput(m) => EngineOpError::Rejected(m),
                other => EngineOpError::Core(other.to_string()),
            })?;
            // Re-read the just-granted mandate to return its full row (active_mandates is the
            // single source of truth; the id we just minted must be present).
            let mandate = log.active_mandates()
                .map_err(|e| EngineOpError::Core(e.to_string()))?
                .into_iter().find(|m| m.mandate_grant_id == id)
                .ok_or_else(|| EngineOpError::Core("granted mandate not found after add".to_string()))?;
            Ok(MandateSummary::from(mandate))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Revoke a mandate by its grant id (sticky; a revoke of an unknown id is a harmless no-op in
    /// the engine). Gated.
    pub async fn revoke_mandate(&self, onboarded: bool, mandate_grant_id: String) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            log.revoke_mandate(&mandate_grant_id).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Every ACTIVE mandate, oldest-first (the engine orders by `granted_at ASC`). Gated.
    pub async fn list_mandates(&self, onboarded: bool) -> Result<Vec<MandateSummary>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            let mandates = log.active_mandates().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(mandates.into_iter().map(MandateSummary::from).collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Every applied write attributed to a mandate (M6c), for the Mandate-activity list. Gated.
    pub async fn mandate_writes(&self, onboarded: bool) -> Result<Vec<MandateWriteSummary>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        spawn_blocking(move || {
            let writes = log.mandate_writes().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(writes.into_iter().map(MandateWriteSummary::from).collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Every current ingested file (one per path). Gated.
    pub async fn list_files(&self, onboarded: bool) -> Result<Vec<bossclaw_core::graph::FileRecord>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.current_files().map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Every open proposal, projected for the Review queue. Gated.
    pub async fn list_proposals(&self, onboarded: bool) -> Result<Vec<ProposalSummary>, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let pending = log.pending_proposals().map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(pending.into_iter().map(ProposalSummary::from_pending).collect())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Build the before/after preview for one open proposal. Fail-closed: an unknown id, a
    /// proposal whose bytes are missing/tampered (`get_proposal_bytes_checked`), or an
    /// unreadable target all return `Err`. Gated.
    pub async fn proposal_preview(&self, onboarded: bool, id: String) -> Result<PreviewData, EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let pending = log.pending_proposals().map_err(|e| EngineOpError::Core(e.to_string()))?;
            let p = pending.into_iter().find(|p| p.id == id)
                .ok_or_else(|| EngineOpError::Core("proposal not found or already resolved".to_string()))?;
            // new bytes — fail closed unless they hash to the signed proposal's recorded hash.
            let new_bytes = log.get_proposal_bytes_checked(&p.id, &p.new_content_hash)
                .map_err(|e| EngineOpError::Core(e.to_string()))?;
            // old bytes — the current on-disk file (local read; the target is canonical).
            let old_bytes = std::fs::read(&p.target)
                .map_err(|e| EngineOpError::Core(format!("could not read target: {e}")))?;
            let folder = std::path::Path::new(&p.target)
                .parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
            // Single-sourced fail-loud default (m2) — read into a local BEFORE the struct moves
            // `p`'s fields. A UI hint only; the authoritative gate is `apply_proposal` (Task 8).
            let requires_loud_modal = p.requires_loud_modal();
            let taint = p.verdict_summary.get("taint").and_then(|v| v.as_str()).unwrap_or("Untrusted").to_string();
            Ok(PreviewData {
                path: p.target,
                folder,
                rationale: p.rationale,
                op: p.op,
                old_text: String::from_utf8_lossy(&old_bytes).to_string(),
                new_text: String::from_utf8_lossy(&new_bytes).to_string(),
                requires_loud_modal,
                taint,
            })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Identity-reset teardown: delete both engine slots, drop the memoized log (reset the
    /// cell to `None`), then delete brain.db.
    pub async fn teardown(&self) -> Result<(), EngineError> {
        self.keystore.delete()?;
        *self.cell.lock().await = None;
        if self.db_path.exists() {
            std::fs::remove_file(&self.db_path).map_err(|e| EngineError::Vault(e.to_string()))?;
        }
        Ok(())
    }
}

/// The outcome of a successful apply: the audit `file_written` id (also the handle for Undo).
#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub file_written_id: String,
}

impl EngineHandle {
    /// Approve + apply one proposal (Lock 2). The anti-clobber check is the EXPLICIT base-hash
    /// compare: read the live target, sha256 it, and if the proposal's recorded
    /// `base_content_hash` differs → fail closed as `Stale` BEFORE proposing or executing (a fresh
    /// re-propose re-bases on live bytes and could not see the drift). Only then does it fetch the
    /// verified bytes, re-gate with a FRESH `propose_write` against the LIVE file + current
    /// write-grant (this still guards the micro-TOCTOU window + grant revocation). The loud-confirm
    /// is decided from the FRESH verdict (NOT the stale propose-time flag): if it is loud and
    /// `acknowledged_loud == false` the op REFUSES (`NeedsLoudConfirm`) — never a silent write.
    /// Only then does it execute (atomic temp+rename, durable undo, signed `file_written`). Nothing
    /// is written on any failure. Gated.
    /// (A Create has no base hash; its anti-clobber is the engine's atomic no-clobber create, so the
    /// base-hash arm is skipped for Create — the op-map runs first, then the base-hash arm gates on
    /// `op != Create`.)
    pub async fn apply_proposal(&self, onboarded: bool, id: String, acknowledged_loud: bool) -> Result<ApplyResult, EngineOpError> {
        use sha2::{Digest, Sha256};
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            let pending = log.pending_proposals().map_err(|e| EngineOpError::Core(e.to_string()))?;
            let p = pending.into_iter().find(|p| p.id == id)
                .ok_or_else(|| EngineOpError::Stale("proposal not found or already resolved".to_string()))?;

            // Map the proposal's OWN op back to a `WriteOp` FIRST (fail-closed on an unknown
            // string — NEVER default to Edit), because the base-hash anti-clobber below applies
            // only to Edit/Delete (a Create has no base).
            let op = match p.op.as_str() {
                "edit" => bossclaw_core::actuator::WriteOp::Edit,
                "create" => bossclaw_core::actuator::WriteOp::Create,
                "delete" => bossclaw_core::actuator::WriteOp::Delete,
                other => return Err(EngineOpError::Core(format!("unknown proposal op: {other}"))),
            };

            // ── ANTI-CLOBBER (Edit/Delete only): compare the live file to the proposal's
            // propose-time fingerprint. This is the TRUE staleness detector (a fresh propose_write
            // below re-bases on the live file and cannot detect that it changed). A CREATE has no
            // base (target absent at propose) — its anti-clobber is the engine's ATOMIC no-clobber
            // create at the syscall (RENAME_NOREPLACE on Linux; statat+renameat on macOS). We do
            // NOT add a desktop absence pre-check: it would be a strictly weaker TOCTOU check than
            // the engine's atomic no-clobber. So skip the base-hash arm entirely for a Create.
            if op != bossclaw_core::actuator::WriteOp::Create {
                let live_bytes = std::fs::read(&p.target)
                    .map_err(|e| EngineOpError::Stale(format!("could not read target: {e}")))?;
                let live_hash = hex::encode(Sha256::digest(&live_bytes));
                match &p.base_content_hash {
                    Some(base) if *base != live_hash => {
                        return Err(EngineOpError::Stale(format!(
                            "the file changed since this was suggested (base {base} != live {live_hash})"
                        )));
                    }
                    None => {
                        // No recorded base on an Edit/Delete (legacy/minimal) → cannot prove freshness.
                        return Err(EngineOpError::Stale("proposal has no base fingerprint to verify against".to_string()));
                    }
                    _ => {} // base matches live → proceed.
                }
            }
            // Verified bytes (fail closed if the side-table row is missing/tampered).
            let bytes = log.get_proposal_bytes_checked(&p.id, &p.new_content_hash)
                .map_err(|e| EngineOpError::Core(e.to_string()))?;
            // FRESH gate against the current disk + grant (never trust the stored verdict). Guards
            // the micro-TOCTOU window between the hash check above and the rename, + grant revoke.
            let gated = log.propose_write(bossclaw_core::actuator::WriteProposal {
                target: std::path::PathBuf::from(&p.target),
                new_content: bytes,
                op,
                source_event_ids: p.source_event_ids.clone(),
                rationale: p.rationale.clone(),
            }).map_err(|e| EngineOpError::Core(e.to_string()))?;
            // reject_reason set ⇒ symlink/op-mismatch/unresolvable; !allowed ⇒ grant revoked.
            if let Some(reason) = gated.verdict.reject_reason.as_deref() {
                return Err(EngineOpError::Stale(reason.to_string()));
            }
            if !gated.verdict.allowed {
                return Err(EngineOpError::Revoked("target not under an active write grant".to_string()));
            }
            // LOUD-CONFIRM on the FRESH verdict (G1/IMP-1a): a loud write requires the explicit ack.
            // This is the authoritative gate — the propose-time flag was only a UI hint.
            if gated.verdict.requires_loud_modal && !acknowledged_loud {
                return Err(EngineOpError::NeedsLoudConfirm(
                    "secret-/value-shaped or delete change — confirm review before applying".to_string(),
                ));
            }
            // execute is atomic temp+rename: it never partially writes, so a failure here also
            // leaves the file untouched. (Defensive: any execute error surfaces as Core.)
            // Thread the caller's ack to the ENGINE loud-gate (SP5 change d): this op already
            // refused above unless acked-or-not-loud, so the engine gate sees a consistent value
            // (defense-in-depth — the same check now lives in execute_write_inner for every caller).
            // Classify the engine's error: its defense-in-depth loud-gate in `execute_write_inner`
            // refuses a loud write → map that to `NeedsLoudConfirm` (not `Core`) so the auto-apply
            // sweep treats it as the benign "risky → leave queued" case and the unexpected-error log
            // channel stays meaningful. Defense in depth only: the line-828 fresh-verdict check above
            // normally fires first because BOTH it and the engine gate read the SAME fresh `gated`
            // verdict — so this path is unreachable today, only IF the two ever diverge in the future.
            let fw_id = log.execute_write_resolving(gated, &p.id, acknowledged_loud)
                .map_err(|e| execute_error_to_engine_op_error(e.to_string()))?;
            Ok(ApplyResult { file_written_id: fw_id })
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Undo a prior apply — re-gated, hash-verified restore of the pre-write bytes (LIFO per
    /// target); fails closed if the file diverged since. Gated.
    pub async fn undo_apply(&self, onboarded: bool, file_written_id: String) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.undo_write(&file_written_id).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// Decline a proposal — terminal `write_declined` (resolves it; the fix never returns). Gated.
    pub async fn decline_proposal(&self, onboarded: bool, id: String, reason: String) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        tokio::task::spawn_blocking(move || {
            log.decline_write_proposal(&id, &reason).map(|_| ()).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
    }

    /// The SP5 auto-apply sweep: after an evolve tick, auto-apply the CLEAN mandate (M6c)
    /// proposals and leave risky ones queued for SP4 Review. Lists pending proposals (oldest-first),
    /// filters to the M6c producer, caps at `MANDATE_AUTOAPPLY_PER_SWEEP`, and for each calls
    /// `apply_proposal(id, acknowledged_loud=false)`:
    ///   • CLEAN → applies (the engine loud-gate permits a non-loud write with ack=false);
    ///   • `NeedsLoudConfirm` (risky) → swallowed; stays open → surfaces in Review;
    ///   • `Stale` / `Revoked` / not-found → swallowed; skipped (retried next tick);
    ///   • any OTHER error → swallowed BUT logged (`eprintln!`) so one bad proposal cannot abort the
    ///     sweep yet the autonomous loop is never silent about an unexpected fault.
    /// Re-reads `mandates_enabled` PER ITEM (fast-kill if the user flips it off mid-sweep).
    /// Returns the number applied. Gated. NOTE: each `apply_proposal` re-folds `pending_proposals`
    /// internally, so a K-item sweep is 1+K O(events) folds — bounded by the cap (projection-table
    /// optimization is the future fix).
    pub async fn mandate_autoapply_sweep(&self, onboarded: bool) -> Result<usize, EngineOpError> {
        // 1. Snapshot the candidate ids (pure filter + cap over the producer-tagged pending list).
        let pending = self.list_proposals(onboarded).await?;
        let pairs: Vec<(String, String)> =
            pending.into_iter().map(|p| (p.id, p.producer)).collect();
        let candidates = crate::engine::scheduler::sweep_candidates(
            &pairs, crate::engine::scheduler::MANDATE_AUTOAPPLY_PER_SWEEP);

        // 2. Apply each, re-reading the kill-switch per item. Risky/stale/revoked are swallowed.
        let mut applied = 0usize;
        for id in candidates {
            // Fast-kill: stop the moment mandates are turned off mid-sweep.
            if !self.mandates_enabled_or_false(onboarded).await {
                break;
            }
            // Keep a copy of the id for an observability message (apply_proposal consumes it).
            let id_for_log = id.clone();
            match self.apply_proposal(onboarded, id, false).await {
                Ok(_) => applied += 1,
                // A loud (risky) proposal refuses without the ack → leave it queued for Review.
                Err(EngineOpError::NeedsLoudConfirm(_)) => {}
                // The file drifted / grant revoked / already resolved → skip; retried next tick.
                Err(EngineOpError::Stale(_)) | Err(EngineOpError::Revoked(_)) => {}
                // Any OTHER error (a transient/unexpected engine fault) is swallowed so one bad
                // proposal cannot abort the whole sweep — but it is LOGGED so the autonomous loop
                // is never silent about an anomaly (MF5 / security L1). Desktop has no log facade,
                // so eprintln! (matching the existing vault.rs convention).
                Err(e) => eprintln!("mandate sweep: proposal {id_for_log} apply failed unexpectedly (skipped): {e}"),
            }
        }
        Ok(applied)
    }

    /// `mandates_enabled`, defaulting to false on any error (the sweep's per-item kill-switch read).
    /// A fail-closed bare getter read via [`Self::flag_enabled_or_false`]; never panics the sweep.
    pub async fn mandates_enabled_or_false(&self, onboarded: bool) -> bool {
        self.flag_enabled_or_false(onboarded, EventLog::mandates_enabled).await
    }

    // ---- Cloud reasoner (Milestone D Phase 2a, spec R1/R5/R8) ----
    // Consumed by the Task 12b IPC commands (`engine_get/set/enable_*reasoner*`); the scheduler
    // mode read (Task 10) is a later consumer of `reasoner_config_or_default`.

    /// The persisted reasoner config, or the fail-SAFE Local default on ANY error (R8 — a
    /// missing/garbage/unreadable record NEVER flips the brain to cloud egress). Gated read.
    /// Consumed by `engine_get_reasoner_config` (+ Task 10 scheduler mode read).
    pub async fn reasoner_config_or_default(&self, onboarded: bool) -> reason::ReasonerConfig {
        let log = match self.get_or_open(onboarded).await {
            Ok(l) => l,
            Err(_) => return reason::ReasonerConfig::default(),
        };
        let raw = spawn_blocking(move || log.reasoner_config_json().ok().flatten())
            .await
            .unwrap_or(None);
        parse_reasoner_config(raw)
    }

    /// Fingerprint of the vault key the given config's provider WOULD use, or `None` when no
    /// non-empty key is stored. Binds the R1 consent to the exact key in the vault, so a
    /// rotation/provider-change makes readiness fail until re-consent. Sync (cached vault read).
    /// Consumed by `reasoner_ready_or_false` + the R5 enable flow below.
    fn current_key_fingerprint(&self, config: &reason::ReasonerConfig) -> Option<String> {
        let key_name = match config.provider {
            cloud_reasoner::CloudProvider::Anthropic => cloud_reasoner::ANTHROPIC_KEY_NAME,
            cloud_reasoner::CloudProvider::OpenAiCompat => cloud_reasoner::OPENAI_COMPAT_KEY_NAME,
            cloud_reasoner::CloudProvider::Gemini => cloud_reasoner::GEMINI_KEY_NAME,
        };
        match crate::vault::secret_get_cached(key_name) {
            Ok(Some(k)) if !k.trim().is_empty() => Some(key_fingerprint(&k)),
            _ => None,
        }
    }

    /// The CLOUD readiness gate, fail-closed to false on ANY error. Reads config + signed
    /// consent + the current vault key fp and defers to `reason::reasoner_ready`. NOTE: this
    /// passes `local_probe_ready = false`, so for a Local config it returns false — LOCAL
    /// readiness stays the scheduler's Ollama probe (Task 10); this method exists to answer
    /// "is the consented CLOUD provider ready right now?" (spec R1). Gated read.
    /// Consumed by `engine_get_reasoner_config` (the DTO's `ready` flag).
    pub async fn reasoner_ready_or_false(&self, onboarded: bool) -> bool {
        let log = match self.get_or_open(onboarded).await {
            Ok(l) => l,
            Err(_) => return false,
        };
        let (raw_config, consent) = spawn_blocking(move || {
            (
                log.reasoner_config_json().ok().flatten(),
                log.cloud_reasoner_consent_json().ok().flatten(),
            )
        })
        .await
        .unwrap_or((None, None));
        let config = parse_reasoner_config(raw_config);
        let fp = self.current_key_fingerprint(&config);
        reason::reasoner_ready(&config, consent.as_ref(), fp.as_deref(), false)
    }

    /// The shared cloud-egress consent barrier (spec I2). `true` when it is safe to run the
    /// reasoner: Local mode (no egress at all), or Cloud mode with a signed consent record matching
    /// the current config + vault key (`reasoner_ready_or_false`, fail-closed). Both `evolve_once`
    /// and the conflict sweep gate on this before building the reasoner.
    pub async fn cloud_consent_ok(&self, onboarded: bool) -> bool {
        if matches!(
            self.reasoner_config_or_default(onboarded).await.mode,
            crate::engine::reason::ReasonerMode::Cloud
        ) {
            self.reasoner_ready_or_false(onboarded).await
        } else {
            true
        }
    }

    /// Persist the NON-security reasoner config (mode/provider/model/base_url). Does NOT grant
    /// consent — flipping to cloud still requires `enable_cloud_reasoner`'s tested opt-in (R1).
    /// Gated + signed (mirrors `set_mandates_enabled`). Consumed by `engine_set_reasoner_config`.
    /// On success, ALSO refreshes the attached provider cell (see [`Self::refresh_reasoner_cell`])
    /// so the flip — including a Cloud→Local revocation — takes effect without a daemon restart.
    pub async fn set_reasoner_config(&self, onboarded: bool, config: serde_json::Value) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        let parsed = parse_reasoner_config(Some(config.clone()));
        spawn_blocking(move || {
            log.set_reasoner_config(config).map_err(|e| EngineOpError::Core(e.to_string()))
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        self.refresh_reasoner_cell(parsed);
        Ok(())
    }

    /// The R5 "test-key-on-enable" flow: prove the provider key works with ONE trivial probe
    /// (no memory/file content) BEFORE writing consent, then sign BOTH the config and the
    /// consent record (binding provider/host/key-fp). A probe failure (bad key, unreachable,
    /// missing key) returns a classified error and writes NOTHING — there is no path to enable
    /// cloud on a bad key (spec R5). Gated + signed. Consumed by `engine_enable_cloud_reasoner`.
    pub async fn enable_cloud_reasoner(&self, onboarded: bool, config: serde_json::Value) -> Result<(), EngineOpError> {
        // Gate first (mirrors the sibling switches): no probe / no write before onboarding.
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;

        let parsed = parse_reasoner_config(Some(config.clone()));
        // One-shot reasoner, byte-identical to what the scheduler would later build (R5). The
        // `probe_reasoner_for_test` override is ALWAYS `None` outside `cfg(test)` (no production
        // setter exists), so production takes the `build_reasoner` arm unconditionally.
        let reasoner = self
            .probe_reasoner_for_test
            .clone()
            .unwrap_or_else(|| reason::build_reasoner(&parsed));
        let schema = bossclaw_core::reason::adjudication_schema();
        // Trivial probe: a fixed prompt with NO memory/file bytes. With no key in the vault this
        // fails fast inside `read_key` BEFORE any network call (the Task 12b IPC-test path).
        let probe = spawn_blocking(move || {
            reasoner.complete_json("Reply with the JSON {\"match\":\"ok\"}.", "candidates: [ok]. text: ok", &schema)
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?;
        // Bad key / unreachable / parse failure -> classified error, DO NOT enable.
        probe.map_err(|e| EngineOpError::Core(e.to_string()))?;

        // Probe succeeded: build the consent record bound to THIS provider/host/key-fp, then
        // sign config + consent together. `config_host` is empty-on-unparseable (the readiness
        // check would then reject) and the fp mirrors `current_key_fingerprint`.
        let host = reason::config_host(&parsed).unwrap_or_default();
        let fp = self.current_key_fingerprint(&parsed).unwrap_or_default();
        // Reuse the consent READER's wire-string map (`reason::provider_str`) so the WRITER
        // can never drift from what `reasoner_ready` compares against (review I-1).
        let consent = serde_json::json!({
            "provider": reason::provider_str(parsed.provider),
            "base_url_host": host,
            "key_fingerprint": fp,
            "consented_at": chrono::Utc::now().to_rfc3339(),
        });
        spawn_blocking(move || -> Result<(), EngineOpError> {
            log.set_reasoner_config(config).map_err(|e| EngineOpError::Core(e.to_string()))?;
            log.set_cloud_reasoner_consent(consent).map_err(|e| EngineOpError::Core(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))??;
        // Probe + persist both succeeded → refresh the provider cell so the evolve loop egresses
        // to cloud from the NEXT tick (mirrors `set_reasoner_config`; never reached on failure).
        self.refresh_reasoner_cell(parsed);
        Ok(())
    }

    /// Write-through: copy `parsed` into the attached reasoner-config cell (no-op when detached).
    /// Called by the two config-writing ops AFTER a successful signed-log persist — never on a
    /// failed persist or probe, so the running provider can only ever read a config that is
    /// actually on file. Poison-recovered like every other cell access. This is the daemon-side
    /// replacement for the pre-M1a app-side write-through (the M1a Task 6 review fix: without it,
    /// a Cloud→Local flip kept the CLOUD reasoner in use until restart).
    fn refresh_reasoner_cell(&self, parsed: reason::ReasonerConfig) {
        if let Some(cell) = &self.reasoner_cell {
            *cell.lock().unwrap_or_else(|p| p.into_inner()) = parsed;
        }
    }
}

/// Map the stored reasoner-config JSON (`{mode, provider, model, base_url}`) to a typed
/// `ReasonerConfig`. Fail-SAFE per field (R8): `None`, a non-object, or any missing/garbage
/// field falls back to the Local default for that part — unknown/garbage NEVER flips to cloud.
/// `mode`: `"cloud"` → Cloud, anything else → Local. `provider`: `"openai-compat"` → OpenAiCompat,
/// anything else → Anthropic. Consumed by the engine reasoner reads above + their tests.
pub(crate) fn parse_reasoner_config(raw: Option<serde_json::Value>) -> reason::ReasonerConfig {
    use reason::{ReasonerConfig, ReasonerMode};
    let default = ReasonerConfig::default();
    let Some(obj) = raw.as_ref().and_then(|v| v.as_object()) else {
        return default;
    };
    let mode = match obj.get("mode").and_then(|v| v.as_str()) {
        Some("cloud") => ReasonerMode::Cloud,
        _ => ReasonerMode::Local,
    };
    let provider = match obj.get("provider").and_then(|v| v.as_str()) {
        Some("openai-compat") => cloud_reasoner::CloudProvider::OpenAiCompat,
        Some("gemini") => cloud_reasoner::CloudProvider::Gemini,
        _ => cloud_reasoner::CloudProvider::Anthropic,
    };
    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or(default.model);
    // A JSON `null` (or absent / non-string) base_url stays `None`.
    let base_url = obj.get("base_url").and_then(|v| v.as_str()).map(|s| s.to_string());
    ReasonerConfig { mode, provider, model, base_url }
}

/// Phase 2b boot reseed: copy the persisted (signed-log) reasoner config into the
/// in-memory cell the provider closure reads each tick, so a Cloud choice survives
/// restart. `async` because it reads the engine's signed log; `main.rs` `block_on`s it.
/// Fail-safe: a read with no signed config (or `onboarded=false`) yields the Local default.
// `pub` (was `pub(crate)` in the app): the daemon's `main.rs` is a SEPARATE crate consuming this
// lib, so the boot reseed must be reachable across the crate boundary.
#[cfg(unix)]
pub async fn reseed_reasoner_cell(
    engine: &EngineHandle,
    cell: &std::sync::Mutex<reason::ReasonerConfig>,
    onboarded: bool,
) {
    let seeded = engine.reasoner_config_or_default(onboarded).await;
    *cell.lock().unwrap_or_else(|p| p.into_inner()) = seeded;
}

/// An 8-hex-char fingerprint of a provider key: the first 4 bytes of its SHA-256, hex-encoded.
/// The R1 signed consent binds this so a key rotation (different fp) forces re-consent. The
/// key itself is never stored or logged — only this digest. Consumed by the engine reads above.
pub(crate) fn key_fingerprint(key: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(key.as_bytes())[..4])
}

/// Pure tick recorder. The lock is poison-RECOVERED (a panicked tick must not wedge the status
/// read path), `last_tick_ms` is always set, and on `Err` `error_count` is bumped and
/// `last_error` stored TRUNCATED to ~512 bytes — engine error strings can embed paths /
/// reasoner output and flow to the webview DTO, so the cap is a security-relevant bound (the
/// Group A review flagged it). It ALSO synthesizes a `last_error` when a CLOUD tick returns
/// Ok-but-processed-zero while the queue had work — a bad or expired key otherwise no-ops
/// silently every tick (spec R4). A local 0-item tick, or any tick over an empty queue, is
/// normal idle and records no synthetic error.
fn record_tick_into(
    tel: &std::sync::Mutex<EvolveTelemetry>,
    ms: u128,
    result: &Result<bossclaw_core::EvolveReport, EngineOpError>,
    cloud_mode: bool,
    queue_depth: usize,
) {
    let mut tel = tel.lock().unwrap_or_else(|p| p.into_inner());
    tel.last_tick_ms = Some(ms);
    // Egress transparency (spec R4): tie the disclosed count to actual cloud egress — a cloud
    // tick's report carries the file-derived snippet count it sent; a local tick never egressed,
    // so its (harmlessly computed) count is NOT surfaced to the banner.
    if cloud_mode {
        if let Ok(report) = result {
            tel.last_tainted_snippets = Some(report.tainted_recall_snippets);
        }
    }
    match result {
        Err(e) => {
            tel.error_count += 1;
            let mut s = e.to_string();
            truncate_on_char_boundary(&mut s, 512);
            tel.last_error = Some(s);
        }
        Ok(report) if cloud_mode && report.memories_processed == 0 && queue_depth > 0 => {
            tel.error_count += 1;
            tel.last_error = Some(
                "cloud reasoner processed 0 of a non-empty queue (check the provider key/endpoint)"
                    .to_string(),
            );
        }
        Ok(_) => {}
    }
}

/// Pure reflect-tick recorder. The lock is poison-RECOVERED (a panicked tick must not wedge the status
/// read); `last_tick_ms` is always set; on `Err` `error_count` bumps and `last_error` is stored TRUNCATED
/// to ~512 bytes; on `Ok` the report's counters fold into the session totals (the §2.4 scoreboard, incl.
/// the transient/reasoner split kept APART so a write hiccup is never blamed on the model).
fn record_reflect_tick_into(
    tel: &std::sync::Mutex<ReflectTelemetry>,
    ms: u128,
    result: &Result<bossclaw_core::ReflectReport, EngineOpError>,
) {
    let mut tel = tel.lock().unwrap_or_else(|p| p.into_inner());
    tel.last_tick_ms = Some(ms);
    match result {
        Err(e) => {
            tel.error_count += 1;
            let mut s = e.to_string();
            truncate_on_char_boundary(&mut s, 512);
            tel.last_error = Some(s);
        }
        Ok(r) => {
            tel.dossiers_refreshed_total += r.dossiers_refreshed;
            tel.no_material_total += r.no_material;
            tel.parked_total += r.parked;
            tel.unhealable_thin_total += r.unhealable_thin;
            tel.reasoner_errors_total += r.reasoner_errors;
            tel.transient_errors_total += r.transient_errors;
        }
    }
}

/// Truncate `s` in place to at most `max` bytes WITHOUT splitting a UTF-8 char (plain
/// `String::truncate` panics on a non-char-boundary). Walks back to the nearest boundary at
/// or below `max`. Used to cap `last_error` before it flows to the webview DTO.
fn truncate_on_char_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

fn map_err_state(e: &EngineError) -> EngineState {
    // Exhaustive on purpose (no `_` wildcard): a new EngineError variant must force a
    // deliberate state mapping here at compile time rather than silently defaulting.
    match e {
        EngineError::NotOnboarded => EngineState::NotOnboarded,
        EngineError::KeystoreInconsistent => EngineState::KeystoreInconsistent,
        EngineError::KeystoreDbMismatch(_) | EngineError::Vault(_) | EngineError::Join(_) => {
            EngineState::KeystoreDbMismatch
        }
    }
}

/// Current time as an RFC3339 string — the audit stamp for a signed language-pack consent record.
/// Reuses the same `chrono` timestamp source the cloud-consent writer (`enable_cloud_reasoner`)
/// uses, so every signed consent record the engine writes is stamped identically.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Rung-2 language-pack activation: the consent-gated, crash-safe migration that swaps the served
/// embedder (U4, I5, I6). These methods take `self: &Arc<Self>` where they hand an owned `Arc` to a
/// background task; the daemon already shares the engine as an `Arc`.
impl EngineHandle {
    /// Enable the multilingual language pack (rung 2; consent-gated — I6). Writes the signed
    /// `InProgress` record (the ONLY authority that starts a GC-bearing migration), then spawns the
    /// crash-safe migration in the background and returns immediately (the UI polls `model_state`).
    /// A folder/sha problem is surfaced synchronously (nothing is written) so the UI shows it at once.
    pub async fn set_active_model(
        self: &Arc<Self>,
        onboarded: bool,
        model_id: String,
        safetensors_sha: String,
    ) -> Result<(), EngineOpError> {
        let log = self.get_or_open(onboarded).await.map_err(EngineOpError::Open)?;
        // Idempotent enable (defense-in-depth against a redundant re-enable of the already-active
        // model): if the requested model is the current, HEALTHY (`Ok`) `Complete` record, a re-enable
        // would pointlessly re-migrate (and, at the command layer, re-download ~530 MB) the SAME model
        // — return without touching the record or spawning a migration. A `Missing`/`Mismatch`/`Failed`
        // state is deliberately EXCLUDED so it still falls through to `build_candidate`'s fail-loud
        // verification below (the short-circuit never weakens the integrity guard). Consent is
        // untouched: the existing signed `Complete` record already carries it.
        let healthy = matches!(self.embedder_provider.model_state(), crate::engine::embed::ModelState::Ok);
        if healthy {
            if let Some(r) = log.language_pack_record().map_err(|e| EngineOpError::Core(e.to_string()))? {
                if r.migration == bossclaw_core::MigrationState::Complete && r.model_id == model_id {
                    return Ok(());
                }
            }
        }
        // Fail fast if the downloaded folder isn't loadable/verifiable (never write a record we can't
        // honour). Built off to the side — the live embedder is untouched until the migration commits.
        let _candidate = self.embedder_provider.build_candidate(&model_id, &safetensors_sha)?;
        // Record consent + the in-progress marker (signed). This is what authorizes the migration (I6).
        let record = bossclaw_core::LanguagePackRecord {
            model_id: model_id.clone(),
            safetensors_sha: safetensors_sha.clone(),
            migration: bossclaw_core::MigrationState::InProgress,
            consented_at: now_rfc3339(),
        };
        let log2 = log.clone();
        spawn_blocking(move || log2.set_language_pack_record(&record))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
            .map_err(|e| EngineOpError::Core(e.to_string()))?;
        // Run the migration in the background (see `run_language_migration`). Errors are surfaced via
        // `model_state`; the record stays InProgress on failure (retryable).
        self.spawn_migration(model_id, safetensors_sha);
        Ok(())
    }

    /// Spawn the background migration task. Extracted so both the enable path and boot-resume use it.
    fn spawn_migration(self: &Arc<Self>, model_id: String, sha: String) {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.run_language_migration(model_id, sha).await {
                eprintln!("bossclawd: language migration failed (old model still active): {e}");
                // Report the failure to the UI (the OLD model still serves; the record stays
                // InProgress = retryable) and clear the now-stale progress bar. Only the reported
                // state changes — the signed record is left untouched so a retry/boot can resume.
                this.embedder_provider.set_failed(e.to_string());
                this.embedder_provider.set_reindex(None);
            }
        });
    }

    /// The crash-safe, all-or-nothing migration body (invariant I5). Prepare (re-embed new vectors +
    /// entity vectors, count-checked) → flip the signed record to `Complete` (the commit point) →
    /// **publish** the new embedder (the atomic swap the running daemon serves from) → GC the old
    /// rows. On any failure BEFORE the flip: nothing is GC'd, the record stays InProgress, the old
    /// model keeps serving (retryable). Calling `publish` (not merely writing the record) is
    /// load-bearing: `embedder_for` caches resolve-once, so on a live daemon ONLY `publish` swaps
    /// the served model — a bare record write would leave the old embedder serving until restart.
    async fn run_language_migration(&self, model_id: String, sha: String) -> Result<(), EngineOpError> {
        let log = self.get_or_open(true).await.map_err(EngineOpError::Open)?;
        let candidate = self.embedder_provider.build_candidate(&model_id, &sha)?;

        // Stage 1: re-embed under the new id (progress-reporting). No GC yet — old vectors intact.
        let (log1, cand1, prov1) = (log.clone(), candidate.clone(), self.embedder_provider.clone());
        spawn_blocking(move || {
            let mut on = |done: u64, total: u64| prov1.set_reindex(Some((done, total)));
            log1.reembed_prepare(&*cand1, &mut on)
        })
        .await
        .map_err(|e| EngineOpError::Join(e.to_string()))?
        .map_err(|e| EngineOpError::Core(e.to_string()))?;

        // Commit point: flip the signed record to Complete, THEN publish the new embedder so the
        // live daemon serves it (a bare record write would not swap the resolve-once cache).
        let done_record = bossclaw_core::LanguagePackRecord {
            model_id: model_id.clone(),
            safetensors_sha: sha,
            migration: bossclaw_core::MigrationState::Complete,
            consented_at: now_rfc3339(),
        };
        let log2 = log.clone();
        spawn_blocking(move || log2.set_language_pack_record(&done_record))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
            .map_err(|e| EngineOpError::Core(e.to_string()))?;
        // Swap the served embedder AND invalidate the recall index in the SAME step (never after GC —
        // see `publish_and_invalidate`'s doc for why the ordering matters).
        self.publish_and_invalidate(candidate.clone()).await;

        // Stage 2: GC the old vectors + entity vectors, rebuild indexes under the new model. A
        // failure here leaves stale old-model rows (harmless — `resume_migration_if_pending` sweeps
        // them on the next boot) but the index is ALREADY invalidated above, so a racing recall still
        // rebuilds correctly against the new model regardless of whether/when this GC succeeds.
        let (log3, cand3) = (log.clone(), candidate);
        spawn_blocking(move || log3.reembed_finalize_gc(&*cand3))
            .await
            .map_err(|e| EngineOpError::Join(e.to_string()))?
            .map_err(|e| EngineOpError::Core(e.to_string()))?;

        self.embedder_provider.set_reindex(None);
        Ok(())
    }

    /// Atomically swap the live embedder cache to `candidate` AND invalidate the recall index, so no
    /// racing `recall`/`evolve_once` can ever observe the NEW embedder (served from the provider's
    /// cache the instant `publish` runs) paired with the OLD in-memory vector index (built under the
    /// old model). New-id vectors already exist at this point — `reembed_prepare` wrote them BEFORE
    /// the commit point — so a rebuild triggered by the invalidation is correct even before GC runs;
    /// the two calls must never be separated (a prior version invalidated only after GC, leaving a
    /// window where a racing recall embedded the query with the NEW model but searched the OLD
    /// in-memory index — cross-embedding-space garbage results for the whole GC+rebuild duration, and
    /// permanently if `reembed_finalize_gc` then failed).
    async fn publish_and_invalidate(&self, candidate: Arc<dyn Embedder>) {
        self.embedder_provider.publish(candidate);
        *self.indexed.lock().await = false;
    }

    /// Boot-time resume (invariant I6): if a consented `InProgress` migration is recorded, finish it;
    /// if `Complete`, GC any stale rows a crash left behind (idempotent); if absent, do nothing. This
    /// is the ONLY boot-time migration — there is NO un-consented "zero vectors" heuristic, so a
    /// fresh brain with no signed record never auto-migrates.
    pub async fn resume_migration_if_pending(self: &Arc<Self>, onboarded: bool) {
        let log = match self.get_or_open(onboarded).await {
            Ok(l) => l,
            Err(_) => return, // not onboarded / open failure — nothing to resume
        };
        let rec = match log.language_pack_record() {
            Ok(Some(r)) => r,
            Ok(None) => return, // no consent recorded — nothing to resume (I6)
            Err(e) => {
                // A real DB/deserialize failure is NOT "nothing to resume" — a corrupt record must
                // never silently skip a needed resume, so it is logged (never surfaced as a fault:
                // this runs unattended at boot, same discipline as the panic-scrubbing hook).
                eprintln!("bossclawd: boot-resume could not read the language-pack record: {e}");
                return;
            }
        };
        match rec.migration {
            bossclaw_core::MigrationState::InProgress => {
                self.spawn_migration(rec.model_id, rec.safetensors_sha);
            }
            bossclaw_core::MigrationState::Complete => {
                // A crash between the flip and the GC can leave stale old-model rows; sweep them.
                let keep = rec.model_id.clone();
                match spawn_blocking(move || log.gc_stale_vectors(&keep)).await {
                    Ok(Ok(_removed)) => {}
                    Ok(Err(e)) => eprintln!("bossclawd: boot-resume GC of stale vectors failed: {e}"),
                    Err(e) => eprintln!("bossclawd: boot-resume GC task failed to join: {e}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::embed;
    use crate::secrets::SecretsVault;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct TestVault { store: Mutex<HashMap<String, String>> }
    impl TestVault { fn new() -> Arc<Self> { Arc::new(Self { store: Mutex::new(HashMap::new()) }) } }
    impl SecretsVault for TestVault {
        fn set(&self, k: &str, v: &str) -> Result<(), String> { self.store.lock().unwrap().insert(k.into(), v.into()); Ok(()) }
        fn get(&self, k: &str) -> Result<Option<String>, String> { Ok(self.store.lock().unwrap().get(k).cloned()) }
        fn delete(&self, k: &str) -> Result<(), String> { self.store.lock().unwrap().remove(k); Ok(()) }
    }

    /// A fresh `TestVault` + tempdir (the dir guard must outlive the handle). Mirrors the
    /// inline setup the SP2 tests use; centralised here for the SP3 recall/evolve tests.
    ///
    /// ALSO seeds the process-global provider-key cache to EMPTY: any test that reaches a
    /// provider-key fingerprint read (`reasoner_ready_or_false` → `current_key_fingerprint` →
    /// `vault::secret_get_cached`, e.g. a cloud-mode `evolve_once`) would otherwise hit the real
    /// OS keychain and block forever on a keychain-ACL prompt (the documented CI-hang hazard).
    /// An empty cache means "no provider key stored" — exactly the fixture these tests want.
    fn test_vault_and_dir() -> (Arc<TestVault>, tempfile::TempDir) {
        crate::vault::seed_secret_cache_for_test(Default::default());
        (TestVault::new(), tempfile::tempdir().unwrap())
    }

    /// A handle wired with the mock embedder + mock reasoner (the common SP3 test shape).
    fn new_test_handle(vault: Arc<TestVault>, dir: &tempfile::TempDir) -> EngineHandle {
        EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(8)),
            Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        )
    }

    /// Task 14 review: the REAL digest line-builder's four gating-branch combinations, byte-exact.
    /// The snapshot preamble tests exercise hand-typed look-alike strings; this pins the actual
    /// `format!` output and both non-zero gates, so a format typo or a dropped branch cannot pass.
    #[test]
    fn build_digest_lines_four_branch_combinations_byte_exact() {
        use bossclaw_core::ConflictDigest;
        let zeros = ConflictDigest::default();
        let activity = ConflictDigest { retired: 2, dismissed: 1, kept: 0, max_seq: 99 };

        // (0, zeros) → no lines at all (an all-quiet brain adds nothing to the preamble).
        assert!(EngineHandle::build_digest_lines(0, &zeros).is_empty());

        // (n>0, zeros) → only the pending line, exact bytes.
        assert_eq!(
            EngineHandle::build_digest_lines(3, &zeros),
            vec!["3 memory conflict(s) pending — ask me to review."]
        );

        // (0, nonzero) → only the activity line, exact bytes.
        assert_eq!(
            EngineHandle::build_digest_lines(0, &activity),
            vec!["Since last session: 2 retired, 1 dismissed, 0 kept-both via conflict resolution."]
        );

        // (n>0, nonzero) → both lines, pending first, exact bytes.
        assert_eq!(
            EngineHandle::build_digest_lines(3, &activity),
            vec![
                "3 memory conflict(s) pending — ask me to review.",
                "Since last session: 2 retired, 1 dismissed, 0 kept-both via conflict resolution.",
            ]
        );

        // The activity gate is the SUM of the three counts — a kept-only window still emits.
        let kept_only = ConflictDigest { retired: 0, dismissed: 0, kept: 4, max_seq: 1 };
        assert_eq!(
            EngineHandle::build_digest_lines(0, &kept_only),
            vec!["Since last session: 0 retired, 0 dismissed, 4 kept-both via conflict resolution."]
        );
    }

    /// Task 13: the REAL reflect digest line-builder (§2.4), byte-exact + gated on `n + m > 0`. Mirrors
    /// the conflict builder's byte-exact test — pins the exact `format!` output and the non-zero gate so a
    /// format typo or a dropped branch cannot pass.
    #[test]
    fn build_reflect_digest_line_is_byte_exact_and_gated_on_nonzero() {
        // Nothing new since last session → no line (an all-quiet reflect brain adds nothing).
        assert_eq!(EngineHandle::build_reflect_digest_line(0, 0), None);
        // Neutral copy, integer-only, pluralized with a bare `(s)` (matches the conflict-line style).
        // Critic M2 lock: NO topic-attribution clause — `n` counts real dossier emits from BOTH the miss
        // pipeline AND the stale-refresh tidy, so "for recently-missed topics" would be false for the
        // tidy's share. Every unit is now true under its label.
        assert_eq!(
            EngineHandle::build_reflect_digest_line(2, 3),
            Some("2 dossier(s) refreshed, 3 unknown-topic gap(s) since last session.".to_string()),
        );
        // Either non-zero alone still renders (both counts always shown for honesty).
        assert_eq!(
            EngineHandle::build_reflect_digest_line(0, 1),
            Some("0 dossier(s) refreshed, 1 unknown-topic gap(s) since last session.".to_string()),
        );
    }

    #[test]
    fn mock_embedder_provider_yields_a_working_embedder() {
        use crate::engine::embed::{EmbedderProvider, MockEmbedderProvider};
        let p = MockEmbedderProvider::new(8);
        let e = p.embedder().expect("mock embedder builds");
        let v = e.embed(&["hello".to_string()]).unwrap();
        assert_eq!(v[0].len(), 8);
        assert_eq!(e.model_id(), "mock-v1");
    }

    #[test]
    fn parse_reasoner_config_defaults_local_on_garbage() {
        use crate::engine::reason::ReasonerMode;
        assert!(matches!(parse_reasoner_config(None).mode, ReasonerMode::Local));
        assert!(matches!(parse_reasoner_config(Some(serde_json::json!("not-an-object"))).mode, ReasonerMode::Local));
        let c = parse_reasoner_config(Some(serde_json::json!({"mode":"cloud","provider":"anthropic","model":"claude-sonnet-4-6","base_url":null})));
        assert!(matches!(c.mode, ReasonerMode::Cloud));
        assert_eq!(c.model, "claude-sonnet-4-6");
    }

    #[test]
    fn key_fingerprint_is_stable_8_hex() {
        let fp = key_fingerprint("sk-test-abc");
        assert_eq!(fp.len(), 8);
        assert_eq!(fp, key_fingerprint("sk-test-abc")); // deterministic
        assert_ne!(fp, key_fingerprint("sk-test-xyz")); // different key -> different fp
    }

    /// The engine's defense-in-depth loud-gate (`execute_write_inner`) refuses a loud write with a
    /// known phrase; the classifier maps that to `NeedsLoudConfirm` so the sweep treats it as a
    /// clean skip, and any other engine error to `Core`. This is a pure classifier test — a full
    /// end-to-end test through `apply_proposal` is NOT possible because that path is unreachable
    /// (its line-828 fresh-verdict check fires first, from the same verdict the engine gate reads).
    #[test]
    fn execute_error_to_engine_op_error_maps_loud_reject_else_core() {
        // The engine wraps the loud-reject as `InvalidInput("execute_write fail-closed: <MSG>
        // (refused fail-closed)")`. Build the sample FROM the shared const so this test can't drift
        // from the real phrase either.
        let loud = execute_error_to_engine_op_error(format!(
            "execute_write fail-closed: {} (refused fail-closed)",
            bossclaw_core::LOUD_ACK_REQUIRED_MSG,
        ));
        assert!(matches!(loud, EngineOpError::NeedsLoudConfirm(_)), "loud-reject phrase ⇒ NeedsLoudConfirm");

        let other = execute_error_to_engine_op_error("some unrelated engine failure".into());
        assert!(matches!(other, EngineOpError::Core(_)), "any other message ⇒ Core");
    }

    #[tokio::test]
    async fn not_onboarded_does_not_open_or_mint() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(vault.clone(), dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        let st = h.status(false).await;
        assert!(matches!(st.state, EngineState::NotOnboarded));
        // No keys minted, no DB created.
        assert!(vault.get("air-agent.engine.dek").unwrap().is_none());
        assert!(!dir.path().join("brain.db").exists());
    }

    #[tokio::test]
    async fn onboarded_opens_fresh_brain_and_memoizes() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(vault, dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        let st = h.status(true).await;
        assert!(matches!(st.state, EngineState::Ready), "state was {:?}", st.state);
        // First open primes the autonomy switches OFF (SP3 `prime_switches`): the three original
        // flags (evolve/proposals/mandates), the SP3 capture force-off, the Rung-3 Phase-2
        // conflict-detect force-off, and the Rung-4 R4-A reflect force-off (design §4 I3) — so a fresh
        // brain holds exactly those 6 sticky `config` events, not zero.
        assert_eq!(st.event_count, 6, "prime_switches wrote the 6 sticky config events");
        assert!(st.chain_ok);
        // I3 dormancy: a fresh brain has reflect forced explicitly OFF, so the sweeper gate is closed.
        assert!(!h.reflect_enabled_or_false(true).await, "fresh brain: reflect is forced off");
        // Second call reuses the same instance (Arc ptr identical).
        let a = h.get_or_open(true).await.unwrap();
        let b = h.get_or_open(true).await.unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn wrong_dek_reports_keystore_db_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        // Open once to create the DB under the real minted DEK.
        let h1 = EngineHandle::new(vault.clone(), dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        h1.get_or_open(true).await.unwrap();
        // Now corrupt the stored DEK and open with a FRESH handle (empty cell).
        vault.set("air-agent.engine.dek", &hex::encode([0u8; 32])).unwrap();
        let h2 = EngineHandle::new(vault, dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        let st = h2.status(true).await;
        assert!(matches!(st.state, EngineState::KeystoreDbMismatch));
    }

    #[tokio::test]
    async fn teardown_removes_keys_db_and_resets_cell() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(vault.clone(), dir.path().to_path_buf(), std::sync::Arc::new(embed::MockEmbedderProvider::new(8)), std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")));
        h.get_or_open(true).await.unwrap();
        assert!(dir.path().join("brain.db").exists());
        h.teardown().await.unwrap();
        assert!(vault.get("air-agent.engine.signing_key").unwrap().is_none());
        assert!(!dir.path().join("brain.db").exists());
        // Cell is empty again: a not-onboarded call after teardown stays NotOnboarded.
        assert!(matches!(h.status(false).await.state, EngineState::NotOnboarded));
    }

    #[tokio::test]
    async fn grant_then_list_then_revoke() {
        let app_dir = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let h = EngineHandle::new(
            vault, app_dir.path().to_path_buf(),
            std::sync::Arc::new(embed::MockEmbedderProvider::new(8)),
            std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        );

        h.add_grant(true, src_dir.path().to_path_buf()).await.unwrap();
        let grants = h.list_grants(true).await.unwrap();
        assert_eq!(grants.len(), 1);
        assert!(!grants[0].revoked);

        h.revoke_grant(true, src_dir.path().to_path_buf()).await.unwrap();
        let grants = h.list_grants(true).await.unwrap();
        assert_eq!(grants.len(), 1);
        assert!(grants[0].revoked);

        // Gate: not-onboarded refuses.
        assert!(matches!(h.add_grant(false, src_dir.path().to_path_buf()).await, Err(EngineOpError::Open(EngineError::NotOnboarded))));
    }

    #[tokio::test]
    async fn ingest_indexes_files_and_records_model() {
        use std::fs;
        let app_dir = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        fs::write(src_dir.path().join("a.txt"), "the quick brown fox").unwrap();
        fs::write(src_dir.path().join("b.md"), "# notes\nhello world").unwrap();

        let vault = TestVault::new();
        let h = EngineHandle::new(
            vault, app_dir.path().to_path_buf(),
            std::sync::Arc::new(embed::MockEmbedderProvider::new(8)),
            std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        );
        h.add_grant(true, src_dir.path().to_path_buf()).await.unwrap();

        let report = h.run_ingest(true).await.unwrap();
        assert_eq!(report.ingested, 2);
        assert_eq!(report.failed.len(), 0);

        let files = h.list_files(true).await.unwrap();
        assert_eq!(files.len(), 2);

        // Re-ingest is a no-op: everything deduped, model already recorded.
        let again = h.run_ingest(true).await.unwrap();
        assert_eq!(again.ingested, 0);
        assert_eq!(again.deduped, 2);
    }

    // ---- SP3: recall core (Tasks 3/4/5/6) ----

    /// Task 3: the handle constructs with BOTH providers, and the recall gate is intact
    /// (not onboarded → `Open(NotOnboarded)`), proving the new field is wired without bypass.
    #[tokio::test]
    async fn handle_constructs_with_both_providers() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let err = handle.recall(false, "q".into(), 3).await.unwrap_err();
        assert!(matches!(err, EngineOpError::Open(EngineError::NotOnboarded)));
    }

    /// Task 4: the first open forces all THREE autonomy switches off (the engine defaults
    /// them on), and priming is idempotent across re-opens (no duplicate config events).
    #[tokio::test]
    async fn first_open_forces_all_autonomy_switches_off() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.expect("opens");
        assert!(!log.evolve_enabled().unwrap(), "evolve off");
        assert!(!log.proposals_enabled().unwrap(), "proposals off");
        assert!(!log.mandates_enabled().unwrap(), "mandates off");
        // Idempotent: a second open (forced via clearing the cell) writes no new config events.
        let n1 = log.count().unwrap();
        drop(log);
        *handle.cell.lock().await = None;
        let log2 = handle.get_or_open(true).await.expect("re-opens");
        assert_eq!(log2.count().unwrap(), n1, "no duplicate config events on re-open");
    }

    #[tokio::test]
    async fn prime_switches_preserves_explicit_proposals_and_mandates() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault.clone(), &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // After first open everything is forced off (never-set defaults).
        assert!(!log.proposals_enabled().unwrap());
        assert!(!log.mandates_enabled().unwrap());

        // The user explicitly enables BOTH proposals and mandates.
        log.set_proposals_enabled(true).unwrap();
        log.set_mandates_enabled(true).unwrap();
        assert!(log.proposals_enabled().unwrap());
        assert!(log.mandates_enabled().unwrap());
        drop(log);

        // Re-open with a FRESH handle (same vault + db_path) → prime_switches runs again.
        let handle2 = new_test_handle(vault, &dir);
        let log2 = handle2.get_or_open(true).await.unwrap();
        assert!(log2.proposals_enabled().unwrap(), "an explicit proposals true MUST persist across opens");
        assert!(log2.mandates_enabled().unwrap(), "an explicit mandates true MUST persist across opens (SP5)");
    }

    /// SP3 A8 §6a (critic Critical C1): a fresh engine reports capture OFF, and the boot cascade
    /// persists an EXPLICIT OFF (`explicitly_set` true) so the getter-default can never later flip
    /// it on. An explicit user ON must then survive a re-open (never clobbered by priming).
    #[tokio::test]
    async fn first_open_forces_capture_off_and_persists_explicit_off() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault.clone(), &dir);
        let log = handle.get_or_open(true).await.expect("opens");

        // Default CLOSED at the getter AND persisted as an explicit OFF at boot.
        assert!(!log.capture_enabled().unwrap(), "capture off on a fresh brain (I10)");
        assert!(!log.backfill_consented().unwrap(), "backfill un-consented on a fresh brain");
        assert!(
            log.explicitly_set(bossclaw_core::ConfigFlag::CaptureEnabled).unwrap(),
            "boot force-off persists an EXPLICIT OFF (the getter-default can't later flip it on)"
        );

        // Idempotent: a second open (cell cleared) writes no new config events.
        let n1 = log.count().unwrap();
        drop(log);
        *handle.cell.lock().await = None;
        let log2 = handle.get_or_open(true).await.expect("re-opens");
        assert_eq!(log2.count().unwrap(), n1, "capture priming is idempotent (no duplicate off event)");

        // The user explicitly enables capture; a fresh handle's prime must NOT clobber it.
        log2.set_capture_enabled(/*enabled=*/ true, /*backfill=*/ false, /*at=*/ 5_000).unwrap();
        drop(log2);
        let handle2 = new_test_handle(vault, &dir);
        let log3 = handle2.get_or_open(true).await.unwrap();
        assert!(log3.capture_enabled().unwrap(), "an explicit capture ON MUST persist across opens");
    }

    /// Rung-3 Phase-2 (§3.6, I3): a fresh brain reports conflict-detect OFF through the infallible
    /// daemon read, and the boot cascade persists an EXPLICIT OFF (`explicitly_set` true) so the
    /// getter-default can never later flip it on.
    #[tokio::test]
    async fn prime_switches_forces_conflict_detect_off_and_or_false_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new(); // TestVault::new() already returns Arc<TestVault>
        let h = EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            std::sync::Arc::new(embed::MockEmbedderProvider::new(8)),
            std::sync::Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        );
        let onboarded = true;
        // Fresh brain: prime_switches (run inside get_or_open's first-open) forced an explicit OFF,
        // and the infallible daemon read reports false.
        assert!(!h.conflict_detect_enabled_or_false(onboarded).await, "off by default after boot");
        // Prove prime_switches wrote the tamper-evident EXPLICIT-OFF record (not just that the
        // default reads false): explicitly_set must be true after boot.
        let log = h.get_or_open(onboarded).await.unwrap();
        assert!(
            spawn_blocking(move || log
                .explicitly_set(bossclaw_core::ConfigFlag::ConflictDetect)
                .unwrap())
            .await
            .unwrap(),
            "prime_switches persisted an explicit OFF (explicitly_set == true after boot)"
        );
    }

    /// SP3 A8: the EngineHandle wrappers mirror the mandates wrappers end to end, and carry the
    /// forward-only invariant (a disable clears the spent backfill; a later forward-only enable does
    /// not resurrect it) across the async seam. Also proves the not-onboarded gate surfaces `Open`.
    #[tokio::test]
    async fn capture_wrappers_roundtrip_and_stay_forward_only() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);

        // Defaults through the wrappers.
        assert!(!handle.capture_enabled(true).await.unwrap(), "default off");
        assert!(!handle.backfill_consented(true).await.unwrap(), "default off");
        assert_eq!(handle.capture_enabled_at(true).await.unwrap(), None);

        // Connect: both flags via one wrapper call.
        handle.set_capture_enabled(/*onboarded=*/ true, /*enabled=*/ true, /*backfill=*/ true, /*at=*/ 1_000).await.unwrap();
        assert!(handle.capture_enabled(true).await.unwrap());
        assert!(handle.backfill_consented(true).await.unwrap());
        assert_eq!(handle.capture_enabled_at(true).await.unwrap(), Some(1_000));

        // Disable then re-enable forward-only: backfill stays cleared, timestamp advances.
        handle.set_capture_enabled(/*onboarded=*/ true, /*enabled=*/ false, /*backfill=*/ false, /*at=*/ 2_000).await.unwrap();
        handle.set_capture_enabled(/*onboarded=*/ true, /*enabled=*/ true, /*backfill=*/ false, /*at=*/ 3_000).await.unwrap();
        assert!(handle.capture_enabled(true).await.unwrap());
        assert!(!handle.backfill_consented(true).await.unwrap(), "forward-only re-enable does not re-import (M4)");
        assert_eq!(handle.capture_enabled_at(true).await.unwrap(), Some(3_000));

        // Not-onboarded → the gate surfaces Open(NotOnboarded), like the mandates wrappers.
        assert!(matches!(
            handle.capture_enabled(false).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }

    /// Tasks 5 + 6: `run_ingest` marks the index current, then `recall` round-trips through
    /// `ensure_indexed` and hydrates the ingested snippet text.
    #[tokio::test]
    async fn ensure_indexed_builds_once_then_recall_finds_ingested_text() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.txt"), "ferris the crab loves rust").unwrap();
        handle.add_grant(true, src.path().to_path_buf()).await.unwrap();
        handle.run_ingest(true).await.unwrap(); // sets indexed=true after its rebuild
        let hits = handle.recall(true, "ferris crab".into(), 5).await.unwrap();
        assert!(hits.iter().any(|h| h.text.contains("ferris")), "recall finds the ingested text");
    }

    // ---- SP3: evolve loop core (Task 7) ----

    /// A prompt-agnostic `Reasoner` test double for the evolve loop. The engine's
    /// `ScriptedReasoner` is SHA-256-keyed on the exact `(system, prompt)`, but
    /// `evolve_once` computes the recall/neighborhood context internally, so reproducing
    /// those keys at the desktop level is fragile. This stub instead inspects the `schema`
    /// arg and returns a schema-valid response:
    /// - extraction schema (Pass A / Pass B): one entity, no relations/retractions — so a
    ///   fresh store deterministically MINTS exactly one entity (no relation ⇒ empty floor
    ///   ⇒ Pass B keeps nothing ⇒ no proposal/link path is ever reached);
    /// - adjudication schema (mid-band entity resolution): `{"match":"none"}` (mint a fresh
    ///   entity). On a fresh store there are no candidates, so this arm is defensive.
    ///
    /// It mints WITHOUT prompt-prediction, which is exactly what the task's "recommended
    /// test approach" calls for.
    struct StubReasoner {
        model_id: String,
    }
    impl StubReasoner {
        fn new(model_id: &str) -> Self {
            Self { model_id: model_id.to_string() }
        }
    }
    impl bossclaw_core::Reasoner for StubReasoner {
        fn complete_json(
            &self,
            _system: &str,
            _prompt: &str,
            schema: &serde_json::Value,
        ) -> Result<serde_json::Value, bossclaw_core::BossclawError> {
            // Dispatch on the schema shape (NOT the prompt) so the response is always
            // valid for the call site regardless of the internally-computed context.
            if schema == &bossclaw_core::reason::adjudication_schema() {
                return Ok(serde_json::json!({ "match": "none" }));
            }
            // Default: the extraction schema (Pass A and Pass B both use it). One entity,
            // no relations ⇒ a fresh mention mints exactly one entity and nothing else.
            Ok(serde_json::json!({
                "entities": [
                    { "mention": "Kenny", "entity_type": "person", "confidence": 0.95 }
                ],
                "relations": [],
                "retractions": []
            }))
        }
        fn model_id(&self) -> &str {
            &self.model_id
        }
    }

    /// A handle wired with the mock embedder + an arbitrary reasoner (the evolve tests
    /// inject the prompt-agnostic `StubReasoner` here).
    fn new_test_handle_with_reasoner(
        vault: Arc<TestVault>,
        dir: &tempfile::TempDir,
        reasoner: Arc<dyn bossclaw_core::Reasoner>,
    ) -> EngineHandle {
        EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(8)),
            Arc::new(crate::engine::reason::MockReasonerProvider::from_reasoner(reasoner)),
        )
    }

    /// Seed one `memory` event directly (the evolve queue consumes `memory` events). The
    /// derive/rebuild lifecycle runs through `ensure_indexed`/`evolve_once`, so this only
    /// needs to land the event.
    fn seed_one_memory(log: &EventLog, text: &str) {
        let ev = bossclaw_core::Event {
            id: String::new(),
            ts: String::new(),
            valid_time: None,
            event_type: "memory".to_string(),
            content: serde_json::json!({ "text": text }),
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: "did:wba:AIR-TEST".to_string(),
            signature: None,
        };
        log.append(ev).unwrap();
    }

    /// Seed one `memory` event and return its id (a lineage source for a Tier-B `write_proposal`).
    fn seed_one_memory_id(log: &EventLog, text: &str) -> String {
        log.append(bossclaw_core::event::Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: "memory".to_string(),
            content: serde_json::json!({ "text": text }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
        }).unwrap()
    }

    /// Count events of a given `event_type` in the log (used to prove zero `write_proposal`s).
    fn count_events_of_type(log: &EventLog, event_type: &str) -> usize {
        log.stream_all().unwrap().into_iter().filter(|e| e.event_type == event_type).count()
    }

    /// Ingest a single written file under a granted dir and return its `file_ingested` id.
    /// Desktop-crate equivalent of `tests/common::ingest_one` (which lives in the bossclaw-core
    /// test crate); built on the public API so the preview/apply tests can store + read bytes.
    fn bossclaw_ingest_one(log: &EventLog, path: &std::path::Path) -> String {
        let embedder = bossclaw_core::embed::MockEmbedder::new(8);
        log.ingest_all(&bossclaw_core::ingest::ParserRouter::native_only(), &embedder).unwrap();
        let canonical = std::fs::canonicalize(path).unwrap().to_string_lossy().to_string();
        log.current_files().unwrap().into_iter()
            .find(|r| r.canonical_path == canonical)
            .map(|r| r.file_event_id)
            .expect("ingested file id")
    }

    #[tokio::test]
    async fn list_proposals_returns_open_summaries() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();

        // Seed a lineage event so the Tier-B proposal append is valid.
        let lineage = seed_one_memory_id(&log, "Alice works at Acme");
        let key = serde_json::json!({"src":"entity:a","relation":"works_at","dst":"entity:acme"});
        let pid = log.append_write_proposal(
            "/tmp/acme/notes.md", "edit", "deadbeef", 0, "Alice now works at Globex",
            &key, &serde_json::json!({"requires_loud_modal": false, "taint": "Clean", "allowed": true}),
            std::slice::from_ref(&lineage),
        ).unwrap();
        drop(log);

        let proposals = handle.list_proposals(true).await.unwrap();
        assert_eq!(proposals.len(), 1);
        let p = &proposals[0];
        assert_eq!(p.id, pid);
        assert_eq!(p.target, "/tmp/acme/notes.md");
        assert_eq!(p.op, "edit");
        assert_eq!(p.new_content_hash, "deadbeef");
        assert_eq!(p.rationale, "Alice now works at Globex");
        assert!(!p.requires_loud_modal, "verdict_summary.requires_loud_modal projected");

        // Not onboarded → gate.
        assert!(matches!(
            handle.list_proposals(false).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }

    #[tokio::test]
    async fn garbled_verdict_summary_projects_requires_loud_modal_true() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let lineage = seed_one_memory_id(&log, "Alice works at Acme");
        let key = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        // verdict_summary is a STRING, not the expected object → `.get("requires_loud_modal")` is
        // None → fail-loud default true.
        let pid = log.append_write_proposal("/tmp/x/notes.md", "edit", "deadbeef", 0, "why",
            &key, &serde_json::json!("garbled"), std::slice::from_ref(&lineage)).unwrap();
        drop(log);

        let proposals = handle.list_proposals(true).await.unwrap();
        assert_eq!(proposals.len(), 1);
        assert!(proposals[0].id == pid && proposals[0].requires_loud_modal,
            "an absent/garbled verdict_summary fails loud (requires_loud_modal == true)");
    }

    #[tokio::test]
    async fn list_proposals_surfaces_producer() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let lineage = seed_one_memory_id(&log, "Alice works at Acme");
        let key = serde_json::json!({"src":"a","relation":"r","dst":"b"});
        let vs = serde_json::json!({"requires_loud_modal": false, "taint": "Clean", "allowed": true});
        let pid = log.append_write_proposal_with("/tmp/x/n.md", "edit", "deadbeef", 0, "why",
            &key, &vs, std::slice::from_ref(&lineage), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
        drop(log);

        let proposals = handle.list_proposals(true).await.unwrap();
        let p = proposals.iter().find(|p| p.id == pid).unwrap();
        assert_eq!(p.producer, "m6c-mandate-proposer", "the M6c producer is surfaced on the summary");
    }

    #[tokio::test]
    async fn proposal_preview_returns_old_and_new_text_fail_closed_on_missing_bytes() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();

        // A real write-granted file with known "old" bytes.
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("notes.md");
        std::fs::write(&path, b"Alice works at Acme.\n").unwrap();
        let file_id = bossclaw_ingest_one(&log, &path); // see helper note below

        // Build a real gated proposal for new bytes, then record it + its bytes (the engine
        // emit path), so preview can read both halves.
        let new_bytes = b"Alice works at Globex.\n".to_vec();
        let gated = log.propose_write(WriteProposal {
            target: path.clone(), new_content: new_bytes.clone(), op: WriteOp::Edit,
            source_event_ids: vec![file_id.clone()], rationale: "correction".to_string(),
        }).unwrap();
        let hash = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(&new_bytes))
        };
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"entity:a","relation":"works_at","dst":"entity:acme"});
        let verdict_summary = serde_json::json!({
            "requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint),
            "allowed": gated.verdict.allowed,
        });
        let pid = log.append_write_proposal(
            &canonical, "edit", &hash, new_bytes.len() as u64, "correction",
            &key, &verdict_summary, std::slice::from_ref(&file_id),
        ).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        let preview = handle.proposal_preview(true, pid.clone()).await.unwrap();
        assert_eq!(preview.path, canonical);
        assert_eq!(preview.old_text, "Alice works at Acme.\n");
        assert_eq!(preview.new_text, "Alice works at Globex.\n");
        assert_eq!(preview.op, "edit");
        assert_eq!(preview.rationale, "correction");

        // Fail-closed: an unknown id errors (no bytes / no proposal).
        assert!(handle.proposal_preview(true, "nonexistent".to_string()).await.is_err());

        // Fail-loud (MIN-1): a proposal whose verdict_summary is garbled previews as loud.
        let log = handle.get_or_open(true).await.unwrap();
        let new2 = b"Alice works at Initech.\n".to_vec();
        let hash2 = { use sha2::{Digest, Sha256}; hex::encode(Sha256::digest(&new2)) };
        let pid2 = log.append_write_proposal(&canonical, "edit", &hash2, new2.len() as u64,
            "correction2", &key, &serde_json::json!("garbled"), std::slice::from_ref(&file_id)).unwrap();
        log.put_proposal_bytes(&pid2, &new2, &hash2).unwrap();
        drop(log);
        let preview2 = handle.proposal_preview(true, pid2).await.unwrap();
        assert!(preview2.requires_loud_modal, "garbled verdict_summary previews fail-loud");
    }

    #[tokio::test]
    async fn apply_proposal_writes_file_and_resolves_then_stale_fails_closed() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};

        // ---- happy path ----
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("notes.md");
        let original = b"Alice works at Acme.\n".to_vec();
        std::fs::write(&path, &original).unwrap();
        let file_id = bossclaw_ingest_one(&log, &path);
        let new_bytes = b"Alice works at Globex.\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        let gated = log.propose_write(WriteProposal {
            target: path.clone(), new_content: new_bytes.clone(), op: WriteOp::Edit,
            source_event_ids: vec![file_id.clone()], rationale: "fix".to_string(),
        }).unwrap();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal(&canonical, "edit", &hash, new_bytes.len() as u64,
            "fix", &key, &vs, std::slice::from_ref(&file_id)).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        // An Edit to a TRACKED file is loud by construction: propose_write's engine-anchored taint
        // (actuator step 4, security-critical) sets taint=Untrusted for any currently-ingested
        // target, so the FRESH re-gate's requires_loud_modal is always true here. The authoritative
        // loud gate therefore needs the explicit ack to apply — exactly what the loud test asserts.
        let result = handle.apply_proposal(true, pid.clone(), true).await.unwrap();
        assert!(!result.file_written_id.is_empty(), "an apply returns the file_written id");
        assert_eq!(std::fs::read(&path).unwrap(), new_bytes, "the file gained the corrected bytes");
        // the proposal is no longer pending (resolved by the file_written).
        assert!(handle.list_proposals(true).await.unwrap().iter().all(|p| p.id != pid));
        // G4: the emitted file_written carries resolves_proposal == pid (the resolution mechanism).
        let log = handle.get_or_open(true).await.unwrap();
        let fw = log.event_by_id(&result.file_written_id).unwrap().unwrap();
        assert_eq!(fw.event_type, "file_written");
        assert_eq!(fw.content["resolves_proposal"], serde_json::json!(pid),
            "the file_written resolves the exact proposal id");
        drop(log);

        // ---- stale path: mutate the file AFTER a fresh propose, assert apply fails closed ----
        let (vault2, dir2) = test_vault_and_dir();
        let handle2 = new_test_handle(vault2, &dir2);
        let log2 = handle2.get_or_open(true).await.unwrap();
        let folder2 = tempfile::tempdir().unwrap();
        log2.add_grant(folder2.path()).unwrap();
        log2.add_write_grant(folder2.path()).unwrap();
        let path2 = folder2.path().join("notes.md");
        let orig2 = b"Alice works at Acme.\n".to_vec();
        std::fs::write(&path2, &orig2).unwrap();
        let fid2 = bossclaw_ingest_one(&log2, &path2);
        let new2 = b"Alice works at Globex.\n".to_vec();
        let hash2 = hex::encode(Sha256::digest(&new2));
        let canon2 = std::fs::canonicalize(&path2).unwrap().to_string_lossy().to_string();
        let k2 = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        let g2 = log2.propose_write(WriteProposal { target: path2.clone(), new_content: new2.clone(),
            op: WriteOp::Edit, source_event_ids: vec![fid2.clone()], rationale: "fix".to_string() }).unwrap();
        // The proposal records its base fingerprint = sha256("Alice works at Acme.") at propose.
        let vs2 = serde_json::json!({"requires_loud_modal": g2.verdict.requires_loud_modal,
            "taint": format!("{:?}", g2.verdict.taint), "allowed": g2.verdict.allowed,
            "base_content_hash": g2.verdict.base_content_hash});
        assert_eq!(g2.verdict.base_content_hash.as_deref(), Some(hex::encode(Sha256::digest(&orig2)).as_str()),
            "the gate fingerprinted the original on-disk bytes");
        let pid2 = log2.append_write_proposal(&canon2, "edit", &hash2, new2.len() as u64, "fix",
            &k2, &vs2, std::slice::from_ref(&fid2)).unwrap();
        log2.put_proposal_bytes(&pid2, &new2, &hash2).unwrap();
        drop(log2);

        // Someone edits the file out from under the proposal (live bytes no longer match base).
        std::fs::write(&path2, b"Alice retired.\n").unwrap();

        let stale = handle2.apply_proposal(true, pid2.clone(), false).await;
        assert!(matches!(stale, Err(EngineOpError::Stale(_))), "a changed file fails closed as Stale: {stale:?}");
        assert_eq!(std::fs::read(&path2).unwrap(), b"Alice retired.\n".to_vec(),
            "the file is untouched when the apply fails closed (no propose, no execute)");
    }

    #[tokio::test]
    async fn apply_proposal_loud_needs_ack_then_applies() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("secrets.md");
        let original = b"placeholder\n".to_vec();
        std::fs::write(&path, &original).unwrap();
        let file_id = bossclaw_ingest_one(&log, &path);
        // A >=32-char unbroken alphanumeric run trips the secret-shaped diff flag → loud.
        let new_bytes = b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"r","dst":"b"});
        let gated = log.propose_write(WriteProposal { target: path.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![file_id.clone()], rationale: "fix".to_string() }).unwrap();
        assert!(gated.verdict.requires_loud_modal, "secret-shaped content forces the loud modal");
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal(&canonical, "edit", &hash, new_bytes.len() as u64,
            "fix", &key, &vs, std::slice::from_ref(&file_id)).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        // acknowledged_loud=false → refuses, file unchanged.
        let needs = handle.apply_proposal(true, pid.clone(), false).await;
        assert!(matches!(needs, Err(EngineOpError::NeedsLoudConfirm(_))),
            "a loud write without ack is refused: {needs:?}");
        assert_eq!(std::fs::read(&path).unwrap(), original, "no write happened without the ack");

        // acknowledged_loud=true → applies.
        handle.apply_proposal(true, pid, true).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), new_bytes, "the ack lets the loud write through");
    }

    #[tokio::test]
    async fn apply_create_proposal_writes_new_file_and_refuses_if_target_reappeared() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};

        // ---- happy path: a Create lands the new file ----
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        // A lineage event so the Tier-B proposal is valid (a Create cites SOME source).
        let lineage = seed_one_memory_id(&log, "make a synced file");
        let target = folder.path().join("new.md"); // does NOT exist yet → Create
        let new_bytes = b"freshly synced content\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        // Gate a Create proposal (base_content_hash is None for a Create).
        let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Create, source_event_ids: vec![lineage.clone()], rationale: "create".to_string() }).unwrap();
        assert!(gated.verdict.base_content_hash.is_none(), "a Create carries no base hash");
        // The recorded target is the canonical PARENT-joined path (Create canonicalizes the parent).
        let canonical = gated.verdict.target_canonical.as_ref().unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"r","dst":"b"});
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal(&canonical, "create", &hash, new_bytes.len() as u64,
            "create", &key, &vs, std::slice::from_ref(&lineage)).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        // A Create is loud (ingested-target Step-4 taint doesn't apply, but a brand-new write to a
        // tracked folder may still be non-loud if Clean; pass acknowledged_loud=false and, if the
        // gate is loud, retry true — robust either way). Try false first:
        let first = handle.apply_proposal(true, pid.clone(), false).await;
        // A loud refuse fires pre-execute, so the first call does NOT consume the proposal — the
        // retry below still finds it via pending_proposals (the same pid is reusable).
        let applied = match first {
            Ok(r) => r,
            Err(EngineOpError::NeedsLoudConfirm(_)) => handle.apply_proposal(true, pid.clone(), true).await.unwrap(),
            Err(e) => panic!("unexpected create apply error: {e:?}"),
        };
        assert!(!applied.file_written_id.is_empty(), "the Create returned a file_written id");
        assert_eq!(std::fs::read(&target).unwrap(), new_bytes, "the new file was created with the bytes");

        // ---- refuse path: a Create whose target now EXISTS is refused (engine atomic no-clobber) ----
        let (vault2, dir2) = test_vault_and_dir();
        let handle2 = new_test_handle(vault2, &dir2);
        let log2 = handle2.get_or_open(true).await.unwrap();
        let folder2 = tempfile::tempdir().unwrap();
        log2.add_grant(folder2.path()).unwrap();
        log2.add_write_grant(folder2.path()).unwrap();
        let lineage2 = seed_one_memory_id(&log2, "make a synced file");
        let target2 = folder2.path().join("appears.md"); // absent at propose
        let new2 = b"would-be content\n".to_vec();
        let hash2 = hex::encode(Sha256::digest(&new2));
        let g2 = log2.propose_write(WriteProposal { target: target2.clone(), new_content: new2.clone(),
            op: WriteOp::Create, source_event_ids: vec![lineage2.clone()], rationale: "create".to_string() }).unwrap();
        let canon2 = g2.verdict.target_canonical.as_ref().unwrap().to_string_lossy().to_string();
        let vs2 = serde_json::json!({"requires_loud_modal": g2.verdict.requires_loud_modal,
            "taint": format!("{:?}", g2.verdict.taint), "allowed": g2.verdict.allowed,
            "base_content_hash": g2.verdict.base_content_hash});
        let pid2 = log2.append_write_proposal(&canon2, "create", &hash2, new2.len() as u64, "create",
            &serde_json::json!({"src":"a","relation":"r","dst":"b"}), &vs2, std::slice::from_ref(&lineage2)).unwrap();
        log2.put_proposal_bytes(&pid2, &new2, &hash2).unwrap();
        drop(log2);

        // The target reappears on disk BEFORE apply (a racer created it).
        std::fs::write(&target2, b"already here\n").unwrap();
        // Apply must fail closed (SF1): the FRESH `propose_write` re-gate runs `classify_op_existence`
        // and, seeing op=Create against an EXISTING target, sets `reject_reason = "create target
        // already exists"`; `apply_proposal` maps a `reject_reason` to `EngineOpError::Stale` (the
        // `gated.verdict.reject_reason.is_some() => Stale` arm), so it fails BEFORE execute — the
        // syscall atomic no-clobber is the deeper backstop but is not what fires here. Assert the
        // SPECIFIC Stale variant, and that the racer's file is untouched.
        let refused = handle2.apply_proposal(true, pid2, true).await;
        assert!(matches!(refused, Err(EngineOpError::Stale(_))),
            "a Create whose target reappeared must fail closed as Stale (re-gate classify_op_existence): {refused:?}");
        assert_eq!(std::fs::read(&target2).unwrap(), b"already here\n".to_vec(),
            "the racer's file is untouched (the apply never reached execute)");
    }

    /// Task 7 (mint): with the stub returning one entity for the extraction schema, an
    /// enabled `evolve_once` over a seeded memory mints ≥1 entity, and a follow-up recall
    /// surfaces the new entity dossier-adjacent content (here: the entity itself is folded,
    /// so recall over the brain still succeeds without error post-evolve).
    #[tokio::test]
    async fn evolve_once_mints_at_least_one_entity() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        let log = handle.get_or_open(true).await.unwrap();
        seed_one_memory(&log, "Kenny works at Acme");
        log.set_evolve_enabled(true).unwrap();
        drop(log);

        let report = handle.evolve_once(true).await.unwrap();
        assert!(report.entities_minted >= 1, "extracted at least one entity");
        // The post-evolve rebuild kept recall healthy (the new vectors fold in cleanly).
        let hits = handle.recall(true, "Kenny".into(), 5).await.unwrap();
        assert!(hits.iter().any(|h| h.text.contains("Kenny")), "recall surfaces the seeded memory");
    }

    /// Task 7 (proposals stay off — NON-VACUOUS): a dummy mandate is registered (so the M6c
    /// mandate path is reachable in principle), evolve is enabled, and a tick runs. The
    /// report MUST show zero proposals AND zero `write_proposal` events landed — proving
    /// BOTH the M6b `proposals_enabled` and M6c `mandates_enabled` gates hold (prime_switches).
    #[tokio::test]
    async fn evolve_once_emits_no_proposals_even_with_a_mandate() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        let log = handle.get_or_open(true).await.unwrap();

        // Register a real mandate so active_mandates() is non-empty (non-vacuous). The
        // target must be write-granted AND outside every read-grant root (Finding A).
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        log.add_grant(src.path()).unwrap();
        log.add_write_grant(out.path()).unwrap();
        log.add_mandate(&out.path().join("index.md"), src.path(), "an index of titles").unwrap();
        assert_eq!(log.active_mandates().unwrap().len(), 1, "mandate is registered (non-vacuous)");

        seed_one_memory(&log, "Kenny works at Acme");
        log.set_evolve_enabled(true).unwrap();
        drop(log);

        let report = handle.evolve_once(true).await.unwrap();
        assert_eq!(report.proposals_emitted, 0, "no write proposals in SP3 (both gates off)");

        let log = handle.get_or_open(true).await.unwrap();
        assert_eq!(
            count_events_of_type(&log, "write_proposal"),
            0,
            "zero write_proposal events landed (proposals + mandates gated off)"
        );
        // The gates themselves are still off after the tick (prime_switches at open).
        assert!(!log.proposals_enabled().unwrap(), "proposals gate stays off");
        assert!(!log.mandates_enabled().unwrap(), "mandates gate stays off");
    }

    /// Milestone D / spec R1+R5+R8 (manual-evolve consent chokepoint): the manual evolve path
    /// (`engine_evolve_now` → `evolve_once`) MUST be gated on the same signed-consent readiness
    /// as the scheduler. With a CLOUD config written but NO signed consent (the exact exploit:
    /// `set_reasoner_config({mode:"cloud",…})` then `evolve_now`), `evolve_once` must REFUSE —
    /// the cloud reasoner is never built and NO network call happens. The gate fires before the
    /// reasoner is constructed, so this test is network-free and sub-second even though the
    /// (unused) provider would otherwise egress recall/memory context to api.anthropic.com.
    #[tokio::test]
    async fn evolve_once_refuses_cloud_without_signed_consent() {
        let (vault, dir) = test_vault_and_dir();
        // The StubReasoner is wired but, in cloud mode with no consent, the gate fires BEFORE
        // any reasoner is built — so it is never invoked (no egress regardless of provider).
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        let log = handle.get_or_open(true).await.unwrap();
        // A non-empty queue so we exercise the cloud gate, not an empty-queue no-op.
        seed_one_memory(&log, "Kenny works at Acme");
        drop(log);

        // Flip the config to CLOUD (writes the signed config + cell, but NO consent record),
        // then enable evolve so we reach the cloud gate rather than the evolve-disabled return.
        handle
            .set_reasoner_config(
                true,
                serde_json::json!({
                    "mode": "cloud",
                    "provider": "anthropic",
                    "model": "claude-sonnet-4-6",
                    "base_url": null
                }),
            )
            .await
            .unwrap();
        handle.set_evolve_enabled(true, true).await.unwrap();

        // Cloud + no signed consent ⇒ refused: no reasoner built, no egress.
        let err = handle.evolve_once(true).await.unwrap_err();
        assert!(
            matches!(err, EngineOpError::Reasoner(_)),
            "cloud-not-ready evolve is a Reasoner error, got {err:?}"
        );
        assert!(
            err.to_string().contains("not ready"),
            "error explains the cloud reasoner is not ready, got {err}"
        );
    }

    /// Task 9 (spec I2): the shared cloud-consent pre-gate `cloud_consent_ok` — the SINGLE
    /// signed-consent barrier both `evolve_once` AND the conflict sweep (Task 11/12) gate on.
    /// BOTH halves of its contract are pinned directly here so the sweep's dependency on
    /// `cloud_consent_ok → false` stays guarded even if `evolve_once`'s gate is later rewritten.
    /// `test_vault_and_dir` seeds an EMPTY provider-key cache, so the Cloud branch is keychain-free
    /// and sub-second — the same setup `evolve_once_refuses_cloud_without_signed_consent` drives
    /// through this exact path.
    #[tokio::test]
    async fn cloud_consent_ok_is_true_for_local_and_gates_unready_cloud() {
        let (vault, dir) = test_vault_and_dir();
        let h = new_test_handle(vault, &dir);
        // Local (default) config → trivially OK: the reasoner egresses nothing.
        assert!(h.cloud_consent_ok(true).await, "local mode is always consent-ok");
        // Cloud config written but NO signed consent → fail-closed to false (no egress path open).
        h.set_reasoner_config(
            true,
            serde_json::json!({
                "mode": "cloud",
                "provider": "anthropic",
                "model": "claude-sonnet-4-6",
                "base_url": null
            }),
        )
        .await
        .unwrap();
        assert!(!h.cloud_consent_ok(true).await, "cloud without signed consent is NOT ok");
    }

    /// Task 7 (evolve_lock): a second concurrent `evolve_once` returns `Busy("evolve")`.
    /// The first tick holds `evolve_lock` across its `spawn_blocking`; the second `try_lock`
    /// fails. We force overlap by holding the lock guard directly while issuing a call.
    #[tokio::test]
    async fn second_concurrent_evolve_is_busy() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        let log = handle.get_or_open(true).await.unwrap();
        seed_one_memory(&log, "Kenny works at Acme");
        log.set_evolve_enabled(true).unwrap();
        drop(log);

        // Hold the evolve lock to simulate an in-flight tick, then a real call must be Busy.
        let guard = handle.evolve_lock.try_lock().expect("first lock acquired");
        let err = handle.evolve_once(true).await.unwrap_err();
        assert!(matches!(err, EngineOpError::Busy("evolve")), "second tick is Busy(\"evolve\")");
        drop(guard);
        // After release, a tick succeeds again.
        handle.evolve_once(true).await.unwrap();
    }

    /// Task 7 (telemetry): after a successful tick, `evolve_status` reports `last_tick_ms`
    /// is set; a forced error path bumps `error_count` and caps `last_error` length.
    #[tokio::test]
    async fn evolve_status_reflects_telemetry_and_error_path() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        let log = handle.get_or_open(true).await.unwrap();
        seed_one_memory(&log, "Kenny works at Acme");
        log.set_evolve_enabled(true).unwrap();
        drop(log);

        handle.evolve_once(true).await.unwrap();
        let (status, tel) = handle.evolve_status(true).await.unwrap();
        // The handle telemetry holds the REAL tick time; the engine's own EvolveStatus
        // honestly stubs last_tick_ms to None (it's the DTO merge that makes telemetry win —
        // covered by `evolve_status_dto_merges_engine_and_telemetry`).
        assert!(tel.last_tick_ms.is_some(), "a tick recorded its duration in telemetry");
        assert_eq!(status.last_tick_ms, None, "the engine status still stubs last_tick_ms");
        assert_eq!(tel.error_count, 0, "the successful tick recorded no error");

        // Force an error through record_tick and assert the bump + length cap directly
        // (the engine error path is exercised end-to-end by the live-Ollama test).
        let huge = "x".repeat(2000);
        handle.record_tick(7, &Err(EngineOpError::Core(huge)), false, 0);
        let (_s, tel2) = handle.evolve_status(true).await.unwrap();
        assert_eq!(tel2.error_count, 1, "the forced error bumped error_count");
        let last = tel2.last_error.expect("last_error stored");
        assert!(last.len() <= 512, "last_error is capped to ~512 bytes (was {})", last.len());
    }

    /// Spec R4 backstop: a CLOUD tick that returns Ok but processed 0 items while the queue
    /// had work is a silent bad/expired key — synthesize a `last_error` so it is visible.
    /// A local 0-item tick (or an empty queue) is normal idle, never a synthetic error.
    #[test]
    fn cloud_zero_item_tick_records_backstop_error() {
        use bossclaw_core::EvolveReport;
        let tel = std::sync::Mutex::new(EvolveTelemetry::default());
        // Ok report, 0 processed, CLOUD tick, queue had work -> last_error set.
        record_tick_into(&tel, 5, &Ok(EvolveReport { memories_processed: 0, ..Default::default() }), true, 3);
        assert!(tel.lock().unwrap().last_error.is_some());
        // Local 0-item tick (or empty queue) -> no synthetic error.
        let tel2 = std::sync::Mutex::new(EvolveTelemetry::default());
        record_tick_into(&tel2, 5, &Ok(EvolveReport { memories_processed: 0, ..Default::default() }), false, 3);
        assert!(tel2.lock().unwrap().last_error.is_none());
    }

    /// Egress transparency (spec R4): a CLOUD tick records its file-derived snippet count into
    /// telemetry; a LOCAL tick (which never egressed) leaves it `None` — so the banner only ever
    /// reports snippets that actually left the device.
    #[test]
    fn cloud_tick_records_tainted_recall_count_local_tick_does_not() {
        use bossclaw_core::EvolveReport;
        // A cloud tick that sent 3 file-derived snippets -> telemetry captures the count.
        let tel = std::sync::Mutex::new(EvolveTelemetry::default());
        record_tick_into(&tel, 5, &Ok(EvolveReport { tainted_recall_snippets: 3, ..Default::default() }), true, 1);
        assert_eq!(tel.lock().unwrap().last_tainted_snippets, Some(3));
        // A local tick leaves it None even with tainted snippets in the in-scope recall set.
        let tel2 = std::sync::Mutex::new(EvolveTelemetry::default());
        record_tick_into(&tel2, 5, &Ok(EvolveReport { tainted_recall_snippets: 7, ..Default::default() }), false, 1);
        assert_eq!(tel2.lock().unwrap().last_tainted_snippets, None);
        // An errored cloud tick has no report -> count stays None (nothing to disclose).
        let tel3 = std::sync::Mutex::new(EvolveTelemetry::default());
        record_tick_into(&tel3, 5, &Err(EngineOpError::Core("boom".into())), true, 1);
        assert_eq!(tel3.lock().unwrap().last_tainted_snippets, None);
    }

    /// Task 7 (toggle): `set_evolve_enabled` flips the sticky engine flag through the gate.
    #[tokio::test]
    async fn set_evolve_enabled_toggles_the_engine_flag() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        // prime_switches forces it off at open.
        let (status0, _t) = handle.evolve_status(true).await.unwrap();
        assert!(!status0.enabled, "evolve starts disabled (prime_switches)");
        handle.set_evolve_enabled(true, true).await.unwrap();
        let (status1, _t) = handle.evolve_status(true).await.unwrap();
        assert!(status1.enabled, "toggle on takes effect");
    }

    /// Task 11: the engine `detect_conflicts_once` wrapper drives the full op pattern (get_or_open →
    /// serialize → Local consent-ok → ensure_indexed → build reasoner → spawn_blocking the core
    /// cycle → record telemetry). Two contradicting NOTES + a scripted judge → exactly one proposal,
    /// and the session telemetry records it. Built INLINE with `MockEmbedderProvider::new(64)` (the
    /// shared `new_test_handle` uses dim=8, which drops the marquee pair below `CANDIDATE_SIM_MIN`).
    #[tokio::test]
    async fn engine_detect_conflicts_once_emits_a_proposal_when_enabled() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new(); // already Arc<TestVault>
        let a = "the default deploy target is vercel";
        let b = "the default deploy target is fly";
        // Two distinct-timestamp notes: older = the first `remember`, so the finder presents the
        // pair in (a, b) order deterministically — one scripted ordering suffices.
        let reasoner: Arc<dyn bossclaw_core::Reasoner> =
            Arc::new(bossclaw_core::ScriptedReasoner::new("test").with_response(
                bossclaw_core::conflict::CONFLICT_SYSTEM,
                &bossclaw_core::conflict::build_conflict_prompt(a, b),
                serde_json::json!({ "contradicts": true, "winner": "newer", "confidence": 92, "why": "renamed" }),
            ));
        let h = EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(64)),
            Arc::new(crate::engine::reason::MockReasonerProvider::from_reasoner(reasoner)),
        );
        let onboarded = true;
        h.remember(onboarded, a.to_string()).await.unwrap();
        h.remember(onboarded, b.to_string()).await.unwrap();
        h.set_conflict_detect_enabled(onboarded, true).await.unwrap();
        let report = h.detect_conflicts_once(onboarded, 100).await.unwrap();
        assert_eq!(report.proposed, 1, "engine cycle emits one proposal");
        assert_eq!(h.conflict_telemetry().proposed_total, 1, "telemetry recorded the proposal");
    }

    /// Task 11 (off-by-default THROUGH the wrapper): with conflict-detect left default-OFF, the
    /// wrapper still opens/consents/indexes/builds the reasoner, but the core flag gate makes the
    /// cycle emit nothing — `skipped_disabled` is true, no proposal, and the session telemetry is
    /// unchanged. Proves the off-switch holds through the engine layer, not just in core.
    #[tokio::test]
    async fn engine_detect_conflicts_once_is_a_no_op_when_disabled() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let vault = TestVault::new();
        let a = "the default deploy target is vercel";
        let b = "the default deploy target is fly";
        // A bare (response-less) scripted reasoner: the core gate skips before any judge call, so it
        // is never consulted — its absence of responses would only bite if the gate leaked.
        let reasoner: Arc<dyn bossclaw_core::Reasoner> =
            Arc::new(bossclaw_core::ScriptedReasoner::new("test"));
        let h = EngineHandle::new(
            vault,
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(64)),
            Arc::new(crate::engine::reason::MockReasonerProvider::from_reasoner(reasoner)),
        );
        let onboarded = true;
        h.remember(onboarded, a.to_string()).await.unwrap();
        h.remember(onboarded, b.to_string()).await.unwrap();
        // Deliberately DO NOT enable conflict-detect (it defaults off via prime_switches).
        let report = h.detect_conflicts_once(onboarded, 100).await.unwrap();
        assert!(report.skipped_disabled, "disabled brain: the core flag gate skips the cycle");
        assert_eq!(report.proposed, 0, "no proposal on a skipped cycle");
        assert_eq!(
            h.conflict_telemetry().proposed_total,
            0,
            "telemetry proposed_total unchanged on a no-op cycle"
        );
    }

    /// Task 11 (conflict_lock): a second overlapping `detect_conflicts_once` returns
    /// `Busy("conflict")`. Mirrors the reliable `second_concurrent_evolve_is_busy` pattern — hold the
    /// lock guard DIRECTLY (deterministic, not a racy two-task overlap) so the guarded call's
    /// `try_lock` fails, then confirm release lets a cycle run again.
    #[tokio::test]
    async fn second_concurrent_detect_conflicts_is_busy() {
        let (vault, dir) = test_vault_and_dir();
        let handle =
            new_test_handle_with_reasoner(vault, &dir, Arc::new(StubReasoner::new("stub-v1")));
        // Open first: `get_or_open` runs BEFORE the lock acquire in the wrapper.
        let _ = handle.get_or_open(true).await.unwrap();

        // Hold the conflict lock to simulate an in-flight cycle, then a real call must be Busy.
        let guard = handle.conflict_lock.try_lock().expect("first lock acquired");
        let err = handle.detect_conflicts_once(true, 100).await.unwrap_err();
        assert!(
            matches!(err, EngineOpError::Busy("conflict")),
            "overlapping cycle is Busy(\"conflict\")"
        );
        drop(guard);
        // After release, a cycle runs again (flag default-off → clean skip, no judge call).
        let report = handle.detect_conflicts_once(true, 100).await.unwrap();
        assert!(report.skipped_disabled, "post-release cycle runs (flag off → skipped)");
    }

    /// Task 10: the engine `reflect_once` wrapper drives the full op pattern (get_or_open → serialize
    /// on the DEDICATED `reflect_lock` → Local consent-ok → non-destructive miss-ring read →
    /// ensure_indexed → build reasoner → spawn_blocking the pre-rebuild + core tick + post-rebuilds →
    /// record telemetry → stamp the floor). A fresh brain with no misses/pages runs an empty-but-
    /// successful tick (NOT `skipped_disabled`) once reflection is enabled. Mirrors the conflict-sweep
    /// engine e2e's inline setup (`MockEmbedderProvider::new(64)` + a scripted reasoner).
    #[tokio::test]
    async fn engine_reflect_once_runs_when_enabled_and_records_telemetry() {
        crate::vault::seed_secret_cache_for_test(Default::default());
        let dir = tempfile::tempdir().unwrap();
        let engine = EngineHandle::new(
            TestVault::new(),
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(64)),
            Arc::new(crate::engine::reason::MockReasonerProvider::from_reasoner(Arc::new(
                bossclaw_core::ScriptedReasoner::new("test"),
            ))),
        );
        let onboarded = true;
        // ARCH minor-a: enable via the CORE setter (T1) through get_or_open — the ENGINE setter method
        // is T12's product surface and does not exist yet at this task; the test must compile+pass here.
        {
            let log = engine.get_or_open(onboarded).await.unwrap();
            tokio::task::spawn_blocking(move || log.set_reflect_enabled(true)).await.unwrap().unwrap();
        }
        // A fresh brain with no misses/pages → an empty-but-successful tick (not skipped_disabled).
        let report = engine.reflect_once(onboarded, 1000).await.expect("reflect tick runs");
        assert!(!report.skipped_disabled, "enabled → runs");
        assert_eq!(report.attempted, 0, "no seeded misses yet");
    }

    /// Task 10 review (reflect_lock): a second overlapping `reflect_once` returns `Busy("reflect")` —
    /// pinning BOTH the dedicated lock (a wrapper that locked `evolve_lock` instead would sail past
    /// the held guard and run) AND the exact tag. Mirrors the reliable
    /// `second_concurrent_detect_conflicts_is_busy` pattern — hold the lock guard DIRECTLY
    /// (deterministic, not a racy two-task overlap) so the guarded call's `try_lock` fails, then
    /// confirm release lets a full tick run again.
    #[tokio::test]
    async fn second_concurrent_reflect_is_busy() {
        crate::vault::seed_secret_cache_for_test(Default::default());
        let dir = tempfile::tempdir().unwrap();
        let engine = EngineHandle::new(
            TestVault::new(),
            dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(64)),
            Arc::new(crate::engine::reason::MockReasonerProvider::from_reasoner(Arc::new(
                bossclaw_core::ScriptedReasoner::new("test"),
            ))),
        );
        let onboarded = true;
        // Same fixture as the enabled-tick test above (core setter through get_or_open; the engine
        // setter is T12's surface). Enabled, so the post-release tick proves FULL service restored.
        {
            let log = engine.get_or_open(onboarded).await.unwrap();
            tokio::task::spawn_blocking(move || log.set_reflect_enabled(true)).await.unwrap().unwrap();
        }
        // Hold the reflect lock to simulate an in-flight tick, then a real call must be Busy.
        let guard = engine.reflect_lock.try_lock().expect("first lock acquired");
        let err = engine.reflect_once(onboarded, 1000).await.unwrap_err();
        assert!(
            matches!(err, EngineOpError::Busy("reflect")),
            "overlapping tick is Busy(\"reflect\") off the DEDICATED lock"
        );
        drop(guard);
        // After release, a full tick runs again (enabled → a real, non-skipped empty tick).
        let report = engine.reflect_once(onboarded, 1000).await.expect("post-release tick runs");
        assert!(!report.skipped_disabled, "lock release restores service");
    }

    /// Task 16 (R4-A final wire-in, §5 exit gate / §4 I3): the END-TO-END reflect path over the
    /// in-process engine — the product enable path (T12 engine setter), the miss classifier, and the
    /// §2.4 neutral digest line, all exercised together. Both classification branches fire and the
    /// digest renders the byte-exact `0 dossier(s) refreshed, 1 unknown-topic gap(s)` (a self-heal
    /// emits NO dossier; the honest gap is the only tally), then a second startup serve is empty
    /// (T13 "since last session" window consumed).
    ///
    /// SEEDING NOTE (verified against real seams, NOT the plan sketch's single tick): recall is
    /// FLOORLESS — [`crate::engine::mod`]'s `ensure_indexed` folds every memory into the vector arm and
    /// `HnswIndex::search` (core `index.rs`) returns the nearest neighbour for ANY query with no
    /// distance cutoff. So the instant one memory is indexed, EVERY re-attempted miss recall-hits at
    /// `attempt_miss` step 1 → `repaired_by_time`, and `no_material` (step-1 recall EMPTY) is
    /// unreachable in that SAME tick. This is the established Rung-4 core behaviour: the canonical
    /// `attempt_miss_classifies_repaired_by_time_and_no_material` (core `log.rs`) proves it by
    /// attempting `no_material` on a memory-LESS index, THEN `repaired_by_time` after indexing. We
    /// mirror that two-index-state ordering across two ENGINE ticks; the durable `reflect_counters`
    /// table accumulates across ticks, so the cumulative digest is still exactly the plan's line.
    #[tokio::test]
    async fn reflect_tick_classifies_misses_and_the_digest_line_appears() {
        crate::vault::seed_secret_cache_for_test(Default::default());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("identity.json"), serde_json::json!({
            "did": "did:wba:example.com:tester", "name": "Tester",
            "created_at": "2026-07-11T00:00:00+00:00" }).to_string()).unwrap();
        let engine = EngineHandle::new(
            crate::server::shared_test_vault(), dir.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(64)),
            Arc::new(reason::MockReasonerProvider::from_reasoner(
                Arc::new(bossclaw_core::ScriptedReasoner::new("test")))));
        let onboarded = true;
        // T12's engine setter (landed) — the product enable path, end to end.
        engine.set_reflect_enabled(onboarded, true).await.unwrap();
        let log = engine.get_or_open(onboarded).await.unwrap();
        let emb = engine.ensure_indexed(&log).await.unwrap(); // the engine's OWN embedder (model ids match)

        // ── Tick 1 (memory-LESS recall index): one unknowable miss → an honest `no_material` gap. ──
        {
            let log = log.clone();
            tokio::task::spawn_blocking(move || {
                log.seed_miss(
                    &bossclaw_core::reflect::normalized_query_key("zzz unknowable gibberish 9182"),
                    "zzz unknowable gibberish 9182", 10).unwrap();
            }).await.unwrap();
        }
        let r1 = engine.reflect_once(onboarded, 1_000).await.expect("tick 1 runs");
        assert!(!r1.skipped_disabled);
        assert_eq!(r1.attempted, 1, "the one open miss is attempted");
        assert_eq!(r1.no_material, 1, "unknowable on a memory-less index is an honest gap");
        assert_eq!(r1.dossiers_refreshed, 0, "a gap emits no dossier");

        // ── Remember one answerable memory + make it recall-visible IN PLACE (the manual rebuild wins
        //    over `ensure_indexed`'s memoized build flag), then seed its exact-text miss. ──
        {
            let log = log.clone();
            let emb = emb.clone();
            tokio::task::spawn_blocking(move || {
                let _m = log.remember(&*emb, "The capital of France is Paris.").unwrap();
                log.rebuild_indexes(&*emb).unwrap();
                log.seed_miss(
                    &bossclaw_core::reflect::normalized_query_key("The capital of France is Paris."),
                    "The capital of France is Paris.", 11).unwrap();
            }).await.unwrap();
        }

        // ── Tick 2 (memory now indexed): the matching miss self-heals via recall → `repaired_by_time`,
        //    emitting NO dossier. The terminal tick-1 gap is NOT re-attempted (open-misses only). ──
        let r2 = engine.reflect_once(onboarded, 2_000).await.expect("tick 2 runs");
        assert!(!r2.skipped_disabled);
        assert_eq!(r2.attempted, 1, "only the still-open miss is attempted (the gap is terminal)");
        assert_eq!(r2.repaired_by_time, 1, "the answerable miss self-heals via recall");
        assert_eq!(r2.no_material, 0, "the gap was already tallied in tick 1");
        assert_eq!(r2.dossiers_refreshed, 0, "a recall self-heal emits no dossier");

        // The NEUTRAL digest line renders byte-exactly on a startup serve — cumulative counters are
        // 0 dossiers refreshed (both branches emit nothing) + 1 gap — then the since-last-session
        // window is consumed (T13 rule) so a second startup serve is empty.
        let lines = engine.serve_reflect_digest_line("startup").await;
        assert_eq!(lines,
            vec!["0 dossier(s) refreshed, 1 unknown-topic gap(s) since last session.".to_string()]);
        assert!(engine.serve_reflect_digest_line("startup").await.is_empty(),
            "startup consumed the since-last-session window (T13 rule)");
    }

    #[tokio::test]
    async fn set_folder_writable_toggles_the_write_grant_and_list_writable_reflects_it() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        // A real folder to grant (must exist; add_write_grant canonicalizes + fails closed).
        let target = tempfile::tempdir().unwrap();
        let path = target.path().to_path_buf();
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();

        // Not onboarded → gate.
        assert!(matches!(
            handle.set_folder_writable(false, path.clone(), true).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));

        // Enable → listed writable.
        handle.set_folder_writable(true, path.clone(), true).await.unwrap();
        let writable = handle.list_writable(true).await.unwrap();
        assert!(writable.contains(&canonical), "enabled root is listed writable");

        // Disable → not listed.
        handle.set_folder_writable(true, path.clone(), false).await.unwrap();
        let writable = handle.list_writable(true).await.unwrap();
        assert!(!writable.contains(&canonical), "revoked root drops from the writable list");
    }

    #[tokio::test]
    async fn set_proposals_enabled_toggles_the_engine_flag() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        assert!(!log.proposals_enabled().unwrap(), "primed off at first open");
        drop(log);

        handle.set_proposals_enabled(true, true).await.unwrap();
        let log = handle.get_or_open(true).await.unwrap();
        assert!(log.proposals_enabled().unwrap(), "the op flips the sticky flag on");

        // Not onboarded → gate.
        assert!(matches!(
            handle.set_proposals_enabled(false, true).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }

    #[tokio::test]
    async fn set_mandates_enabled_toggles_the_engine_flag() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        assert!(!log.mandates_enabled().unwrap(), "primed off at first open");
        drop(log);

        handle.set_mandates_enabled(true, true).await.unwrap();
        let log = handle.get_or_open(true).await.unwrap();
        assert!(log.mandates_enabled().unwrap(), "the op flips the sticky flag on");
        drop(log);

        // Not onboarded → gate.
        assert!(matches!(
            handle.set_mandates_enabled(false, true).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }

    #[tokio::test]
    async fn mandates_enabled_reflects_the_persisted_flag() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        // Off by default at first open.
        assert!(!handle.mandates_enabled(true).await.unwrap(), "default off");
        handle.set_mandates_enabled(true, true).await.unwrap();
        assert!(handle.mandates_enabled(true).await.unwrap(), "the getter reflects the flip");
        // Not onboarded → gate.
        assert!(matches!(
            handle.mandates_enabled(false).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }

    #[tokio::test]
    async fn decline_proposal_removes_it_from_pending() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let lineage = seed_one_memory_id(&log, "Alice works at Acme");
        let key = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        let pid = log.append_write_proposal("/tmp/x/notes.md", "edit", "deadbeef", 0, "why",
            &key, &serde_json::json!({"requires_loud_modal": false, "taint": "Clean", "allowed": true}),
            std::slice::from_ref(&lineage)).unwrap();
        drop(log);

        assert_eq!(handle.list_proposals(true).await.unwrap().len(), 1);
        handle.decline_proposal(true, pid.clone(), "not now".to_string()).await.unwrap();
        assert!(handle.list_proposals(true).await.unwrap().is_empty(), "declined → no longer pending");

        assert!(matches!(
            handle.decline_proposal(false, pid, "x".to_string()).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }

    #[tokio::test]
    async fn undo_apply_restores_the_original_bytes() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("notes.md");
        let original = b"Alice works at Acme.\n".to_vec();
        std::fs::write(&path, &original).unwrap();
        let file_id = bossclaw_ingest_one(&log, &path);
        let new_bytes = b"Alice works at Globex.\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"works_at","dst":"acme"});
        let gated = log.propose_write(WriteProposal { target: path.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![file_id.clone()], rationale: "fix".to_string() }).unwrap();
        // Mirror the happy-path apply flow (apply_proposal_writes_file_and_resolves_then_stale_fails_closed):
        // the proposal MUST carry base_content_hash in its verdict_summary or Task 8's anti-clobber gate
        // fails closed with Stale("proposal has no base fingerprint to verify against").
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal(&canonical, "edit", &hash, new_bytes.len() as u64, "fix",
            &key, &vs, std::slice::from_ref(&file_id)).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        // An Edit to a TRACKED file is loud by construction (engine-anchored taint=Untrusted), so the
        // FRESH re-gate requires the explicit ack — pass `true`, like the happy-path apply flow.
        let applied = handle.apply_proposal(true, pid, true).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), new_bytes, "applied");
        handle.undo_apply(true, applied.file_written_id.clone()).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original, "undo restored the original bytes");

        assert!(matches!(
            handle.undo_apply(false, applied.file_written_id).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }

    #[tokio::test]
    async fn mandate_crud_round_trip_and_grant_rejection_is_typed() {
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // A mandate target must be WRITE-granted AND outside every read root (add_mandate guard #4),
        // so `dest` is WRITE-ONLY (no add_grant); the read-granted `scope` holds the sources.
        let dest = tempfile::tempdir().unwrap();
        let scope = tempfile::tempdir().unwrap();
        log.add_write_grant(dest.path()).unwrap(); // write-ONLY → valid mandate target root.
        log.add_grant(scope.path()).unwrap();
        let target = dest.path().join("synced.md");
        std::fs::write(&target, b"x\n").unwrap();
        drop(log);

        // add → returns a MandateSummary with the canonical fields.
        let m = handle.add_mandate(true, target.clone(), scope.path().to_path_buf(),
            "keep it synced".to_string()).await.unwrap();
        assert!(!m.mandate_grant_id.is_empty());
        assert_eq!(m.recipe, "keep it synced");
        assert!(!m.revoked);

        // list → one active mandate.
        let listed = handle.list_mandates(true).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].mandate_grant_id, m.mandate_grant_id);

        // revoke → list empty.
        handle.revoke_mandate(true, m.mandate_grant_id.clone()).await.unwrap();
        assert!(handle.list_mandates(true).await.unwrap().is_empty(), "revoked → no active mandates");

        // Grant rejection surfaces as a TYPED Rejected error. The recipe cap is guard #1 in
        // add_mandate (log.rs:2807) — it fires BEFORE the write-grant (#3) and read-root (#4) checks,
        // so a > MAX_RECIPE_LEN (2048) recipe rejects for the recipe reason, mapped to Rejected.
        let huge = "a".repeat(3000);
        let err = handle.add_mandate(true, target, scope.path().to_path_buf(), huge).await.unwrap_err();
        assert!(matches!(err, EngineOpError::Rejected(_)),
            "a grant-time guard failure (here: recipe too long) maps to a typed Rejected error: {err:?}");

        // Not onboarded → gate.
        assert!(matches!(
            handle.list_mandates(false).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }

    #[tokio::test]
    async fn mandate_writes_op_returns_applied_m6c_writes() {
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let path = folder.path().join("mandated.md");
        std::fs::write(&path, b"old\n").unwrap();
        let fid = bossclaw_ingest_one(&log, &path);
        let new_bytes = b"new\n".to_vec();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&path).unwrap().to_string_lossy().to_string();
        let key = serde_json::json!({"src":"a","relation":"r","dst":"b"});
        // Stamp the proposal M6c so it is attributable as a mandate write.
        let gated = log.propose_write(bossclaw_core::actuator::WriteProposal {
            target: path.clone(), new_content: new_bytes.clone(),
            op: bossclaw_core::actuator::WriteOp::Edit, source_event_ids: vec![fid.clone()],
            rationale: "sync".to_string() }).unwrap();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
            "sync", &key, &vs, std::slice::from_ref(&fid), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        // Apply (loud because ingested ⇒ Untrusted → ack=true).
        handle.apply_proposal(true, pid, true).await.unwrap();
        let writes = handle.mandate_writes(true).await.unwrap();
        assert_eq!(writes.len(), 1, "the applied M6c write is listed");
        assert_eq!(writes[0].target, canonical);
        assert!(!writes[0].undone);
        assert!(!writes[0].file_written_id.is_empty());

        // Not onboarded → gate.
        assert!(matches!(
            handle.mandate_writes(false).await,
            Err(EngineOpError::Open(EngineError::NotOnboarded))
        ));
    }

    // Shared builder: ingest a source under a read-granted `scope`, grant a mandate for a target
    // in a write-granted `dest`, and emit an M6c proposal rewriting the target from the source.
    // Returns (handle, dest-keepalive, scope-keepalive, target path, canonical target, pid).
    #[cfg(unix)]
    async fn seed_clean_mandate_proposal(
    ) -> (EngineHandle, tempfile::TempDir, tempfile::TempDir, tempfile::TempDir, std::path::PathBuf, String, String) {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // Mandate target dir is WRITE-ONLY (outside every read root — add_mandate guard #4); the
        // read-granted `scope` holds the source.
        let dest = tempfile::tempdir().unwrap();
        let scope = tempfile::tempdir().unwrap();
        log.add_write_grant(dest.path()).unwrap();
        log.add_grant(scope.path()).unwrap();
        let target = dest.path().join("synced.md");
        std::fs::write(&target, b"old\n").unwrap();
        let src = scope.path().join("s.md");
        std::fs::write(&src, b"clean source body\n").unwrap();
        let src_id = bossclaw_ingest_one(&log, &src);
        log.add_mandate(&target, scope.path(), "sync from scope").unwrap();
        log.rebuild_graph().unwrap();
        // A CLEAN rewrite: in-scope authorized source + non-secret content ⇒ not loud (Task 1 rule).
        let new_bytes = b"clean new content\n".to_vec();
        let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![src_id.clone()], rationale: "sync".to_string() }).unwrap();
        assert!(!gated.verdict.requires_loud_modal, "fixture must be CLEAN for the auto-apply test");
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
            "sync", &serde_json::json!({"src":"a","relation":"r","dst":"b"}), &vs,
            std::slice::from_ref(&src_id), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);
        (handle, dir, dest, scope, target, canonical, pid)
    }

    #[tokio::test]
    async fn sweep_auto_applies_a_clean_mandate_proposal() {
        let (handle, _dir, _dest, _scope, target, _canonical, pid) = seed_clean_mandate_proposal().await;
        // Mandates ON (the sweep re-reads it per item).
        handle.set_mandates_enabled(true, true).await.unwrap();
        let applied = handle.mandate_autoapply_sweep(true).await.unwrap();
        assert_eq!(applied, 1, "the clean mandate proposal was auto-applied");
        // POSITIVE mutation-verify: the file changed AND the proposal is resolved.
        assert_eq!(std::fs::read(&target).unwrap(), b"clean new content\n".to_vec(), "the file gained the synced bytes");
        assert!(handle.list_proposals(true).await.unwrap().iter().all(|p| p.id != pid), "proposal resolved");
        // And it appears in the mandate-activity list with Undo.
        let writes = handle.mandate_writes(true).await.unwrap();
        assert_eq!(writes.len(), 1, "the auto-applied write is recorded for Undo");
    }

    #[tokio::test]
    async fn sweep_leaves_a_risky_mandate_proposal_queued() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // Mandate target dir is WRITE-ONLY (add_mandate guard #4); read-granted `scope` holds the source.
        let dest = tempfile::tempdir().unwrap();
        let scope = tempfile::tempdir().unwrap();
        log.add_write_grant(dest.path()).unwrap();
        log.add_grant(scope.path()).unwrap();
        let target = dest.path().join("synced.md");
        std::fs::write(&target, b"old\n").unwrap();
        let src = scope.path().join("s.md");
        std::fs::write(&src, b"clean source\n").unwrap();
        let src_id = bossclaw_ingest_one(&log, &src);
        log.add_mandate(&target, scope.path(), "sync").unwrap();
        log.rebuild_graph().unwrap();
        // RISKY: secret-shaped new content forces loud even though the source is in-scope.
        let new_bytes = b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd\n".to_vec();
        let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![src_id.clone()], rationale: "sync".to_string() }).unwrap();
        assert!(gated.verdict.requires_loud_modal, "secret-shaped ⇒ loud");
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
            "sync", &serde_json::json!({"src":"a","relation":"r","dst":"b"}), &vs,
            std::slice::from_ref(&src_id), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        handle.set_mandates_enabled(true, true).await.unwrap();
        let applied = handle.mandate_autoapply_sweep(true).await.unwrap();
        assert_eq!(applied, 0, "a risky (loud) mandate proposal is NOT auto-applied");
        assert_eq!(std::fs::read(&target).unwrap(), b"old\n".to_vec(), "the file is untouched");
        // SF3: the risky proposal stays queued AND still carries the m6c producer, so the Review
        // surface can render its "from a mandate" label (the label path survives the sweep).
        let queued = handle.list_proposals(true).await.unwrap();
        let row = queued.iter().find(|p| p.id == pid).expect("the risky proposal stays queued for SP4 Review");
        assert_eq!(row.producer, "m6c-mandate-proposer",
            "the queued risky proposal keeps its m6c producer (the 'from mandate' label path)");
    }

    #[tokio::test]
    async fn sweep_never_auto_applies_an_m6b_reconcile_proposal() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        let folder = tempfile::tempdir().unwrap();
        log.add_grant(folder.path()).unwrap();
        log.add_write_grant(folder.path()).unwrap();
        let target = folder.path().join("note.md");
        std::fs::write(&target, b"old\n").unwrap();
        let fid = bossclaw_ingest_one(&log, &target);
        let new_bytes = b"new\n".to_vec();
        let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
            op: WriteOp::Edit, source_event_ids: vec![fid.clone()], rationale: "reconcile".to_string() }).unwrap();
        let hash = hex::encode(Sha256::digest(&new_bytes));
        let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
        let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
            "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
            "base_content_hash": gated.verdict.base_content_hash});
        // Stamp M6b (the reconciler) — the producer filter must exclude it from the sweep.
        let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
            "reconcile", &serde_json::json!({"src":"a","relation":"r","dst":"b"}), &vs,
            std::slice::from_ref(&fid), bossclaw_core::graph::M6B_PROPOSER_PRODUCER).unwrap();
        log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
        drop(log);

        handle.set_mandates_enabled(true, true).await.unwrap();
        let applied = handle.mandate_autoapply_sweep(true).await.unwrap();
        assert_eq!(applied, 0, "an M6b reconcile proposal is NEVER auto-applied (producer filter)");
        assert_eq!(std::fs::read(&target).unwrap(), b"old\n".to_vec(), "the file is untouched");
        assert!(handle.list_proposals(true).await.unwrap().iter().any(|p| p.id == pid),
            "the M6b proposal stays queued for human review (SP4 unchanged)");
    }

    // Security L2: the per-item fast-kill. With TWO clean M6c proposals queued but mandates turned
    // OFF, the sweep's per-item `mandates_enabled_or_false` read gates the FIRST iteration and breaks,
    // so NOTHING is applied — proving the guard reads the LIVE flag, not a snapshot taken before the
    // loop. (A true "flip AFTER the first apply, stop the second" assertion would need a production
    // test-hook between iterations, which is out of scope; gating-with-the-flag-off pins the same
    // live-read invariant deterministically.)
    #[tokio::test]
    async fn sweep_fast_kills_when_mandates_off_even_with_clean_candidates() {
        use bossclaw_core::actuator::{WriteOp, WriteProposal};
        use sha2::{Digest, Sha256};
        let (vault, dir) = test_vault_and_dir();
        let handle = new_test_handle(vault, &dir);
        let log = handle.get_or_open(true).await.unwrap();
        // Both mandate targets live under a WRITE-ONLY dest (add_mandate guard #4); read-granted
        // `scope` holds the shared source.
        let dest = tempfile::tempdir().unwrap();
        let scope = tempfile::tempdir().unwrap();
        log.add_write_grant(dest.path()).unwrap();
        log.add_grant(scope.path()).unwrap();
        let src = scope.path().join("s.md");
        std::fs::write(&src, b"clean source\n").unwrap();
        let src_id = bossclaw_ingest_one(&log, &src);
        // Two distinct mandates+targets, each yielding a CLEAN (in-scope, non-secret) M6c proposal.
        let mut pids = Vec::new();
        for name in ["a.md", "b.md"] {
            let target = dest.path().join(name);
            std::fs::write(&target, b"old\n").unwrap();
            log.add_mandate(&target, scope.path(), "sync").unwrap();
            log.rebuild_graph().unwrap();
            let new_bytes = format!("clean new {name}\n").into_bytes();
            let gated = log.propose_write(WriteProposal { target: target.clone(), new_content: new_bytes.clone(),
                op: WriteOp::Edit, source_event_ids: vec![src_id.clone()], rationale: "sync".to_string() }).unwrap();
            assert!(!gated.verdict.requires_loud_modal, "fixture proposals must be CLEAN");
            let hash = hex::encode(Sha256::digest(&new_bytes));
            let canonical = std::fs::canonicalize(&target).unwrap().to_string_lossy().to_string();
            let vs = serde_json::json!({"requires_loud_modal": gated.verdict.requires_loud_modal,
                "taint": format!("{:?}", gated.verdict.taint), "allowed": gated.verdict.allowed,
                "base_content_hash": gated.verdict.base_content_hash});
            let pid = log.append_write_proposal_with(&canonical, "edit", &hash, new_bytes.len() as u64,
                "sync", &serde_json::json!({"src":"a","relation":"r","dst":name}), &vs,
                std::slice::from_ref(&src_id), bossclaw_core::graph::M6C_PROPOSER_PRODUCER).unwrap();
            log.put_proposal_bytes(&pid, &new_bytes, &hash).unwrap();
            pids.push(pid);
        }
        drop(log);

        // Mandates OFF (never enabled). The per-item kill-switch read gates the first iteration.
        let applied = handle.mandate_autoapply_sweep(true).await.unwrap();
        assert_eq!(applied, 0, "with mandates off the per-item fast-kill applies NOTHING");
        // Both clean proposals are untouched on disk and still queued.
        assert_eq!(std::fs::read(dest.path().join("a.md")).unwrap(), b"old\n".to_vec(), "a.md untouched");
        assert_eq!(std::fs::read(dest.path().join("b.md")).unwrap(), b"old\n".to_vec(), "b.md untouched");
        let queued = handle.list_proposals(true).await.unwrap();
        assert!(pids.iter().all(|pid| queued.iter().any(|p| &p.id == pid)),
            "both clean proposals stay queued when the sweep fast-kills");
    }

    #[tokio::test]
    async fn reseed_reasoner_cell_loads_signed_cloud_config_into_the_cell() {
        // Restart-persistence WIRING (Phase 2b): on boot, reseed_reasoner_cell must copy
        // the signed-log config into the in-memory cell the provider closure reads.
        let (vault, dir) = test_vault_and_dir();
        let h = new_test_handle(vault, &dir);
        let cell = std::sync::Mutex::new(crate::engine::reason::ReasonerConfig::default());
        assert_eq!(cell.lock().unwrap().mode, crate::engine::reason::ReasonerMode::Local);

        h.set_reasoner_config(true, serde_json::json!({
            "mode": "cloud", "provider": "anthropic",
            "model": "claude-sonnet-4-6", "base_url": null
        })).await.expect("set cloud config");

        super::reseed_reasoner_cell(&h, &cell, true).await;

        let got = cell.lock().unwrap().clone();
        assert_eq!(got.mode, crate::engine::reason::ReasonerMode::Cloud);
        assert_eq!(got.model, "claude-sonnet-4-6");
    }

    /// DAEMON-ADDED (M1a Task 4 follow-up; egress-security review L-1): revocation continuity.
    /// A Cloud→Local mode flip via `set_reasoner_config` persists through the SAME signed log
    /// the scheduler re-reads EVERY tick (`reasoner_config_or_default` — the per-wake read in
    /// `scheduler::spawn`, scheduler.rs:98), so revocation-by-flip takes effect within one tick:
    /// the next wake sees Local, probes only Ollama, and builds no cloud arm. Consent is written
    /// directly to the log (simulating a prior successful R5 enable — a REAL enable needs a live
    /// provider key + network, which tests never touch) and is intentionally LEFT BEHIND by the
    /// flip: mode, not consent removal, is what stops the cloud arm.
    #[tokio::test]
    async fn mode_flip_to_local_revokes_cloud_within_one_scheduler_read() {
        let (vault, dir) = test_vault_and_dir();
        let h = new_test_handle(vault, &dir);

        // A signed Cloud config + a consent record shaped exactly as enable_cloud_reasoner writes.
        h.set_reasoner_config(true, serde_json::json!({
            "mode": "cloud", "provider": "anthropic",
            "model": "claude-sonnet-4-6", "base_url": null
        })).await.expect("set cloud config");
        let log = h.get_or_open(true).await.unwrap();
        log.set_cloud_reasoner_consent(serde_json::json!({
            "provider": "anthropic",
            "base_url_host": "api.anthropic.com",
            "key_fingerprint": "abc123",
            "consented_at": "2026-07-02T00:00:00Z",
        })).unwrap();
        drop(log);

        // Precondition: the scheduler's per-tick read sees the Cloud config.
        let cfg = h.reasoner_config_or_default(true).await;
        assert!(
            matches!(cfg.mode, crate::engine::reason::ReasonerMode::Cloud),
            "precondition: the signed cloud config is what the per-tick read returns"
        );

        // The user flips back to Local (revocation-by-flip; the consent record stays behind).
        h.set_reasoner_config(true, serde_json::json!({
            "mode": "local", "provider": "anthropic", "model": "", "base_url": null
        })).await.expect("flip to local");

        // The EXACT per-tick read now returns Local: the very next scheduler wake takes the
        // Ollama-probe arm and never constructs the cloud reasoner (≤1 tick to take effect).
        let cfg = h.reasoner_config_or_default(true).await;
        assert!(
            matches!(cfg.mode, crate::engine::reason::ReasonerMode::Local),
            "the Local flip persisted through the same signed log the scheduler reads per tick"
        );
    }

    // ---- M1a Task 6 review fix: config write-through to the PROVIDER CELL ----
    //
    // The sibling test above proves the scheduler's per-tick GATE reads the flip from the signed
    // log — but the reasoner INSTANCE comes from the `ConfigReasonerProvider` closure, which reads
    // the in-memory CELL. Pre-M1a the APP wrote every config change through to that cell;
    // post-extraction the daemon persisted to the log but only reseeded the cell at BOOT — so a
    // Cloud→Local flip could keep the CLOUD reasoner in use until restart (a revocation-latency
    // hole; the consent record stays on file so readiness stays true). These tests pin the fix:
    // BOTH config-writing ops refresh the attached cell immediately after a successful persist.

    #[tokio::test]
    async fn set_reasoner_config_refreshes_the_provider_cell_without_restart() {
        let (vault, dir) = test_vault_and_dir();
        let cell = Arc::new(std::sync::Mutex::new(crate::engine::reason::ReasonerConfig::default()));
        let h = new_test_handle(vault, &dir).with_reasoner_cell(cell.clone());

        // A cloud-shaped persist: the cell the provider closure reads must flip to Cloud
        // immediately — no daemon restart, no boot reseed.
        h.set_reasoner_config(true, serde_json::json!({
            "mode": "cloud", "provider": "anthropic",
            "model": "claude-sonnet-4-6", "base_url": null
        })).await.expect("persist cloud config");
        assert!(
            matches!(cell.lock().unwrap().mode, crate::engine::reason::ReasonerMode::Cloud),
            "the cell reads Cloud right after the persist (no restart)"
        );

        // Flip BACK to Local: the revocation must reach the cell immediately — this is the exact
        // hole the review found (a stale cell kept the CLOUD reasoner in use until restart).
        h.set_reasoner_config(true, serde_json::json!({
            "mode": "local", "provider": "anthropic", "model": "", "base_url": null
        })).await.expect("persist local config");
        assert!(
            matches!(cell.lock().unwrap().mode, crate::engine::reason::ReasonerMode::Local),
            "Cloud→Local revocation is effective in the cell without a restart"
        );
    }

    #[tokio::test]
    async fn enable_cloud_reasoner_success_refreshes_the_provider_cell() {
        let (vault, dir) = test_vault_and_dir();
        let cell = Arc::new(std::sync::Mutex::new(crate::engine::reason::ReasonerConfig::default()));
        // Script the EXACT fixed probe turn `enable_cloud_reasoner` sends (a canned prompt with
        // no memory/file bytes), so the R5 probe succeeds HERMETICALLY — no key, no egress. The
        // returned JSON is discarded by the enable flow (only Ok/Err matters).
        let probe = bossclaw_core::ScriptedReasoner::new("scripted-probe").with_response(
            "Reply with the JSON {\"match\":\"ok\"}.",
            "candidates: [ok]. text: ok",
            serde_json::json!({ "match": "ok" }),
        );
        let h = new_test_handle(vault, &dir)
            .with_reasoner_cell(cell.clone())
            .with_probe_reasoner_for_test(Arc::new(probe));

        h.enable_cloud_reasoner(true, serde_json::json!({
            "mode": "cloud", "provider": "anthropic",
            "model": "claude-sonnet-4-6", "base_url": null
        })).await.expect("probe succeeds → config + consent persist");
        assert!(
            matches!(cell.lock().unwrap().mode, crate::engine::reason::ReasonerMode::Cloud),
            "the cell reads Cloud right after a successful enable (no restart)"
        );
    }

    #[tokio::test]
    async fn enable_cloud_reasoner_probe_failure_leaves_the_provider_cell_unchanged() {
        let (vault, dir) = test_vault_and_dir(); // seeds the provider-key cache EMPTY
        let cell = Arc::new(std::sync::Mutex::new(crate::engine::reason::ReasonerConfig::default()));
        // NO probe override: the real builder makes a CloudReasoner whose `read_key` finds no key
        // in the (empty-seeded) cache and fails fast — hermetic, no network, nothing persisted.
        let h = new_test_handle(vault, &dir).with_reasoner_cell(cell.clone());

        h.enable_cloud_reasoner(true, serde_json::json!({
            "mode": "cloud", "provider": "anthropic",
            "model": "claude-sonnet-4-6", "base_url": null
        })).await.expect_err("no provider key → the R5 probe fails closed");
        assert!(
            matches!(cell.lock().unwrap().mode, crate::engine::reason::ReasonerMode::Local),
            "a failed enable must never touch the cell (fail-closed write-through)"
        );
    }

    // ---- Rung 2: resolution-aware recall + consent-gated language migration (A5/A6) ----

    /// A resolution-aware handle wired with a caller-supplied vault + embedder provider (rung 2). The
    /// vault is a parameter so two handles over ONE data dir (crash → boot-resume) can SHARE a DEK
    /// and thus decrypt the same brain.db. Returns an `Arc` because the migration entry points
    /// (`set_active_model`/`resume_migration_if_pending`) take `self: &Arc<Self>` to spawn their
    /// background task. Seeds the provider-key cache EMPTY (no keychain prompt).
    fn test_handle_with_vault_and_provider(
        vault: Arc<TestVault>,
        home: std::path::PathBuf,
        provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
    ) -> Arc<EngineHandle> {
        crate::vault::seed_secret_cache_for_test(Default::default());
        Arc::new(EngineHandle::new(
            vault,
            home,
            provider,
            Arc::new(crate::engine::reason::MockReasonerProvider::new("m")),
        ))
    }

    /// A resolution-aware handle over a FRESH vault (single-handle tests). Mirrors `new_test_handle`.
    fn test_handle_with_provider(
        home: std::path::PathBuf,
        provider: Arc<dyn crate::engine::embed::EmbedderProvider>,
    ) -> Arc<EngineHandle> {
        test_handle_with_vault_and_provider(TestVault::new(), home, provider)
    }

    /// U5/I3: a recall against a signed `Complete` language pack whose files are absent must REFUSE
    /// loudly (never silently serve the wrong/empty model), and the provider must surface `Missing`
    /// so the UI can prompt a re-download. Proves `ensure_indexed` resolves via `embedder_for`.
    #[tokio::test]
    async fn recall_refuses_loudly_when_signed_model_missing() {
        use crate::engine::embed::ResourceModel2Vec;
        use bossclaw_core::{LanguagePackRecord, MigrationState};
        // Build a resolution-aware provider whose data root has NO folder for the enabled model.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("models");
        std::fs::create_dir_all(root.join("potion-base-8M")).unwrap();
        let provider = std::sync::Arc::new(ResourceModel2Vec::with_resolution(
            None, root.join("potion-base-8M"), root, crate::engine::embed::MODEL_ID.to_string(),
        ));
        let handle = test_handle_with_provider(tmp.path().to_path_buf(), provider);
        // Onboard + record a Complete multilingual intent whose folder is absent.
        let log = handle.get_or_open(true).await.unwrap();
        log.set_language_pack_record(&LanguagePackRecord {
            model_id: "minishlab/potion-multilingual-128M".into(),
            safetensors_sha: "abc".into(),
            migration: MigrationState::Complete,
            consented_at: "t".into(),
        }).unwrap();
        let err = handle.recall(true, "anything".into(), 5).await.unwrap_err();
        assert!(matches!(err, crate::engine::EngineOpError::Embedder(_)),
            "recall must refuse when the signed model is missing (I3), got {err:?}");
        // The resolution path (not a generic embedder failure) ran: the provider surfaces Missing.
        assert!(matches!(handle.model_state().0, crate::engine::embed::ModelState::Missing { .. }),
            "the provider surfaces Missing so the UI can prompt a re-download (U5)");
    }

    // ---- A6 helpers: staged mock model + id-reporting loader + memory factory + status poll ----

    /// An embedder that wraps a `MockEmbedder` but reports the RESOLVED model id, so a migration's
    /// re-embed writes rows under the new id without real weights (mirrors embed.rs's `IdOverride`).
    struct IdReportingEmbedder { inner: Arc<dyn bossclaw_core::Embedder>, id: String }
    impl bossclaw_core::Embedder for IdReportingEmbedder {
        fn embed(&self, t: &[String]) -> Result<Vec<Vec<f32>>, bossclaw_core::BossclawError> { self.inner.embed(t) }
        fn dim(&self) -> usize { self.inner.dim() }
        fn model_id(&self) -> &str { &self.id }
    }

    /// A loader yielding a dim-8 `MockEmbedder` that reports whatever id resolution asks for, so the
    /// migration can be driven end-to-end without real weights (reuses A4's IdOverride pattern).
    fn mock_loader_reporting_ids() -> crate::engine::embed::LoaderFn {
        Arc::new(|_dir: &std::path::Path, id: &str| {
            let inner = Arc::new(bossclaw_core::MockEmbedder::new(8)) as Arc<dyn bossclaw_core::Embedder>;
            Ok(Arc::new(IdReportingEmbedder { inner, id: id.to_string() }) as Arc<dyn bossclaw_core::Embedder>)
        })
    }

    /// Stage a downloadable model folder under `root/<id>` (fake weights) and return `(id, sha256)`
    /// so a signed record / `build_candidate` can bind + verify it.
    fn stage_mock_model(root: &std::path::Path, id: &str) -> (String, String) {
        use sha2::{Digest, Sha256};
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = b"mock-weights";
        std::fs::write(dir.join("model.safetensors"), bytes).unwrap();
        let sha = hex::encode(Sha256::digest(bytes));
        (id.to_string(), sha)
    }

    /// Build one `memory` event (the embed/evolve queue consumes these). Mirrors `seed_one_memory`
    /// but returns the event for the caller to append.
    fn mk_test_memory(text: &str) -> bossclaw_core::Event {
        bossclaw_core::Event {
            id: String::new(), ts: String::new(), valid_time: None,
            event_type: "memory".to_string(),
            content: serde_json::json!({ "text": text }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
        }
    }

    /// Poll until the WHOLE migration has finished for `expected_id` (bounded). The signed record
    /// flips to `Complete` at the commit point, but the GC + index rebuild run AFTER it; the provider
    /// clears its re-index progress to `None` only once the whole migration finishes, so gate on both
    /// to never observe a half-done (record Complete, old rows not yet GC'd) state.
    async fn wait_until_active(handle: &Arc<EngineHandle>, expected_id: &str) {
        for _ in 0..200 {
            let rec = handle.get_or_open(true).await.unwrap().language_pack_record().unwrap();
            let complete = matches!(&rec, Some(r)
                if r.migration == bossclaw_core::MigrationState::Complete && r.model_id == expected_id);
            if complete && handle.model_state().1.is_none() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("migration did not complete within the bound");
    }

    /// I5 enable path: `set_active_model` drives the migration to completion — new-id vectors cover
    /// every event, the old model's rows are GC'd, and the signed record ends `Complete`.
    #[tokio::test]
    async fn set_active_model_migrates_to_completion() {
        use crate::engine::embed::ResourceModel2Vec;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("models");
        std::fs::create_dir_all(root.join("potion-base-8M")).unwrap();
        let (id, sha) = stage_mock_model(&root, "ml/v1");
        let provider = std::sync::Arc::new(
            ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, crate::engine::embed::MODEL_ID.to_string())
                .with_loader_for_test(mock_loader_reporting_ids()),
        );
        let handle = test_handle_with_provider(tmp.path().to_path_buf(), provider);
        let log = handle.get_or_open(true).await.unwrap();
        for t in ["ocean waves", "forest trees"] { log.append(mk_test_memory(t)).unwrap(); }
        handle.run_ingest(true).await.unwrap(); // records the bundled model; seeds any English vectors

        handle.set_active_model(true, id.clone(), sha).await.unwrap();
        // set_active_model spawns a background task; await completion via the status poll.
        wait_until_active(&handle, &id).await;

        assert_eq!(log.vectors_for_model(&id).unwrap().len(), 2, "new-id vectors cover all events");
        assert!(log.vectors_for_model(crate::engine::embed::MODEL_ID).unwrap().is_empty(), "old GC'd");
        assert_eq!(log.language_pack_record().unwrap().unwrap().migration, bossclaw_core::MigrationState::Complete);
    }

    /// U6 (review MAJOR): `model_status` must report WHICH model is served so the Settings card can
    /// show "Multilingual active" instead of a stale Enable button. Before any enable it is the bundled
    /// English base id (an absent record keeps English serving); after a completed migration it is the
    /// migrated model id (the `Complete` record's id).
    #[tokio::test]
    async fn model_status_reports_active_model_id() {
        use crate::engine::embed::ResourceModel2Vec;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("models");
        std::fs::create_dir_all(root.join("potion-base-8M")).unwrap();
        let (id, sha) = stage_mock_model(&root, "ml/v1");
        let provider = std::sync::Arc::new(
            ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, crate::engine::embed::MODEL_ID.to_string())
                .with_loader_for_test(mock_loader_reporting_ids()),
        );
        let handle = test_handle_with_provider(tmp.path().to_path_buf(), provider);
        let log = handle.get_or_open(true).await.unwrap();
        for t in ["ocean waves", "forest trees"] { log.append(mk_test_memory(t)).unwrap(); }
        handle.run_ingest(true).await.unwrap();

        // Before enable: the bundled English base id (nothing has been migrated yet).
        assert_eq!(handle.model_status(true).await.2, crate::engine::embed::MODEL_ID,
            "with no language pack, model_status reports the bundled English base id");

        handle.set_active_model(true, id.clone(), sha).await.unwrap();
        wait_until_active(&handle, &id).await;

        // After a completed migration: the migrated model id (the served model).
        assert_eq!(handle.model_status(true).await.2, id,
            "after the migration completes, model_status reports the migrated model id");
    }

    /// U6 (review MAJOR): re-enabling the ALREADY-ACTIVE model must be a no-op — no re-download, no
    /// re-migration. `set_active_model` writes a fresh `InProgress` record SYNCHRONOUSLY before it
    /// returns whenever it starts a migration, so a record still `Complete` immediately after the call
    /// proves the migration never spawned; and the served vectors are left untouched.
    #[tokio::test]
    async fn set_active_model_is_idempotent_for_the_active_model() {
        use crate::engine::embed::ResourceModel2Vec;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("models");
        std::fs::create_dir_all(root.join("potion-base-8M")).unwrap();
        let (id, sha) = stage_mock_model(&root, "ml/v1");
        let provider = std::sync::Arc::new(
            ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root, crate::engine::embed::MODEL_ID.to_string())
                .with_loader_for_test(mock_loader_reporting_ids()),
        );
        let handle = test_handle_with_provider(tmp.path().to_path_buf(), provider);
        let log = handle.get_or_open(true).await.unwrap();
        log.append(mk_test_memory("ocean waves")).unwrap();
        handle.run_ingest(true).await.unwrap();

        // First enable migrates to completion (record Complete, state Ok, id's vectors written).
        handle.set_active_model(true, id.clone(), sha.clone()).await.unwrap();
        wait_until_active(&handle, &id).await;
        let vectors_before = log.vectors_for_model(&id).unwrap().len();

        // Second enable of the SAME active model: the short-circuit returns Ok without migrating.
        handle.set_active_model(true, id.clone(), sha.clone()).await.unwrap();
        assert_eq!(
            log.language_pack_record().unwrap().unwrap().migration,
            bossclaw_core::MigrationState::Complete,
            "a re-enable of the active model must NOT write a fresh InProgress record (no re-migration)",
        );
        assert_eq!(log.vectors_for_model(&id).unwrap().len(), vectors_before,
            "no re-embed ran on the redundant re-enable — the served vectors are untouched");
    }

    /// I6: a bare "zero vectors for the loaded model" state must NOT auto-migrate — only an explicit
    /// `set_active_model` writes a language-pack record. A recall over an un-consented store leaves
    /// the record absent.
    #[tokio::test]
    async fn zero_vectors_never_auto_migrates() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_handle_with_provider(
            tmp.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(8)),
        );
        let log = handle.get_or_open(true).await.unwrap();
        log.append(mk_test_memory("lonely event")).unwrap();
        // No set_active_model call. A recall must NOT write a language_pack record.
        let _ = handle.recall(true, "lonely".into(), 5).await;
        assert!(log.language_pack_record().unwrap().is_none(), "no consent → no migration record (I6)");
    }

    /// I6: an interrupted-but-consented migration (InProgress record + partial vectors) resumes on
    /// boot via the SAME all-or-nothing flow, finishing the re-embed and flipping to Complete.
    #[tokio::test]
    async fn interrupted_migration_resumes_on_boot() {
        use crate::engine::embed::ResourceModel2Vec;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("models");
        std::fs::create_dir_all(root.join("potion-base-8M")).unwrap();
        let (id, sha) = stage_mock_model(&root, "ml/v1");
        // The two handles share ONE vault so the boot handle decrypts the crash handle's brain.db.
        let vault = TestVault::new();
        // Simulate a crash mid-migration: InProgress record written, English vectors intact, NO new.
        {
            let provider = std::sync::Arc::new(
                ResourceModel2Vec::new(root.join("potion-base-8M")).with_loader_for_test(mock_loader_reporting_ids()),
            );
            let handle = test_handle_with_vault_and_provider(vault.clone(), tmp.path().to_path_buf(), provider);
            let log = handle.get_or_open(true).await.unwrap();
            log.append(mk_test_memory("resume me")).unwrap();
            handle.run_ingest(true).await.unwrap();
            log.set_language_pack_record(&bossclaw_core::LanguagePackRecord {
                model_id: id.clone(), safetensors_sha: sha.clone(),
                migration: bossclaw_core::MigrationState::InProgress, consented_at: "t".into(),
            }).unwrap();
        }
        // Fresh handle (new process) with the resolution-aware provider → boot resume.
        let provider = std::sync::Arc::new(
            ResourceModel2Vec::with_resolution(None, root.join("potion-base-8M"), root.clone(), crate::engine::embed::MODEL_ID.to_string())
                .with_loader_for_test(mock_loader_reporting_ids()),
        );
        let handle = test_handle_with_vault_and_provider(vault.clone(), tmp.path().to_path_buf(), provider);
        handle.resume_migration_if_pending(true).await;
        wait_until_active(&handle, &id).await;
        let log = handle.get_or_open(true).await.unwrap();
        assert_eq!(log.language_pack_record().unwrap().unwrap().migration, bossclaw_core::MigrationState::Complete);
        assert_eq!(log.vectors_for_model(&id).unwrap().len(), 1, "resume finished the re-embed");
    }

    /// Regression guard (review MAJOR): the embedder swap and the recall-index invalidation MUST
    /// happen in the SAME step, never separated. A prior version invalidated the index only AFTER
    /// `reembed_finalize_gc`, leaving a window where a racing recall got the NEW embedder from the
    /// provider's cache (swapped by `publish`) but skipped rebuilding because `indexed` was still
    /// `true` — searching the new-model query embedding against the OLD in-memory vector index
    /// (cross-embedding-space garbage), for the whole GC+rebuild duration, and permanently if the GC
    /// then failed. Pins `publish_and_invalidate` as the ONLY path `run_language_migration` uses to
    /// swap the embedder, so a future reorder that calls `publish` and the index-reset separately
    /// cannot silently reintroduce the window.
    #[tokio::test]
    async fn publish_and_invalidate_clears_the_index_atomically_with_the_swap() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_handle_with_provider(
            tmp.path().to_path_buf(),
            Arc::new(embed::MockEmbedderProvider::new(8)),
        );
        // Simulate an already-built index under the OLD model (what a prior recall would have set).
        *handle.indexed.lock().await = true;
        let candidate: Arc<dyn bossclaw_core::Embedder> = Arc::new(bossclaw_core::MockEmbedder::new(8));
        handle.publish_and_invalidate(candidate).await;
        assert!(
            !*handle.indexed.lock().await,
            "the index must be invalidated in the SAME step as the embedder swap — never a step \
             later — so no racing recall can pair the NEW embedder with the OLD in-memory index"
        );
    }

    /// A provider whose `build_candidate` succeeds the FIRST time — so `set_active_model`'s
    /// synchronous pre-check passes and the InProgress record is written — then FAILS every later
    /// call, so the background migration's OWN `build_candidate` errors. Drives the failure catch
    /// that must surface `ModelState::Failed` (old model still serving) while leaving the signed
    /// record InProgress (retryable).
    struct FailSecondCandidateProvider {
        candidate_calls: std::sync::atomic::AtomicUsize,
        state: Mutex<embed::ModelState>,
        reindex: Mutex<Option<(u64, u64)>>,
    }
    impl FailSecondCandidateProvider {
        fn new() -> Self {
            Self {
                candidate_calls: std::sync::atomic::AtomicUsize::new(0),
                state: Mutex::new(embed::ModelState::Ok),
                reindex: Mutex::new(None),
            }
        }
    }
    impl embed::EmbedderProvider for FailSecondCandidateProvider {
        fn embedder(&self) -> Result<Arc<dyn bossclaw_core::Embedder>, EngineOpError> {
            Ok(Arc::new(bossclaw_core::MockEmbedder::new(8)))
        }
        fn build_candidate(
            &self,
            _model_id: &str,
            _sha: &str,
        ) -> Result<Arc<dyn bossclaw_core::Embedder>, EngineOpError> {
            if self.candidate_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(Arc::new(bossclaw_core::MockEmbedder::new(8)))
            } else {
                Err(EngineOpError::Embedder("candidate build failed (test)".into()))
            }
        }
        fn model_state(&self) -> embed::ModelState {
            self.state.lock().unwrap().clone()
        }
        fn set_failed(&self, reason: String) {
            *self.state.lock().unwrap() = embed::ModelState::Failed { reason };
        }
        fn set_reindex(&self, progress: Option<(u64, u64)>) {
            *self.reindex.lock().unwrap() = progress;
        }
        fn reindex(&self) -> Option<(u64, u64)> {
            *self.reindex.lock().unwrap()
        }
    }

    /// A background migration that fails must surface `ModelState::Failed` via `model_status` (so the
    /// UI can tell "migration failed, old model still serving" apart from "migration still running"),
    /// while leaving the signed record InProgress (retryable) and clearing the stale progress bar.
    #[tokio::test]
    async fn failed_migration_surfaces_failed_state() {
        let tmp = tempfile::tempdir().unwrap();
        let handle =
            test_handle_with_provider(tmp.path().to_path_buf(), Arc::new(FailSecondCandidateProvider::new()));
        let log = handle.get_or_open(true).await.unwrap();
        log.append(mk_test_memory("event")).unwrap();
        handle.run_ingest(true).await.unwrap();

        // The synchronous pre-check (build_candidate #1) passes and the InProgress record is written;
        // the background migration's own build_candidate (#2) then fails.
        handle.set_active_model(true, "ml/v1".into(), "sha".into()).await.unwrap();

        // Poll until the background task reports the failure through `model_status`.
        let mut failed = false;
        for _ in 0..200 {
            if matches!(handle.model_status(true).await.0, embed::ModelState::Failed { .. }) {
                failed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(failed, "a failed migration must surface ModelState::Failed via model_status");
        // The record stays InProgress (retryable) — only the reported state changed.
        assert_eq!(
            log.language_pack_record().unwrap().unwrap().migration,
            bossclaw_core::MigrationState::InProgress,
            "the signed record is left InProgress on failure (retryable)",
        );
        // The stale progress bar is cleared alongside the failure.
        assert!(handle.model_status(true).await.1.is_none(), "progress cleared on failure");
    }
}
