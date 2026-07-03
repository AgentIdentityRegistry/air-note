//! Synthetic known-item queries: 1–2 per SAMPLED page, stratified across top-level category
//! dirs AND language, source page = gold (spec §3). Page SELECTION is seeded + deterministic;
//! generation is a trait seam (tests never need Ollama).

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

/// Deterministically select up to `total` pages, stratified by top-level category dir:
/// round-robin one page per category (seeded shuffle within each) until `total` — every
/// category with pages is represented before any gets a second pick.
pub fn stratified_sample<R: Rng>(pages: &[PageRef], total: usize, rng: &mut R) -> Vec<PageRef> {
    let mut buckets: BTreeMap<String, Vec<PageRef>> = BTreeMap::new();
    for p in pages {
        let cat = p.page_id.split('/').next().unwrap_or("").to_string();
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
        let excerpt: String = page_text.chars().take(4000).collect();
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
        ]
        .into_iter()
        .map(|(id, l)| PageRef { page_id: id.into(), lang: l.into() })
        .collect();
        let mut r1 = ChaCha8Rng::seed_from_u64(42);
        let mut r2 = ChaCha8Rng::seed_from_u64(42);
        let sel1 = stratified_sample(&pages, 4, &mut r1);
        assert_eq!(sel1, stratified_sample(&pages, 4, &mut r2), "seeded → deterministic");
        let cats: std::collections::HashSet<_> =
            sel1.iter().map(|p| p.page_id.split('/').next().unwrap()).collect();
        assert!(cats.contains("air") && cats.contains("people") && cats.contains("ko"),
            "every category represented before any gets a second pick");
    }
}
