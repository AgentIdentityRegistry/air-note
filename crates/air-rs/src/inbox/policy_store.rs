//! `{home}/agent-policy.json` — the per-contact autonomy dial (design §7). Written ONLY by the
//! desktop (one writer per file; `contacts.json` is the CLI's). Corrupt/missing → all draft.
//!
//! Phase B adds the security core of the AI auto-send loop: [`decide`] (pure, testable) plus
//! [`reserve`]/[`confirm`]/[`cancel`], which serialize the whole load→decide→write through a
//! module-level [`POLICY_LOCK`] so two concurrent auto-replies can never both pass the same budget
//! cap (the C2 race fix). Budgets count *pending* reservations as well as confirmed sends, so the
//! gate is honored even before an ack lands.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

/// Per-contact AI autonomy level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    /// The AI does nothing for this contact.
    Off,
    /// The AI drafts a reply for human review (the default).
    #[default]
    Draft,
    /// The AI may auto-send (subject to Phase B loop guards).
    Auto,
}

/// One auto-send record — both the *reservation* (between [`reserve`] and [`confirm`]) and the
/// confirmed send afterward. Lives in [`ContactPolicy::auto_sends`]; drives both the structural
/// guard (D2) and the budget caps in [`decide`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoSend {
    /// Reservation id (uuid). The confirm/cancel key.
    pub token: String,
    /// The real envelope_id — empty until [`confirm`] fills it.
    #[serde(default)]
    pub envelope_id: String,
    /// The thread this auto-send belongs to (structural guard D2). Empty if unthreaded.
    pub thread_id: String,
    /// ISO-8601 UTC reserve time (RFC 3339). The recency/budget/structural clock.
    pub at: String,
    /// True from [`reserve`] until [`confirm`]; a stale pending (>15m) is crash debris.
    #[serde(default)]
    pub pending: bool,
}

/// Policy for one contact.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContactPolicy {
    /// The autonomy dial for this contact.
    #[serde(default)]
    pub ai_autonomy: Autonomy,
    /// Confirmed auto-sent envelope_ids (append-only audit trail, capped at 500).
    #[serde(default)]
    pub auto_ledger: Vec<String>,
    /// Structural + budget records: pending reservations AND confirmed sends.
    #[serde(default)]
    pub auto_sends: Vec<AutoSend>,
}

/// The whole policy file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Schema version.
    pub version: u32,
    /// Per-contact policies keyed by DID.
    #[serde(default)]
    pub contacts: HashMap<String, ContactPolicy>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            version: 1,
            contacts: HashMap::new(),
        }
    }
}

fn policy_path(home: &Path) -> std::path::PathBuf {
    home.join("agent-policy.json")
}

