//! `air-verify` — the offline `.airmem` verifier. Usage: `air-verify <file.airmem> [--offline]`.
//!
//! A receiver runs this on a CLEAN machine, with no AIR install and no keys, and gets one of three
//! answers: ✅ the file is internally consistent (L1), ❌ it is not (with the SPECIFIC reason), or a
//! usage/IO complaint. Zero external dependencies beyond `bossclaw-bundle` + `serde_json` — the arg
//! handling is hand-rolled, per the `air-memory-mcp` zero-dep precedent.
//!
//! THE PARSE BOUNDARY IS THIS BINARY'S JOB. [`bossclaw_bundle::verify`] takes an ALREADY-PARSED
//! `&Airmem`, so it cannot defend against a huge document; it carries a defensive `MAX_ITEMS` floor,
//! but the raw-BYTE ceiling can only be enforced by whoever does the reading. That is here (and, for
//! their own inputs, Task 10's app command and SP-V2's WASM host). An attacker mints their own brain
//! key, so an arbitrarily large INTERNALLY VALID bundle is trivially producible; without
//! [`MAX_INPUT_BYTES`] a ~100 MB file costs a multi-hundred-MB `serde_json` allocation and tens of
//! seconds of parse before `verify` is ever called. See [`read_bounded`].
//!
//! HONESTY CONTRACT (spec §2.5 C3): an L1 pass is NEVER identity evidence. Every key L1 touches is
//! chosen by whoever produced the file, so the offline verdict renders the did as a CLAIM and the
//! identity as "unverified (offline)". Only [`IdentityLevel::RegistryResolved`] — unreachable in
//! SP-V1, where [`OfflineResolver`] is the only resolver that ships — may speak about who recorded
//! the bytes. The per-item labels are rendered verbatim from the verdict; this binary never
//! re-derives them.
//!
//! `main` is a thin shell over [`run`], which RETURNS its text instead of printing it. That is what
//! lets the parse-boundary guard be timed in-process: a wall-clock assertion taken around a spawned
//! child measures this machine's exec latency (observed: milliseconds usually, seconds sometimes)
//! rather than the guard, and would flake at any bound tight enough to be worth asserting.
#![forbid(unsafe_code)]

use std::io::Read;
use std::process::ExitCode;

use bossclaw_bundle::{verify, Airmem, IdentityLevel, OfflineResolver, Verdict, VerifyError};

/// Ceiling on the RAW BYTES this verifier will read before handing anything to `serde_json`.
///
/// Sized off the write side: `bossclaw_core::log::MAX_EXPORT_BYTES` is 30 MiB, so every bundle AIR
/// itself can produce fits with room to spare, and anything beyond this is not an AIR export but an
/// attacker-minted document. Not shared as a constant with the export side on purpose — that is a
/// cap on the OWNER's own data at build time, while this is a refusal to spend memory and time on a
/// stranger's file, and the two must be free to move independently.
const MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;

/// Exit code for a VERIFIED document.
const EXIT_VERIFIED: u8 = 0;

/// Exit code for a REFUSED document: unreadable as `.airmem`, over [`MAX_INPUT_BYTES`], or verified
/// and found inconsistent. One code for "do not trust this file", whatever the reason.
const EXIT_REJECTED: u8 = 1;

/// Exit code for a bad invocation or an unreadable path — a problem with the command, not a verdict
/// about a document. Printed to stderr so it never pollutes a piped verdict.
const EXIT_USAGE: u8 = 2;

/// The one-line usage string.
const USAGE: &str = "usage: air-verify <file.airmem> [--offline]";

/// What a run produced: the text the receiver sees, and the process exit code.
#[derive(Debug, PartialEq, Eq)]
struct Report {
    /// The verdict (stdout), or the complaint (stderr, when `code` is [`EXIT_USAGE`]).
    text: String,
    /// The process exit code.
    code: u8,
}

impl Report {
    fn new(code: u8, text: impl Into<String>) -> Self {
        Self { text: text.into(), code }
    }
}

fn main() -> ExitCode {
    let report = match parse_args(std::env::args().skip(1)) {
        Ok(path) => run(&path),
        Err(complaint) => Report::new(EXIT_USAGE, complaint),
    };
    if report.code == EXIT_USAGE {
        eprintln!("{}", report.text);
    } else {
        println!("{}", report.text);
    }
    ExitCode::from(report.code)
}

