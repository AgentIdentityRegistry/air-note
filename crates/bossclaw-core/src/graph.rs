//! Pure bi-temporal graph types and fold helpers (spec §5.6 / M3 §4-5).
//!
//! This module is deliberately PURE — no SQL, no I/O, no `Store`. It mirrors the
//! split used by [`crate::recall`] and [`crate::keyword`]: the database work
//! (folding events into tables, running graph queries) lives on
//! [`crate::log::EventLog`]; everything here is data types and pure helpers.

/// `model_id` recorded on a hand/test-asserted (non-LLM) `link`/`invalidate`
/// event. M4's reasoner replaces this with its real model id. Kept as a named
/// constant so the convention is single-sourced (no magic string).
pub const MANUAL_LINK_PRODUCER: &str = "manual";

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
}
