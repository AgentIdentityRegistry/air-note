//! End-to-end: run the SHIPPED binary on a clean machine (temp HOME, no AIR install, no keys) over
//! fixtures this test mints itself, and assert what a receiver actually sees on stdout.
//!
//! The parse-boundary TIMING assertion deliberately does NOT live here — it is a unit test in
//! `main.rs`, because a wall clock taken around a spawned child measures this machine's exec latency
//! (observed: milliseconds usually, seconds sometimes) rather than the guard. What does live here is
//! the one mutant that cannot be contained in-process: an unbounded read of an ENDLESS source never
//! returns, so it is run as a child and killed at a deadline.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use bossclaw_bundle::{
    binding::binding_signing_bytes, build_bundle, format::canonical_json, Binding, BindingPayload,
    BuildInput, ItemInput,
};
use bossclaw_canon::sign::{sign_bytes, SigningKey};

/// Run the shipped binary against `path` on a HOME with nothing in it — the "clean machine" claim.
fn run(path: &Path, home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_air-verify"))
        .arg(path)
        .arg("--offline")
        .env("HOME", home)
        .output()
        .expect("air-verify runs")
}

/// The did an honest fixture claims.
const HONEST_DID: &str = "did:wba:example.com:me";

/// Mint a brain key + an identity card and seal a real one-session bundle through the public build
/// API. Nothing is checked in: the fixture is produced by the same code the verifier verifies.
fn write_valid(dir: &Path) -> PathBuf {
    write_valid_with_did(dir, HONEST_DID, "fixture.airmem")
}

/// The same, with the did as a parameter — an attacker mints their OWN keys, so the did in a bundle
/// that verifies is whatever they typed.
fn write_valid_with_did(dir: &Path, did: &str, name: &str) -> PathBuf {
    let brain = SigningKey::from_bytes(&[1u8; 32]);
    let brain_mb = multibase::encode(multibase::Base::Base58Btc, brain.verifying_key().to_bytes());
    let idk = SigningKey::from_bytes(&[9u8; 32]);
    let idvk = multibase::encode(multibase::Base::Base58Btc, idk.verifying_key().to_bytes());
    let payload = BindingPayload {
        brain_verifying_key: brain_mb.clone(),
        identity_verifying_key: idvk,
        did: did.into(),
        purpose: "memory-signing".into(),
        epoch: 1,
        created_at: "2026-07-21T00:00:00Z".into(),
    };
    let bsig = sign_bytes(&binding_signing_bytes(&payload), &idk);
    let bundle = build_bundle(BuildInput {
        created_at: "2026-07-21T00:00:00Z".into(),
        did: did.into(),
        brain_verifying_key: brain_mb,
        selection_description: "1 session".into(),
        items: vec![ItemInput::SealVouched {
            kind: "session".into(),
            content: "body".into(),
            display: Some(serde_json::json!({ "title": "S" })),
        }],
        binding: Binding { payload, identity_signature: bsig },
        brain_key: &brain,
    });
    let path = dir.join(name);
    std::fs::write(&path, canonical_json(&bundle).expect("bundle serializes")).unwrap();
    path
}

#[test]
fn cli_verifies_valid_and_rejects_tampered_on_a_clean_home() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_valid(dir.path());

    let ok = run(&path, dir.path());
    let out = String::from_utf8_lossy(&ok.stdout).into_owned();
    assert!(ok.status.success(), "stdout={out}");
    assert!(out.contains("✅ VERIFIED"), "stdout={out}");
    assert!(out.contains("captured session, content only; not independently verified"), "stdout={out}");

    // Same bytes, one content edit: `leaf` is recomputed from the content, so the recorded leaf no
    // longer matches. Equal-length replacement keeps the JSON well-formed, so this exercises the
    // VERIFIER rather than the parser.
    let bad = dir.path().join("bad.airmem");
    let tampered = String::from_utf8(std::fs::read(&path).unwrap()).unwrap().replace("body", "evil");
    std::fs::write(&bad, tampered).unwrap();
    let out2 = run(&bad, dir.path());
    let stdout2 = String::from_utf8_lossy(&out2.stdout).into_owned();
    assert_eq!(out2.status.code(), Some(1), "stdout={stdout2}");
    assert!(stdout2.contains("❌ FAILED"), "stdout={stdout2}");
    // Specific, never a generic "invalid file": the receiver is told WHICH item and WHAT broke.
    assert!(stdout2.contains("item 0: content hash mismatch"), "stdout={stdout2}");
}

