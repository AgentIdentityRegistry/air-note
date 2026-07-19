//! The Rung-4 R4-A reflection sweep loop. A PURE `decide_reflect` gate (capture-style, unit-tested truth
//! table) + a thin tokio loop reading the wall clock at the boundary (conflict-sweeper style). All heavy
//! work is one gated + serialized + spawn_blocking `EngineHandle::reflect_once` call.
pub mod sweeper;
