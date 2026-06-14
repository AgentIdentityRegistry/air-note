use bossclaw_core::store::Store;
use std::io::Read;

fn dek() -> [u8; 32] {
    [42u8; 32]
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