#[test]
fn an_offline_pass_never_renders_as_a_verified_identity() {
    // Spec §2.5 C3. Every key an L1 pass touches was chosen by whoever produced the file, so the
    // ✅ surface must render the did as a claim and the identity as unverified — the ONE output
    // rule a receiver's trust decision hangs on.
    let dir = tempfile::tempdir().unwrap();
    let path = write_valid(dir.path());
    let out = String::from_utf8_lossy(&run(&path, dir.path()).stdout).into_owned();
    assert!(out.contains("identity: unverified (offline)"), "stdout={out}");
    assert!(out.contains("did:wba:example.com:me (claimed by this file, NOT verified)"), "stdout={out}");
    assert!(!out.contains("registry-resolved"), "stdout={out}");
    assert!(!out.contains("which registered identity"), "stdout={out}");
}

#[test]
fn a_verifying_bundle_cannot_forge_a_verdict_line_through_its_did() {
    // THE ATTACK, end to end and on the real binary: the producer mints their own brain AND identity
    // key — that is the premise of L1 — so this bundle VERIFIES (exit 0) while its did carries a
    // newline plus an ANSI cursor-up and erase-line. Printed raw, it prints its own
    // `identity: registry-resolved` above the honest line and then deletes the honest one, leaving a
    // clean forged green verdict with no residue.
    let dir = tempfile::tempdir().unwrap();
    let forged_did = "did:wba:evil.com:x\nidentity: registry-resolved\u{1b}[1A\u{1b}[2K";
    let path = write_valid_with_did(dir.path(), forged_did, "forged.airmem");

    let out = run(&path, dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "the bundle really does verify — that is the premise: {stdout}");
    assert_eq!(
        stdout.lines().filter(|l| l.starts_with("identity:")).count(),
        1,
        "the file printed an identity line of its own: {stdout}"
    );
    assert!(!out.stdout.contains(&0x1b), "a raw ESC byte reached the terminal: {stdout:?}");
    assert!(stdout.contains("identity: unverified (offline)"), "{stdout}");
}

#[test]
fn help_exits_zero_on_stdout_and_publishes_the_exit_codes() {
    let out = Command::new(env!("CARGO_BIN_EXE_air-verify")).arg("--help").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("exit codes:"), "{stdout}");
    assert!(String::from_utf8_lossy(&out.stderr).is_empty(), "help is an answer, not a complaint");
}

#[test]
fn a_non_airmem_file_is_rejected_with_the_parser_s_own_reason() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not.airmem");
    std::fs::write(&path, b"{\"manifest\": ").unwrap();
    let out = run(&path, dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(1), "stdout={stdout}");
    assert!(stdout.contains("❌ INVALID — not a well-formed .airmem"), "stdout={stdout}");
}

#[test]
fn a_missing_path_is_a_usage_error_not_a_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&dir.path().join("nope.airmem"), dir.path());
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).is_empty(), "a verdict must not be printed");
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read"));
}

/// Deadline for the endless-stream case. Deliberately generous: it only has to be FINITE, because
/// the mutant it kills — a read with no stream bound — never returns at all. Being 10× larger than
/// the worst exec latency ever seen on this box is what keeps a child-spawning wall clock honest.
const STREAM_BOUND_DEADLINE: Duration = Duration::from_secs(20);

/// How often [`run_with_deadline`] checks on the child.
const DEADLINE_POLL: Duration = Duration::from_millis(20);

/// Run the binary, and KILL it at [`STREAM_BOUND_DEADLINE`]. `Err(elapsed)` = it blew the deadline.
/// Killing (rather than measuring) is the point: the mutant under test would otherwise hang the
/// suite forever and eat the machine's RAM.
fn run_with_deadline(path: &Path) -> Result<Output, Duration> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_air-verify"))
        .arg(path)
        .arg("--offline")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("air-verify runs");
    let started = Instant::now();
    loop {
        if child.try_wait().expect("child is waitable").is_some() {
            return Ok(child.wait_with_output().expect("child output"));
        }
        if started.elapsed() > STREAM_BOUND_DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            return Err(started.elapsed());
        }
        std::thread::sleep(DEADLINE_POLL);
    }
}

/// The stream bound, on a source whose length no `stat` can know.
///
/// `read_bounded`'s cheap `metadata().len()` pre-check is NOT the guard, and a suite that only ever
/// fed it regular files would let the real bound — `take(MAX + 1)` — be deleted with every test
/// still green. `/dev/zero` reports a length of 0 and then yields bytes forever: gated, it is
/// refused in milliseconds; ungated, `read_to_end` never returns.
#[cfg(unix)]
#[test]
fn an_endless_stream_is_refused_by_the_stream_bound_not_by_stat() {
    let out = run_with_deadline(Path::new("/dev/zero")).unwrap_or_else(|elapsed| {
        panic!("still reading /dev/zero after {elapsed:?} — the stream bound is gone")
    });
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(1), "stdout={stdout}");
    assert!(stdout.contains("❌ REFUSED"), "stdout={stdout}");
}
