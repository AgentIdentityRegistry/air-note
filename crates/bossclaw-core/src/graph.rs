//! Pure bi-temporal graph types and fold helpers (spec §5.6 / M3 §4-5).
//!
//! This module is deliberately PURE — no SQL, no I/O, no `Store`. It mirrors the
//! split used by [`crate::recall`] and [`crate::keyword`]: the database work
//! (folding events into tables, running graph queries) lives on
//! [`crate::log::EventLog`]; everything here is data types and pure helpers.

use std::collections::HashMap;

use crate::event::Event;

/// `model_id` recorded on a hand/test-asserted (non-LLM) `link`/`invalidate`
/// event. M4's reasoner replaces this with its real model id. Kept as a named
/// constant so the convention is single-sourced (no magic string).
pub const MANUAL_LINK_PRODUCER: &str = "manual";

/// The `event_type` discriminator for entity events (the fold filters on it and
/// `EventLog::entity` stamps it — single-sourced so they cannot drift).
pub const ENTITY_EVENT_TYPE: &str = "entity";
/// The `event_type` discriminator for memory events — the evolve unit of work
/// (the loop's `unprocessed_memories_since` / queue-depth queries filter on it).
/// Single-sourced so the SQL filters cannot drift from what `append` stamps.
pub const MEMORY_EVENT_TYPE: &str = "memory";
/// The `event_type` discriminator for control `config` events (active-model
/// rotation + the evolve on/off switch). Single-sourced so the config reads and
/// writes in the [`crate::log::EventLog`] config cluster cannot drift.
pub const CONFIG_EVENT_TYPE: &str = "config";
/// The `event_type` discriminator for a dossier (`page`) event — single-sourced
/// so every stamp site, fold filter, and recall filter reference the same string.
pub const PAGE_EVENT_TYPE: &str = "page";
/// The `event_type` discriminator for a `supersede` event — single-sourced so
/// the atomic-pair writer and the fold filter reference the same string.
pub const SUPERSEDE_EVENT_TYPE: &str = "supersede";
/// The neutral entity type stamped on an entity the model named ONLY as a bare
/// relation/retraction endpoint (never listed in `entities[]`, so it carries no
/// declared type). Single-sourced so the endpoint-mint site has no magic string.
pub const UNRESOLVED_ENTITY_TYPE: &str = "unknown";
/// `nodes.kind` for an entity-backed node (precedence over memory/external).
pub const ENTITY_NODE_KIND: &str = "entity";
/// `nodes.kind` for a node that resolves to a memory/page event.
pub const MEMORY_NODE_KIND: &str = "memory";
/// `nodes.kind` for a node that resolves to no known event.
pub const EXTERNAL_NODE_KIND: &str = "external";

/// The `event_type` discriminator for an ingested-file event (M5a). Ground-truth
/// (plain `append`, `model_meta: None`); added to `EMBEDDABLE_EVENT_TYPES` so its
/// `content["text"]` is embedded + FTS-indexed + recallable. Single-sourced so the
/// stamp site, the fold filter, and the recall arm reference the same string.
pub const FILE_INGESTED_EVENT_TYPE: &str = "file_ingested";
/// The `event_type` discriminator for a folder-grant event (M5a). Ground-truth.
pub const GRANT_EVENT_TYPE: &str = "grant";
/// The `event_type` discriminator for a folder-revoke event (M5a). Ground-truth.
pub const REVOKE_EVENT_TYPE: &str = "revoke";
/// The taint stamp written at `content["origin"]` of every `file_ingested` event
/// (M5a, D4). Distinct from the `edges.origin` column (`"manual"`/`"machine"`):
/// this marks external-origin content so the M6 lineage walk can fail closed.
/// Single-sourced so the stamp site and the `is_external` classifier cannot drift.
pub const EXTERNAL_ORIGIN: &str = "external";

