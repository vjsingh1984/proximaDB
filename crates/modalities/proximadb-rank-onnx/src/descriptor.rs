//! Model descriptors — how a scorer artifact is identified, addressed,
//! and typed at the wire layer.
//!
//! See spec §4.7. The descriptor is what xCatalog persists; the actual
//! model blob lives content-addressed in object storage (R-5b will wire
//! the download path).

use serde::{Deserialize, Serialize};

/// Numeric precision of model tensors. Drives memory budgeting in the
/// LRU cache (smaller dtypes resident in less memory per parameter).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum DType {
    Fp32,
    Fp16,
    Int8,
    Bf16,
}

impl DType {
    /// Bytes per element. Used by `ModelDescriptor::estimated_memory_bytes`.
    pub fn bytes_per_elem(self) -> usize {
        match self {
            DType::Fp32 => 4,
            DType::Fp16 | DType::Bf16 => 2,
            DType::Int8 => 1,
        }
    }
}

/// Backing framework. Only `Onnx` is in-tree today; the enum reserves
/// space for Candle / Burn / safetensors-direct variants.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum ModelFramework {
    Onnx,
    /// R-5b+: Hugging Face Candle (Rust-native).
    Candle,
    /// R-5b+: Burn (Rust-native).
    Burn,
}

/// One input or output tensor declared on the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TensorIoSpec {
    pub name: String,
    /// Tensor shape; `None` entries indicate dynamic dimensions.
    pub shape: Vec<Option<i64>>,
    pub dtype: DType,
}

/// Composite key the cache and registry use to identify a specific
/// (model, version). Hashable for `DashMap` keys.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ModelKey {
    pub model_id: String,
    pub version: String,
}

impl ModelKey {
    pub fn new(model_id: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            version: version.into(),
        }
    }
}

impl std::fmt::Display for ModelKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.model_id, self.version)
    }
}

/// Catalog descriptor for a model artifact. Persisted via
/// `ModelRegistry`; consumed by `OnnxModelCache::acquire` (R-5b will add
/// the download step).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub key: ModelKey,
    /// RLS scope. `None` = global / shared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// Where the blob lives (s3://…, gs://…, file://…).
    pub uri: String,
    /// Content hash for verification on download.
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub framework: ModelFramework,
    pub dtype: DType,
    pub input_spec: Vec<TensorIoSpec>,
    pub output_spec: Vec<TensorIoSpec>,
    /// Max rows the model accepts per call. Drives `BatchedScorer`
    /// chunking. Default to a sensible 32 if unspecified.
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
    /// Registry-assigned: monotonically-increasing version-number-within-
    /// model-id. Distinct from the user-facing `version` string in
    /// `ModelKey` which may be a semantic tag like "v3".
    #[serde(default)]
    pub seq: u64,
    /// Registry-assigned creation ms-since-epoch.
    #[serde(default)]
    pub created_at_ms: i64,
}

fn default_max_batch_size() -> usize {
    32
}

impl ModelDescriptor {
    /// Rough estimate of resident memory after the session loads. Used
    /// by the LRU policy. Defaults to the raw `size_bytes` (mmap'd
    /// weights); real `ort` sessions add modest overhead which R-5b
    /// will refine.
    pub fn estimated_memory_bytes(&self) -> usize {
        self.size_bytes as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtype_bytes_match_known_widths() {
        assert_eq!(DType::Fp32.bytes_per_elem(), 4);
        assert_eq!(DType::Fp16.bytes_per_elem(), 2);
        assert_eq!(DType::Bf16.bytes_per_elem(), 2);
        assert_eq!(DType::Int8.bytes_per_elem(), 1);
    }

    #[test]
    fn model_key_display_is_at_separator() {
        let k = ModelKey::new("rerank", "v3");
        assert_eq!(k.to_string(), "rerank@v3");
    }

    #[test]
    fn model_key_is_hashable() {
        let mut s = std::collections::HashSet::new();
        s.insert(ModelKey::new("a", "1"));
        s.insert(ModelKey::new("a", "1"));
        s.insert(ModelKey::new("a", "2"));
        assert_eq!(s.len(), 2);
    }

    fn sample_descriptor() -> ModelDescriptor {
        ModelDescriptor {
            key: ModelKey::new("rerank-v3", "1.0.0"),
            tenant: Some("tenant-a".into()),
            uri: "s3://models/rerank-v3-1.0.0.onnx".into(),
            sha256: [0xAB; 32],
            size_bytes: 64 * 1024 * 1024,
            framework: ModelFramework::Onnx,
            dtype: DType::Int8,
            input_spec: vec![TensorIoSpec {
                name: "input_ids".into(),
                shape: vec![None, Some(512)],
                dtype: DType::Int8,
            }],
            output_spec: vec![TensorIoSpec {
                name: "logits".into(),
                shape: vec![None, Some(1)],
                dtype: DType::Fp32,
            }],
            max_batch_size: 32,
            seq: 7,
            created_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn descriptor_round_trips_through_serde() {
        let d = sample_descriptor();
        let j = serde_json::to_string(&d).unwrap();
        let back: ModelDescriptor = serde_json::from_str(&j).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn estimated_memory_defaults_to_size_bytes() {
        let d = sample_descriptor();
        assert_eq!(d.estimated_memory_bytes(), 64 * 1024 * 1024);
    }

    #[test]
    fn default_max_batch_size_when_omitted() {
        // Construct a descriptor JSON without max_batch_size to verify
        // the serde default fires.
        let json = r#"{
            "key": {"model_id":"x","version":"1"},
            "uri": "file:///tmp/x.onnx",
            "sha256": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "size_bytes": 100,
            "framework": "Onnx",
            "dtype": "Fp32",
            "input_spec": [],
            "output_spec": []
        }"#;
        let d: ModelDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(d.max_batch_size, 32);
        assert_eq!(d.seq, 0);
        assert_eq!(d.created_at_ms, 0);
    }
}
