//! The `Embedder` trait and a deterministic `MockEmbedder` for hermetic tests.
//!
//! Real embedder implementations (Model2Vec, FastEmbed, etc.) live in later
//! milestones. The trait lives here so callers can depend on the abstraction
//! without depending on any specific model.

use crate::error::BossclawError;

/// FNV-1a 64-bit offset basis (Fowler–Noll–Vo spec).
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
/// FNV-1a 64-bit prime (Fowler–Noll–Vo spec).
const FNV_PRIME: u64 = 0x00000100000001b3;

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
/// vector using FNV-1a (offset_basis=0xcbf29ce484222325,
/// prime=0x100000001b3), then L2-normalises the result so cosine math is
/// consistent with real embedders introduced in later milestones.
///
/// Determinism is contractual across toolchains: FNV-1a is computed inline
/// over the token bytes with no reliance on the standard library's `Hasher`
/// trait (whose output is explicitly not stable across compiler versions).
///
/// NOT production-quality recall — it exists solely for fast, offline tests.
pub struct MockEmbedder {
    dim: usize,
}

impl MockEmbedder {
    /// Create a `MockEmbedder` that returns vectors of length `dim`.
    ///
    /// Returns the struct directly; `dim == 0` is rejected by `embed` rather
    /// than at construction time so that the error is surfaced through the
    /// same `Result` path as every other failure.
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl Embedder for MockEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError> {
        if self.dim == 0 {
            return Err(BossclawError::InvalidInput(
                "dim must be greater than zero".into(),
            ));
        }
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
/// Each whitespace-split token is hashed via FNV-1a into a bucket `[0, dim)`
/// and that bucket is incremented by 1.0. The raw counts are then
/// L2-normalised. Empty text (or all-whitespace) returns an all-zero vector.
fn embed_one(text: &str, dim: usize) -> Result<Vec<f32>, BossclawError> {
    let mut vec = vec![0.0f32; dim];
    for token in text.split_whitespace() {
        let bucket = fnv1a(token.as_bytes()) % dim;
        vec[bucket] += 1.0;
    }
    l2_normalize(&mut vec);
    Ok(vec)
}

/// FNV-1a 64-bit hash over raw bytes. Stable across toolchains.
fn fnv1a(bytes: &[u8]) -> usize {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash as usize
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