/// Namespace prefix for entity node ids: `entity:<event-ulid>`. Mint-once, stable;
/// links reference this exact form, so it is single-sourced.
pub const ENTITY_NODE_PREFIX: &str = "entity:";
/// Build the namespaced entity node id for an entity event id.
pub fn entity_node_id(event_id: &str) -> String {
    format!("{ENTITY_NODE_PREFIX}{event_id}")
}

/// A folded graph edge: one assertion from a `link` event, possibly closed by a
/// later `invalidate`. Two clocks (spec §5): `valid_*` is world-time ("true in
/// the world"); `ingested_at`/`invalidated_at` is learned-time ("when we knew").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// The originating `link` event's ULID — the edge's stable identity.
    pub edge_id: String,
    /// Source node id.
    pub src: String,
    /// Relation label (e.g. `"works_at"`).
    pub relation: String,
    /// Destination node id.
    pub dst: String,
    /// World-clock start: the `link`'s `valid_time`, else its ingestion `ts`.
    pub valid_from: String,
    /// World-clock end: `None` until invalidated.
    pub valid_to: Option<String>,
    /// Learned-clock start: the `link` event's `ts`.
    pub ingested_at: String,
    /// Learned-clock end: `None` until invalidated.
    pub invalidated_at: Option<String>,
    /// The `invalidate` event id that closed this edge, if any.
    pub invalidated_by: Option<String>,
    /// Edge origin: `"manual"` iff the producing `link`'s `model_id ==
    /// [`MANUAL_LINK_PRODUCER`], else `"machine"` (spec §4). A pure function of the
    /// signed event field → the byte-identical-rebuild gate holds.
    pub origin: String,
    /// Machine-link confidence as an INTEGER milli-unit in `0..=1000` read from the
    /// signed `link` content's `confidence_milli` field; `None` for a manual link
    /// (spec §4 / Rev 2 F3). Stored as an integer — never a raw `f32` — because a
    /// float in JCS-canonicalized signed content is a cross-`serde_jcs`-version
    /// determinism hazard that could break [`crate::log::EventLog::verify_chain`].
    /// The recall trust gate (spec §7) compares it against the integer threshold
    /// derived from [`crate::extract::TRUST_MIN`].
    pub confidence_milli: Option<i64>,
}

impl Edge {
    /// True when this edge has not been invalidated (part of the current graph).
    pub fn is_current(&self) -> bool {
        self.invalidated_at.is_none()
    }
}

/// A graph node = a distinct edge endpoint. `kind` is `"memory"` if the id
/// resolves to a `memory`/`page` event, else `"external"` (M4 adds `"entity"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The node's opaque string id (a memory event id in v1).
    pub node_id: String,
    /// Node kind: `"memory"` or `"external"` in v1.
    pub kind: String,
}

/// Two-axis bi-temporal point for [`crate::log::EventLog::as_of`]. Both optional;
/// an all-`None` `AsOf` means "current" (identical to `neighbors`). (spec §5)
#[derive(Debug, Clone, Default)]
pub struct AsOf {
    /// World-clock: keep edges valid in the world at this RFC 3339 time
    /// (`valid_from <= t` AND (`valid_to` is null OR `t < valid_to`)).
    pub valid_time: Option<String>,
    /// Learned-clock: keep edges we knew at this RFC 3339 time
    /// (`ingested_at <= t` AND (`invalidated_at` is null OR `t < invalidated_at`)).
    pub known_as_of: Option<String>,
}

/// Normalize a **parseable RFC 3339** timestamp to a fixed-width (27-char) UTC
/// `YYYY-MM-DDTHH:MM:SS.ffffffZ` string, so lexicographic (SQL `TEXT`) comparison
/// equals chronological comparison regardless of the source offset or sub-second
/// precision. The fixed-width / lexical-==-chronological guarantee holds **only**
/// for input that parses as RFC 3339 (which always carries a 4-digit year).
///
/// An **unparseable** input is returned as the raw string unchanged (best-effort:
/// it degrades to raw-string compare rather than failing the fold). Such a value
/// is therefore **NOT** width- or ordering-normalized — that fallback is a
/// documented degradation, not a guarantee.
pub fn normalize_ts(ts: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => dt
            .with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string(),
        Err(_) => ts.to_string(),
    }
}

