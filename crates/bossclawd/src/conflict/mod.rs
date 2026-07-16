//! Rung-3 Phase-2 conflict DETECTION — the background sweep that notices contradictions between
//! the brain's own memories and records signed `conflict_proposal`s. Off-by-default; never blocks
//! recall/writes; emits records only (no UI, no mutation). Sibling of `crate::capture`.

pub mod sweeper;
