//! BGE family (bge-small / bge-large / bge-m3) ONNX inference.
//!
//! Phase 1 scaffold: this module exposes the public surface and a
//! deterministic fallback that returns hash-derived synthetic vectors when
//! the `onnx` feature is off (default in CI to avoid the system ONNX
//! dependency). Real ONNX integration via the `ort` crate lands when the
//! `onnx` feature is wired in Phase 1 follow-up.
//!
//! The synthetic fallback preserves the dimension contract — bge-small
//! returns 384-dim vectors, bge-large/m3 return 1024-dim — so downstream
//! HNSW indexing and search work end-to-end during integration tests.

use std::path::PathBuf;

use crate::Result;

#[derive(Debug, Clone, Copy)]
pub enum Variant {
    Small, // 384-dim
    Large, // 1024-dim
    M3,    // 1024-dim, multilingual
}

impl Variant {
    pub fn dimension(self) -> usize {
        match self {
            Self::Small => 384,
            Self::Large => 1024,
            Self::M3 => 1024,
        }
    }

    /// Path to the ONNX model file. Override via env:
    ///   PROXIMADB_EMBED_MODEL_DIR  (root directory for model files)
    pub fn onnx_path(self) -> PathBuf {
        let root = std::env::var("PROXIMADB_EMBED_MODEL_DIR")
            .unwrap_or_else(|_| "/var/lib/proximadb/models".to_string());
        let file = match self {
            Self::Small => "bge-small-en-v1.5.onnx",
            Self::Large => "bge-large-en-v1.5.onnx",
            Self::M3 => "bge-m3.onnx",
        };
        PathBuf::from(root).join(file)
    }
}

pub struct BgeModel {
    variant: Variant,
    #[cfg(feature = "onnx")]
    session: ort::Session,
}

impl BgeModel {
    pub fn initialize(variant: Variant) -> Result<Self> {
        #[cfg(feature = "onnx")]
        {
            // Real ONNX session load (mmap-backed; thread-safe parallel inference).
            // Wired in Phase 1 follow-up when the `onnx` feature is enabled.
            // See: https://github.com/pykeio/ort
            let session = ort::Session::builder()
                .map_err(|e| crate::EmbeddingError::ModelUnavailable(e.to_string()))?
                .commit_from_file(variant.onnx_path())
                .map_err(|e| crate::EmbeddingError::ModelUnavailable(e.to_string()))?;
            Ok(Self { variant, session })
        }
        #[cfg(not(feature = "onnx"))]
        {
            tracing::warn!(
                variant = ?variant,
                "BGE model loaded without `onnx` feature — using deterministic fallback. \
                 Enable feature `onnx` for production inference."
            );
            Ok(Self { variant })
        }
    }

    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        #[cfg(feature = "onnx")]
        {
            // Real inference path: tokenize → run session → extract last hidden
            // → mean-pool → L2-normalize. Wired in Phase 1 follow-up.
            //
            // Sketch:
            //   let tokens = SharedTokenizer::global().encode_batch(texts);
            //   let inputs = build_onnx_inputs(tokens);
            //   let outputs = self.session.run(inputs)?;
            //   let embeddings = mean_pool_and_normalize(outputs);
            //   Ok(embeddings)
            unimplemented!("real ONNX inference lands in Phase 1 follow-up")
        }
        #[cfg(not(feature = "onnx"))]
        {
            // Deterministic synthetic vectors keyed on text hash. Sufficient
            // for end-to-end testing of the scheduler, WAL, and index paths
            // without an ONNX runtime.
            let dim = self.variant.dimension();
            Ok(texts.iter().map(|t| synthetic_vector(t, dim)).collect())
        }
    }
}

#[cfg(not(feature = "onnx"))]
fn synthetic_vector(text: &str, dim: usize) -> Vec<f32> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    let seed = hasher.finish();
    // Simple PRNG seeded from the text hash — deterministic per text.
    let mut state = seed;
    let mut v = Vec::with_capacity(dim);
    let mut norm_sq = 0.0_f32;
    for _ in 0..dim {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let x = ((state >> 33) as i32 as f32) / (i32::MAX as f32);
        v.push(x);
        norm_sq += x * x;
    }
    // L2 normalize so cosine similarity behaves sanely.
    let norm = norm_sq.sqrt().max(f32::EPSILON);
    for x in v.iter_mut() {
        *x /= norm;
    }
    v
}
