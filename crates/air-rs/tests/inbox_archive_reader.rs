use air_rs::inbox::archive_reader::{archive_exists, ArchiveReader};
use rusqlite::Connection;
use tempfile::TempDir;

/// Build an archive.db exactly as the daemon would (schema from archive.mjs + migrations, WAL set
/// writer-side). Returns the temp home holding it.
fn seed_archive() -> TempDir {
    let home = TempDir::new().unwrap();
    let path = home.path().join("archive.db");
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "busy_timeout", 5000).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(
        "CREATE TABLE messages (
            envelope_id TEXT NOT NULL, direction TEXT NOT NULL, thread_id TEXT NOT NULL,
            peer_did TEXT NOT NULL, from_did TEXT NOT NULL, to_did TEXT NOT NULL,
            timestamp TEXT NOT NULL, body_json TEXT NOT NULL, encrypted INTEGER NOT NULL,
            verified INTEGER NOT NULL, key_changed INTEGER NOT NULL DEFAULT 0, relay_seq INTEGER,
            spam INTEGER NOT NULL DEFAULT 0, room_id TEXT, archived_at TEXT NOT NULL,
            PRIMARY KEY (envelope_id, direction));
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    ).unwrap();
    let mut ins = |env: &str, dir: &str, from: &str, relay_seq: Option<i64>, verified: i64, key_changed: i64, spam: i64, ts: &str| {
        conn.execute(
            "INSERT INTO messages (envelope_id,direction,thread_id,peer_did,from_did,to_did,timestamp,body_json,encrypted,verified,key_changed,relay_seq,spam,room_id,archived_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,1,?9,?10,?11,?12,NULL,?7)",
            rusqlite::params![env, dir, "th", from, from, "me", ts, r#"{"type":"text","text":"hi"}"#, verified, key_changed, relay_seq, spam],
        ).unwrap();
    };
    ins("e1", "received", "did:peer", Some(1), 1, 0, 0, "2026-06-11T00:00:01Z");
    ins("e2", "received", "did:peer", Some(2), 1, 0, 0, "2026-06-11T00:00:02Z");
    ins("e3", "sent",     "did:peer", Some(3), 1, 0, 0, "2026-06-11T00:00:03Z");
    ins("e4", "received", "did:peer", Some(4), 1, 0, 1, "2026-06-11T00:00:04Z");
    ins("room1:joined", "received", "did:peer", Some(5), 1, 0, 0, "2026-06-11T00:00:05Z");
    conn.execute("INSERT INTO meta (key,value) VALUES ('pull_cursor','2')", []).unwrap();
    home
}

#[test]
fn archive_exists_probe() {
    let h = TempDir::new().unwrap();
    assert!(!archive_exists(h.path()));
    let s = seed_archive();
    assert!(archive_exists(s.path()));
}

#[test]
fn replay_since_applies_sql_invariants_1_to_3() {
    let h = seed_archive();
    let r = ArchiveReader::open(h.path()).unwrap();
    let rows = r.replay_since(0, 500).unwrap();
    let ids: Vec<_> = rows.iter().map(|x| x.envelope_id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2"]);
    assert_eq!(r.replay_since(1, 500).unwrap().iter().map(|x| x.envelope_id.clone()).collect::<Vec<_>>(), vec!["e2"]);
}

#[test]
fn get_cursor_reads_meta() {
    let h = seed_archive();
    let r = ArchiveReader::open(h.path()).unwrap();
    assert_eq!(r.get_cursor().unwrap(), 2);
}

#[test]
fn history_reads_back_and_conversations_group_by_peer_not_thread() {
    let h = seed_archive();
    {
        let w = Connection::open(h.path().join("archive.db")).unwrap();
        for (env, thread, sec) in [("c1", "thread-A", 10), ("c2", "thread-B", 11)] {
            w.execute(
                "INSERT INTO messages (envelope_id,direction,thread_id,peer_did,from_did,to_did,timestamp,body_json,encrypted,verified,key_changed,relay_seq,spam,room_id,archived_at) \
                 VALUES (?1,'received',?2,'did:peer','did:peer','me',?3,'{\"type\":\"text\",\"text\":\"x\"}',1,1,0,?4,0,NULL,?3)",
                rusqlite::params![env, thread, format!("2026-06-11T00:02:{sec}Z"), sec],
            ).unwrap();
        }
    }
    let r = ArchiveReader::open(h.path()).unwrap();
    let hist = r.history(Some("did:peer"), None, None, None, 50, false).unwrap();
    assert!(hist.iter().all(|x| !x.spam));
    assert!(hist.iter().any(|x| x.envelope_id == "e3"));
    let convs = r.conversations().unwrap();
    let peer_convs: Vec<_> = convs.iter().filter(|c| c.conv_key == "did:peer").collect();
    assert_eq!(peer_convs.len(), 1, "different thread_ids for one peer must NOT fragment the sidebar");
    assert_eq!(peer_convs[0].kind, "peer");
}

#[test]
fn reads_during_same_process_write_fast_check() {
    let h = seed_archive();
    let writer = Connection::open(h.path().join("archive.db")).unwrap();
    writer.busy_timeout(std::time::Duration::from_millis(5000)).unwrap();
    let reader = ArchiveReader::open(h.path()).unwrap();
    for i in 6..40 {
        writer.execute(
            "INSERT INTO messages (envelope_id,direction,thread_id,peer_did,from_did,to_did,timestamp,body_json,encrypted,verified,key_changed,relay_seq,spam,room_id,archived_at) \
             VALUES (?1,'received','th','did:peer','did:peer','me',?2,'{\"type\":\"text\",\"text\":\"x\"}',1,1,0,?3,0,NULL,?2)",
            rusqlite::params![format!("e{i}"), format!("2026-06-11T00:01:{:02}Z", i), i],
        ).unwrap();
        let rows = reader.replay_since(0, 500).unwrap();
        assert!(rows.len() >= 2, "reader must keep seeing rows during writes");
    }
}

#[test]
fn reads_while_a_separate_process_writes() {
    use std::process::Command;
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipping cross-process soak: python3 not found");
        return;
    }
    let h = seed_archive();
    let db = h.path().join("archive.db");
    let script = h.path().join("writer.py");
    std::fs::write(&script, r#"
import sqlite3, sys, time
c = sqlite3.connect(sys.argv[1])
c.execute("PRAGMA busy_timeout=5000")
c.execute("PRAGMA journal_mode=WAL")
body = '{"type":"text","text":"x"}'
for i in range(100, 160):
    c.execute(
        "INSERT INTO messages (envelope_id,direction,thread_id,peer_did,from_did,to_did,timestamp,body_json,encrypted,verified,key_changed,relay_seq,spam,room_id,archived_at) "
        "VALUES (?,'received','th','did:peer','did:peer','me',?,?,1,1,0,?,0,NULL,?)",
        ("e" + str(i), "2026-06-11T00:03:" + str(i), body, i, "2026-06-11T00:03:" + str(i)),
    )
    c.commit()
    time.sleep(0.04)
"#).unwrap();
    let mut child = Command::new("python3").arg(&script).arg(&db).spawn().unwrap();
    let reader = ArchiveReader::open(h.path()).unwrap();
    let mut peak = 0usize;
    for _ in 0..60 {
        peak = peak.max(reader.replay_since(0, 1000).unwrap().len());
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.wait();
    let final_n = reader.replay_since(0, 1000).unwrap().len();
    assert!(final_n >= 60, "RO reader must read rows a SEPARATE process wrote (final {final_n}, peak {peak})");
}
