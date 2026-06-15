use bossclaw_core::event::Event;
use bossclaw_core::highwater::FileHighWater;
use bossclaw_core::log::EventLog;
use bossclaw_core::store::Store;
use ed25519_dalek::SigningKey;
use std::io::Read;
use std::sync::Arc;

fn dek() -> [u8; 32] {
    [42u8; 32]
}

fn mk_event(text: &str) -> Event {
    Event {
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
    }
}

#[test]
fn append_then_verify_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let log = EventLog::open(&dir.path().join("m.db"), &[42u8; 32], key).unwrap();
    for t in ["a", "b", "c"] {
        log.append(mk_event(t)).unwrap();
    }
    assert_eq!(log.count().unwrap(), 3);
    log.verify_chain().expect("chain verifies");
}

#[test]
fn concurrent_appends_do_not_fork_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let log = Arc::new(EventLog::open(&dir.path().join("m.db"), &[42u8; 32], key).unwrap());
    let mut handles = vec![];
    for i in 0..16 {
        let log = Arc::clone(&log);
        handles.push(std::thread::spawn(move || {
            log.append(mk_event(&format!("e{i}"))).unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(log.count().unwrap(), 16);
    log.verify_chain().expect("no fork under concurrency");
}

#[test]
fn tampering_a_row_breaks_verify_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let path = dir.path().join("m.db");
    {
        let log = EventLog::open(&path, &[42u8; 32], key.clone()).unwrap();
        log.append(mk_event("a")).unwrap();
        log.append(mk_event("b")).unwrap();
    }
    {
        let store = bossclaw_core::store::Store::open(&path, &[42u8; 32]).unwrap();
        store
            .exec("UPDATE events SET payload = replace(payload, '\"a\"', '\"HACKED\"') WHERE event_type='memory' AND payload LIKE '%\"a\"%'")
            .unwrap();
    }
    let log = EventLog::open(&path, &[42u8; 32], key).unwrap();
    assert!(log.verify_chain().is_err(), "tamper must be detected");
}

#[test]
fn tail_truncation_is_detected_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let db = dir.path().join("m.db");
    let hw = dir.path().join("hw.json");

    {
        let log = EventLog::open_with_highwater(&db, &[42u8; 32], key.clone(),
            Box::new(FileHighWater::new(&hw))).unwrap();
        for t in ["a","b","c"] { log.append(mk_event(t)).unwrap(); }
        log.checkpoint_highwater().unwrap(); // persist {count=3}
    }

    // Attacker deletes the last row (tail truncation); remaining rows still link.
    {
        let store = bossclaw_core::store::Store::open(&db, &[42u8; 32]).unwrap();
        store.exec("DELETE FROM events WHERE seq = (SELECT max(seq) FROM events)").unwrap();
    }

    // Reopen: live count (2) is BEHIND the signed high-water (3) → detected.
    let reopened = EventLog::open_with_highwater(&db, &[42u8; 32], key,
        Box::new(FileHighWater::new(&hw)));
    assert!(matches!(reopened, Err(bossclaw_core::BossclawError::Truncation(_))),
        "tail truncation must be detected on open");
}

#[test]
fn store_is_encrypted_on_disk_and_keyed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");

    {
        let store = Store::open(&path, &dek()).unwrap();
        store.exec("CREATE TABLE t(x TEXT)").unwrap();
        store
            .exec("INSERT INTO t(x) VALUES ('secret-marker')")
            .unwrap();
    }

    // The on-disk header must NOT be the plaintext "SQLite format 3" magic.
    let mut buf = [0u8; 16];
    std::fs::File::open(&path)
        .unwrap()
        .read_exact(&mut buf)
        .unwrap();
    assert_ne!(
        &buf,
        b"SQLite format 3\0",
        "db must be encrypted at rest"
    );

    // Wrong key cannot open it.
    let wrong = Store::open(&path, &[0u8; 32]);
    assert!(wrong.is_err(), "wrong DEK must fail to open");

    // Right key round-trips.
    let store = Store::open(&path, &dek()).unwrap();
    let got: String = store.query_one("SELECT x FROM t LIMIT 1").unwrap();
    assert_eq!(got, "secret-marker");
}