/// The whole pipeline: bounded read → parse from bytes → verify → render.
fn run(path: &str) -> Report {
    let bytes = match read_bounded(path) {
        Ok(b) => b,
        Err(InputError::Io(detail)) => return Report::new(EXIT_USAGE, detail),
        Err(InputError::TooLarge) => {
            return Report::new(
                EXIT_REJECTED,
                format!(
                    "❌ REFUSED — {path} is larger than the {MAX_INPUT_BYTES} bytes this verifier \
                     will read; refused before parsing (an AIR export is at most 30 MiB)"
                ),
            )
        }
    };
    // From BYTES, never a String round-trip and never a pre-parsed value: `serde_json`'s 128-level
    // recursion limit and its out-of-range-float rejection only apply to ITS OWN parser.
    let bundle: Airmem = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => {
            return Report::new(EXIT_REJECTED, format!("❌ INVALID — not a well-formed .airmem: {e}"))
        }
    };
    match verify(&bundle, &OfflineResolver) {
        Ok(verdict) => Report::new(EXIT_VERIFIED, render_verdict(&bundle, &verdict)),
        Err(e) => Report::new(EXIT_REJECTED, format!("❌ FAILED — {}", render_err(&e))),
    }
}

/// Hand-rolled arg matching: exactly one path, plus the inert `--offline` flag.
///
/// `--offline` is accepted and does nothing because offline is the ONLY mode SP-V1 has
/// ([`OfflineResolver`] is the only resolver that ships). Taking it now means the flag a receiver is
/// told to pass keeps working unchanged when SP-V2 adds registry resolution and makes it meaningful.
fn parse_args(args: impl Iterator<Item = String>) -> Result<String, String> {
    let mut file: Option<String> = None;
    for a in args {
        match a.as_str() {
            "--offline" => {}
            "-h" | "--help" => return Err(USAGE.to_string()),
            s if s.starts_with('-') => return Err(format!("unknown flag: {s}\n{USAGE}")),
            s => {
                if file.replace(s.to_string()).is_some() {
                    return Err(format!("only one file argument allowed\n{USAGE}"));
                }
            }
        }
    }
    file.ok_or_else(|| USAGE.to_string())
}

/// Why an input never reached the parser.
enum InputError {
    /// The path could not be opened or read — the invocation is wrong, so this is not a verdict.
    Io(String),
    /// The file is over [`MAX_INPUT_BYTES`]. Detected WITHOUT reading past the ceiling.
    TooLarge,
}

/// Read `path`, refusing anything over [`MAX_INPUT_BYTES`] BEFORE `serde_json` sees a byte.
///
/// `take(MAX + 1)` is what makes the guard cheap as well as correct: the reader stops one byte past
/// the ceiling, so a 100 MB file costs a bounded read and an immediate refusal instead of a
/// multi-hundred-MB allocation and a tens-of-seconds parse. That one extra byte is the whole
/// detection: `len > MAX` can only be true if the file had more to give.
///
/// That stream bound is the AUTHORITATIVE one. The `metadata().len()` pre-check is never the guard:
/// it is a claim about a file that can grow between the stat and the read, and it is absent or a lie
/// for the non-regular paths an argument can name (`/dev/zero` reports 0 and yields bytes forever).
fn read_bounded(path: &str) -> Result<Vec<u8>, InputError> {
    let file = std::fs::File::open(path)
        .map_err(|e| InputError::Io(format!("cannot read {path}: {e}")))?;
    // Fast path: when the size is already known, refusing costs a stat instead of a 32 MiB read
    // (measured: 0.41 s off a cold cache, for a file this verifier was never going to look at).
    // `file.metadata()` fstats THE OPEN HANDLE, so it cannot be raced onto a different file, and an
    // unknowable size falls through to the stream bound rather than guessing.
    if file.metadata().map(|m| m.len()).unwrap_or(0) > MAX_INPUT_BYTES {
        return Err(InputError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| InputError::Io(format!("cannot read {path}: {e}")))?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(InputError::TooLarge);
    }
    Ok(bytes)
}

/// Render a PASSING verdict. Both the did line and the closing note are level-aware, because at L1
/// the did is an unverified claim the file makes about itself: the brain key in the manifest and the
/// identity key in the binding were both chosen by whoever produced the file, so a self-consistent
/// bundle proves internal consistency and NOTHING about who recorded it (spec §2.5 C3).
fn render_verdict(bundle: &Airmem, verdict: &Verdict) -> String {
    let (identity, did_qualifier, note) = match verdict.identity {
        IdentityLevel::UnverifiedOffline => (
            "unverified (offline)",
            "claimed by this file, NOT verified",
            "note: this proves only that the file is internally consistent and sealed by ONE key — \
             a key whoever produced the file chose. It does NOT prove who recorded these bytes, and \
             it does not make the content true. Resolving the did above against the AIR registry is \
             what would turn that claim into an identity.",
        ),
        IdentityLevel::RegistryResolved => (
            "registry-resolved",
            "resolved via the AIR registry",
            "note: ✅ proves which registered identity's brain recorded these exact bytes, dated by \
             its own clock — NOT that the content is true.",
        ),
    };
    let mut lines = vec![
        "✅ VERIFIED (L1 self-consistent)".to_string(),
        format!("did: {} ({did_qualifier})", bundle.manifest.did),
        format!("identity: {identity}"),
    ];
    lines.extend(
        verdict.item_labels.iter().enumerate().map(|(i, label)| format!("  item {i}: {label}")),
    );
    lines.push(note.to_string());
    lines.join("\n")
}

