//! A7: the daemon's capture STORE — where a rendered Claude Code transcript lands on
//! disk and gets tied to a signed engine event.
//!
//! # The write, in order (spec §4b crash consistency)
//! [`store_capture`] does **file-THEN-event**:
//! 1. validate the session id (A5 D1 — before ANY path is built);
//! 2. compose the on-disk document = ONE coherent, alphabetically-sorted front-matter
//!    block (the session fields the renderer never knew — `session_id`/`project`/`tool`
//!    — merged with the renderer's own `sha256`/timestamps/diagnostics) + the renderer's
//!    body ([`Rendered::body`]);
//! 3. atomic-write it `0600` under a `0700` `sessions/` dir (born-private temp + `rename`,
//!    NO world-readable window — I2);
//! 4. THEN append the signed, external-tainted `session_captured` event (A2
//!    `capture_session`), recording the `.md` path + the render's `sha256` as metadata.
//!
//! If the process dies between (3) and (4) the file exists with no event; that is the
//! window [`heal_orphans`] reconciles. The reverse window — an event whose `.md` was
//! deleted out of band — is reconciled by regenerating the file from the signed event
//! (see [`heal_orphans`]).
//!
//! # Plaintext at rest
//! The `.md` body is written in the clear (spec §4b — the deliberate, later-disclosed
//! decision). The confidentiality control here is the `0600`/`0700` owner-only discipline via
//! `bossclawd_paths::{atomic_write_0600, make_private_dir}` — the SINGLE shared copy of that
//! security-critical primitive (the desktop SP2 config-writer uses the same one, so the two
//! can never drift).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::capture::paths::valid_session_id;
use crate::capture::render::Rendered;
use crate::engine::{EngineHandle, EngineOpError};
use bossclaw_core::log::{CurrentSession, SessionMeta};
// The 0600/0700 discipline is a SECURITY-critical primitive shared with the desktop SP2
// config-writer; it lives ONCE in `bossclawd-paths` (single source of truth, no drift).
use bossclawd_paths::{atomic_write_0600, make_private_dir};

/// The coding agent that produces the captured transcripts — the canonical value for
/// [`CaptureIdentity::tool`]. Lives here (beside the field it fills) so BOTH capture paths — the
/// sweeper (A9) and the immediate `CaptureNotify` dispatch (A10) — tag `tool` identically, without
/// either path importing the const from the other.
pub const CAPTURE_TOOL: &str = "claude-code";

/// Session identity the renderer never sees (derived by the caller: the sweeper from the
/// transcript path, dispatch from the `CaptureNotify` request). `session_id` is A5-validated
/// at every store entry point (defense in depth; A5 contract D1).
pub struct CaptureIdentity {
    /// The stable per-session key (A5 allowlist: `[A-Za-z0-9_-]`, ≤128 bytes).
    pub session_id: String,
    /// The project/repo the session ran against.
    pub project: String,
    /// The coding agent that produced the session (e.g. [`CAPTURE_TOOL`]).
    pub tool: String,
}

/// What [`heal_orphans`] reconciled this pass. Both counts are 0 on a consistent store.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HealReport {
    /// Window (a): `sessions/*.md` files that had no engine event and were captured.
    pub orphan_files_captured: usize,
    /// Window (b): current events whose `.md` was missing and was regenerated.
    pub dangling_events_regenerated: usize,
}

/// Why a capture-store operation could not complete.
#[derive(Debug, Error)]
pub enum CaptureStoreError {
    /// The session id failed the A5 allowlist — refused before any path is built (D1). The
    /// hostile id is deliberately NOT echoed (no attacker-influenced input in logs).
    #[error("invalid session id (rejected by the capture-path allowlist)")]
    InvalidSessionId,
    /// A filesystem error writing/removing the `.md` or creating its dir.
    #[error("capture store i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The engine refused or failed to record the `session_captured` event / tombstone.
    #[error("engine error: {0}")]
    Engine(String),
}

/// `<data_dir>/sessions` — the single home for capture `.md` files.
fn sessions_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sessions")
}