/// Extract `(src, relation, dst, confidence_milli?)` from a `link`/`invalidate`
/// event's content, or `None` if `src`/`relation`/`dst` are missing or non-string
/// (malformed — skipped by the fold rather than failing it).
///
/// `confidence_milli` is OPTIONAL and INTEGER (Rev 2 F3): absent or non-integer ⇒
/// `None`. Back-compatible — M3 manual links have no confidence and keep
/// byte-identical rebuilds. The value is an integer milli-unit (0..=1000) and is
/// read tolerantly; the fold (and the trust gate) treat `None` as "no machine
/// confidence". `confidence` lives in the signed content, never in `ModelMeta`
/// (spec §4) — and as an integer, never a raw `f32` (Rev 2 F3 signing-integrity).
pub fn parse_link_content(
    content: &serde_json::Value,
) -> Option<(String, String, String, Option<i64>)> {
    let src = content.get("src")?.as_str()?.to_string();
    let relation = content.get("relation")?.as_str()?.to_string();
    let dst = content.get("dst")?.as_str()?.to_string();
    let confidence_milli = content.get("confidence_milli").and_then(|c| c.as_i64());
    Some((src, relation, dst, confidence_milli))
}

/// Edge origin from a producing link's `model_id` (spec §4): `"manual"` iff it
/// equals [`MANUAL_LINK_PRODUCER`], else `"machine"`. Single-sourced so the
/// derivation is identical everywhere (the fold, the trust gate's expectations).
pub fn origin_of(model_id: &str) -> String {
    if model_id == MANUAL_LINK_PRODUCER {
        "manual".to_string()
    } else {
        "machine".to_string()
    }
}

/// A folded entity record: one `entity` Tier-B event projected into the
/// `entities` table (spec §4). The id is `entity:<the entity event's ULID>` —
/// stable, mint-once; the `label` is a property, never the id (names collide and
/// change, the id does not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    /// The stable namespaced node id, `entity:<ulid>`.
    pub entity_id: String,
    /// Human-readable label (display name).
    pub label: String,
    /// Known aliases (other surface forms that resolve to this entity).
    pub aliases: Vec<String>,
    /// Coarse type discriminator (e.g. `"person"`, `"org"`).
    pub entity_type: String,
}

/// Extract `(label, aliases, entity_type)` from an `entity` event's content, or
/// `None` if `label`/`entity_type` are missing or non-string (malformed —
/// skipped by the fold rather than failing it). `aliases` defaults to empty when
/// absent or non-array; non-string alias items are dropped.
pub fn parse_entity_content(
    content: &serde_json::Value,
) -> Option<(String, Vec<String>, String)> {
    let label = content.get("label")?.as_str()?.to_string();
    let entity_type = content.get("entity_type")?.as_str()?.to_string();
    let aliases = content
        .get("aliases")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    Some((label, aliases, entity_type))
}

/// Fold `entity` events (which MUST already be in `seq` order) into the entity
/// set. Deterministic: one [`Entity`] per well-formed `entity` event, in event
/// order, id = `entity:<event id>`. Malformed events (no label/entity_type) are
/// skipped. Mint-once: an entity event id is unique, so there is no merge here —
/// resolution (spec §6) decides reuse-vs-mint BEFORE an event is appended.
pub fn fold_entities(events: &[Event]) -> Vec<Entity> {
    let mut out = Vec::new();
    for ev in events {
        if ev.event_type != ENTITY_EVENT_TYPE {
            continue;
        }
        if let Some((label, aliases, entity_type)) = parse_entity_content(&ev.content) {
            out.push(Entity {
                entity_id: entity_node_id(&ev.id),
                label,
                aliases,
                entity_type,
            });
        }
    }
    out
}

