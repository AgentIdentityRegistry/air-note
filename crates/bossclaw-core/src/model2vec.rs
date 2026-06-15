//! Pure-Rust Model2Vec embedder — the v1 default for BossClaw Recall.
//!
//! Wraps [`model2vec_rs::model::StaticModel`] and implements the [`Embedder`]
//! trait. All embedding is done locally; no network access is required or
//! possible (the `hf-hub` feature of `model2vec-rs` is disabled in this
//! crate's dependency declaration).
//!
//! # Loading a model from disk
//!
//! ```no_run
//! use std::path::Path;
//! use bossclaw_core::model2vec::Model2Vec;
//!
//! let model = Model2Vec::from_pretrained(
//!     Path::new("/path/to/potion-base-8M"),
//!     "minishlab/potion-base-8M",
//! ).expect("model load failed");
//! ```

use std::path::Path;

use model2vec_rs::model::StaticModel;
use tokenizers::Tokenizer;

use crate::embed::Embedder;
use crate::error::BossclawError;

/// Probe string used once at construction time to discover the embedding
/// dimensionality from a loaded [`StaticModel`].
///
/// The actual content does not matter — any non-empty input is fine because
/// every vocabulary always contains at least one token.
const DIM_PROBE: &str = "hello";

/// Pure-Rust Model2Vec embedder.
///
/// Wraps a [`StaticModel`] loaded from a local directory (or from static
/// in-memory bytes via [`Model2Vec::from_borrowed`]). Output vectors are
/// L2-unit-normalised because `normalize = Some(true)` is always passed at
/// construction time, which keeps cosine math consistent with the ANN index
/// introduced in M2 T5.
pub struct Model2Vec {
    inner: StaticModel,
    model_id: String,
    dim: usize,
}

impl Model2Vec {
    /// Load a Model2Vec model from a local directory and wrap it.
    ///
    /// # Arguments
    /// * `dir` — path to the directory that contains `model.safetensors`,
    ///   `tokenizer.json`, and `config.json` (or the sentence-transformers
    ///   layout — model2vec-rs probes several layouts automatically).
    /// * `model_id` — stable identifier stored alongside every vector; used to
    ///   guard cross-model cosine comparisons. Typically the HuggingFace repo
    ///   slug, e.g. `"minishlab/potion-base-8M"`.
    ///
    /// # Errors
    /// Returns [`BossclawError::Embed`] if model2vec-rs cannot load the files
    /// (missing files, corrupt weights, unsupported tensor dtype, etc.).
    pub fn from_pretrained(
        dir: &Path,
        model_id: impl Into<String>,
    ) -> Result<Self, BossclawError> {
        let inner = StaticModel::from_pretrained(dir, None, Some(true), None)
            .map_err(|e| BossclawError::Embed(e.to_string()))?;
        let dim = Self::probe_dim(&inner);
        if dim == 0 {
            return Err(BossclawError::Embed(
                "probe embedding has zero length — model may be corrupt".into(),
            ));
        }
        Ok(Self {
            inner,
            model_id: model_id.into(),
            dim,
        })
    }

    /// Construct a Model2Vec embedder from pre-loaded static byte slices.
    ///
    /// This is the zero-copy path for the single-binary desktop bundle (M7).
    /// Callers supply the raw bytes of the three model files — typically via
    /// `include_bytes!` — and a pre-deserialised [`Tokenizer`]. The actual
    /// bundling decision (which model to bake in) is deferred to M7; this
    /// constructor merely exposes the capability.
    ///
    /// # Arguments
    /// * `tokenizer` — pre-deserialised tokenizer (from `tokenizer.json` bytes
    ///   via [`Tokenizer::from_bytes`]).
    /// * `embeddings` — static `&'static [f32]` slice of the flat embedding
    ///   matrix in row-major order.
    /// * `rows` — number of vocabulary entries (first dimension).
    /// * `cols` — embedding dimensionality (second dimension).
    /// * `weights` — optional static slice of per-token weights (quantised
    ///   models only; pass `None` for standard models).
    /// * `token_mapping` — optional static slice of token-ID remapping indices
    ///   (quantised models only; pass `None` for standard models).
    /// * `model_id` — stable identifier for cross-model guard.
    ///
    /// # Errors
    /// Returns [`BossclawError::Embed`] if the shape is inconsistent (e.g.
    /// `embeddings.len() != rows * cols`).
    pub fn from_borrowed(
        tokenizer: Tokenizer,
        embeddings: &'static [f32],
        rows: usize,
        cols: usize,
        weights: Option<&'static [f32]>,
        token_mapping: Option<&'static [usize]>,
        model_id: impl Into<String>,
    ) -> Result<Self, BossclawError> {
        let inner =
            StaticModel::from_borrowed(tokenizer, embeddings, rows, cols, /* normalize */ true, weights, token_mapping)
                .map_err(|e| BossclawError::Embed(e.to_string()))?;
        // cols is the authoritative dim; no probe needed here.
        Ok(Self {
            inner,
            model_id: model_id.into(),
            dim: cols,
        })
    }

    /// Discover the embedding dimensionality by encoding a single probe
    /// string and inspecting the output length.
    ///
    /// This is necessary because [`StaticModel`] does not expose a public
    /// `dim()` accessor in model2vec-rs 0.2.1.
    fn probe_dim(model: &StaticModel) -> usize {
        model.encode_single(DIM_PROBE).len()
    }
}

impl Embedder for Model2Vec {
    /// Embed a batch of texts. Each returned vector has length `self.dim()`.
    ///
    /// Returns an empty `Vec` if `texts` is empty.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BossclawError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self.inner.encode(texts))
    }

    /// Dimensionality of the embedding vectors produced by this model.
    fn dim(&self) -> usize {
        self.dim
    }

    /// Stable identifier for this model (e.g. `"minishlab/potion-base-8M"`).
    fn model_id(&self) -> &str {
        &self.model_id
    }
}
