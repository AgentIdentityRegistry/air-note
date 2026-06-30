//! Desktop-side cloud reasoner (Anthropic + OpenAI-compat). The brain's first
//! deliberate network egress: off-by-default, fail-closed, host-pinned, signed
//! consent. Lives here (not bossclaw-core) because the engine crate's CI jail
//! forbids `reqwest`. See docs/superpowers/specs/2026-06-30-milestone-d2-cloud-reasoner-design.md §8.

// Implemented task-by-task in the Phase 2a plan.
