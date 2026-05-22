//! BGE family (bge-small / bge-large / bge-m3) ONNX inference.
//!
//! Production builds use the `ort` crate to run the model from a file on
//! disk. The file path is resolved from the `PROXIMADB_EMBED_MODEL_DIR`
//! environment variable (default `/var/lib/proximadb/models`) plus the
//! variant-specific filename.
//!
//! ## Test mode
//!
//! Unit and integration tests do not require an ONNX runtime. The
//! `testing::synthetic_vector` helper is `#[cfg(test)]` only and returns
//! deterministic hash-derived vectors that preserve the dimension
//! contract. Production code MUST NOT call it; production code MUST fail
//! with `ModelUnavailable` when the model file is missing.

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
    ///   `PROXIMADB_EMBED_MODEL_DIR`  (root directory for model files)
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

    /// Max sequence length the encoder will see. BGE family uses 512.
    pub fn max_seq_len(self) -> usize {
        512
    }
}

#[cfg(feature = "onnx")]
pub struct BgeModel {
    variant: Variant,
    /// Pool of N independent ONNX sessions for the same variant. Each session
    /// is wrapped in its own `Mutex` because `ort::Session::run` is `&mut self`
    /// in ort 2.0.0-rc.x. With N sessions, up to N concurrent inferences can
    /// run in parallel (limited by CPU cores in practice).
    ///
    /// Pool size is controlled by `PROXIMADB_EMBED_SESSIONS` at process start.
    /// Default is 1 to preserve the conservative memory profile; bump to
    /// `num_cpus / 2` (or `4`) on dedicated embedding nodes for ~Nx throughput.
    /// Each additional session adds the model's runtime state to RAM (a few MB
    /// on top of the mmapped weights which are shared across sessions).
    sessions: Vec<std::sync::Mutex<ort::session::Session>>,
    /// Round-robin pointer for session selection. Wraps modulo `sessions.len()`.
    next: std::sync::atomic::AtomicUsize,
    tokenizer: std::sync::Arc<tokenizers::Tokenizer>,
}

#[cfg(not(feature = "onnx"))]
pub struct BgeModel {
    variant: Variant,
}

impl BgeModel {
    /// Load the ONNX session + tokenizer for the given variant.
    ///
    /// Returns `ModelUnavailable` if the model file cannot be opened or the
    /// `onnx` feature is not enabled. Callers must surface the error;
    /// silent synthetic fallback is forbidden in production paths.
    pub fn initialize(variant: Variant) -> Result<Self> {
        #[cfg(feature = "onnx")]
        {
            let model_path = variant.onnx_path();
            let tokenizer_path = std::env::var("PROXIMADB_TOKENIZER_PATH")
                .unwrap_or_else(|_| {
                    let root = std::env::var("PROXIMADB_EMBED_MODEL_DIR")
                        .unwrap_or_else(|_| "/var/lib/proximadb/models".to_string());
                    PathBuf::from(root)
                        .join("tokenizer.json")
                        .to_string_lossy()
                        .into_owned()
                });

            // Pool size: PROXIMADB_EMBED_SESSIONS env var, default 1, clamped
            // to [1, 32]. Each session is one ONNX inference context; weights
            // are shared via mmap so memory cost is mostly the per-session
            // runtime arenas (single-digit MB on bge-small).
            let pool_size = std::env::var("PROXIMADB_EMBED_SESSIONS")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1)
                .clamp(1, 32);

            tracing::info!(
                variant = ?variant,
                model = %model_path.display(),
                tokenizer = %tokenizer_path,
                pool_size,
                "loading BGE ONNX session pool"
            );

            let mut sessions = Vec::with_capacity(pool_size);
            for idx in 0..pool_size {
                let session = ort::session::Session::builder()
                    .map_err(|e| crate::EmbeddingError::ModelUnavailable(format!(
                        "ort builder (session {idx}): {e}"
                    )))?
                    .commit_from_file(&model_path)
                    .map_err(|e| crate::EmbeddingError::ModelUnavailable(format!(
                        "ort commit_from_file({}) (session {idx}): {e}",
                        model_path.display()
                    )))?;
                sessions.push(std::sync::Mutex::new(session));
            }

            let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| crate::EmbeddingError::ModelUnavailable(format!(
                    "tokenizer load({}): {e}",
                    tokenizer_path
                )))?;

