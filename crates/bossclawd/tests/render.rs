//! A6: deterministic bounded transcript renderer.
//!
//! These tests pin the renderer's contract from the OUTSIDE (integration crate),
//! so they exercise exactly the public API `capture::render` exposes. The fixtures
//! under `tests/fixtures/` are TRUSTED inputs for `render_transcript_path` (the
//! test/convenience entry point); production reaches the renderer through
//! `render_transcript(File)` on a handle A5's confined open already vetted.

use std::time::Duration;

use bossclawd::capture::render::{render_transcript_path, RenderBounds, RenderError};

/// Production defaults (spec §4a bounds): 64 MiB transcript, 2 MiB line, 30 s.
fn bounds() -> RenderBounds {
    RenderBounds {
        max_transcript_bytes: 64 * 1024 * 1024,
        max_line_bytes: 2 * 1024 * 1024,
        wall_clock: Duration::from_secs(30),
    }
}

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn renders_synthetic_fixture_deterministically() {
    let a = render_transcript_path(&fixture("transcript_synthetic.jsonl"), &bounds()).unwrap();
    let b = render_transcript_path(&fixture("transcript_synthetic.jsonl"), &bounds()).unwrap();
    assert_eq!(a.markdown, b.markdown, "determinism (I5)");
    assert!(a.markdown.starts_with("---\n"), "front-matter");
    assert!(!a.title.is_empty());
    assert!(
        a.markdown.contains(&a.title) || a.markdown.contains("## "),
        "body present"
    );
    // sha256 is over the bytes actually read and is stable across renders.
    assert_eq!(a.sha256, b.sha256);
    assert_eq!(a.sha256.len(), 64, "hex sha256");
}

#[test]
fn title_is_first_user_prompt_truncated() {
    let r = render_transcript_path(&fixture("transcript_synthetic.jsonl"), &bounds()).unwrap();
    assert!(r.title.len() <= 120);
    assert!(
        r.title.starts_with("How do I render a transcript"),
        "title = first user prompt, got {:?}",
        r.title
    );
}

#[test]
fn torn_tail_dropped_silently() {
    let r = render_transcript_path(&fixture("transcript_torn_tail.jsonl"), &bounds()).unwrap();
    assert!(r.dropped_torn_tail);
    // the valid lines before the torn tail still rendered:
    assert!(!r.markdown.is_empty());
    assert!(r.markdown.contains("first valid prompt"));
    assert!(r.markdown.contains("a valid assistant reply"));
    // the torn fragment's content did NOT leak into the body.
    assert!(!r.markdown.contains("this line is cut off mid"));
}

#[test]
fn oversized_line_dropped_and_counted() {
    let r = render_transcript_path(&fixture("transcript_oversized_line.jsonl"), &bounds()).unwrap();
    assert_eq!(r.oversized_lines, 1);
    // the two valid lines around it still rendered
    assert!(r.markdown.contains("small valid prompt before"));
    assert!(r.markdown.contains("small valid reply after"));
    // the 3 MiB payload is not slurped into the output.
    assert!(!r.markdown.contains("AAAAAAAAAA"));
}

#[test]
fn over_budget_file_refused_loudly() {
    let tight = RenderBounds {
        max_transcript_bytes: 16,
        ..bounds()
    };
    assert!(matches!(
        render_transcript_path(&fixture("transcript_synthetic.jsonl"), &tight),
        Err(RenderError::TooLarge { .. })
    ));
}

#[test]
fn injection_payload_stays_inert_data_in_body() {
    let r = render_transcript_path(&fixture("transcript_injection.jsonl"), &bounds()).unwrap();
    // A6 contract: the payload text appears in the body as data; the renderer does not crash
    // and does not act on it. (A11 later fences/sanitizes it for snapshot injection — NOT here.)
    assert!(r.markdown.contains("SYSTEM") || r.markdown.contains("exfiltrate"));
    // control chars in the payload do not crash the renderer; output is still valid UTF-8.
    assert!(r.markdown.is_char_boundary(r.markdown.len()));
}

#[test]
fn unknown_and_noise_lines_skipped_not_fatal() {
    // synthetic fixture includes queue-operation + hook attachment + system + an
    // unknown future `type` line; rendering succeeds and none of those are emitted
    // as content (each carries a distinct marker string only present in that line).
    let r = render_transcript_path(&fixture("transcript_synthetic.jsonl"), &bounds()).unwrap();
    assert!(!r.markdown.contains("QUEUEOP_MARKER_SHOULD_NOT_RENDER"));
    assert!(!r.markdown.contains("HOOK_NOISE_SHOULD_NOT_RENDER"));
    assert!(!r.markdown.contains("SYSTEM_NOISE_SHOULD_NOT_RENDER"));
    assert!(!r.markdown.contains("UNKNOWN_MARKER_SHOULD_NOT_RENDER"));
    // thinking blocks are internal reasoning, never rendered.
    assert!(!r.markdown.contains("THINKING_SHOULD_NOT_RENDER"));
    // queue-operation + attachment + system + unknown = 4 skipped noise/unknown lines.
    assert_eq!(r.skipped_unknown, 4);
    // the recognized turns DID render.
    assert!(r.markdown.contains("You iterate the JSONL in file order"));
    assert!(r.markdown.contains("\u{25b8} Bash:"), "tool_use one-liner");
}
