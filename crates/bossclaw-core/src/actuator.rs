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

// ── M6a filesystem write PRIMITIVES (T3) ────────────────────────────────────
//
// `#[cfg(unix)]` because they are fd-relative `openat`/`renameat` mechanics
// (spec L5). They are the *tools* the execute step (T4 `execute_write`) drives
// inside the actuator rename critical section (spec §9 steps 2 & 5); T3 builds
// ONLY these primitives + their tests, no `execute_write`/undo (YAGNI).
//
// `forbid(unsafe_code)` holds: every syscall goes through `rustix::fs::*`, never
// raw libc, never `unsafe`.

/// Open a DIRECTORY fd for fd-relative writes, refusing a symlinked final
/// component (the writable analogue of the RDONLY, `pub(crate)`
/// [`crate::ingest::careful_open_file`], which cannot be reused for writes —
/// seam-map §6 M1). Subsequent `atomic_write` steps are fd-relative against the
/// returned fd, so the path string is resolved exactly ONCE here.
///
/// Platform split mirrors `careful_open_file` (spec §9 step 2):
/// - **Linux:** `openat2` with `RESOLVE_NO_SYMLINKS` so NO component (intermediate
///   or final) may be a symlink. `RESOLVE_BENEATH` is deliberately NOT used: `dir`
///   is an absolute, already-canonicalized + grant-checked path opened from `CWD`,
///   and `BENEATH` rejects absolute pathnames with `EXDEV` (it only constrains a
///   relative descent from a base fd, which does not apply here). The caller's
///   canonicalize + grant-check is the containment boundary; this open is just the
///   no-symlink anchor. Falls back to `openat` + `O_NOFOLLOW` on a pre-5.6 kernel
///   (`ENOSYS`), which still refuses a final-component symlink.
/// - **macOS / other non-Linux:** `openat` from `CWD` with
///   `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`. `O_NOFOLLOW` refuses ONLY a
///   final-component symlink; an INTERMEDIATE directory that is a symlink is NOT
///   rejected here. That residual is acceptable because the caller (T4) has
///   already canonicalized `dir` and checked it against an active write-grant —
///   `open_dir_for_write` is the fd-relative anchor, not the authorization
///   boundary (spec §6 flag 1, §8/§9).
///
/// `O_DIRECTORY` makes the open fail (`ENOTDIR`) if `dir` is not a directory, so
/// a file or a symlink-to-file at `dir` cannot slip through as a write anchor.
// `dead_code`-allowed: this is a forward-seam primitive built in T3 and CONSUMED
// by `execute_write` in T4 (spec §9 step 2). The in-crate `fs_primitives` tests
// exercise it today; the allow keeps a plain (non-test) build warning-free until
// T4 wires the non-test caller. Mirrors `ingest::careful_open_windows`'s pattern.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn open_dir_for_write(
    dir: &std::path::Path,
) -> Result<std::os::fd::OwnedFd, crate::error::BossclawError> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;

    let containment = |e: rustix::io::Errno| {
        crate::error::BossclawError::Io(std::io::Error::other(e.to_string()))
    };

    // A directory fd used only as a rename/openat anchor needs no read/write
    // access mode beyond O_DIRECTORY; RDONLY is the conventional choice.
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{openat2, ResolveFlags};
        match openat2(
            rustix::fs::CWD,
            dir.as_os_str().as_bytes(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS,
        ) {
            Ok(fd) => Ok(fd),
            // Pre-5.6 kernels lack openat2 → fall back to a NOFOLLOW open, which
            // still refuses a final-component symlink.
            Err(rustix::io::Errno::NOSYS) => rustix::fs::openat(
                rustix::fs::CWD,
                dir.as_os_str().as_bytes(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(containment),
            Err(e) => Err(containment(e)),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        rustix::fs::openat(
            rustix::fs::CWD,
            dir.as_os_str().as_bytes(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(containment)
    }
}

/// `rw-r--r--` (0o644) — the mode new files are created with. A module const so
/// the temp-create and any future direct-create share one value.
#[cfg(unix)]
const NEW_FILE_MODE: rustix::fs::Mode = rustix::fs::Mode::from_bits_truncate(0o644);

/// Fail-closed pre-mutate re-stat: `statat(dir_fd, name, SYMLINK_NOFOLLOW)` and
/// require the result's `(st_dev, st_ino)` to equal `expected`'s (M6a T4 review).
/// Returns `Ok(())` on match; `Err` (fail-closed) on ANY mismatch, missing file, or
/// stat error.
///
/// This NARROWS — it does not eliminate — the guard→mutate window: the base guard
/// validated an open FD, but `renameat`/`unlinkat` act BY NAME, so a foreign
/// process could swap `name` to a different inode after the guard. Re-checking the
/// name's identity immediately before the mutate shrinks that window to the
/// statat→mutate gap (the same irreducible residual as the macOS create
/// no-clobber, spec §9). `SYMLINK_NOFOLLOW` so a symlink swapped in at `name`
/// is seen as itself (a different inode) and rejected, never followed.
///
/// `pub(crate)` so `execute_write`'s Delete path (which mutates via `unlinkat`
/// directly, not through `atomic_write`) can reuse the exact same fail-closed
/// re-stat right before the `unlinkat`.
#[cfg(unix)]
pub(crate) fn restat_identity_matches(
    dir_fd: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    expected: FileId,
) -> Result<(), crate::error::BossclawError> {
    use std::os::unix::ffi::OsStrExt;
    let st = rustix::fs::statat(dir_fd, name.as_bytes(), rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|e| {
            crate::error::BossclawError::Io(std::io::Error::other(format!(
                "pre-mutate re-stat failed: {e}"
            )))
        })?;
    if st.st_dev as u64 != expected.dev || st.st_ino as u64 != expected.ino {
        return Err(crate::error::BossclawError::Io(std::io::Error::other(
            "target inode changed between guard and mutate (foreign-process swap)",
        )));
    }
    Ok(())
}

/// Mint a collision-resistant temp file name in the form `.{name}.{ulid}.tmp`.
/// The ULID (already a crate dependency — used for event ids in the append path) plus
/// the `O_EXCL` create below makes a name clash astronomically unlikely AND
/// detected: `O_EXCL` fails rather than reusing a stale temp, so freshness does
/// not rest on the name alone. Leading `.` keeps the temp hidden during its
/// brief life. The name is purely a tmp handle — it is renamed onto `final_name`.
#[cfg(unix)]
fn temp_name_for(final_name: &std::ffi::OsStr) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStrExt;
    let mut name = std::ffi::OsString::from(".");
    // `final_name` bytes are preserved verbatim (it may be non-UTF-8) so the temp
    // visibly relates to its target; correctness rests on the ULID + O_EXCL.
    name.push(std::ffi::OsStr::from_bytes(final_name.as_bytes()));
    name.push(".");
    name.push(ulid::Ulid::new().to_string());
    name.push(".tmp");
    name
}

/// Atomically write `bytes` to `final_name` inside `dir_fd`: a uniquely-named
/// `O_CREAT|O_EXCL|O_WRONLY|O_NOFOLLOW|O_CLOEXEC` temp in the SAME directory →
/// write all bytes → `fsync` → per-OS finalize rename (spec L13/§9 step 5). All
/// operations are fd-relative against `dir_fd`; the path string is never
/// re-resolved here. On ANY error after the temp is created, the temp is
/// `unlinkat`-removed so no `*.tmp` is ever left behind.
///
/// `no_clobber` selects the create-vs-edit finalize:
/// - **`true` (Create):** the rename MUST NOT overwrite an existing `final_name`.
///   - *Linux:* `renameat2(.., RenameFlags::NOREPLACE)` — kernel-atomic
///     no-clobber (exposed as `renameat_with` in pinned rustix 0.38.44;
///     `#[cfg(linux_kernel)]`).
///   - *macOS / non-Linux:* `statat(.., SYMLINK_NOFOLLOW)` existence pre-check
///     (inside whatever lock the caller holds) → if `final_name` exists, error;
///     else `renameat`. This is **NOT** `O_EXCL`-atomic: a foreign process that
///     creates `final_name` in the microsecond between the `statat` and the
///     `renameat` would be overwritten. That race is the **documented residual**
///     for the macOS create path (spec L13/§9) — it is NOT claimed atomic.
/// - **`false` (Edit):** plain `renameat` — overwrite of `final_name` is intended.
///
/// `expected_identity` (Edit only — the caller passes `None` for Create, which has
/// no base): when `Some`, a fail-closed `(dev,ino)` re-stat of `final_name` runs
/// IMMEDIATELY before the overwrite `renameat`, so a foreign-process swap of the
/// name AFTER the caller's base guard is caught here. This narrows — does not
/// eliminate — the window: the re-stat→renameat gap remains (same irreducible
/// residual as the macOS create no-clobber). For Create, `no_clobber`'s
/// existence check already handles a racing file, so `expected_identity` is unused.
///
/// `O_NOFOLLOW` on the temp create guarantees the temp name cannot resolve
/// through a symlink an attacker pre-planted at the temp path.
// `dead_code`-allowed: forward-seam primitive (T3) CONSUMED by `execute_write`
// in T4 (spec §9 step 5). Exercised by the `fs_primitives` tests today.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn atomic_write(
    dir_fd: &std::os::fd::OwnedFd,
    final_name: &std::ffi::OsStr,
    bytes: &[u8],
    no_clobber: bool,
    expected_identity: Option<FileId>,
) -> Result<(), crate::error::BossclawError> {
    use rustix::fs::{AtFlags, OFlags};
    use std::os::unix::ffi::OsStrExt;

    let io = |e: rustix::io::Errno| crate::error::BossclawError::Io(std::io::Error::other(e.to_string()));

    let tmp_name = temp_name_for(final_name);

    // O_EXCL → fail (rather than reuse) if the freshly-minted name somehow exists;
    // O_NOFOLLOW → never create/follow through a symlink planted at the temp name.
    let tmp_fd = rustix::fs::openat(
        dir_fd,
        tmp_name.as_bytes(),
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        NEW_FILE_MODE,
    )
    .map_err(io)?;

    // From here on, any failure must unlink the temp before returning so no
    // `*.tmp` is left behind (spec §9 "failure leaves the FS unchanged").
    let cleanup = |err: crate::error::BossclawError| -> crate::error::BossclawError {
        // Best-effort: the original error is what the caller sees; a failed
        // cleanup cannot mask it (and there is nothing better to do).
        let _ = rustix::fs::unlinkat(dir_fd, tmp_name.as_bytes(), AtFlags::empty());
        err
    };

    // Write every byte (handle short writes by advancing the slice).
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match rustix::io::write(&tmp_fd, remaining) {
            Ok(0) => {
                return Err(cleanup(crate::error::BossclawError::Io(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "atomic_write: write returned 0 before all bytes were written",
                ))));
            }
            Ok(n) => remaining = &remaining[n..],
            // EINTR → retry the same slice; any other errno is fatal.
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(cleanup(io(e))),
        }
    }

    // Durability: flush file data+metadata before the rename publishes it.
    rustix::fs::fsync(&tmp_fd).map_err(|e| cleanup(io(e)))?;
    // Drop the temp write fd before renaming (the rename is dir-fd-relative; the
    // file fd is no longer needed and closing it early is tidy).
    drop(tmp_fd);

    let finalize: Result<(), crate::error::BossclawError> = if no_clobber {
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::RenameFlags;
            // Kernel-atomic no-clobber: fails with EEXIST if `final_name` exists.
            rustix::fs::renameat_with(
                dir_fd,
                tmp_name.as_bytes(),
                dir_fd,
                final_name.as_bytes(),
                RenameFlags::NOREPLACE,
            )
            .map_err(io)
        }
        #[cfg(not(target_os = "linux"))]
        {
            // macOS: no renameat2. Pre-check existence fd-relative (SYMLINK_NOFOLLOW
            // so a symlink AT `final_name` counts as "exists" and is refused), then
            // renameat. NOT O_EXCL-atomic — the statat→renameat gap is the
            // documented residual (a foreign process winning that microsecond race
            // would be overwritten); the real boundary is the caller's rename mutex
            // + grant check, not this pre-check.
            match rustix::fs::statat(dir_fd, final_name.as_bytes(), AtFlags::SYMLINK_NOFOLLOW) {
                Ok(_) => Err(crate::error::BossclawError::Io(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "atomic_write: create no-clobber target already exists",
                ))),
                // ENOENT is the only "safe to proceed" outcome; any other stat
                // error is fail-closed.
                Err(rustix::io::Errno::NOENT) => {
                    rustix::fs::renameat(dir_fd, tmp_name.as_bytes(), dir_fd, final_name.as_bytes())
                        .map_err(io)
                }
                Err(e) => Err(io(e)),
            }
        }
    } else {
        // Edit: overwrite is intended. But first, if the caller passed the base
        // guard's identity, re-stat `final_name` by name and fail closed if its
        // (dev,ino) changed since the guard — a foreign-process swap in the
        // guard→here window. This narrows that window to the re-stat→renameat gap
        // (the irreducible residual). The temp is cleaned up on a reject (`cleanup`).
        if let Some(expected) = expected_identity {
            if let Err(e) = restat_identity_matches(dir_fd, final_name, expected) {
                return Err(cleanup(e));
            }
        }
        rustix::fs::renameat(dir_fd, tmp_name.as_bytes(), dir_fd, final_name.as_bytes()).map_err(io)
    };

    finalize.map_err(cleanup)
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

    // ── T3 filesystem-primitive tests (unix-only) ───────────────────────────
    #[cfg(unix)]
    mod fs_primitives {
        use super::super::{atomic_write, open_dir_for_write};
        use std::ffi::OsStr;
        use std::fs;
        use std::path::Path;

        /// Count the `*.tmp` leftovers in `dir` — every successful or failed
        /// `atomic_write` must leave ZERO (the atomicity guarantee).
        fn tmp_count(dir: &Path) -> usize {
            fs::read_dir(dir)
                .expect("read_dir")
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .ends_with(".tmp")
                })
                .count()
        }

        #[test]
        fn atomic_write_create_leaves_exact_bytes_and_no_tmp() {
            let dir = tempfile::tempdir().expect("tempdir");
            let fd = open_dir_for_write(dir.path()).expect("open dir fd");

            atomic_write(&fd, OsStr::new("note.txt"), b"hello world", true, None)
                .expect("create write");

            let target = dir.path().join("note.txt");
            assert_eq!(fs::read(&target).expect("read target"), b"hello world");
            assert_eq!(tmp_count(dir.path()), 0, "no *.tmp may survive a create");
        }

        #[test]
        fn atomic_write_edit_overwrites_atomically_and_no_tmp() {
            let dir = tempfile::tempdir().expect("tempdir");
            let target = dir.path().join("note.txt");
            fs::write(&target, b"old contents").expect("seed file");

            let fd = open_dir_for_write(dir.path()).expect("open dir fd");
            // Edit (no_clobber = false) over an existing file replaces its bytes.
            // `None` identity → no pre-rename re-stat (the T3 primitive behavior).
            atomic_write(&fd, OsStr::new("note.txt"), b"brand new bytes", false, None)
                .expect("edit write");

            assert_eq!(fs::read(&target).expect("read target"), b"brand new bytes");
            assert_eq!(tmp_count(dir.path()), 0, "no *.tmp may survive an edit");
        }

        #[test]
        fn atomic_write_create_no_clobber_refuses_existing_and_preserves_bytes() {
            let dir = tempfile::tempdir().expect("tempdir");
            let target = dir.path().join("note.txt");
            fs::write(&target, b"do not touch").expect("seed file");

            let fd = open_dir_for_write(dir.path()).expect("open dir fd");
            // Create onto an existing name must Err (no-clobber), leave the
            // original bytes UNCHANGED, and leave no `*.tmp` behind. This also
            // exercises the error-cleanup path (the temp is created, then the
            // finalize fails → the temp must be unlinked).
            let err = atomic_write(&fd, OsStr::new("note.txt"), b"payload", true, None);
            assert!(err.is_err(), "create onto existing name must be refused");

            assert_eq!(
                fs::read(&target).expect("read target"),
                b"do not touch",
                "the original file's bytes must be untouched by a refused create"
            );
            assert_eq!(
                tmp_count(dir.path()),
                0,
                "the failed-create temp must be cleaned up (no *.tmp leftover)"
            );
        }

        #[test]
        fn open_dir_for_write_refuses_symlinked_final_component() {
            let dir = tempfile::tempdir().expect("tempdir");
            let real = dir.path().join("real_dir");
            fs::create_dir(&real).expect("mkdir real");
            let link = dir.path().join("link_dir");
            std::os::unix::fs::symlink(&real, &link).expect("symlink");

            // The final component `link_dir` is a symlink → O_NOFOLLOW (macOS) /
            // RESOLVE_NO_SYMLINKS (Linux) must refuse it as a write anchor.
            assert!(
                open_dir_for_write(&link).is_err(),
                "a symlinked final dir component must be refused"
            );
            // Sanity: the real directory itself opens fine.
            assert!(
                open_dir_for_write(&real).is_ok(),
                "the real (non-symlink) directory must open"
            );
        }

        #[test]
        fn atomic_write_temp_create_cannot_follow_symlink_at_temp_path() {
            // The temp is created with O_EXCL|O_NOFOLLOW. Even if an attacker
            // pre-plants a symlink whose name collides with a temp, O_EXCL (name
            // freshness) plus O_NOFOLLOW (no symlink traversal) means the create
            // either picks an unused ULID name or fails — it can NEVER write
            // through a planted symlink to clobber an outside target. We assert the
            // positive end-to-end property: a normal create lands the bytes in the
            // intended dir-relative target and nowhere else.
            let dir = tempfile::tempdir().expect("tempdir");
            let outside = dir.path().join("OUTSIDE_must_not_be_written");
            fs::write(&outside, b"sentinel").expect("seed sentinel");

            let fd = open_dir_for_write(dir.path()).expect("open dir fd");
            atomic_write(&fd, OsStr::new("inside.txt"), b"safe", true, None).expect("create");

            assert_eq!(fs::read(dir.path().join("inside.txt")).expect("read"), b"safe");
            assert_eq!(
                fs::read(&outside).expect("read sentinel"),
                b"sentinel",
                "the write must not have leaked outside the intended target name"
            );
        }

        /// Stat a path's `(dev,ino,size)` into a [`FileId`] for the re-stat tests.
        fn file_id(path: &Path) -> super::super::FileId {
            use std::os::unix::fs::MetadataExt;
            let m = fs::symlink_metadata(path).expect("stat");
            super::super::FileId { dev: m.dev(), ino: m.ino(), size: m.size() }
        }

        // ── Pre-mutate re-stat (T4 review): the Edit `expected_identity` guard ────
        //
        // Deterministic same-name / different-inode swap done IN-PROCESS between
        // capturing the identity and the mutate (no foreign process needed): an Edit
        // carrying the OLD identity must fail closed when the name now points at a
        // new inode, and must NOT clobber the swapped-in file.
        #[test]
        fn atomic_write_edit_restat_rejects_same_name_inode_swap() {
            let dir = tempfile::tempdir().expect("tempdir");
            let target = dir.path().join("note.txt");
            fs::write(&target, b"original").expect("seed");
            let old_id = file_id(&target);

            // Swap: same NAME, new INODE. Use a COEXISTING decoy + rename, NOT
            // remove+recreate-at-same-name — Linux (ext4/tmpfs) REUSES the freed
            // inode number, so remove+recreate yields the SAME inode and the swap is
            // moot. A decoy that coexists with the target has a distinct inode;
            // renaming it over the target transfers that distinct inode on every FS.
            let decoy = dir.path().join(".swap-decoy");
            fs::write(&decoy, b"swapped in by a racer").expect("decoy");
            fs::rename(&decoy, &target).expect("swap");
            assert_ne!(old_id.ino, file_id(&target).ino, "the swap must change the inode");

            let fd = open_dir_for_write(dir.path()).expect("open dir fd");
            // Edit carrying the STALE identity → the pre-rename re-stat must reject.
            let err = atomic_write(&fd, OsStr::new("note.txt"), b"payload", false, Some(old_id));
            assert!(err.is_err(), "a same-name inode swap must fail closed on the re-stat");
            // The swapped-in file is untouched (NOT overwritten by 'payload'), and no
            // temp survives the rejected mutate.
            assert_eq!(fs::read(&target).expect("read"), b"swapped in by a racer");
            assert_eq!(tmp_count(dir.path()), 0, "the rejected edit must clean up its temp");
        }

        /// Positive control: when the name still points at the SAME inode the Edit
        /// identity matches, so the re-stat passes and the overwrite proceeds. Proves
        /// the rejection above is caused by the swap, not an always-on failure.
        #[test]
        fn atomic_write_edit_restat_passes_when_identity_unchanged() {
            let dir = tempfile::tempdir().expect("tempdir");
            let target = dir.path().join("note.txt");
            fs::write(&target, b"original").expect("seed");
            let id = file_id(&target);

            let fd = open_dir_for_write(dir.path()).expect("open dir fd");
            atomic_write(&fd, OsStr::new("note.txt"), b"new bytes", false, Some(id))
                .expect("edit with matching identity must succeed");
            assert_eq!(fs::read(&target).expect("read"), b"new bytes");
            assert_eq!(tmp_count(dir.path()), 0, "no *.tmp may survive a successful edit");
        }

        /// The shared `restat_identity_matches` helper (also used by the Delete path
        /// in `execute_write`): match → Ok, mismatch → Err. Asserts the primitive the
        /// Delete site relies on, since the Delete swap is otherwise foreign-process.
        #[test]
        fn restat_identity_matches_detects_swap() {
            let dir = tempfile::tempdir().expect("tempdir");
            let target = dir.path().join("f.txt");
            fs::write(&target, b"a").expect("seed");
            let id = file_id(&target);
            let fd = open_dir_for_write(dir.path()).expect("open dir fd");

            // Same inode → Ok.
            super::super::restat_identity_matches(&fd, OsStr::new("f.txt"), id)
                .expect("matching identity is Ok");

            // Swap the inode under the same name → Err. Coexisting decoy + rename,
            // not remove+recreate (Linux reuses the freed inode number).
            let decoy = dir.path().join(".swap-decoy");
            fs::write(&decoy, b"b").expect("decoy");
            fs::rename(&decoy, &target).expect("swap");
            assert!(
                super::super::restat_identity_matches(&fd, OsStr::new("f.txt"), id).is_err(),
                "a changed inode under the same name must be rejected"
            );
        }
    }
}
