//! FastEmbed (`bge-small-en-v1.5`) embedder — opt-in quality upgrade.
//!
//! This module is compiled **only** when the `fastembed` Cargo feature is
//! enabled. The default build depends on no ONNX/ort machinery.
//!
//! # Usage
//!
//! ```no_run
//! # #[cfg(feature = "fastembed")]
//! use bossclaw_core::fastembed::FastEmbed;
//!
//! # #[cfg(feature = "fastembed")]
//! let embedder = FastEmbed::new().expect("model init failed");
//! ```
//!
//! On first use the ONNX model weights are downloaded from Hugging Face into
//! the fastembed cache directory (`.fastembed_cache` by default, or the
//! `FASTEMBED_CACHE_DIR` env var). Subsequent calls are purely local.

#[cfg(feature = "fastembed")]
mod inner {
    use std::sync::Mutex;

    use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

    use crate::embed::Embedder;
    use crate::error::BossclawError;

    /// Stable model identifier returned by [`FastEmbed::model_id`].
    const MODEL_ID: &str = "bge-small-en-v1.5";

    /// Probe string used to discover embedding dimensionality at construction
    /// time (same pattern as [`crate::model2vec::Model2Vec`]).
    const DIM_PROBE: &str = "hello";

    /// FastEmbed-backed embedder using `BAAI/bge-small-en-v1.5`.
    ///
    /// Output vectors are **L2-unit-normalised** by the fastembed pipeline
    /// (confirmed in `fastembed-5.16.2/src/text_embedding/output.rs` — every
    /// row is passed through `common::normalize` before being returned). No
    /// additional normalisation is applied here.
    ///
    /// The inner [`TextEmbedding`] requires `&mut self` for inference (ONNX
    /// Runtime session state), so it is wrapped in a [`Mutex`] to satisfy the
    /// `Send + Sync` bound demanded by the [`Embedder`] trait.
    pub struct FastEmbed {
        inner: Mutex<TextEmbedding>,
        dim: usize,
    }

    impl FastEmbed {
        /// Initialise the embedder.
        ///
        /// Downloads the ONNX weights and tokeniser on first call if they are
        /// not already present in the fastembed cache directory. The `dim` is
        /// derived by embedding a single probe string rather than being
        /// hard-coded.
        ///
        /// # Errors
        /// Returns [`BossclawError::Embed`] if the model cannot be loaded
        /// (network unavailable on first use, corrupt cache, ONNX Runtime
        /// initialisation failure, etc.).
        pub fn new() -> Result<Self, BossclawError> {
            let mut model =
                TextEmbedding::try_new(TextInitOptions::new(EmbeddingModel::BGESmallENV15))
                    .map_err(|e| BossclawError::Embed(e.to_string()))?;

            let dim = Self::probe_dim(&mut model)?;
            if dim == 0 {
                return Err(BossclawError::Embed(
                    "probe embedding has zero length — model may be corrupt".into(),
                ));
            }

            Ok(Self {
                inner: Mutex::new(model),
                dim,
            })
        }

        /// Embed a single probe string and return the vector length.
        ///
        /// Mirrors the `probe_dim` pattern from [`crate::model2vec::Model2Vec`].
        fn probe_dim(model: &mut TextEmbedding) -> Result<usize, BossclawError> {
            let vecs = model
                .embed(vec![DIM_PROBE], None)
                .map_err(|e| BossclawError::Embed(e.to_string()))?;
            Ok(vecs.into_iter().next().map(|v| v.len()).unwrap_or(0))
        }
    }

    impl Embedder for FastEmbed {
        /// Embed a batch of texts. Each returned vector has length `self.dim()`
        /// and is **L2-unit-normalised** (applied by fastembed internally).
        ///
        /// Returns an empty `Vec` if `texts` is empty.
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError> {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let mut guard = self
                .inner
                .lock()
                .map_err(|e| BossclawError::Embed(format!("mutex poisoned: {e}")))?;
            guard
                .embed(texts, None)
                .map_err(|e| BossclawError::Embed(e.to_string()))
        }

        /// Dimensionality of the vectors produced by `bge-small-en-v1.5` (384).
        fn dim(&self) -> usize {
            self.dim
        }

        /// Stable identifier for this model.
        fn model_id(&self) -> &str {
            MODEL_ID
        }
    }
}

#[cfg(feature = "fastembed")]
pub use inner::FastEmbed;

// --- tests -------------------------------------------------------------------

#[cfg(all(test, feature = "fastembed"))]
mod tests {
    use super::FastEmbed;
    use crate::embed::Embedder;

    /// Compute the cosine similarity between two L2-unit-norm vectors.
    /// Because fastembed normalises output, the dot product IS the cosine.
    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }

    /// Verify the L2 norm of a vector is within floating-point epsilon of 1.
    fn assert_unit_norm(v: &[f32]) {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0_f32).abs() < 1e-4,
            "expected unit norm, got {norm}"
        );
    }

    /// Full integration test — downloads the ONNX model on first run.
    ///
    /// Marked `#[ignore]` so the hermetic suite stays offline; run explicitly
    /// with `cargo test -p bossclaw-core --features fastembed -- --ignored`.
    #[test]
    #[ignore]
    fn fastembed_bge_small_basic() {
        let embedder = FastEmbed::new().expect("model init failed");

        assert!(embedder.dim() > 0, "dim must be positive");
        assert_eq!(embedder.model_id(), "bge-small-en-v1.5");

        let texts = vec![
            "The quick brown fox jumps over the lazy dog".to_string(),
            "A fast auburn fox leaps above a sleepy hound".to_string(),
            "Quantum chromodynamics describes the strong nuclear force".to_string(),
        ];

        let vecs = embedder.embed(&texts).expect("embed failed");
        assert_eq!(vecs.len(), 3);
        assert_eq!(vecs[0].len(), embedder.dim());

        // All outputs must be L2-unit-norm (fastembed normalises by default).
        for v in &vecs {
            assert_unit_norm(v);
        }

        // Near-paraphrase (texts[0] vs texts[1]) must be closer than
        // near-paraphrase vs unrelated (texts[0] vs texts[2]).
        let sim_paraphrase = cosine(&vecs[0], &vecs[1]);
        let sim_unrelated = cosine(&vecs[0], &vecs[2]);
        assert!(
            sim_paraphrase > sim_unrelated,
            "paraphrase similarity ({sim_paraphrase:.4}) should exceed \
             unrelated similarity ({sim_unrelated:.4})"
        );
    }
}
