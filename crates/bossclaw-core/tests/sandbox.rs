//! Real-subprocess jail proofs. Gated: require --features sandbox-test-hooks AND a built
//! venv at $BOSSCLAW_MARKITDOWN_VENV AND the OS jail tool present.
//! Run: BOSSCLAW_MARKITDOWN_VENV=/tmp/m5b-venv cargo test -p bossclaw-core --features sandbox-test-hooks --test sandbox -- --ignored
#![cfg(all(any(target_os = "macos", target_os = "linux"), feature = "sandbox-test-hooks"))]

use bossclaw_core::Parser;

#[test]
#[ignore]
fn egress_probe_proves_network_denied_with_teeth() {
    assert!(bossclaw_core::sandbox_test_hooks::probe_egress_blocks(), "the jail MUST deny network (jailed connect refused)");
    assert!(!bossclaw_core::sandbox_test_hooks::unjailed_probe_blocks(), "un-jailed MUST connect — proves the probe has teeth");
}

#[test]
#[ignore]
fn converts_real_pdf_and_reports_parser_id() {
    let p = bossclaw_core::SandboxedMarkitdownParser::discover().expect("venv + jail");
    let bytes = std::fs::read("tests/fixtures/hello.pdf").unwrap();
    let hint = bossclaw_core::ingest::PathHint { ext: Some("pdf".into()) };
    let md = p.convert(&bytes, &hint).expect("convert");
    assert!(md.to_lowercase().contains("hello"), "expected 'hello' in: {md}");
    assert!(p.parser_id().starts_with("markitdown-sandboxed-v"));
}

#[test]
#[ignore]
fn hostile_document_makes_no_outbound_connection() {
    // HTML referencing our listener + an RSS-ish body — neither should fetch.
    let html = "<html><body><img src=\"http://127.0.0.1:{PORT}/x\"><a href=\"http://127.0.0.1:{PORT}/y\">z</a></body></html>";
    assert!(!bossclaw_core::sandbox_test_hooks::hostile_doc_connects(html, "html"), "jailed parser must make NO outbound connection");
}

#[test]
#[ignore]
fn malformed_pdf_fails_without_hang() {
    let p = bossclaw_core::SandboxedMarkitdownParser::discover().expect("venv + jail");
    let hint = bossclaw_core::ingest::PathHint { ext: Some("pdf".into()) };
    // Garbage claiming to be a PDF. The point: the call RETURNS (no hang, no host
    // crash) — markitdown either errors or yields trivial/empty text.
    let r = p.convert(b"%PDF-1.4\nnot a real pdf \x00\xff\xfe garbage", &hint);
    let acceptable = r.is_err() || r.as_ref().map(|s| s.trim().len() < 50).unwrap_or(true);
    assert!(acceptable, "malformed input should fail or yield trivial text, got: {r:?}");
}