/// Fold `link`/`invalidate` events (which MUST already be in `seq` order) into
/// the current edge set. Deterministic: edges are produced in link-event order;
/// each `link` opens an assertion (`edge_id` = the link event id), each
/// `invalidate` closes ALL currently-active assertions for its `(src, relation,
/// dst)` key. Re-linking after an invalidate opens a fresh assertion (a new
/// validity interval). Malformed events (no src/relation/dst) are skipped.
pub fn fold_edges(events: &[Event]) -> Vec<Edge> {
    let mut edges: Vec<Edge> = Vec::new();
    // edge-key → indices into `edges` that are still open for that key.
    let mut active: HashMap<(String, String, String), Vec<usize>> = HashMap::new();
    for ev in events {
        let (src, relation, dst, confidence_milli) = match parse_link_content(&ev.content) {
            Some(t) => t,
            None => continue,
        };
        let key = (src.clone(), relation.clone(), dst.clone());
        match ev.event_type.as_str() {
            "link" => {
                let valid_from = normalize_ts(ev.valid_time.as_deref().unwrap_or(&ev.ts));
                let ingested_at = normalize_ts(&ev.ts);
                // origin from the signed model_id; a link with no model_meta is
                // treated as manual (M3 hand-links always had model_meta, so this
                // is a defensive default, not a normal path).
                let origin = ev
                    .model_meta
                    .as_ref()
                    .map(|m| origin_of(&m.model_id))
                    .unwrap_or_else(|| "manual".to_string());
                // Manual links carry NULL confidence even if a stray value appears.
                let confidence_milli = if origin == "manual" { None } else { confidence_milli };
                edges.push(Edge {
                    edge_id: ev.id.clone(),
                    src,
                    relation,
                    dst,
                    valid_from,
                    valid_to: None,
                    ingested_at,
                    invalidated_at: None,
                    invalidated_by: None,
                    origin,
                    confidence_milli,
                });
                active.entry(key).or_default().push(edges.len() - 1);
            }
            "invalidate" => {
                if let Some(indices) = active.remove(&key) {
                    let valid_to = normalize_ts(ev.valid_time.as_deref().unwrap_or(&ev.ts));
                    let invalidated_at = normalize_ts(&ev.ts);
                    for i in indices {
                        edges[i].valid_to = Some(valid_to.clone());
                        edges[i].invalidated_at = Some(invalidated_at.clone());
                        edges[i].invalidated_by = Some(ev.id.clone());
                    }
                }
            }
            _ => {}
        }
    }
    edges
}

/// A folded summary page: the CURRENT (un-superseded) `page` event for a topic,
/// projected into the `pages` table (spec §4). `page_event_id` is the signed
/// event's id (what a `supersede` references); the prose lives in `text` (also
/// the embedded text). At most one `Page` per `topic_id` (F9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The topic this dossier is about — an `entity:<ulid>` node id.
    pub topic_id: String,
    /// The current page event's id.
    pub page_event_id: String,
    /// Human-readable dossier title.
    pub title: String,
    /// The rendered markdown body (also the embeddable/recall text).
    pub text: String,
}

/// Extract `(topic_id, title, text)` from a `page` event's content, or `None` if
/// any is missing/non-string (malformed — skipped by the fold, not fatal).
pub fn parse_page_content(content: &serde_json::Value) -> Option<(String, String, String)> {
    let topic_id = content.get("topic_id")?.as_str()?.to_string();
    let title = content.get("title")?.as_str()?.to_string();
    let text = content.get("text")?.as_str()?.to_string();
    Some((topic_id, title, text))
}

/// Fold `page`/`supersede` events (MUST be in `seq` order) into the current
/// dossier per topic (spec §4 / F9). A `supersede{supersedes: P}` retires page
/// id `P`; the current page for a topic is the latest (`seq`-max) `page` for that
/// `topic_id` not retired by any supersede. **At most one** per topic (zero is a
/// benign, transient orphan-supersede state). Deterministic → byte-identical
/// rebuild. Malformed events are skipped.
pub fn fold_pages(events: &[Event]) -> Vec<Page> {
    use std::collections::HashSet;
    let mut superseded: HashSet<String> = HashSet::new();
    for ev in events {
        if ev.event_type == SUPERSEDE_EVENT_TYPE {
            if let Some(p) = ev.content.get("supersedes").and_then(|v| v.as_str()) {
                superseded.insert(p.to_string());
            }
        }
    }
    // Latest non-superseded page per topic, walking in seq order so the last
    // write wins deterministically.
    let mut by_topic: std::collections::BTreeMap<String, Page> = std::collections::BTreeMap::new();
    for ev in events {
        if ev.event_type != PAGE_EVENT_TYPE || superseded.contains(&ev.id) {
            continue;
        }
        if let Some((topic_id, title, text)) = parse_page_content(&ev.content) {
            by_topic.insert(topic_id.clone(), Page { topic_id, page_event_id: ev.id.clone(), title, text });
        }
    }
    by_topic.into_values().collect()
}