            Ok(Self {
                variant,
                sessions,
                next: std::sync::atomic::AtomicUsize::new(0),
                tokenizer: std::sync::Arc::new(tokenizer),
            })
        }
        #[cfg(not(feature = "onnx"))]
        {
            Err(crate::EmbeddingError::ModelUnavailable(format!(
                "BGE variant {variant:?} requested but the `onnx` feature is disabled in this build; \
                 rebuild proximadb-embedding with --features onnx, or use a non-BGE route."
            )))
        }
    }

    pub fn variant(&self) -> Variant {
        self.variant
    }

    /// Run the model on a batch of texts and return one L2-normalized
    /// embedding per text.
    ///
    /// Padding/truncation are applied to bring every input to the same
    /// length within the batch (so the ONNX tensors are rectangular). The
    /// pooled output uses masked mean-pool followed by L2 normalization,
    /// matching the BGE family's published recipe.
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        #[cfg(feature = "onnx")]
        {
            self.embed_batch_onnx(texts)
        }
        #[cfg(not(feature = "onnx"))]
        {
            let _ = texts;
            Err(crate::EmbeddingError::ModelUnavailable(
                "BGE embed_batch called without the `onnx` feature".to_string(),
            ))
        }
    }

    #[cfg(feature = "onnx")]
    fn embed_batch_onnx(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        use ndarray::Array2;
        use ort::value::Value;

        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Tokenize the whole batch in one call; the tokenizers crate handles
        // multi-thread parallelism internally.
        let owned_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let encodings = self
            .tokenizer
            .encode_batch(owned_refs, true)
            .map_err(|e| crate::EmbeddingError::Other(anyhow::anyhow!("tokenize: {}", e)))?;

        let max_seq = self.variant.max_seq_len();
        let batch_seq = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(0)
            .min(max_seq);
        let seq_len = batch_seq.max(1);
        let batch = encodings.len();

        // Pad/truncate to [batch, seq_len].
        let mut input_ids = Array2::<i64>::zeros((batch, seq_len));
        let mut attention_mask = Array2::<i64>::zeros((batch, seq_len));
        let mut token_type_ids = Array2::<i64>::zeros((batch, seq_len));
        for (b, enc) in encodings.iter().enumerate() {
            let ids = enc.get_ids();
            let mask = enc.get_attention_mask();
            let types = enc.get_type_ids();
            let take = ids.len().min(seq_len);
            for i in 0..take {
                input_ids[(b, i)] = ids[i] as i64;
                attention_mask[(b, i)] = mask[i] as i64;
                token_type_ids[(b, i)] = types[i] as i64;
            }
        }

        // Build session inputs.
        let inputs = ort::inputs![
            "input_ids" => Value::from_array(input_ids).map_err(onnx_err)?,
            "attention_mask" => Value::from_array(attention_mask.clone()).map_err(onnx_err)?,
            "token_type_ids" => Value::from_array(token_type_ids).map_err(onnx_err)?,
        ];

        // Materialize the hidden tensor into an owned Vec<f32> + shape so it
        // outlives the SessionOutputs borrow (which dies at end of scope).
        let (hidden_data, hidden_shape) = {
            // Pool selection: bump the round-robin pointer once, then probe
            // each session with try_lock starting at that index. First idle
            // session wins; if all are busy, block on the round-robin slot.
            // This minimizes contention when sessions are unevenly busy
            // (e.g., one running a long batch).
            let pool_len = self.sessions.len();
            let start_idx = self
                .next
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut acquired: Option<std::sync::MutexGuard<'_, ort::session::Session>> = None;
            for i in 0..pool_len {
                let idx = (start_idx.wrapping_add(i)) % pool_len;
                if let Ok(guard) = self.sessions[idx].try_lock() {
                    acquired = Some(guard);
                    break;
                }
            }
            let mut session = match acquired {
                Some(guard) => guard,
                None => self.sessions[start_idx % pool_len].lock().map_err(|e| {
                    crate::EmbeddingError::Other(anyhow::anyhow!(
                        "session mutex poisoned: {}",
                        e
                    ))
                })?,
            };
            let outputs = session.run(inputs).map_err(onnx_err)?;
            // BGE exports expose the last hidden state under the first output
            // (sometimes named `last_hidden_state`, sometimes `output_0`).
            // Pick the first rank-3 f32 output.
            let mut found: Option<(Vec<f32>, Vec<usize>)> = None;
            for (_, value) in outputs.iter() {
                if let Ok(arr) = value.try_extract_array::<f32>() {
                    let shape = arr.shape().to_vec();
                    if shape.len() == 3 {
                        let data: Vec<f32> = arr.iter().copied().collect();
                        found = Some((data, shape));
                        break;
                    }
                }
            }
            found.ok_or_else(|| {
                crate::EmbeddingError::Other(anyhow::anyhow!(
                    "no rank-3 f32 output found in ONNX run"
                ))
            })?
        };

        let hidden = hidden_shape[2];
        let target_dim = self.variant.dimension();
        if hidden != target_dim {
            return Err(crate::EmbeddingError::Other(anyhow::anyhow!(
                "model hidden size {} != variant dim {}",
                hidden,
                target_dim
            )));
        }

        // Strides for [batch, seq, hidden] row-major: index = b*S*H + s*H + h.
        let stride_b = seq_len * hidden;
        let stride_s = hidden;

        // Masked mean-pool: sum hidden * mask along the seq axis, divide by
        // mask count per row. Then L2-normalize the result.
        let mut out = Vec::with_capacity(batch);
        for b in 0..batch {
            let mut sums = vec![0.0_f32; hidden];
            let mut mask_count = 0_f32;
            for s in 0..seq_len {
                let m = attention_mask[(b, s)] as f32;
                if m == 0.0 {
                    continue;
                }
                mask_count += m;
                let row_base = b * stride_b + s * stride_s;
                for h in 0..hidden {
                    sums[h] += hidden_data[row_base + h] * m;
                }
            }
            let denom = mask_count.max(1.0);
            let mut norm_sq = 0.0_f32;
            for v in sums.iter_mut() {
                *v /= denom;
                norm_sq += *v * *v;
            }
            let norm = norm_sq.sqrt().max(f32::EPSILON);
            for v in sums.iter_mut() {
                *v /= norm;
            }
            out.push(sums);
        }

        Ok(out)
    }
}

#[cfg(feature = "onnx")]
fn onnx_err(e: ort::Error) -> crate::EmbeddingError {
    crate::EmbeddingError::Other(anyhow::anyhow!("ort: {}", e))
}

/// Test-only deterministic vector helpers.
///
/// This module is gated on `#[cfg(test)]` so production binaries cannot
/// link or call it. Test code that needs an embedding without a real
/// ONNX model (most unit tests of the scheduler, queue, and storage
/// paths) calls `testing::synthetic_vector` directly.
#[cfg(test)]
pub mod testing {
    /// Hash-derived deterministic vector. Same shape contract as a real
    /// embedding (`dim` floats, L2-normalized) but with zero semantic
    /// quality. For tests only.
    pub fn synthetic_vector(text: &str, dim: usize) -> Vec<f32> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let seed = hasher.finish();
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
        let norm = norm_sq.sqrt().max(f32::EPSILON);
        for x in v.iter_mut() {
            *x /= norm;
        }
        v
    }

    /// Deterministic batch helper for the scheduler tests.
    pub fn synthetic_batch(texts: &[String], dim: usize) -> Vec<Vec<f32>> {
        texts.iter().map(|t| synthetic_vector(t, dim)).collect()
    }
}
