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
//! decision). The confidentiality control here is the `0600`/`0700` owner-only discipline,
//! re-implemented std-only (no dependency on the desktop crate, mirroring the SP2
//! `atomic_write_0600`/`make_private_dir` semantics).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::capture::paths::valid_session_id;
use crate::capture::render::Rendered;
use crate::engine::{EngineHandle, EngineOpError};
use bossclaw_core::log::{CurrentSession, SessionMeta};

/// Session identity the renderer never sees (derived by the caller: the sweeper from the
/// transcript path, dispatch from the `CaptureNotify` request). `session_id` is A5-validated
/// at every store entry point (defense in depth; A5 contract D1).
pub struct CaptureIdentity {
    /// The stable per-session key (A5 allowlist: `[A-Za-z0-9_-]`, ≤128 bytes).
    pub session_id: String,
    /// The project/repo the session ran against.
    pub project: String,
    /// The coding agent that produced the session (e.g. `claude-code`).
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
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue; // unreadable — leave it, don't abort the heal
            };
            let Some(meta) = parse_captured_meta(&text, &path) else {
                continue; // malformed front-matter — not a healable orphan, skip
            };
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
    body: &str,
) -> String {
    let mut md = String::with_capacity(body.len() + 320);
    md.push_str("---\n");
    md.push_str(&format!("approx_bytes: {approx_bytes}\n"));
    md.push_str(&format!("ended_at: {ended_at}\n"));
    md.push_str(&format!("lines_oversized: {oversized_lines}\n"));
    md.push_str(&format!("lines_skipped: {skipped_unknown}\n"));
    md.push_str(&format!("project: {}\n", one_line(project)));
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

/// The recovery stub for heal window (b): the same front-matter schema, rebuilt from the signed
/// event's metadata, with a body that HONESTLY marks the transcript as unrecovered (it invents no
/// content). Render-only diagnostics (`lines_oversized`/`lines_skipped`/`torn_tail`) aren't carried
/// on the event, so they default here; the next sweep replaces the whole file if the source exists.
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

// ── 0600/0700 discipline, re-implemented std-only (mirrors SP2 integrations::mod) ────────────

/// Atomically write `bytes` to `target` at mode `0600` with NO world-readable window: create a
/// temp file in the SAME dir **born 0600** (the `O_CREAT` creation mode — NOT a chmod-after, so
/// the file is never briefly world-readable), fsync, then `rename` over the target (atomic within
/// one filesystem; symlink-safe — `rename` replaces the target, it does not write through a link).
/// I2. Std-only, zero new deps — mirrors the reviewed SP2 `atomic_write_0600`.
fn atomic_write_0600(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("capture");
    let tmp = dir.join(format!(
        ".{name}.air-tmp.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let write = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true) // fail if the temp name exists → never write into another file
            .mode(0o600) // born 0600, not chmod-after
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    if let Err(e) = write().and_then(|()| std::fs::rename(&tmp, target)) {
        let _ = std::fs::remove_file(&tmp); // best-effort: never leave a temp behind
        return Err(e);
    }
    Ok(())
}

/// Create `dir` (and parents) at mode `0700` **only when we create it**; a pre-existing dir keeps
/// its perms untouched (mirrors SP2 `make_private_dir`). The daemon owns the data dir, so the
/// `sessions/` subdir is born `0700` on the first capture.
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.exists() {
        return Ok(());
    }
    std::fs::DirBuilder::new().mode(0o700).recursive(true).create(dir)
}