/// A folded folder grant (M5a): the CURRENT state of one granted root. A
/// deterministic fold over `grant`/`revoke` events; rebuilt by `rebuild_graph`.
/// `revoked` files are kept in the log forever but excluded from recall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The canonicalized absolute folder path (the grant's identity key).
    pub canonical_root: String,
    /// RFC 3339 ts of the latest `grant` event for this root (provenance).
    pub granted_at: String,
    /// True iff the latest event for this root is a `revoke`.
    pub revoked: bool,
}

/// Extract `canonical_root` from a `grant`/`revoke` event's content, or `None`.
fn parse_grant_content(content: &serde_json::Value) -> Option<String> {
    Some(content.get("canonical_root")?.as_str()?.to_string())
}

/// Fold `grant`/`revoke` events (MUST be in `seq` order) into the current grant
/// per root (last-writer-wins). A `grant` (re)activates and stamps `granted_at`;
/// a `revoke` marks an EXISTING root revoked. A `revoke` with no prior grant is
/// ignored. Deterministic → byte-identical rebuild.
pub fn fold_grants(events: &[Event]) -> Vec<Grant> {
    use std::collections::BTreeMap;
    let mut by_root: BTreeMap<String, Grant> = BTreeMap::new();
    for ev in events {
        let root = match parse_grant_content(&ev.content) {
            Some(r) => r,
            None => continue,
        };
        if ev.event_type == GRANT_EVENT_TYPE {
            by_root.insert(root.clone(), Grant { canonical_root: root, granted_at: ev.ts.clone(), revoked: false });
        } else if ev.event_type == REVOKE_EVENT_TYPE {
            if let Some(g) = by_root.get_mut(&root) {
                g.revoked = true;
            }
        }
    }
    by_root.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ts_preserves_chronological_order_lexically() {
        // Across a 4-digit year range, the normalized strings sort the same way the
        // instants do — the property the SQL TEXT comparisons rely on.
        let early = normalize_ts("0099-01-01T00:00:00Z");
        let mid = normalize_ts("2020-06-15T10:00:00Z");
        let late = normalize_ts("9999-12-31T23:59:59Z");
        assert!(early < mid, "{early} should sort before {mid}");
        assert!(mid < late, "{mid} should sort before {late}");
    }

    #[test]
    fn normalize_ts_converts_offset_to_utc() {
        // Midnight in +09:00 is 15:00 the previous day in UTC; both normalize
        // to the same instant string.
        assert_eq!(
            normalize_ts("2020-01-01T00:00:00+09:00"),
            normalize_ts("2019-12-31T15:00:00Z"),
        );
    }

    #[test]
    fn normalize_ts_is_fixed_width_27_for_valid_rfc3339() {
        // YYYY-MM-DDTHH:MM:SS.ffffffZ = 27 chars.
        assert_eq!(normalize_ts("2020-06-15T10:00:00Z").len(), 27);
    }

    /// Minimal `entity` event for the fold unit test (no signing/hashing needed —
    /// `fold_entities` reads only `id`/`event_type`/`content`).
    fn mk_entity_event(id: &str, content: serde_json::Value) -> Event {
        Event {
            id: id.to_string(),
            ts: String::new(),
            valid_time: None,
            event_type: ENTITY_EVENT_TYPE.to_string(),
            content,
            model_meta: None,
            prev_hash: String::new(),
            hash: None,
            signed_by_did: String::new(),
            signature: None,
        }
    }

    #[test]
    fn fold_entities_skips_malformed_events_not_fatal() {
        // A well-formed entity folds; ones missing `label` or `entity_type` are
        // SKIPPED (the degrade path the spec emphasizes) — never panic, never a row.
        let events = vec![
            mk_entity_event(
                "01GOOD",
                serde_json::json!({ "label": "Kenny", "aliases": [], "entity_type": "person" }),
            ),
            // Missing `entity_type` → skipped.
            mk_entity_event("01NOTYPE", serde_json::json!({ "label": "Acme" })),
            // Missing `label` → skipped.
            mk_entity_event("01NOLABEL", serde_json::json!({ "entity_type": "org" })),
        ];
        let folded = fold_entities(&events);
        assert_eq!(folded.len(), 1, "only the well-formed entity becomes a row");
        assert_eq!(folded[0].entity_id, entity_node_id("01GOOD"));
        assert_eq!(folded[0].label, "Kenny");
        assert_eq!(folded[0].entity_type, "person");
    }

    #[test]
    fn m5a_event_type_consts_are_distinct_and_stable() {
        // Stable wire strings (these land in signed content + the byte-identical rebuild).
        assert_eq!(FILE_INGESTED_EVENT_TYPE, "file_ingested");
        assert_eq!(GRANT_EVENT_TYPE, "grant");
        assert_eq!(REVOKE_EVENT_TYPE, "revoke");
        assert_eq!(EXTERNAL_ORIGIN, "external");
        // Must not collide with existing discriminators.
        for other in [MEMORY_EVENT_TYPE, PAGE_EVENT_TYPE, SUPERSEDE_EVENT_TYPE, ENTITY_EVENT_TYPE, CONFIG_EVENT_TYPE] {
            assert_ne!(FILE_INGESTED_EVENT_TYPE, other);
            assert_ne!(GRANT_EVENT_TYPE, other);
            assert_ne!(REVOKE_EVENT_TYPE, other);
        }
    }

    #[test]
    fn fold_grants_is_last_writer_wins_per_root() {
        // grant A, grant B, revoke A, grant A again → A active (re-granted), B active.
        let mk = |etype: &str, root: &str, ts: &str| Event {
            id: String::new(), ts: ts.to_string(), valid_time: None,
            event_type: etype.to_string(),
            content: serde_json::json!({ "canonical_root": root }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
        };
        let events = vec![
            mk(GRANT_EVENT_TYPE, "/a", "2026-06-18T00:00:00Z"),
            mk(GRANT_EVENT_TYPE, "/b", "2026-06-18T00:00:01Z"),
            mk(REVOKE_EVENT_TYPE, "/a", "2026-06-18T00:00:02Z"),
            mk(GRANT_EVENT_TYPE, "/a", "2026-06-18T00:00:03Z"),
        ];
        let mut grants = fold_grants(&events);
        grants.sort_by(|x, y| x.canonical_root.cmp(&y.canonical_root));
        assert_eq!(grants.len(), 2);
        assert_eq!(grants[0].canonical_root, "/a");
        assert!(!grants[0].revoked, "/a was re-granted after revoke → active");
        assert_eq!(grants[0].granted_at, "2026-06-18T00:00:03Z", "granted_at = latest grant ts");
        assert!(!grants[1].revoked, "/b never revoked");
    }

    #[test]
    fn fold_grants_revoke_without_grant_is_ignored() {
        let ev = Event {
            id: String::new(), ts: "2026-06-18T00:00:00Z".to_string(), valid_time: None,
            event_type: REVOKE_EVENT_TYPE.to_string(),
            content: serde_json::json!({ "canonical_root": "/never-granted" }),
            model_meta: None, prev_hash: String::new(), hash: None,
            signed_by_did: "did:wba:AIR-TEST".to_string(), signature: None,
        };
        assert!(fold_grants(&[ev]).is_empty(), "a revoke with no prior grant yields no row");
    }
}
