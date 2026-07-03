//! Hermetic e2e #1: mini-corpus → REAL in-process daemon → real wire ingest → ListFiles bridge
//! → real recall → the REAL resolver path (map_hits → dedup_by_page → gold_rank) → scoring.
//!
//! MOCK EMBEDDER: PLUMBING TEST ONLY — quality numbers come from the live run with the real
//! embedder (spec §1 Rev 2). No Ollama / gbrain / Anthropic anywhere.
#![cfg(unix)]

use std::path::Path;

use memharness::arms::{dedup_by_page, gold_rank, map_hits};
use memharness::client::WireClient;
use memharness::corpus::prepare_corpus;
use memharness::daemon::HarnessDaemon;
use memharness::resolve::PageResolver;
use memharness::stats::mean_success_at_k;

#[tokio::test]
async fn gold_page_is_found_through_the_real_resolver_path() {
    // 1. Prepare the mini-corpus into the daemon's home (strips frontmatter, manifest).
    let mut daemon = HarnessDaemon::spawn_mock_for_plumbing_tests().unwrap();
    let corpus_home = daemon.home().join("corpus");
    std::fs::create_dir_all(&corpus_home).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini_corpus");
    let manifest = prepare_corpus(&fixture, &corpus_home, true).unwrap();
    assert_eq!(manifest.file_count, 3, "3 mini pages prepared");
    let alpha = std::fs::read_to_string(corpus_home.join("en/alpha.md")).unwrap();
    assert!(!alpha.starts_with("---"), "frontmatter stripped in the prepared copy");

    // 2. Grant + ingest + ListFiles over the REAL wire; build the REAL bridge.
    let mut client = WireClient::connect(daemon.socket_path()).await.unwrap();
    client.add_grant(&corpus_home).await.unwrap();
    let report = client.run_ingest().await.unwrap();
    assert_eq!(report.ingested, 3, "all 3 pages ingested over the wire");
    let records = client.list_files().await.unwrap();
    assert_eq!(records.len(), 3);
    let resolver = PageResolver::from_file_records(&records, &corpus_home).unwrap();

    // 3. Recall → the REAL mapping (fail-loud). k=10 ≥ corpus size, so with the mock
    //    embedder + keyword source the gold page is retrievable; EVERY hit must map (the
    //    no-evolve invariant, exercised live here).
    let wire_hits = client.recall("ferris crab alpha particles", 10).await.unwrap();
    assert!(!wire_hits.is_empty(), "recall returned hits");
    let hits = dedup_by_page(map_hits(&resolver, wire_hits).unwrap());

    // 4. The gold is found through the REAL path — no hand-built vec (Rev 2, finding 2).
    let rank = gold_rank(&hits, "en/alpha");
    assert!(rank.is_some(), "gold page en/alpha retrieved and resolved: {hits:?}");
    assert!((mean_success_at_k(&[rank], 10) - 1.0).abs() < 1e-9);

    drop(client);
    daemon.kill();
}
