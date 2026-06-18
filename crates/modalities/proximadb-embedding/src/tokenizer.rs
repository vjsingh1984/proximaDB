//! Shared tokenizer wrapper. One `Arc<Tokenizer>` instance per process,
//! used by both the chunker (token counting + splitting) and any future
//! pre-embedding token pre-processing.

use std::sync::Arc;

use crate::{EmbeddingError, Result};

pub struct SharedTokenizer {
    inner: Arc<tokenizers::Tokenizer>,
}

impl SharedTokenizer {
    pub fn initialize() -> Result<Self> {
        // Default tokenizer: the BGE-family tokenizer.json loaded from disk,
        // path overridable via PROXIMADB_TOKENIZER_PATH. When the file is
        // missing we fall back to an approximate whitespace tokenizer so the
        // server still boots (e.g. bring-your-own-vector deployments that
        // never chunk/embed server-side). The fallback only degrades chunk
        // token-counting (word-based instead of subword-accurate) — it does
        // NOT affect embeddings: the BGE/ONNX path fails loud if its model is
        // missing (see models/bge.rs), it never silently uses this fallback.
        let tokenizer_path = std::env::var("PROXIMADB_TOKENIZER_PATH")
            .unwrap_or_else(|_| "/var/lib/proximadb/models/tokenizer.json".to_string());

        let inner = match tokenizers::Tokenizer::from_file(&tokenizer_path) {
            Ok(t) => Arc::new(t),
            Err(err) => {
                tracing::warn!(
                    path = %tokenizer_path,
                    error = ?err,
                    "BGE tokenizer.json not found; falling back to an approximate \
                     whitespace tokenizer. Chunk token counts will be word-based \
                     rather than subword-accurate; embeddings are unaffected. Mount \
                     the real tokenizer at this path or set PROXIMADB_TOKENIZER_PATH \
                     for accurate token-based chunking."
                );
                Arc::new(synthetic_tokenizer()?)
            }
        };
        Ok(Self { inner })
    }

    /// Count tokens (approximate, used for chunking decisions).
    pub fn count_tokens(&self, text: &str) -> Result<usize> {
        self.inner
            .encode(text, false)
            .map(|e| e.len())
            .map_err(|e| EmbeddingError::Other(anyhow::anyhow!("tokenize: {}", e)))
    }

    /// Inner Arc for handing to downstream consumers that need the raw tokenizer.
    pub fn inner(&self) -> Arc<tokenizers::Tokenizer> {
        self.inner.clone()
    }
}

fn synthetic_tokenizer() -> Result<tokenizers::Tokenizer> {
    // Minimal whitespace tokenizer — sufficient for byte-counting & dev/test.
    // For production deployments the real BGE tokenizer.json must be mounted.
    use tokenizers::models::wordlevel::{WordLevel, WordLevelTrainerBuilder};
    use tokenizers::pre_tokenizers::whitespace::Whitespace;
    use tokenizers::tokenizer::Tokenizer;
    let model = WordLevel::builder()
        .unk_token("[UNK]".to_string())
        .build()
        .map_err(|e| EmbeddingError::Other(anyhow::anyhow!("synth tokenizer: {}", e)))?;
    let mut tk = Tokenizer::new(model);
    tk.with_pre_tokenizer(Some(Whitespace {}));
    let _ = WordLevelTrainerBuilder::default(); // silence unused-import lint
    Ok(tk)
}
