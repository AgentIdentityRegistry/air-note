//! Recall@K empirical gate — `#[ignore]` harness for real embedding models.
//!
//! These tests exercise the full recall pipeline (append → rederive → rebuild →
//! recall) against the labelled corpus in `fixtures/recall.json`. They are
//! excluded from the hermetic default suite (`cargo test`) because they require
//! a real model on disk (Model2Vec) or an ONNX download (FastEmbed).
//!
//! # Running the Model2Vec gate
//!
//! ```sh
//! # 1. Fetch the model (requires Python + huggingface_hub):
//! #    pip install huggingface_hub
//! #    python -c "from huggingface_hub import snapshot_download; \
//! #               print(snapshot_download('minishlab/potion-base-8M'))"
//! #
//! # 2. Run with the path printed above:
//! BOSSCLAW_TEST_MODEL_DIR=<path> \
//!   cargo test -p bossclaw-core \
//!     --test recall_fixture \
//!     -- --ignored --nocapture
//! ```
//!
//! # Running the FastEmbed gate (optional, downloads ONNX model on first run)
//!
//! ```sh
//! cargo test -p bossclaw-core --features fastembed \
//!   --test recall_fixture \
//!   -- --ignored --nocapture
//! ```

use std::collections::HashMap;

use bossclaw_core::log::EventLog;
use bossclaw_core::recall::RecallOptions;
use bossclaw_core::Embedder;
use ed25519_dalek::SigningKey;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Fixture types
// ---------------------------------------------------------------------------

/// A single memory document in the labelled corpus.
#[derive(Debug, Deserialize)]
struct FixtureDoc {
    id: String,
    text: String,
}

/// A single query with its ground-truth relevant doc id(s).
#[derive(Debug, Deserialize)]
struct FixtureQuery {
    q: String,
    relevant: Vec<String>,
}

