//! The `Embedder` trait and a deterministic `MockEmbedder` for hermetic tests.
//!
//! Real embedder implementations (Model2Vec, FastEmbed, etc.) live in later
//! milestones. The trait lives here so callers can depend on the abstraction
//! without depending on any specific model.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::error::BossclawError;

/// Text → fixed-length f32 vector. Exactly one ACTIVE model per store (the
/// latest `config` event determines which model is active). Vectors are only
/// ever compared within one `model_id`.
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts. Returns one vector per input, each of length
    /// `self.dim()`.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError>;

    /// Dimensionality of the vectors produced by this embedder.
    fn dim(&self) -> usize;

    /// Stable identifier for this model. Used to guard cross-model comparisons.
    fn model_id(&self) -> &str;
}

/// Deterministic, dependency-free embedder for hermetic tests.
///
/// Hashes each whitespace-separated token into a fixed-`dim` bag-of-words
/// vector, then L2-normalises the result so cosine math is consistent with
/// real embedders introduced in later milestones.
///
/// NOT production-quality recall — it exists solely for fast, offline tests.
pub struct MockEmbedder {
    dim: usize,
}

impl MockEmbedder {
    /// Create a `MockEmbedder` that returns vectors of length `dim`.
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl Embedder for MockEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError> {
        texts.iter().map(|text| embed_one(text, self.dim)).collect()
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        "mock-v1"
    }
}

/// Produce an L2-normalised bag-of-words vector for a single text.
///
/// Each whitespace-split token is hashed (deterministically) into a bucket
/// `[0, dim)` using Rust's `DefaultHasher`, and the bucket value is
/// incremented by 1.0. The raw counts are then L2-normalised. All-zero input
/// (empty text or tokens that all hash identically and cancel out) returns a
/// zero vector.
fn embed_one(text: &str, dim: usize) -> Result<Vec<f32>, BossclawError> {
    let mut vec = vec![0.0f32; dim];
    for token in text.split_whitespace() {
        let bucket = hash_token(token) % dim;
        vec[bucket] += 1.0;
    }
    l2_normalize(&mut vec);
    Ok(vec)
}

/// Deterministic bucket index for a token.
fn hash_token(token: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish() as usize
}

/// In-place L2 normalisation. No-op (leaves zeros) if the norm is zero.
fn l2_normalize(vec: &mut [f32]) {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in vec.iter_mut() {
            *x /= norm;
        }
    }
}