/// Upper bound on the bytes [`read_capture_markdown`] returns for the `GetSession` detail
/// view. Comfortably above any realistic rendered session yet well under the 32 MiB wire
/// frame cap, so the `SessionDetailWire` always encodes — a pathologically large `.md`
/// (e.g. a tampered file) is truncated (lossy) rather than allowed to blow the frame/memory.
const MAX_CAPTURE_MD_BYTES: u64 = 16 * 1024 * 1024;

/// Read a capture `.md` in full (bounded to [`MAX_CAPTURE_MD_BYTES`]) for the App-only
/// `GetSession` detail view. The path is daemon-authored ([`store_capture`]/[`heal_orphans`]
/// write it under the `0700` `sessions/` dir) and the caller has A5-validated the session id,
/// so this is a plain bounded read — NOT a confinement boundary (that is `open_transcript_confined`
/// for attacker-supplied transcript paths). Lossy-UTF8 so a byte-mangled file still renders.
pub fn read_capture_markdown(md_path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(md_path)?.take(MAX_CAPTURE_MD_BYTES).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Store a rendered capture: validate → compose → atomic-write `0600` under a `0700` dir →
/// signed event (file-THEN-event, spec §4b). Idempotent: A2's same-`sha256` dedup makes a
/// repeat store a no-op (the atomic rename is safe to redo). See the module docs.
pub async fn store_capture(
    engine: &EngineHandle,
    data_dir: &Path,
    id: &CaptureIdentity,
    r: &Rendered,
) -> Result<(), CaptureStoreError> {
    // (1) A5 D1: validate the session id BEFORE building any path (defense in depth).
    if !valid_session_id(&id.session_id) {
        return Err(CaptureStoreError::InvalidSessionId);
    }
    let sessions = sessions_dir(data_dir);
    let md_path = sessions.join(format!("{}.md", id.session_id));

    // (2) compose the merged document (one sorted front-matter block + the renderer's body).
    let doc = compose_document(
        &id.session_id,
        &r.title,
        &id.project,
        &id.tool,
        &r.sha256,
        r.started_at,
        r.ended_at,
        r.approx_bytes,
        r.oversized_lines,
        r.skipped_unknown,
        r.dropped_torn_tail,
        /*recovered_stub=*/ false, // a real render — omit the `recovered` key.
        &r.body,
    );

    // (3) atomic-write it 0600 under a 0700 sessions dir (born-private, temp+rename — I2).
    make_private_dir(&sessions)?;
    atomic_write_0600(&md_path, doc.as_bytes())?;

    // (4) THEN the signed event. On failure the file is LEFT (file-then-event): heal_orphans
    // window (a) appends the event later. SessionMeta.path = the .md; sha256 = the render's.
    let meta = SessionMeta {
        session_id: id.session_id.clone(),
        title: r.title.clone(),
        project: id.project.clone(),
        tool: id.tool.clone(),
        started_at: r.started_at,
        ended_at: r.ended_at,
        path: md_path.to_string_lossy().into_owned(),
        sha256: r.sha256.clone(),
        approx_bytes: r.approx_bytes,
    };
    engine
        .capture_session(meta)
        .await
        .map_err(|e| CaptureStoreError::Engine(e.to_string()))?;
    Ok(())
}

/// Delete a capture (I7, app-only — the caller enforces that at dispatch, not here): remove
/// `<data_dir>/sessions/<sid>.md` (a missing file is OK — already healed / never written) AND
/// append the `session_deleted` tombstone. A tombstone for a session with no current capture
/// (already gone / superseded / never captured) is the engine's `Rejected` — benign here, so
/// deleting twice is an idempotent no-op.
pub async fn delete_capture(
    engine: &EngineHandle,
    data_dir: &Path,
    session_id: &str,
) -> Result<(), CaptureStoreError> {
    if !valid_session_id(session_id) {
        return Err(CaptureStoreError::InvalidSessionId);
    }
    let md_path = sessions_dir(data_dir).join(format!("{session_id}.md"));
    match std::fs::remove_file(&md_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // already gone — fine
        Err(e) => return Err(CaptureStoreError::Io(e)),
    }
    match engine.delete_session(session_id.to_string()).await {
        Ok(_) => Ok(()),
        // No current capture with that id → already tombstoned/gone. Idempotent no-op.
        Err(EngineOpError::Rejected(_)) => Ok(()),
        Err(e) => Err(CaptureStoreError::Engine(e.to_string())),
    }
}

/// Reconcile the two crash windows between the `.md` write and the signed event. Called on
/// daemon boot (A9 wires it) + before each sweep. Idempotent.
///
/// - **(a) a `sessions/*.md` with no current event** (crash after the file write, before the
///   append) → append the event from the file's own front-matter. A file whose session was
///   owner-DELETED (the engine rejects a recapture, I9) is a stale leftover → removed, so
///   `no event ⇔ no file` holds.
/// - **(b) a current event whose `.md` is missing** (an out-of-band deletion; NOT a store crash
///   window, since the file is written first) → **regenerate a minimal `.md` from the signed
///   event's metadata**. The append-only signed event is the durable source of truth and the
///   `.md` is a derived view, so we rebuild the view rather than tombstone a memory the owner
///   never asked to delete (I7 — deletion is owner-commanded). The regenerated file marks the
///   body as unrecovered (it invents no transcript content); it self-heals to the real body if
///   the source transcript is re-swept (store_capture always writes the file first). Either way
///   the event⇔file invariant is restored.
pub async fn heal_orphans(
    engine: &EngineHandle,
    data_dir: &Path,
) -> Result<HealReport, CaptureStoreError> {
    let sessions = sessions_dir(data_dir);
    let mut report = HealReport::default();

    // Which session_ids already have a current event.
    let current = engine
        .current_sessions()
        .await
        .map_err(|e| CaptureStoreError::Engine(e.to_string()))?;
    let current_ids: BTreeSet<&str> = current.iter().map(|c| c.session_id.as_str()).collect();

    // ── Window (a): a sessions/*.md with valid front-matter but no current event.
    if let Ok(entries) = std::fs::read_dir(&sessions) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            // Cheap pre-filter: a capture file is `<session_id>.md`, so a stem already in the
            // current fold is a consistent capture — skip it WITHOUT any read (the common,
            // already-consistent case that dominates the now-timered heal; store_capture always
            // names a file by its own session_id, so the stem IS that session_id).
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if current_ids.contains(stem) {
                    continue;
                }
            }
            // Reconciliation needs ONLY the front-matter (the session_id), so read a BOUNDED
            // prefix — never the up-to-64-MiB body (window-(a) does not touch body content).
            let Ok(text) = read_front_matter_prefix(&path) else {
                continue; // unreadable — leave it, don't abort the heal
            };
            let Some(meta) = parse_captured_meta(&text, &path) else {
                continue; // malformed front-matter — not a healable orphan, skip
            };
            // Defensive re-check on the file's OWN front-matter session_id: a stem⇄front-matter
            // mismatch is only reachable by out-of-band tampering, and this keeps the outcome
            // byte-identical to the pre-optimization read-everything path.
            if current_ids.contains(meta.session_id.as_str()) {
                continue; // already consistent (has an event)
            }
            match engine.capture_session(meta).await {
                Ok(_) => report.orphan_files_captured += 1,
                // Owner-deleted (tombstoned, I9): the leftover .md is stale → remove it.
                Err(EngineOpError::Rejected(_)) => {
                    let _ = std::fs::remove_file(&path);
                }
                Err(e) => return Err(CaptureStoreError::Engine(e.to_string())),
            }
        }
    }

    // ── Window (b): a current event whose .md is missing → regenerate it. Re-read the fold so
    // a file just healed in (a) counts as present (its session is now current AND on disk).
    let current = engine
        .current_sessions()
        .await
        .map_err(|e| CaptureStoreError::Engine(e.to_string()))?;
    for cs in &current {
        if !valid_session_id(&cs.session_id) {
            continue; // defense in depth — never build a path from an invalid id
        }
        let md_path = sessions.join(format!("{}.md", cs.session_id));
        if md_path.exists() {
            continue;
        }
        make_private_dir(&sessions)?;
        atomic_write_0600(&md_path, regenerated_document(cs).as_bytes())?;
        report.dangling_events_regenerated += 1;
    }
    Ok(report)
}