#[test]
fn stream_returns_events_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let log = EventLog::open(&dir.path().join("m.db"), &[42u8; 32], key).unwrap();
    for t in ["a", "b", "c"] { log.append(mk_event(t)).unwrap(); }

    let all = log.stream_all().unwrap();
    assert_eq!(all.len(), 3);
    let texts: Vec<String> = all.iter()
        .map(|e| e.content["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(texts, vec!["a", "b", "c"]);
}

// ── verify_chain_since tests ──────────────────────────────────────────────────

/// Test 1: A valid tail after a trusted cursor verifies successfully.
#[test]
fn verify_chain_since_valid_tail_ok() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let log = EventLog::open(&dir.path().join("m.db"), &[42u8; 32], key).unwrap();

    // Append 6 events; capture the id of the 4th.
    let mut ids = Vec::new();
    for t in ["a", "b", "c", "d", "e", "f"] {
        ids.push(log.append(mk_event(t)).unwrap());
    }
    let cursor_id = &ids[3]; // 4th event ("d")

    log.verify_chain_since(Some(cursor_id.as_str()))
        .expect("tail after cursor must verify");
}

/// Test 2: Tampering a post-cursor row is detected.
#[test]
fn verify_chain_since_detects_tail_tamper() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let path = dir.path().join("m.db");

    let cursor_id;
    {
        let log = EventLog::open(&path, &[42u8; 32], key.clone()).unwrap();
        // Append cursor + 2 post-cursor events.
        cursor_id = log.append(mk_event("cursor")).unwrap();
        log.append(mk_event("post-a")).unwrap();
        log.append(mk_event("post-b")).unwrap();
    }

    // Tamper a post-cursor payload.
    {
        let store = bossclaw_core::store::Store::open(&path, &[42u8; 32]).unwrap();
        store.exec(
            "UPDATE events SET payload = replace(payload, '\"post-a\"', '\"HACKED\"') \
             WHERE event_type='memory' AND payload LIKE '%\"post-a\"%'",
        ).unwrap();
    }

    let log = EventLog::open(&path, &[42u8; 32], key).unwrap();
    assert!(
        log.verify_chain_since(Some(cursor_id.as_str())).is_err(),
        "post-cursor tamper must be detected"
    );
}

/// Test 3 (bounded proof): A pre-cursor tamper is INVISIBLE to verify_chain_since
/// but VISIBLE to verify_chain (full), demonstrating the bounded behaviour.
#[test]
fn verify_chain_since_bounded_pre_cursor_tamper_invisible() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let path = dir.path().join("m.db");

    let cursor_id;
    {
        let log = EventLog::open(&path, &[42u8; 32], key.clone()).unwrap();
        log.append(mk_event("before")).unwrap(); // will be tampered
        cursor_id = log.append(mk_event("cursor")).unwrap();
        log.append(mk_event("after")).unwrap();
    }

    // Tamper the row BEFORE the cursor.
    {
        let store = bossclaw_core::store::Store::open(&path, &[42u8; 32]).unwrap();
        store.exec(
            "UPDATE events SET payload = replace(payload, '\"before\"', '\"HACKED\"') \
             WHERE event_type='memory' AND payload LIKE '%\"before\"%'",
        ).unwrap();
    }

    let log = EventLog::open(&path, &[42u8; 32], key).unwrap();

    // Bounded tail check does NOT re-verify the trusted prefix → Ok.
    log.verify_chain_since(Some(cursor_id.as_str()))
        .expect("bounded check must not detect pre-cursor tamper");

    // Full chain check DOES re-verify the prefix → Err.
    assert!(
        log.verify_chain().is_err(),
        "full verify_chain must detect the pre-cursor tamper"
    );
}

/// Test 4: verify_chain_since(None) behaves like verify_chain — detects a tamper.
#[test]
fn verify_chain_since_none_behaves_like_verify_chain() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let path = dir.path().join("m.db");

    {
        let log = EventLog::open(&path, &[42u8; 32], key.clone()).unwrap();
        log.append(mk_event("x")).unwrap();
        log.append(mk_event("y")).unwrap();
    }

    // Tamper a row.
    {
        let store = bossclaw_core::store::Store::open(&path, &[42u8; 32]).unwrap();
        store.exec(
            "UPDATE events SET payload = replace(payload, '\"x\"', '\"HACKED\"') \
             WHERE event_type='memory' AND payload LIKE '%\"x\"%'",
        ).unwrap();
    }

    let log = EventLog::open(&path, &[42u8; 32], key).unwrap();
    assert!(
        log.verify_chain_since(None).is_err(),
        "verify_chain_since(None) must detect the same tamper as verify_chain"
    );
}

/// Test 5: An unknown cursor id returns Err.
#[test]
fn verify_chain_since_unknown_cursor_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let log = EventLog::open(&dir.path().join("m.db"), &[42u8; 32], key).unwrap();
    log.append(mk_event("only")).unwrap();

    let result = log.verify_chain_since(Some("does-not-exist"));
    assert!(result.is_err(), "unknown cursor id must return Err");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("cursor event") && msg.contains("does-not-exist"),
        "error message must name the missing cursor: {msg}"
    );
}
