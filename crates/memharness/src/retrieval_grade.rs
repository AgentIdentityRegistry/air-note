//! Judge-FREE retrieval grader (Rung-3 §9/§13): does the body-passage conflict index surface
//! conflicts that live in the session BODY — where a GENERIC title misses them — better than a
//! title-only index, WITHOUT over-surfacing legitimately coexisting facts (the hard negatives)?
//!
//! This is a STRUCTURAL claim, not a semantic-paraphrase one, so it runs hermetically on
//! [`MockEmbedder`] (bossclaw-core's deterministic bag-of-words FNV embedder). MockEmbedder is a
//! FAITHFUL stand-in for THIS claim: the property under test is WHERE the conflicting vocabulary
//! lives — in the body vs absent from a generic title — which is a LEXICAL property, exactly what a
//! bag-of-words embedder measures. Paraphrase-semantic quality is measured ELSEWHERE (the
//! frozen-corpus rung gates + the P0 judge harness in `conflict_grade.rs`). The fixture's separating
//! signal is lexical BY DESIGN — each `contradicts` query shares distinctive tokens with its BODY
//! (never with its generic title); each `coexist` hard-negative's query shares tokens with NOTHING
//! — so the bars hold by construction, deterministically, in CI, with zero model files. It is NOT
//! tuned to pass: a failing bar is fixed by editing the fixture's vocabulary, never the thresholds.

use anyhow::Context as _;

use bossclaw_core::index::{encode_chunk_key, HnswIndex, VectorIndex};
use bossclaw_core::log::{EventLog, SessionMeta};
use bossclaw_core::{chunk_text, Embedder, MockEmbedder};
use ed25519_dalek::SigningKey;

/// Embedding width for the hermetic grader. Large enough that the fixture's distinct tokens almost
/// never collide into the same FNV bucket (a bucket collision would FAKE a lexical match); the
/// `#[test]` is the verification — it clears the bars with margin only when no harmful collision
/// occurred. Not a tuning knob for the pass/fail bars.
const GRADE_DIM: usize = 16384;

/// Retrieval floor separating GENUINE vocabulary overlap from orthogonality. With L2-normalised
/// bag-of-words vectors, a query that shares ≥1 token with a passage has cosine ≥ ~0.15; a query
/// that shares NONE is orthogonal (cosine 0). `0.05` cleanly rejects the orthogonal case. This is a
/// FIXED retrieval-front-end parameter (a real conflict detector only surfaces a candidate that
/// actually matches), NOT a per-fixture tuning knob: it makes `coexist` hard-negatives
/// (zero-overlap queries) DETERMINISTICALLY non-matching regardless of HNSW rank order.
const MATCH_FLOOR: f32 = 0.05;

/// `k` for every conflict/title search. Generous (≥ the fixture's session count) so the matching
/// passage is always among the returned candidates; the [`MATCH_FLOOR`] — not the rank cutoff —
/// does the gating, so HNSW deep-rank non-determinism cannot affect the verdict.
const SEARCH_K: usize = 64;

/// Ground-truth label for a session pair. `Contradicts` = a real body-level conflict the front-end
/// SHOULD surface; `Coexist` = a legitimate coexistence (a hard negative) it must NOT surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionLabel {
    Contradicts,
    Coexist,
}

/// One labelled session: a generic `title`, a `body` carrying the distinctive fact, and the
/// `conflicting_query` a later session would raise. For `contradicts` the query's distinctive
/// tokens live in the BODY (not the title); for `coexist` the query overlaps nothing.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SessionPair {
    pub session_id: String,
    pub title: String,
    pub body: String,
    pub conflicting_query: String,
    pub label: SessionLabel,
}

/// Which index the retrieval front-end searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retrieval {
    /// One vector per session = `embed(title)`. The pre-Rung-3 baseline arm.
    TitleOnly,
    /// The Rung-3 body-passage conflict index (`embed` of each body chunk).
    PassageIndex,
}

/// One arm's aggregate score. `recall` = fraction of `contradicts` pairs the arm flagged;
/// `precision` = flagged-contradicts / all-flagged (a flagged `coexist` is a false positive).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetrievalGrade {
    pub recall: f64,
    pub precision: f64,
}

