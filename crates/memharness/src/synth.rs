//! Synthetic known-item queries: 1–2 per SAMPLED page, stratified by top-level category dir
//! (seeded round-robin); language is carried per-page and measured POST-HOC as report
//! segments — KO coverage depends on the categories picked, so the run reports per-language
//! sample counts (an underpowered KO segment is visible, never silent). Source page = gold
//! (spec §3). Page SELECTION is seeded + deterministic; generation is a trait seam (tests
//! never need Ollama).

use std::collections::BTreeMap;

use rand::seq::SliceRandom;
use rand::Rng;

/// A page eligible for synthetic-query generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRef {
    pub page_id: String,
    pub lang: String, // "en" | "ko"
}

/// A synthetic query: generated text + source page (gold) + language.
#[derive(Debug, Clone)]
pub struct SynthQuery {
    pub text: String,
    pub gold_page_id: String,
    pub lang: String,
}

/// Bucket key for page ids with no '/' — ONE shared bucket, so a handful of root-level
/// index/meta pages don't each become a guaranteed round-1 singleton pick.
const ROOT_BUCKET: &str = "(root)";

/// Deterministically select up to `total` pages, stratified by top-level category dir:
/// round-robin one page per category (seeded shuffle within each) until `total` — every
/// category with pages is represented before any gets a second pick.
pub fn stratified_sample<R: Rng>(pages: &[PageRef], total: usize, rng: &mut R) -> Vec<PageRef> {
    let mut buckets: BTreeMap<String, Vec<PageRef>> = BTreeMap::new();
    for p in pages {
        let cat = match p.page_id.split_once('/') {
            Some((cat, _)) => cat.to_string(),
            None => ROOT_BUCKET.to_string(),
        };
        buckets.entry(cat).or_default().push(p.clone());
    }
    for v in buckets.values_mut() {
        v.shuffle(rng);
    }
    let mut cat_order: Vec<String> = buckets.keys().cloned().collect();
    cat_order.shuffle(rng);
    let mut cursors: BTreeMap<String, usize> =
        cat_order.iter().map(|c| (c.clone(), 0)).collect();
    let mut selected = Vec::new();
    while selected.len() < total {
        let mut progressed = false;
        for cat in &cat_order {
            if selected.len() >= total {
                break;
            }
            let bucket = &buckets[cat];
            let cur = cursors.get_mut(cat).expect("cursor exists");
            if *cur < bucket.len() {
                selected.push(bucket[*cur].clone());
                *cur += 1;
                progressed = true;
            }
        }
        if !progressed {
            break; // all buckets exhausted
        }
    }
    selected
}

/// The generation seam: 1–2 known-item queries for a page. Live = Ollama; tests inject doubles.
pub trait QueryGenerator {
    fn generate_queries(&self, page: &PageRef, page_text: &str) -> anyhow::Result<Vec<SynthQuery>>;
}

/// Char cap on the page excerpt fed to the query generator — keeps 7B prompts bounded.
/// Semantically unrelated to `arms::PER_SNIPPET_CHAR_CAP` (a retrieval-packing cap) — do
/// NOT alias them.
const GENERATOR_EXCERPT_CHAR_CAP: usize = 4000;

/// Live generator: asks the local model for ONE specific question the page answers, in the
/// page's language (Korean pages get Korean queries, spec §3).
pub struct OllamaQueryGenerator {
    pub model: String,
}

impl QueryGenerator for OllamaQueryGenerator {
    fn generate_queries(&self, page: &PageRef, page_text: &str) -> anyhow::Result<Vec<SynthQuery>> {
        let lang_instr = if page.lang == "ko" {
            "Write the question in Korean."
        } else {
            "Write the question in English."
        };
        let excerpt: String = page_text.chars().take(GENERATOR_EXCERPT_CHAR_CAP).collect();
        let prompt = format!(
            "Read the note below. Write ONE specific question that this note (and ideally only \
             this note) answers. {lang_instr} Output only the question.\n\nNote:\n{excerpt}\n"
        );
        let text = crate::ollama::generate(&self.model, &prompt)?.trim().to_string();
        if text.is_empty() {
            anyhow::bail!("generator returned an empty query for {}", page.page_id);
        }
        Ok(vec![SynthQuery { text, gold_page_id: page.page_id.clone(), lang: page.lang.clone() }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn stratified_selection_is_seeded_and_covers_categories() {
        let pages: Vec<PageRef> = [
            ("air/a", "en"), ("air/b", "en"), ("air/c", "en"),
            ("people/d", "en"), ("people/e", "en"),
            ("ko/f", "ko"),
            ("claude", "en"), ("concepts", "en"), // no '/' — must SHARE one "(root)" bucket
        ]
        .into_iter()
        .map(|(id, l)| PageRef { page_id: id.into(), lang: l.into() })
        .collect();
        let mut r1 = ChaCha8Rng::seed_from_u64(42);
        let mut r2 = ChaCha8Rng::seed_from_u64(42);
        // total = number of buckets {air, people, ko, (root)} → round 1 picks one per bucket.
        let sel1 = stratified_sample(&pages, 4, &mut r1);
        assert_eq!(sel1, stratified_sample(&pages, 4, &mut r2), "seeded → deterministic");
        let cats: std::collections::HashSet<_> = sel1
            .iter()
            .filter_map(|p| p.page_id.split_once('/').map(|(cat, _)| cat))
            .collect();
        assert!(cats.contains("air") && cats.contains("people") && cats.contains("ko"),
            "every category represented before any gets a second pick");
        let root_picks = sel1.iter().filter(|p| !p.page_id.contains('/')).count();
        assert_eq!(root_picks, 1,
            "root-level pages share ONE bucket — exactly one round-1 pick, never two");
    }
}
