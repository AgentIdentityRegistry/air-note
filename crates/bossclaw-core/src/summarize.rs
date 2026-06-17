//! The summarizer pipeline (spec §5), PURE: build the compose prompt from a
//! bounded fact-set, parse the model's draft, run the deterministic citation
//! floor (Task 3), and assemble the surviving claims into a rendered dossier.
//! No SQL, no I/O — takes a [`FactSet`] + (in the caller) a [`crate::reason::Reasoner`].
//! Mirrors `extract.rs`. The model's prose is DATA, never authority (spec §8).

use std::collections::HashSet;

use crate::error::BossclawError;
use crate::graph::Entity;

/// How far a dossier reaches into the graph (spec §6). `Tight` is the v1 default;
/// `Wide` is a deferred dial flipped in dogfooding once real dossiers are visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageReach {
    /// Entity + its own edges + their lineage memories.
    Tight,
    /// Also fold 1-hop neighbors' lineage memories (deferred default).
    Wide,
}

/// Fact-set reach (spec §11). `Tight`: bounded, no cross-page duplication.
pub const PAGE_REACH: PageReach = PageReach::Tight;

/// Min facts (edges + lineage memories) before a topic earns a page (spec §11).
pub const PAGE_MIN_FACTS: usize = 2;

/// Cap on claims accepted from one draft (spec §11 / F7) — applied before signing.
pub const MAX_CLAIMS_PER_PAGE: usize = 32;

/// The bounded, already-signed inputs for ONE dossier (built by the evolve phase,
/// spec §6): the anchor entity, its current edges as lines, and the cited memory
/// texts. NEVER contains a `page` (the one-way rule, enforced upstream — F3).
pub struct FactSet {
    /// The topic this dossier is about.
    pub entity: Entity,
    /// Current edges as `src -relation-> dst` lines (each edge_id-backed).
    pub edges: Vec<String>,
    /// `(event_id, text)` of the cited memories.
    pub memories: Vec<(String, String)>,
}

impl FactSet {
    /// The set of every event id present (memory ids) — the citation floor's
    /// whitelist (spec §5/§8). Edge lines carry node ids, not citable event ids;
    /// the model cites the MEMORY ids it drew from.
    pub fn fact_ids(&self) -> HashSet<String> {
        self.memories.iter().map(|(id, _)| id.clone()).collect()
    }

    /// Total facts (edges + memories) — gates `PAGE_MIN_FACTS` (spec §6).
    pub fn fact_count(&self) -> usize {
        self.edges.len() + self.memories.len()
    }
}

/// A drafted dossier before the citation floor: the model's title + claims, each
/// attributed to the source event ids it drew from.
pub struct DraftPage {
    /// Proposed dossier title.
    pub title: String,
    /// Proposed claims (pre-floor).
    pub claims: Vec<DraftClaim>,
}

/// One drafted claim: a sentence + the event ids it cites.
pub struct DraftClaim {
    /// The synthesized sentence.
    pub text: String,
    /// The event ids this sentence draws from.
    pub cites: Vec<String>,
}

/// A dossier that cleared the floor: the rendered body + the union of surviving
/// cites (the page event's non-empty `source_event_ids`).
pub struct RenderedPage {
    /// The dossier title.
    pub title: String,
    /// The rendered markdown body (also the embedded text).
    pub text: String,
    /// Sorted+deduped union of surviving claims' cites (F7).
    pub cites: Vec<String>,
}

/// JSON Schema constraining the compose output (spec §5): `{title, claims:[{text,
/// cites:[string]}]}`. Passed to the backend as the structured-output constraint.
pub fn compose_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "claims": { "type": "array", "items": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "cites": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["text", "cites"]
            }}
        },
        "required": ["title", "claims"]
    })
}

/// Build the Pass-A compose prompt (spec §5): the fenced fact-set (each memory
/// tagged with its event id so the model can cite it; edges as lines) + the
/// instruction to write a concise dossier where EACH claim cites the source ids
/// it draws from. Untrusted memory text is fenced via the M4a source-fence helper.
pub fn build_compose_prompt(facts: &FactSet) -> String {
    let mut p = String::new();
    p.push_str(&format!(
        "Write a concise factual dossier about {} ({}). Output ONLY claims you can \
         support from the sources below; for EACH claim list the source ids (the \
         [id] tags) it draws from in `cites`. Do not invent facts or citations.\n\n",
        facts.entity.label, facts.entity.entity_type,
    ));
    if !facts.edges.is_empty() {
        p.push_str("Known relationships:\n");
        for e in &facts.edges {
            p.push_str(&format!("- {e}\n"));
        }
        p.push('\n');
    }
    p.push_str("Sources (cite by [id]):\n");
    for (id, text) in &facts.memories {
        p.push_str(&format!("[{id}] "));
        crate::extract::push_fenced_source(&mut p, text); // <<<SOURCE_BEGIN ... SOURCE_END>>>
        p.push('\n');
    }
    p
}

/// Parse a reasoner draft value into a [`DraftPage`] (spec §5). Missing `title`
/// defaults to empty; a claim missing `text` is dropped; missing/non-array
/// `cites` becomes empty (the floor then drops it). Tolerant — a malformed draft
/// degrades to fewer claims, never a panic.
pub fn parse_draft(raw: &serde_json::Value) -> Result<DraftPage, BossclawError> {
    let title = raw.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let mut claims = Vec::new();
    if let Some(arr) = raw.get("claims").and_then(|c| c.as_array()) {
        for item in arr {
            let text = match item.get("text").and_then(|t| t.as_str()) {
                Some(t) if !t.trim().is_empty() => t.to_string(),
                _ => continue,
            };
            let cites = item
                .get("cites")
                .and_then(|c| c.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            claims.push(DraftClaim { text, cites });
        }
    }
    Ok(DraftPage { title, claims })
}