/// Parse the session-conflict-pairs JSONL. Fails loud on an empty set (mirrors
/// [`crate::conflict_grade::parse_pairs`] — a silent 0-pair grade is a false "pass").
pub fn load_session_pairs(body: &str) -> anyhow::Result<Vec<SessionPair>> {
    let pairs: Vec<SessionPair> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<SessionPair>(l).map_err(anyhow::Error::from))
        .collect::<anyhow::Result<_>>()
        .context("parsing session-conflict pairs JSONL")?;
    anyhow::ensure!(!pairs.is_empty(), "no session-conflict pairs found");
    Ok(pairs)
}

/// Grade one retrieval arm over the session pairs. A pair is FLAGGED when the arm surfaces the
/// pair's OWN session — for the pair's `conflicting_query` — with cosine similarity ≥
/// [`MATCH_FLOOR`]. Then `recall = flagged-contradicts / contradicts`, and
/// `precision = flagged-contradicts / all-flagged` (a flagged `coexist` is an over-surfaced
/// legitimate coexistence = a false positive). Fully deterministic on [`MockEmbedder`].
pub fn grade_retrieval(pairs: &[SessionPair], mode: Retrieval) -> RetrievalGrade {
    let emb = MockEmbedder::new(GRADE_DIM);
    let flagged: Vec<bool> = match mode {
        Retrieval::TitleOnly => flag_title_only(pairs, &emb),
        Retrieval::PassageIndex => flag_passage_index(pairs, &emb),
    };
    grade_from_flags(pairs, &flagged)
}

/// TITLE-ONLY arm: one vector per session = `embed(title)`, keyed by the session id. The ONLY
/// signal is the (generic) title, so a `contradicts` query whose distinctive tokens live in the
/// body — not the title — cannot match: title-only genuinely misses body conflicts.
fn flag_title_only(pairs: &[SessionPair], emb: &MockEmbedder) -> Vec<bool> {
    let mut index = HnswIndex::with_capacity(pairs.len());
    for p in pairs {
        index.add(&encode_chunk_key(&p.session_id, 0), &embed_one(emb, &p.title));
    }
    pairs
        .iter()
        .map(|p| {
            let want = encode_chunk_key(&p.session_id, 0);
            let qv = embed_one(emb, &p.conflicting_query);
            index
                .search(&qv, SEARCH_K)
                .into_iter()
                .any(|(key, dist)| key == want && sim(dist) >= MATCH_FLOOR)
        })
        .collect()
}

/// PASSAGE-INDEX arm: capture every session (title → recall event) + persist its BODY passages,
/// build the ONE shared conflict index over all sessions' passages, then ask whether each pair's
/// `conflicting_query` surfaces the pair's own session. The conflicting fact lives in the body, so
/// this arm catches it. Uses the real [`EventLog`] end-to-end (no wire op) over a throwaway tempdir.
fn flag_passage_index(pairs: &[SessionPair], emb: &MockEmbedder) -> Vec<bool> {
    let dir = tempfile::tempdir().expect("hermetic grader: tempdir");
    let log = open_grader_log(dir.path());
    for p in pairs {
        let ev = log
            .capture_session(emb, &session_meta_for(p))
            .expect("hermetic grader: capture_session");
        let chunks = chunk_text(&p.body);
        // A non-empty body always yields ≥1 chunk; assert so an accidentally-blank fixture body
        // fails LOUD rather than silently contributing no passage (a silent 0-passage session
        // would deflate recall without any signal that the fixture is broken).
        assert!(!chunks.is_empty(), "fixture body for {} produced no passages", p.session_id);
        log.store_session_passages(emb, &ev, &chunks)
            .expect("hermetic grader: store_session_passages");
    }
    log.rebuild_conflict_index(emb).expect("hermetic grader: rebuild_conflict_index");
    pairs
        .iter()
        .map(|p| {
            let qv = embed_one(emb, &p.conflicting_query);
            log.conflict_search(&qv, SEARCH_K)
                .into_iter()
                .any(|(sid, _ix, dist)| sid == p.session_id && sim(dist) >= MATCH_FLOOR)
        })
        .collect()
}