// ── Document composition + parsing ──────────────────────────────────────────────────────

/// Compose the on-disk `.md`: ONE alphabetically-sorted front-matter block (the session fields
/// merged with the renderer's derived fields) + the body. Keys are sorted so the bytes are
/// reproducible. `project`/`title`/`tool` are flattened to a single line so a value can never
/// inject a fake front-matter key (`session_id`/`sha256` are already line-safe by construction —
/// A5-validated / hex).
///
/// Note: the on-disk `title` is a lossy [`one_line`] projection — the signed event stores the raw
/// title, so a crash-heal that reconstructs `SessionMeta` from this file gets the FLATTENED title.
/// This is negligible: dedup/supersede is keyed on `sha256` (never the title), and the title only
/// feeds the recall embedding, where CR/LF → space is inconsequential.
/// The front-matter marker A7's heal-(b) stamps on a RECOVERY STUB — a `.md` regenerated from
/// the signed event when the body was lost. A real [`store_capture`] render OMITS this key
/// entirely (omission ⇔ recovered). A stub carries the event's REAL `sha256` (so dedup/supersede
/// keep working) and gets a fresh mtime from the atomic write, which makes it
/// sha-AND-mtime-indistinguishable from a real render — so this marker is the ONLY signal that
/// tells the sweeper "this body is a placeholder, re-render it when the transcript is present"
/// (SP3 A9). Keyed `recovered` so it sorts between `project` and `session_id` in the one block.
const RECOVERED_STUB_MARKER: &str = "recovered: false";

