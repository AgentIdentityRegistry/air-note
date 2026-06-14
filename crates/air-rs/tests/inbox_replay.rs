use air_rs::inbox::archive_reader::ArchiveReader;
use air_rs::inbox::frames::Message;
use air_rs::inbox::replay::Replayer;
use rusqlite::Connection;
use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;

/// Archive with: e1 received from a PINNED+verified peer (admitted), e2 from an UNVERIFIED peer
/// (gate #5 rejects), e3 from a peer about to be BLOCKED (#4), room1:joined (SQL #3), e6 spam (#2).
fn seed(home: &TempDir) {
    let conn = Connection::open(home.path().join("archive.db")).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(
        "CREATE TABLE messages (envelope_id TEXT,direction TEXT,thread_id TEXT,peer_did TEXT,from_did TEXT,to_did TEXT,timestamp TEXT,body_json TEXT,encrypted INT,verified INT,key_changed INT DEFAULT 0,relay_seq INT,spam INT DEFAULT 0,room_id TEXT,archived_at TEXT,PRIMARY KEY(envelope_id,direction));
         CREATE TABLE meta (key TEXT PRIMARY KEY,value TEXT NOT NULL);",
    ).unwrap();
    let ins = |env: &str, from: &str, verified: i64, seq: i64, spam: i64| {
        conn.execute(
            "INSERT INTO messages VALUES (?1,'received','th',?2,?2,'me',?3,'{\"type\":\"text\",\"text\":\"hi\"}',1,?4,0,?5,?6,NULL,?3)",
            rusqlite::params![env, from, format!("2026-06-11T00:00:0{seq}Z"), verified, seq, spam],
        ).unwrap();
    };
    ins("e1", "did:pinned", 1, 1, 0);
    ins("e2", "did:unverified", 0, 2, 0);
    ins("e3", "did:blocked", 1, 3, 0);
    ins("room1:joined", "did:pinned", 1, 4, 0);
    ins("e6", "did:pinned", 1, 5, 1);
    fs::write(home.path().join("contacts.json"),
        r#"{"version":1,"contacts":{"did:pinned":{"alias":"pat"},"did:blocked":{"alias":"mal"}}}"#).unwrap();
    fs::write(home.path().join("blocklist.json"),
        r#"{"version":1,"blocked":{"did:blocked":{"air_id":"AIR-MAL"}}}"#).unwrap();
}

#[test]
fn gap_replay_applies_all_five_invariants() {
    let home = TempDir::new().unwrap();
    seed(&home);
    let reader = ArchiveReader::open(home.path()).unwrap();
    let mut r = Replayer::new(HashSet::new());
    let out = r.gap(&reader, home.path(), 0).unwrap();
    let ids: Vec<_> = out.iter().map(|m| m.envelope_id.as_str()).collect();
    assert_eq!(ids, vec!["e1"]);
    assert_eq!(out[0].contact.as_deref(), Some("pat"));
}

#[test]
fn dedup_prevents_double_push_across_live_and_replay() {
    let home = TempDir::new().unwrap();
    seed(&home);
    let reader = ArchiveReader::open(home.path()).unwrap();
    let mut r = Replayer::new(HashSet::new());
    let live = Message {
        seq: 1, relay_seq: 1, envelope_id: "e1".into(), from: "did:pinned".into(),
        verified: true, encrypted: true, received_at: "t".into(), contact: Some("pat".into()),
        key_changed: None, thread_id: None, room_id: None, body: None,
    };
    assert!(r.live(live).is_some());
    let out = r.gap(&reader, home.path(), 0).unwrap();
    assert!(out.iter().all(|m| m.envelope_id != "e1"));
}

#[test]
fn unpinned_after_receipt_is_withheld_on_replay() {
    let home = TempDir::new().unwrap();
    seed(&home);
    fs::write(home.path().join("contacts.json"), r#"{"version":1,"contacts":{}}"#).unwrap();
    let reader = ArchiveReader::open(home.path()).unwrap();
    let mut r = Replayer::new(HashSet::new());
    let out = r.gap(&reader, home.path(), 0).unwrap();
    assert!(out.is_empty(), "an unpinned-after-receipt sender must be withheld on replay");
}

/// CF3: the replayed `received_at` must be the daemon's INGEST time (`archived_at`), NOT the
/// sender-chosen `timestamp` (which a verified sender can future-date). Seed one admitted row whose
/// `timestamp` is far in the future but whose `archived_at` is a real past ingest time, and prove the
/// replayed message carries the ingest time — so a forged future `timestamp` can't qualify a stale
/// replayed message as "recent" for the D11 recency guard.
#[test]
fn replay_received_at_is_ingest_time_not_sender_timestamp() {
    let home = TempDir::new().unwrap();
    let conn = Connection::open(home.path().join("archive.db")).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(
        "CREATE TABLE messages (envelope_id TEXT,direction TEXT,thread_id TEXT,peer_did TEXT,from_did TEXT,to_did TEXT,timestamp TEXT,body_json TEXT,encrypted INT,verified INT,key_changed INT DEFAULT 0,relay_seq INT,spam INT DEFAULT 0,room_id TEXT,archived_at TEXT,PRIMARY KEY(envelope_id,direction));
         CREATE TABLE meta (key TEXT PRIMARY KEY,value TEXT NOT NULL);",
    ).unwrap();
    let future_sender_ts = "2999-01-01T00:00:00Z"; // attacker-controlled envelope.timestamp
    let real_ingest_ts = "2026-06-11T00:00:01Z"; // daemon-set archived_at
    conn.execute(
        "INSERT INTO messages VALUES ('e1','received','th','did:pinned','did:pinned','me',?1,'{\"type\":\"text\",\"text\":\"hi\"}',1,1,0,1,0,NULL,?2)",
        rusqlite::params![future_sender_ts, real_ingest_ts],
    ).unwrap();
    fs::write(home.path().join("contacts.json"),
        r#"{"version":1,"contacts":{"did:pinned":{"alias":"pat"}}}"#).unwrap();

    let reader = ArchiveReader::open(home.path()).unwrap();
    let mut r = Replayer::new(HashSet::new());
    let out = r.gap(&reader, home.path(), 0).unwrap();

    assert_eq!(out.len(), 1, "the admitted row should replay");
    assert_eq!(
        out[0].received_at, real_ingest_ts,
        "received_at must be the daemon ingest time (archived_at), not the sender timestamp"
    );
    assert_ne!(
        out[0].received_at, future_sender_ts,
        "a future-dated sender timestamp must NOT leak into received_at"
    );
}