/// Root structure of `fixtures/recall.json`.
#[derive(Debug, Deserialize)]
struct RecallFixture {
    docs: Vec<FixtureDoc>,
    queries: Vec<FixtureQuery>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Encryption key used for the test `EventLog` (arbitrary, non-zero).
const DEK: [u8; 32] = [17u8; 32];

/// Signing key bytes for the test `EventLog`.
const KEY_BYTES: [u8; 32] = [23u8; 32];

/// Top-K for the recall@K metric.
///
/// K=3 is deliberately small — a good embedding model should rank the single
/// correct answer within the top 3 of a 15-document corpus.
const K: usize = 3;

/// Regression floor for Model2Vec (potion-base-8M) recall@K.
///
/// This is calibrated below the observed range so the test catches genuine
/// regressions without being brittle to HNSW reseed non-determinism on a
/// small corpus (hnsw_rs re-seeds its level-assignment RNG from OS
/// randomness at each `Hnsw::new` construction — same source of variation
/// documented in T5). It is NOT a quality target.
///
/// Observed across 5 consecutive runs: 1.0000 (8/8) every time.
/// Floor set at 0.625 (5/8) — well below the lowest observed value,
/// giving headroom for any run-to-run variation on tiny corpora.
const FLOOR: f32 = 0.625;

/// Regression floor for the FastEmbed (bge-small-en-v1.5) gate.
///
/// Set identically to the Model2Vec floor; tighten after observing a real run.
#[cfg(feature = "fastembed")]
const FASTEMBED_FLOOR: f32 = 0.625;

// ---------------------------------------------------------------------------
// Shared fixture loader
// ---------------------------------------------------------------------------

/// Parse the labelled corpus from the bundled JSON fixture.
fn load_fixture() -> RecallFixture {
    let raw = include_str!("fixtures/recall.json");
    serde_json::from_str(raw).expect("fixtures/recall.json must parse cleanly")
}

/// Append all docs from `fixture` to `log` as `memory` events (one per doc),
/// returning a map from fixture doc id (e.g. `"d01"`) to the assigned ULID
/// event id.
fn seed_log(log: &EventLog, fixture: &RecallFixture) -> HashMap<String, String> {
    use bossclaw_core::event::Event;

    fixture
        .docs
        .iter()
        .map(|doc| {
            let event = Event {
                id: String::new(),
                ts: String::new(),
                valid_time: None,
                event_type: "memory".to_string(),
                content: serde_json::json!({ "text": doc.text }),
                model_meta: None,
                prev_hash: String::new(),
                hash: None,
                signed_by_did: "did:wba:AIR-TEST".to_string(),
                signature: None,
            };
            let event_id = log.append(event).expect("append must succeed");
            (doc.id.clone(), event_id)
        })
        .collect()
}

/// Core metric: for each query, check whether any relevant event id appears
/// in the top-K recall results. Returns `(hits, total)`.
fn count_hits<E: bossclaw_core::Embedder>(
    log: &EventLog,
    embedder: &E,
    fixture: &RecallFixture,
    id_map: &HashMap<String, String>,
) -> (usize, usize) {
    let mut hit_count = 0usize;

    for fq in &fixture.queries {
        let results = log
            .recall(embedder, &fq.q, K, &RecallOptions::default())
            .expect("recall must not error");

        let result_ids: Vec<&str> = results.iter().map(|h| h.event_id.as_str()).collect();

        // A query "hits" if ANY of its relevant docs appears in the top-K.
        let hit = fq.relevant.iter().any(|rel_fixture_id| {
            id_map
                .get(rel_fixture_id)
                .map(|eid| result_ids.contains(&eid.as_str()))
                .unwrap_or(false)
        });

        if hit {
            hit_count += 1;
        }
    }

    (hit_count, fixture.queries.len())
}

// ---------------------------------------------------------------------------
// Model2Vec gate
// ---------------------------------------------------------------------------

/// Recall@K empirical gate for the default Model2Vec embedder
/// (`minishlab/potion-base-8M`).
///
/// Skipped automatically unless `BOSSCLAW_TEST_MODEL_DIR` points at a local
/// copy of the model directory. See the module-level doc for download
/// instructions.
///
/// The test appends all fixture docs to a fresh `EventLog`, derives vectors,
/// rebuilds both indexes, then for each query calls `recall(..., K, ...)` and
/// checks whether the ground-truth doc id is in the top-K results. It prints
/// `recall@K = hits/total` and asserts the result exceeds `FLOOR`.
#[test]
#[ignore = "requires BOSSCLAW_TEST_MODEL_DIR pointing at a local potion-base-8M model dir"]
fn model2vec_recall_at_k() {
    use bossclaw_core::Model2Vec;

    // ------------------------------------------------------------------
    // 1. Resolve model directory from env; skip gracefully if absent.
    // ------------------------------------------------------------------
    let model_dir = match std::env::var("BOSSCLAW_TEST_MODEL_DIR") {
        Ok(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => {
            eprintln!(
                "BOSSCLAW_TEST_MODEL_DIR not set — skipping model2vec_recall_at_k.\n\
                 See test module doc-comment for download instructions."
            );
            return;
        }
    };

    // ------------------------------------------------------------------
    // 2. Load the model.
    // ------------------------------------------------------------------
    let embedder = Model2Vec::from_pretrained(&model_dir, "potion-base-8M")
        .expect("Model2Vec::from_pretrained must succeed");

    eprintln!(
        "model2vec_recall_at_k: loaded model dim={}, model_id={}",
        embedder.dim(),
        embedder.model_id(),
    );

    // ------------------------------------------------------------------
    // 3. Build EventLog, seed corpus, derive vectors, rebuild indexes.
    // ------------------------------------------------------------------
    let fixture = load_fixture();
    let tmp = tempfile::tempdir().expect("tempdir");
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&tmp.path().join("recall_fixture.db"), &DEK, key)
        .expect("EventLog::open must succeed");

    let id_map = seed_log(&log, &fixture);

    let derived = log
        .rederive_pending(&embedder)
        .expect("rederive_pending must succeed");
    assert_eq!(
        derived,
        fixture.docs.len(),
        "all {n} docs must be embedded",
        n = fixture.docs.len()
    );

    log.rebuild_indexes(&embedder)
        .expect("rebuild_indexes must succeed");

    // ------------------------------------------------------------------
    // 4. Compute recall@K.
    // ------------------------------------------------------------------
    let (hits, total) = count_hits(&log, &embedder, &fixture, &id_map);
    let recall_at_k = hits as f32 / total as f32;

    println!(
        "\n=== model2vec recall@{K} ===\n\
         corpus: {} docs, {} queries\n\
         hits:   {hits}/{total}\n\
         recall@{K}: {recall_at_k:.4} (floor: {FLOOR:.4})\n",
        fixture.docs.len(),
        fixture.queries.len(),
    );

    assert!(
        recall_at_k >= FLOOR,
        "model2vec recall@{K} = {recall_at_k:.4} is below regression floor {FLOOR:.4} \
         ({hits}/{total} queries hit)"
    );
}

// ---------------------------------------------------------------------------
// FastEmbed gate (opt-in feature)
// ---------------------------------------------------------------------------

/// Recall@K empirical gate for the FastEmbed embedder (bge-small-en-v1.5).
///
/// Requires `--features fastembed`. Downloads the ONNX model on first run
/// (expected behaviour for this `#[ignore]` gate).
#[cfg(feature = "fastembed")]
#[test]
#[ignore = "requires --features fastembed; downloads bge-small-en-v1.5 ONNX model on first run"]
fn fastembed_recall_at_k() {
    use bossclaw_core::FastEmbed;

    // ------------------------------------------------------------------
    // 1. Instantiate FastEmbed (triggers ONNX download if not cached).
    // ------------------------------------------------------------------
    let embedder = FastEmbed::new().expect("FastEmbed::new must succeed");

    eprintln!(
        "fastembed_recall_at_k: loaded model dim={}, model_id={}",
        embedder.dim(),
        embedder.model_id(),
    );

    // ------------------------------------------------------------------
    // 2. Build EventLog, seed corpus, derive vectors, rebuild indexes.
    // ------------------------------------------------------------------
    let fixture = load_fixture();
    let tmp = tempfile::tempdir().expect("tempdir");
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&tmp.path().join("recall_fixture_fe.db"), &DEK, key)
        .expect("EventLog::open must succeed");