#[allow(clippy::too_many_arguments)]
fn compose_document(
    session_id: &str,
    title: &str,
    project: &str,
    tool: &str,
    sha256: &str,
    started_at: i64,
    ended_at: i64,
    approx_bytes: u64,
    oversized_lines: u32,
    skipped_unknown: u32,
    torn_tail: bool,
    recovered_stub: bool,
    body: &str,
) -> String {
    let mut md = String::with_capacity(body.len() + 320);
    md.push_str("---\n");
    md.push_str(&format!("approx_bytes: {approx_bytes}\n"));
    md.push_str(&format!("ended_at: {ended_at}\n"));
    md.push_str(&format!("lines_oversized: {oversized_lines}\n"));
    md.push_str(&format!("lines_skipped: {skipped_unknown}\n"));
    md.push_str(&format!("project: {}\n", one_line(project)));
    // A recovery stub declares `recovered: false` (a real render omits the key); in sorted
    // position so the block stays ONE reproducible alphabetically-keyed frame.
    if recovered_stub {
        md.push_str(RECOVERED_STUB_MARKER);
        md.push('\n');
    }
    md.push_str(&format!("session_id: {session_id}\n"));
    md.push_str(&format!("sha256: {sha256}\n"));
    md.push_str(&format!("started_at: {started_at}\n"));
    md.push_str(&format!("title: {}\n", one_line(title)));
    md.push_str(&format!("tool: {}\n", one_line(tool)));
    md.push_str(&format!("torn_tail: {torn_tail}\n"));
    md.push_str("---\n\n");
    md.push_str(body);
    md
}

/// True iff `md_path` is a RECOVERY STUB — its front-matter carries [`RECOVERED_STUB_MARKER`].
/// A cheap, BOUNDED read of only the front-matter block (never the possibly-large body): the
/// sweeper (A9) calls this so a stub whose transcript is still present re-renders on the next
/// sweep instead of being skipped forever (a stub is sha-indistinguishable from a real render).
/// Returns `true` ONLY on a positively-confirmed marker; any read error, an unframed file, or an
/// absent marker → `false` = "treat it as a real render", which makes the file ELIGIBLE for the
/// cheap skip (it does NOT force a re-render). That is safe because the only file that must read
/// as a stub is one this daemon itself just wrote via [`regenerated_document`] — always framed,
/// readable, and carrying the marker — so the `false` branches never hide a genuine stub, and a
/// real render (which has no marker) correctly reads as `false`.
pub fn is_recovery_stub(md_path: &Path) -> bool {
    let Ok(prefix) = read_front_matter_prefix(md_path) else {
        return false;
    };
    match front_matter_block(&prefix) {
        Some(block) => block.lines().any(|line| line == RECOVERED_STUB_MARKER),
        None => false,
    }
}

