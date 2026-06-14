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