/// Fold per-pair flags into recall/precision. TP = flagged `contradicts`; FP = flagged `coexist`;
/// recall over `contradicts` pairs, precision over ALL flags. 0/0 → 0.0 (an arm that flags nothing
/// has precision 0, not NaN — mirrors [`crate::conflict_grade`]'s ratio convention).
fn grade_from_flags(pairs: &[SessionPair], flagged: &[bool]) -> RetrievalGrade {
    let mut contradicts_total = 0usize;
    let mut tp = 0usize; // flagged contradicts
    let mut fp = 0usize; // flagged coexist (over-surfaced hard negative)
    for (p, &f) in pairs.iter().zip(flagged) {
        match p.label {
            SessionLabel::Contradicts => {
                contradicts_total += 1;
                if f {
                    tp += 1;
                }
            }
            SessionLabel::Coexist => {
                if f {
                    fp += 1;
                }
            }
        }
    }
    RetrievalGrade {
        recall: ratio(tp, contradicts_total),
        precision: ratio(tp, tp + fp),
    }
}

/// n/d as f64, with 0/0 → 0.0 (no positives or no flags → the metric is 0, not NaN).
fn ratio(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        n as f64 / d as f64
    }
}

/// DistCosine distance ∈ [0, 2] → cosine similarity = 1 − distance (see `resolve_mention`).
fn sim(dist: f32) -> f32 {
    1.0 - dist
}

/// Embed one text to its vector (the hermetic grader never fails to embed a non-empty string).
fn embed_one(emb: &MockEmbedder, text: &str) -> Vec<f32> {
    emb.embed(&[text.to_string()]).expect("hermetic grader: embed")[0].clone()
}

/// A `SessionMeta` for a fixture pair. The `sha256` embeds the session id so every session is a
/// DISTINCT capture (no A2 same-sha dedup collapse). Timestamps/paths are inert metadata.
fn session_meta_for(p: &SessionPair) -> SessionMeta {
    SessionMeta {
        session_id: p.session_id.clone(),
        title: p.title.clone(),
        project: "/grader".into(),
        tool: "memharness".into(),
        started_at: 1,
        ended_at: 2,
        path: format!("/grader/{}.md", p.session_id),
        sha256: format!("sha-{}", p.session_id),
        approx_bytes: p.body.len() as u64,
    }
}

/// Open a raw in-process [`EventLog`] over a throwaway tempdir with FIXED, non-secret dev keys —
/// this is a hermetic grader over a disposable store, never a real user log. Mirrors
/// bossclaw-core's own `open_log` test helper.
fn open_grader_log(dir: &std::path::Path) -> EventLog {
    let dek = [42u8; 32];
    let key = SigningKey::from_bytes(&[7u8; 32]);
    EventLog::open(&dir.join("grader.db"), &dek, key).expect("hermetic grader: open EventLog")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_session_pairs_parses_and_fails_loud_on_empty() {
        let jsonl = "\
{\"session_id\":\"s1\",\"title\":\"deployment notes\",\"body\":\"we moved to vercel\",\"conflicting_query\":\"vercel\",\"label\":\"contradicts\"}
{\"session_id\":\"s2\",\"title\":\"sync notes\",\"body\":\"the app got offline drafts\",\"conflicting_query\":\"budget forecast\",\"label\":\"coexist\"}
";
        let pairs = load_session_pairs(jsonl).expect("parse");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].label, SessionLabel::Contradicts);
        assert_eq!(pairs[1].label, SessionLabel::Coexist);
        // Empty input is a loud error, never a silent zero-pair grade (memharness convention).
        assert!(load_session_pairs("   \n").is_err());
    }

    #[test]
    fn passage_index_meaningfully_beats_title_only() {
        // MockEmbedder is a FAITHFUL stand-in for THIS structural claim: whether the conflicting
        // vocabulary lives in the BODY (caught by the passage index) or is absent from a GENERIC
        // title (missed by title-only) is a LEXICAL property, exactly what a bag-of-words embedder
        // measures. Paraphrase-semantic quality is measured elsewhere (the frozen-corpus rung gates
        // + the P0 judge harness). The fixture's separating signal is lexical BY DESIGN — NOT tuned
        // to pass: a failing bar is fixed by editing the fixture's vocabulary, never the bars here.
        let f = load_session_pairs(include_str!("../fixtures/session-conflict-pairs.jsonl"))
            .expect("fixture loads");
        let (title, passage) = (
            grade_retrieval(&f, Retrieval::TitleOnly),
            grade_retrieval(&f, Retrieval::PassageIndex),
        );
        assert!(title.recall < 0.30, "title-only genuinely misses body conflicts: {}", title.recall);
        assert!(passage.recall >= 0.70, "passage index catches them: {}", passage.recall);
        assert!(
            passage.precision >= title.precision,
            "passage index does not over-surface (hard negatives): passage={} title={}",
            passage.precision,
            title.precision,
        );
    }
}