/// Read the policy; a missing OR corrupt file yields the safe default (everything = draft).
pub fn load(home: &Path) -> Policy {
    std::fs::read_to_string(policy_path(home))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// The dial for one contact — default `Draft` when absent (design §7).
pub fn autonomy_for(home: &Path, did: &str) -> Autonomy {
    load(home)
        .contacts
        .get(did)
        .map(|c| c.ai_autonomy)
        .unwrap_or_default()
}

/// Set a contact's dial and persist 0600. Returns the written Policy.
///
/// Takes [`POLICY_LOCK`] so a dial change can't interleave with a [`reserve`] and corrupt the file
/// or lose a reservation. The whole load→modify→write is one critical section.
pub fn set_autonomy(home: &Path, did: &str, value: Autonomy) -> std::io::Result<Policy> {
    let _g = POLICY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_autonomy_locked(home, did, value)
}

/// The non-locking body of [`set_autonomy`]. The caller MUST already hold [`POLICY_LOCK`]
/// (`std::sync::Mutex` is not reentrant — never re-acquire it here).
fn set_autonomy_locked(home: &Path, did: &str, value: Autonomy) -> std::io::Result<Policy> {
    let mut p = load(home);
    p.contacts.entry(did.to_string()).or_default().ai_autonomy = value;
    write_atomic(home, &p)?;
    Ok(p)
}

/// Reset a contact's dial to [`Autonomy::Draft`] (design §7 key-rotation mandate: a re-pin must
/// force a deliberate human re-arm against the new key). Same locked path as [`set_autonomy`].
pub fn reset_to_draft(home: &Path, did: &str) -> std::io::Result<Policy> {
    let _g = POLICY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_autonomy_locked(home, did, Autonomy::Draft)
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Phase B — the AI auto-send security core.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Max auto-sends to one contact per rolling hour (D1).
pub const PER_CONTACT_HOURLY: usize = 3;
/// Max auto-sends to one contact per rolling 24h (D1).
pub const PER_CONTACT_DAILY: usize = 10;
/// Max auto-sends across ALL contacts per rolling 24h (D1).
pub const GLOBAL_DAILY: usize = 30;
/// D2: after auto-replying in a thread, pause auto for 30 min in that thread.
const STRUCTURAL_WINDOW_SECS: i64 = 1800;
/// D11: never auto-reply to a message older than 10 min (replay defense).
const AUTO_RECENCY_SECS: i64 = 600;
/// Crash-safety: a pending reservation older than 15 min is debris and is pruned.
const STALE_PENDING_SECS: i64 = 900;
/// Retention: drop any auto-send record older than 24h.
const RETENTION_SECS: i64 = 86400;
/// Cap on the confirmed-envelope audit ledger per contact.
const LEDGER_CAP: usize = 500;

/// `(now - at) <= secs`, INCLUSIVE. Both args are RFC-3339 timestamps; an unparseable `at` (debris)
/// is treated as out-of-window (`false`) so it never counts toward a cap or a guard.
fn within(at: &str, now: &str, secs: i64) -> bool {
    let (at, now) = match (
        chrono::DateTime::parse_from_rfc3339(at),
        chrono::DateTime::parse_from_rfc3339(now),
    ) {
        (Ok(at), Ok(now)) => (at, now),
        _ => return false,
    };
    let delta = now.signed_duration_since(at).num_seconds();
    delta <= secs
}

/// A fresh reservation token.
fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The pure decision (no I/O). Counts *pending + confirmed* auto-sends so a reservation is honored
/// before its ack lands. `received_at` is the incoming message time (recency D11); `now` is the
/// decision time. Returns `(decision, reason)`.
pub fn decide(
    p: &Policy,
    did: &str,
    thread_id: Option<&str>,
    received_at: &str,
    now: &str,
) -> (Autonomy, &'static str) {
    let dial = p
        .contacts
        .get(did)
        .map(|c| c.ai_autonomy)
        .unwrap_or_default();
    match dial {
        Autonomy::Off => (Autonomy::Off, "dial off"),
        Autonomy::Draft => (Autonomy::Draft, "dial draft"),
        Autonomy::Auto => {
            // D11: don't auto-reply to stale mail (replay / backlog flush).
            if !within(received_at, now, AUTO_RECENCY_SECS) {
                return (Autonomy::Draft, "auto skipped: message too old (replay)");
            }
            // D2: one auto-reply per thread per structural window.
            if let (Some(tid), Some(c)) = (thread_id, p.contacts.get(did)) {
                if c.auto_sends
                    .iter()
                    .any(|a| a.thread_id == tid && within(&a.at, now, STRUCTURAL_WINDOW_SECS))
                {
                    return (
                        Autonomy::Draft,
                        "auto paused: just auto-replied in this thread",
                    );
                }
            }
            // Budget caps (D1) — pending reservations count, which is what blocks a concurrent burst.
            let per: Vec<&AutoSend> = p
                .contacts
                .get(did)
                .map(|c| c.auto_sends.iter().collect())
                .unwrap_or_default();
            let hourly = per.iter().filter(|a| within(&a.at, now, 3600)).count();
            let daily = per
                .iter()
                .filter(|a| within(&a.at, now, RETENTION_SECS))
                .count();
            let global = p
                .contacts
                .values()
                .flat_map(|c| &c.auto_sends)
                .filter(|a| within(&a.at, now, RETENTION_SECS))
                .count();
            if hourly >= PER_CONTACT_HOURLY {
                return (Autonomy::Draft, "auto paused: hourly cap");
            }
            if daily >= PER_CONTACT_DAILY {
                return (Autonomy::Draft, "auto paused: daily cap");
            }
            if global >= GLOBAL_DAILY {
                return (Autonomy::Draft, "auto paused: global cap");
            }
            (Autonomy::Auto, "auto")
        }
    }
}

/// One writer, one lock: every mutator of `agent-policy.json` serializes through this. The lock
/// guards the WHOLE load→decide→write critical section in [`reserve`], which is what makes two
/// concurrent reserves unable to both pass the same cap (C2). NEVER re-acquire while held —
/// `std::sync::Mutex` is not reentrant; the locked mutators call the non-locking [`load`] /
/// [`write_atomic`] / [`set_autonomy_locked`] helpers only.
static POLICY_LOCK: Mutex<()> = Mutex::new(());

/// The outcome of [`reserve`]: the decision, a human-readable reason, and (if `Auto`) the token
/// that [`confirm`]/[`cancel`] use to finalize or drop the reservation.
pub struct Reservation {
    /// What the loop should do: `Auto` = send now, anything else = don't.
    pub decision: Autonomy,
    /// Why — surfaced to the UI/log.
    pub reason: String,
    /// `Some` iff `decision == Auto`; the reservation key.
    pub token: Option<String>,
}

/// Atomically decide against the CURRENT on-disk state (pending reservations included) and, if the
/// answer is `Auto`, persist a pending reservation before returning. The lock spans the entire
/// load→decide→write, so two concurrent callers can't both slip past a cap (C2).
pub fn reserve(
    home: &Path,
    did: &str,
    thread_id: Option<&str>,
    received_at: &str,
    now: &str,
) -> std::io::Result<Reservation> {
    let _g = POLICY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = load(home);
    let (decision, reason) = decide(&p, did, thread_id, received_at, now);
    if decision == Autonomy::Auto {
        let token = new_uuid();
        p.contacts
            .entry(did.to_string())
            .or_default()
            .auto_sends
            .push(AutoSend {
                token: token.clone(),
                envelope_id: String::new(),
                thread_id: thread_id.unwrap_or("").to_string(),
                at: now.to_string(),
                pending: true,
            });
        prune(&mut p, now);
        write_atomic(home, &p)?;
        return Ok(Reservation {
            decision,
            reason: reason.into(),
            token: Some(token),
        });
    }
    Ok(Reservation {
        decision,
        reason: reason.into(),
        token: None,
    })
}

/// Finalize a reservation with the real `envelope_id` (on `inbox_send_ok`). If the pending record
/// was already pruned before the (slow) ack arrived, RE-INSERT a confirmed record (I-A) so a real
/// auto-send is never lost from the ledger OR the structural guard. Also appends to the capped
/// audit ledger.
pub fn confirm(
    home: &Path,
    did: &str,
    token: &str,
    envelope_id: &str,
    thread_id: &str,
    now: &str,
) -> std::io::Result<()> {
    let _g = POLICY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = load(home);
    let c = p.contacts.entry(did.to_string()).or_default();
    match c.auto_sends.iter_mut().find(|a| a.token == token) {
        Some(a) => {
            a.pending = false;
            a.envelope_id = envelope_id.to_string();
        }
        // Pruned before the ack landed → restore a confirmed record (I-A).
        None => c.auto_sends.push(AutoSend {
            token: token.to_string(),
            envelope_id: envelope_id.to_string(),
            thread_id: thread_id.to_string(),
            at: now.to_string(),
            pending: false,
        }),
    }
    c.auto_ledger.push(envelope_id.to_string());
    if c.auto_ledger.len() > LEDGER_CAP {
        let drop = c.auto_ledger.len() - LEDGER_CAP;
        c.auto_ledger.drain(0..drop);
    }
    write_atomic(home, &p)
}

/// Drop a reservation (on send_err / llm_error / discard) so it never counts against budget.
/// M-A: only ever removes a STILL-PENDING reservation, so a late cancel for a token that has since
/// been confirmed can't delete the confirmed send.
pub fn cancel(home: &Path, did: &str, token: &str) -> std::io::Result<()> {
    let _g = POLICY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = load(home);
    if let Some(c) = p.contacts.get_mut(did) {
        c.auto_sends.retain(|a| a.token != token || !a.pending);
    }
    write_atomic(home, &p)
}

/// D2 retention + crash-safety: keep records ≤24h old, and additionally drop a PENDING record whose
/// reservation has gone stale (>15m) — that's debris from a crash between reserve and confirm.
fn prune(p: &mut Policy, now: &str) {
    for c in p.contacts.values_mut() {
        c.auto_sends.retain(|a| {
            let within_retention = within(&a.at, now, RETENTION_SECS);
            // A pending reservation only survives while it's still fresh; confirmed records always do.
            let not_stale_pending = !a.pending || within(&a.at, now, STALE_PENDING_SECS);
            within_retention && not_stale_pending
        });
    }
}

fn write_atomic(home: &Path, p: &Policy) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let path = policy_path(home);
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(p).expect("policy serializes");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.flush()?;
    }
    set_0600(&tmp);
    std::fs::rename(&tmp, &path)?;
    set_0600(&path);
    Ok(())
}

#[cfg(unix)]
fn set_0600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn set_0600(_path: &Path) {}