/// The bytes needed to hold any capture's front-matter block with room to spare — the keys are
/// fixed and short, so the closing `\n---\n` fence always lands well within this prefix. A larger
/// (malformed / non-stub) header simply misses the fence and reads as "not a stub".
const FRONT_MATTER_SCAN_BYTES: u64 = 8 * 1024;

/// Read at most [`FRONT_MATTER_SCAN_BYTES`] from the head of `md_path` (lossy-UTF8) — enough to
/// contain the whole front-matter frame without ever slurping a large body.
fn read_front_matter_prefix(md_path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(md_path)?
        .take(FRONT_MATTER_SCAN_BYTES)
        .read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// The recovery stub for heal window (b): the same front-matter schema, rebuilt from the signed
/// event's metadata, with a body that HONESTLY marks the transcript as unrecovered (it invents no
/// content) AND a `recovered: false` front-matter marker ([`RECOVERED_STUB_MARKER`]). The marker
/// is load-bearing: the stub carries the event's REAL `sha256` and a fresh mtime, so without it the
/// sweeper (A9) could not tell the placeholder from a real render and would skip it forever — the
/// marker is what makes "the next sweep replaces this with the full rendering" actually true.
/// Render-only diagnostics (`lines_oversized`/`lines_skipped`/`torn_tail`) aren't carried on the
/// event, so they default here; the next sweep replaces the whole file if the source exists.
fn regenerated_document(cs: &CurrentSession) -> String {
    let body = "_(session body was not recovered from disk; metadata restored from the signed \
                event. If the source transcript is still present, the next sweep replaces this \
                with the full rendering.)_\n";
    compose_document(
        &cs.session_id,
        &cs.title,
        &cs.project,
        &cs.tool,
        &cs.sha256,
        cs.started_at,
        cs.ended_at,
        cs.approx_bytes,
        0,
        0,
        false,
        /*recovered_stub=*/ true, // a placeholder body → mark it so the sweeper re-renders it.
        body,
    )
}

/// Flatten any CR/LF to a space so a front-matter value stays on one line (prevents a hostile
/// path/title from injecting a fake key). Everything else is preserved.
fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Parse a stored capture's front-matter back into the `SessionMeta` `capture_session` needs
/// (heal window (a)). Returns `None` if the block is absent or a required key is missing/
/// unparseable/an invalid id — a malformed file is skipped by the heal, never fatal. `md_path`
/// (the file's own location) becomes `SessionMeta.path`.
fn parse_captured_meta(md: &str, md_path: &Path) -> Option<SessionMeta> {
    let inner = front_matter_block(md)?;
    let mut map: BTreeMap<&str, &str> = BTreeMap::new();
    for line in inner.lines() {
        if let Some((k, v)) = line.split_once(": ") {
            map.insert(k, v);
        } else if let Some(k) = line.strip_suffix(':') {
            map.insert(k, ""); // an empty value, defensively
        }
    }
    let session_id = (*map.get("session_id")?).to_string();
    // A5 D1: never build/act on an invalid id, even from our own dir (a tampered file).
    if !valid_session_id(&session_id) {
        return None;
    }
    Some(SessionMeta {
        session_id,
        title: (*map.get("title")?).to_string(),
        project: (*map.get("project")?).to_string(),
        tool: (*map.get("tool")?).to_string(),
        started_at: map.get("started_at")?.parse().ok()?,
        ended_at: map.get("ended_at")?.parse().ok()?,
        path: md_path.to_string_lossy().into_owned(),
        sha256: (*map.get("sha256")?).to_string(),
        approx_bytes: map.get("approx_bytes")?.parse().ok()?,
    })
}

/// The text BETWEEN the opening `---\n` and the closing `\n---\n` fence, or `None` if the
/// document is not front-matter-framed. The closing fence's `---` is a full line (our keys never
/// are), so the first `\n---\n` is always the fence — body text after it is never matched first.
fn front_matter_block(md: &str) -> Option<&str> {
    let rest = md.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some(&rest[..end])
}
