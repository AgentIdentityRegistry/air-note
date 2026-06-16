//! The reasoner seam (spec §5): a thin, backend-agnostic interface whose output
//! is DATA, never authority. The default real backend is [`crate::ollama`]'s
//! `OllamaReasoner` (feature-gated); this module holds the trait, the
//! deterministic [`ScriptedReasoner`] test double, and the JSON-schema builders
//! that constrain the model's structured output.
//!
//! PURE: no network, no `Store`, no SQL. The only I/O reasoner lives behind the
//! `ollama` feature in [`crate::ollama`], mirroring M2's `fastembed`-behind-a-
//! feature precedent so the default build stays dependency-light and the live
//! gate can live in-crate.
//!
//! # Schema note on `confidence` vs `confidence_milli`
//!
//! The extraction schema emitted here uses a raw float `confidence` (0.0–1.0) in
//! the LLM response — that is what the model produces. Downstream code in
//! `extract.rs` converts to `confidence_milli` (integer 0–1000, Rev 2 F3) before
//! writing it into the signed event content, eliminating f32→JCS non-determinism.
//! The adjudication schema carries no confidence value.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::error::BossclawError;

/// The thin, backend-agnostic seam (spec §5). An implementation performs a
/// schema-constrained structured completion; the engine parses the result as
/// *proposals*, never as commands (the untrusted-content fence, parent §8.4).
pub trait Reasoner: Send + Sync {
    /// Schema-constrained structured completion. `schema` constrains the JSON the
    /// model may emit; the implementation is responsible for honoring it (Ollama
    /// passes it as the `format` field). `system` is the instruction channel and
    /// `prompt` is the (fenced) data channel. Returns the parsed JSON value or
    /// [`BossclawError::Reasoner`] on transport/decoding failure.
    fn complete_json(
        &self,
        system: &str,
        prompt: &str,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value, BossclawError>;

    /// The model id stamped into every emitted event's `model_meta.model_id`
    /// (provenance, not trust — parent §16 / M3 §12.1). A 7b→14b upgrade is
    /// non-destructive: new events carry the better id; old ones stay tagged.
    /// Production form: `qwen2.5:7b-instruct@sha256:<hex>` (digest-pinned).
    fn model_id(&self) -> &str;
}

/// Deterministic, dependency-free reasoner double for the hermetic suite
/// (spec §2.2). Returns canned JSON keyed by a SHA-256 of `(system, prompt)`,
/// so a given prompt always yields the same value across toolchains — the only
/// way to test the byte-identical Tier-A layer (a live LLM has no byte-identity).
///
/// NOT a production path. Real intelligence is proven by the `#[ignore]` live
/// gate against the actual model.
pub struct ScriptedReasoner {
    model_id: String,
    /// SHA-256 hex of `system \u{1f} prompt` → the canned response.
    responses: HashMap<String, serde_json::Value>,
}

impl ScriptedReasoner {
    /// Create a scripted reasoner stamping `model_id`, with no responses yet.
    pub fn new(model_id: &str) -> Self {
        Self { model_id: model_id.to_string(), responses: HashMap::new() }
    }

    /// Register the canned `response` for an exact `(system, prompt)` pair.
    /// Builder-style so a test can chain several scripted turns.
    pub fn with_response(
        mut self,
        system: &str,
        prompt: &str,
        response: serde_json::Value,
    ) -> Self {
        self.responses.insert(Self::key(system, prompt), response);
        self
    }

    /// SHA-256 hex of `system`, a unit separator, and `prompt`. The separator
    /// (`U+001F`) cannot appear in normal text, so distinct `(system, prompt)`
    /// pairs can never collide by concatenation (`"a"+"bc"` vs `"ab"+"c"`).
    fn key(system: &str, prompt: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(system.as_bytes());
        hasher.update([0x1f]);
        hasher.update(prompt.as_bytes());
        hex::encode(hasher.finalize())
    }
}

impl Reasoner for ScriptedReasoner {
    fn complete_json(
        &self,
        system: &str,
        prompt: &str,
        _schema: &serde_json::Value,
    ) -> Result<serde_json::Value, BossclawError> {
        // The schema is intentionally ignored by the double — it exercises the
        // SAME parse path the real backend feeds, so a scripted value that the
        // parser rejects fails the test exactly as a bad real completion would.
        self.responses
            .get(&Self::key(system, prompt))
            .cloned()
            .ok_or_else(|| {
                BossclawError::Reasoner(format!(
                    "ScriptedReasoner: no canned response for this (system, prompt) \
                     [key={}]",
                    Self::key(system, prompt)
                ))
            })
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// JSON Schema constraining the Pass-A / Pass-B extraction output (spec §6). The
/// model may emit ONLY `{entities[], relations[], retractions[]}`, each item
/// carrying a float `confidence` (0.0–1.0) and `supported_by` the parser reads.
/// Passed verbatim to the backend as the structured-output constraint.
///
/// Note: `confidence` here is the raw float in the LLM response. Downstream code
/// converts to `confidence_milli` (integer 0–1000) before signing (Rev 2 F3).
pub fn extraction_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "entities": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "mention": { "type": "string" },
                        "entity_type": { "type": "string" },
                        "confidence": { "type": "number" }
                    },
                    "required": ["mention", "entity_type", "confidence"]
                }
            },
            "relations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "src": { "type": "string" },
                        "relation": { "type": "string" },
                        "dst": { "type": "string" },
                        "confidence": { "type": "number" },
                        "supported_by": { "type": "string" }
                    },
                    "required": ["src", "relation", "dst", "confidence", "supported_by"]
                }
            },
            "retractions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "src": { "type": "string" },
                        "relation": { "type": "string" },
                        "dst": { "type": "string" },
                        "reason": { "type": "string" },
                        "confidence": { "type": "number" }
                    },
                    "required": ["src", "relation", "dst", "reason", "confidence"]
                }
            }
        },
        "required": ["entities", "relations", "retractions"]
    })
}

/// JSON Schema constraining the entity-resolution adjudication (spec §6). When a
/// mention's cosine similarity lands in the mid-band, the model picks which
/// candidate id it matches, or the sentinel for "none". `match` is a string so
/// the result is a single, parseable choice.
pub fn adjudication_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "match": {
                "type": "string",
                "description": "the chosen candidate entity id, or \"none\""
            }
        },
        "required": ["match"]
    })
}