    let id_map = seed_log(&log, &fixture);

    let derived = log
        .rederive_pending(&embedder)
        .expect("rederive_pending must succeed");
    assert_eq!(
        derived,
        fixture.docs.len(),
        "all {n} docs must be embedded",
        n = fixture.docs.len()
    );

    log.rebuild_indexes(&embedder)
        .expect("rebuild_indexes must succeed");

    // ------------------------------------------------------------------
    // 3. Compute recall@K.
    // ------------------------------------------------------------------
    let (hits, total) = count_hits(&log, &embedder, &fixture, &id_map);
    let recall_at_k = hits as f32 / total as f32;

    println!(
        "\n=== fastembed recall@{K} ===\n\
         corpus: {} docs, {} queries\n\
         hits:   {hits}/{total}\n\
         recall@{K}: {recall_at_k:.4} (floor: {FASTEMBED_FLOOR:.4})\n",
        fixture.docs.len(),
        fixture.queries.len(),
    );

    assert!(
        recall_at_k >= FASTEMBED_FLOOR,
        "fastembed recall@{K} = {recall_at_k:.4} is below regression floor \
         {FASTEMBED_FLOOR:.4} ({hits}/{total} queries hit)"
    );
}

// ---------------------------------------------------------------------------
// §15 re-embed time budget (Model2Vec)
// ---------------------------------------------------------------------------

/// Measures `reembed_migration` throughput over the fixture corpus with the
/// default Model2Vec backend — the spec §15 re-embed time-budget figure.
///
/// `#[ignore]` (needs `BOSSCLAW_TEST_MODEL_DIR`). Prints `ReembedStats` so the
/// throughput can be recorded in the CHANGELOG and extrapolated to real corpus
/// sizes ("re-embedding N memories ≈ N / throughput seconds").
#[test]
#[ignore = "requires BOSSCLAW_TEST_MODEL_DIR pointing at a local potion-base-8M model dir"]
fn model2vec_reembed_budget() {
    use bossclaw_core::Model2Vec;

    let model_dir = match std::env::var("BOSSCLAW_TEST_MODEL_DIR") {
        Ok(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => {
            eprintln!("BOSSCLAW_TEST_MODEL_DIR not set — skipping model2vec_reembed_budget.");
            return;
        }
    };

    let embedder = Model2Vec::from_pretrained(&model_dir, "potion-base-8M")
        .expect("Model2Vec::from_pretrained must succeed");

    let fixture = load_fixture();
    let tmp = tempfile::tempdir().expect("tempdir");
    let key = SigningKey::from_bytes(&KEY_BYTES);
    let log = EventLog::open(&tmp.path().join("reembed_budget.db"), &DEK, key)
        .expect("EventLog::open must succeed");

    // Append the corpus (no vectors yet); the migration embeds everything.
    let _ = seed_log(&log, &fixture);

    let stats = log
        .reembed_migration(&embedder)
        .expect("reembed_migration must succeed");

    let per_sec = if stats.elapsed_ms > 0 {
        (stats.reembedded as f64) * 1000.0 / (stats.elapsed_ms as f64)
    } else {
        f64::INFINITY
    };

    println!(
        "\n=== model2vec re-embed budget (§15) ===\n\
         reembedded: {} docs\n\
         elapsed:    {} ms\n\
         throughput: {per_sec:.0} events/sec\n",
        stats.reembedded, stats.elapsed_ms,
    );

    assert_eq!(
        stats.reembedded,
        fixture.docs.len(),
        "all fixture docs must be re-embedded"
    );
}