/// One plain-English line per [`VerifyError`] variant. Exhaustive by design — a generic "invalid
/// file" would tell a receiver holding a tampered bundle nothing about WHAT was tampered with, and
/// the compiler's exhaustiveness check is what keeps a future variant from silently landing in a
/// catch-all.
fn render_err(e: &VerifyError) -> String {
    match e {
        VerifyError::SealInvalid => "master seal invalid".into(),
        VerifyError::ItemStampInvalid(i) => format!("item {i}: stamp invalid (or not by this brain)"),
        VerifyError::ItemHashMismatch(i) => format!("item {i}: content hash mismatch"),
        VerifyError::TreeMismatch => "Merkle root mismatch".into(),
        VerifyError::BindingInvalid => "identity binding signature invalid".into(),
        VerifyError::BindingKeyMismatch => "binding brain key ≠ manifest".into(),
        VerifyError::BindingDidMismatch => "binding did ≠ manifest".into(),
        VerifyError::BindingHashMismatch => "binding hash ≠ sealed value".into(),
        VerifyError::OriginMismatch(i) => format!("item {i}: origin label disagrees with signed bytes"),
        VerifyError::IdentityUnresolved => "identity did not resolve (registry)".into(),
        VerifyError::FormatTooNew => "bundle format is newer than this verifier".into(),
        VerifyError::Malformed(detail) => format!("malformed: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn args(list: &[&str]) -> Result<String, String> {
        parse_args(list.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn parse_args_takes_one_path_and_the_inert_offline_flag() {
        assert_eq!(args(&["a.airmem"]).unwrap(), "a.airmem");
        assert_eq!(args(&["a.airmem", "--offline"]).unwrap(), "a.airmem");
        assert_eq!(args(&["--offline", "a.airmem"]).unwrap(), "a.airmem");
    }

    #[test]
    fn parse_args_refuses_no_path_two_paths_and_unknown_flags() {
        assert!(args(&[]).is_err());
        assert!(args(&["a.airmem", "b.airmem"]).is_err());
        assert!(args(&["--nope", "a.airmem"]).unwrap_err().contains("unknown flag: --nope"));
        assert!(args(&["-h"]).unwrap_err().contains("usage:"));
    }

    #[test]
    fn every_verify_error_renders_a_specific_reason() {
        // A receiver holding a tampered bundle must be told WHAT is wrong, so no variant may render
        // as a generic complaint, and no two may render identically.
        let all = [
            VerifyError::SealInvalid,
            VerifyError::ItemStampInvalid(3),
            VerifyError::ItemHashMismatch(3),
            VerifyError::TreeMismatch,
            VerifyError::BindingInvalid,
            VerifyError::BindingKeyMismatch,
            VerifyError::BindingDidMismatch,
            VerifyError::BindingHashMismatch,
            VerifyError::OriginMismatch(3),
            VerifyError::IdentityUnresolved,
            VerifyError::FormatTooNew,
            VerifyError::Malformed("items: must not be empty".into()),
        ];
        let rendered: Vec<String> = all.iter().map(render_err).collect();
        for line in &rendered {
            assert!(!line.is_empty() && !line.contains("invalid file"), "vague: {line}");
        }
        let mut unique = rendered.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), rendered.len(), "two errors render the same line");
    }

    #[test]
    fn an_unreadable_path_is_a_usage_complaint_not_a_verdict() {
        let report = run("/nonexistent/nope.airmem");
        assert_eq!(report.code, EXIT_USAGE);
        assert!(report.text.starts_with("cannot read "), "text={}", report.text);
    }

    // ---- The parse boundary. ----

    /// Size of the oversized fixture — the scale the Task-6 security review measured the hang at
    /// (~100 MB). Every field it carries is a KNOWN field of a well-typed `.airmem`, so a verifier
    /// without the byte cap does not bail early on a parse error: it pays the full multi-hundred-MB
    /// `serde_json` allocation, which is the cost the cap exists to refuse.
    const OVERSIZED_FIXTURE_BYTES: usize = 100 * 1024 * 1024;

    /// Items per `write_all`, so building the fixture costs ~1k writes instead of ~640k.
    const OVERSIZED_CHUNK_ITEMS: usize = 1024;

    /// Nesting depth of each item's `display`. This is the fixture's whole cost model: one nesting
    /// level is six bytes of text but a whole `serde_json::Map` allocation, so a nested chain buys
    /// far more parse work per byte than any flat shape — which is what an attacker paying per byte
    /// would choose. Comfortably inside `serde_json`'s 128-level recursion limit, which its own
    /// parser enforces for us and which is the OTHER reason this binary parses from bytes.
    const OVERSIZED_DISPLAY_DEPTH: usize = 16;

    /// Wall-clock ceiling on the refusal. The measurement is IN-PROCESS (no `Command`, no exec
    /// latency in the number), so the gated path — an `fstat` and a rejection — has ~400× headroom
    /// under this and cannot flake, while the ungated path is 4× to 13× OVER it.
    const REFUSAL_DEADLINE: Duration = Duration::from_secs(2);

    /// A well-typed `.airmem` prefix, up to and including the opening bracket of `items`. Values are
    /// deliberately junk: parsing is the cost under test, and verification never gets that far.
    const OVERSIZED_HEADER: &str = concat!(
        r#"{"manifest":{"format_version":"1.0.0","created_at":"2026-07-21T00:00:00Z","#,
        r#""did":"did:wba:example.com:me","brain_verifying_key":"z11","#,
        r#""selection_description":"huge","item_count":1,"merkle_root":"00","binding_hash":"00"},"#,
        r#""binding":{"payload":{"brain_verifying_key":"z11","identity_verifying_key":"z11","#,
        r#""did":"did:wba:example.com:me","purpose":"memory-signing","epoch":1,"#,
        r#""created_at":"2026-07-21T00:00:00Z"},"identity_signature":"z11"},"seal":"z11","items":["#,
    );

    /// One well-typed, deliberately allocation-dense item: `{"k":{"k":…{"k":"z"}…}}` under `display`.
    fn oversized_item() -> String {
        let mut display = String::from(r#""z""#);
        for _ in 0..OVERSIZED_DISPLAY_DEPTH {
            display = format!(r#"{{"k":{display}}}"#);
        }
        format!(r#"{{"leaf":"00","class":"seal_vouched","kind":"session","display":{display}}}"#)
    }

    fn write_oversized(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("huge.airmem");
        let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        out.write_all(OVERSIZED_HEADER.as_bytes()).unwrap();
        let item = oversized_item();
        let chunk = [item.as_str(), ","].concat().repeat(OVERSIZED_CHUNK_ITEMS);
        let mut written = OVERSIZED_HEADER.len();
        while written + chunk.len() < OVERSIZED_FIXTURE_BYTES {
            out.write_all(chunk.as_bytes()).unwrap();
            written += chunk.len();
        }
        // The last item carries no trailing comma, so the document is valid JSON end to end.
        out.write_all(item.as_bytes()).unwrap();
        out.write_all(b"]}").unwrap();
        out.flush().unwrap();
        path
    }

    #[test]
    fn oversized_input_is_refused_before_parsing_and_refused_fast() {
        // THE DEADLINE IS THE TEST. The byte cap is a guard whose removal changes SPEED, not
        // OUTCOME: without it this fixture is still rejected (`items` blows past the verifier's own
        // MAX_ITEMS), just after a full parse. An outcome-only assertion therefore SURVIVES deleting
        // the cap and only looks like coverage. MEASURED by deleting the ceiling from `read_bounded`
        // and rerunning this fixture: 8.5 s at the fastest, 24–27 s across five further runs on a
        // loaded box, versus ~1 ms gated — so even the fastest mutant run is 4× over this deadline.
        let dir = tempfile::tempdir().unwrap();
        let path = write_oversized(dir.path());
        assert!(
            std::fs::metadata(&path).unwrap().len() > MAX_INPUT_BYTES,
            "the fixture must actually exceed the verifier's ceiling"
        );

        let started = Instant::now();
        let report = run(path.to_str().unwrap());
        let elapsed = started.elapsed();

        // The deadline is asserted FIRST, deliberately: it is the assertion the mutant must die by,
        // and an outcome assertion ahead of it would mask that by failing on the message instead.
        assert!(
            elapsed < REFUSAL_DEADLINE,
            "refusal took {elapsed:?}, over the {REFUSAL_DEADLINE:?} deadline — the byte cap is not \
             gating the parse, which is exactly what this assertion exists to catch. text={}",
            report.text
        );
        assert_eq!(report.code, EXIT_REJECTED, "text={}", report.text);
        assert!(report.text.contains("❌ REFUSED"), "text={}", report.text);
        assert!(report.text.contains("refused before parsing"), "text={}", report.text);
    }

    // The OTHER half of the bound — that `take(MAX + 1)`, not the `metadata()` pre-check, is what
    // refuses a source whose length no `stat` can know — is pinned by the `/dev/zero` case in
    // `tests/clean_machine.rs`. It lives there, in a spawned child with a kill deadline, because
    // that mutant does not merely run slowly: it never returns, and an in-process test of it would
    // hang the suite and eat the machine's RAM instead of failing.
}
