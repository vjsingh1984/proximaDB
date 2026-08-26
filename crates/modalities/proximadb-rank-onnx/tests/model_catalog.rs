//! Pinned-model catalog discipline (TD-SELECTOR-1, gate 5).
//!
//! "Model-catalog operationalization" reduces to a data contract: every reranker
//! an evidence claim names is registered here with the artifact's real content
//! hash, so a BENCHMARK_EVIDENCE entry and the artifact actually serving traffic
//! cannot silently drift apart. This test needs no model file and no feature
//! flags — it validates the *catalog*, and runs in every CI configuration.

use proximadb_rank_onnx::descriptor::{ModelDescriptor, ModelFramework};
use proximadb_rank_onnx::registry::{InMemoryModelRegistry, ModelRegistry};
use serde::Deserialize;

/// Wire format of `fixtures/model_catalog.json`. `sha256` is hex in the file
/// (the human convention) and converted to the descriptor's `[u8; 32]` here, at
/// the boundary, so the checked-in catalog stays diffable.
#[derive(Deserialize)]
struct CatalogEntry {
    key: proximadb_rank_onnx::descriptor::ModelKey,
    uri: String,
    sha256: String,
    size_bytes: u64,
    input_spec: Vec<IoSpecJson>,
    output_spec: Vec<IoSpecJson>,
    max_batch_size: usize,
}

#[derive(Deserialize)]
struct IoSpecJson {
    name: String,
    /// Present in the JSON for human readers; the session binds slot names, so
    /// the descriptor's io dtype stays the weight dtype (Fp32).
    #[serde(skip)]
    _dtype: Option<String>,
}

#[derive(Deserialize)]
struct Catalog {
    models: Vec<CatalogEntry>,
}

/// Decode a 64-char hex string into the descriptor's `[u8; 32]`. Hand-rolled so
/// the test adds no dependency; strict about length and hex digits.
fn decode_hex32(hex: &str) -> [u8; 32] {
    assert_eq!(
        hex.len(),
        64,
        "sha256 hex must be 64 chars, got {}",
        hex.len()
    );
    let nibble = |c: u8| -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("non-hex byte {:?} in sha256", c as char),
        }
    };
    let bytes = hex.as_bytes();
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = (nibble(bytes[2 * i]) << 4) | nibble(bytes[2 * i + 1]);
    }
    out
}

impl From<&CatalogEntry> for ModelDescriptor {
    fn from(e: &CatalogEntry) -> Self {
        let spec = |s: &IoSpecJson| proximadb_rank_onnx::descriptor::TensorIoSpec {
            name: s.name.clone(),
            shape: Vec::new(),
            dtype: proximadb_rank_onnx::descriptor::DType::Fp32,
        };
        ModelDescriptor {
            key: e.key.clone(),
            tenant: None,
            uri: e.uri.clone(),
            sha256: decode_hex32(&e.sha256),
            size_bytes: e.size_bytes,
            framework: ModelFramework::Onnx,
            dtype: proximadb_rank_onnx::descriptor::DType::Fp32,
            input_spec: e.input_spec.iter().map(spec).collect(),
            output_spec: e.output_spec.iter().map(spec).collect(),
            max_batch_size: e.max_batch_size,
            seq: 0,
            created_at_ms: 0,
        }
    }
}

#[derive(Deserialize)]
struct ParityFixtureMeta {
    onnx_sha256: String,
    revision: String,
}

/// model_id → checked-in parity fixture, so the cross-file hash check covers
/// every catalog entry that has one.
const PARITY_FIXTURES: &[(&str, &str)] = &[
    (
        "cross-encoder/ms-marco-MiniLM-L6-v2",
        include_str!("fixtures/onnx_parity_fixture.json"),
    ),
    (
        "BAAI/bge-reranker-large",
        include_str!("fixtures/bge_parity_fixture.json"),
    ),
];

fn catalog() -> Vec<ModelDescriptor> {
    let raw: Catalog = serde_json::from_str(include_str!("fixtures/model_catalog.json"))
        .expect("catalog fixture must parse as catalog entries");
    assert!(
        !raw.models.is_empty(),
        "the pinned-model catalog must carry at least the MiniLM cross-encoder"
    );
    raw.models.iter().map(ModelDescriptor::from).collect()
}

#[tokio::test]
async fn catalog_entries_register_and_round_trip() {
    let registry = InMemoryModelRegistry::new();
    let models = catalog();

    for desc in &models {
        let seq = registry
            .register(desc.clone())
            .await
            .unwrap_or_else(|e| panic!("register {}: {e}", desc.key));
        assert!(seq >= 1, "registration assigns a monotonic seq");

        let fetched = registry
            .get(&desc.key)
            .await
            .expect("get after register")
            .unwrap_or_else(|| panic!("{} must be retrievable", desc.key));
        assert_eq!(fetched.key, desc.key);
        assert_eq!(fetched.sha256, desc.sha256);
        assert_eq!(fetched.size_bytes, desc.size_bytes);
        assert_eq!(fetched.framework, ModelFramework::Onnx);
        assert_eq!(fetched.uri, desc.uri);
    }

    // Registering the same key twice must be refused — a silent overwrite would
    // let a swapped artifact keep the old provenance.
    let dup = registry
        .register(models[0].clone())
        .await
        .expect_err("duplicate registration must fail");
    assert!(
        dup.to_string().contains("already registered"),
        "unexpected error: {dup}"
    );
}

#[tokio::test]
async fn every_entry_is_fully_pinned() {
    for desc in &catalog() {
        assert!(
            desc.sha256 != [0u8; 32],
            "{}: sha256 must be the real artifact digest, not the null placeholder",
            desc.key
        );
        assert!(
            desc.size_bytes > 0,
            "{}: size_bytes must reflect the artifact",
            desc.key
        );
        assert!(
            desc.uri.starts_with("file://")
                || desc.uri.starts_with("s3://")
                || desc.uri.starts_with("gs://"),
            "{}: uri must name where the artifact lives, got {:?}",
            desc.key,
            desc.uri
        );
        assert!(
            desc.key.version.len() >= 7,
            "{}: version must be a real upstream revision pin (7+ hex chars), got {:?}",
            desc.key,
            desc.key.version
        );
    }
}

#[test]
fn catalog_io_spec_matches_the_serving_session_contract() {
    for desc in &catalog() {
        assert!(
            (2..=3).contains(&desc.input_spec.len()),
            "{}: OrtTokenizedScorerSession binds 2 or 3 input slots, catalog declares {}",
            desc.key,
            desc.input_spec.len()
        );
        assert_eq!(
            desc.input_spec[0].name, "input_ids",
            "{}: slot 0 must be input_ids",
            desc.key
        );
        assert_eq!(
            desc.input_spec[1].name, "attention_mask",
            "{}: slot 1 must be attention_mask",
            desc.key
        );
        if desc.input_spec.len() == 3 {
            assert_eq!(
                desc.input_spec[2].name, "token_type_ids",
                "{}: slot 2 must be token_type_ids",
                desc.key
            );
        }
        assert!(
            !desc.output_spec.is_empty(),
            "{}: at least one output slot",
            desc.key
        );
        assert!(
            desc.max_batch_size >= 1,
            "{}: max_batch_size must be at least 1",
            desc.key
        );
    }
}

#[test]
fn catalog_hash_agrees_with_the_parity_fixture() {
    // The catalog's content hash and each parity fixture's reference-artifact
    // hash describe the SAME export; if they disagree, one of the two files was
    // regenerated without the other — exactly the drift this gate exists to stop.
    for desc in &catalog() {
        let Some((_, src)) = PARITY_FIXTURES
            .iter()
            .find(|(model_id, _)| *model_id == desc.key.model_id)
        else {
            continue;
        };
        let fixture: ParityFixtureMeta =
            serde_json::from_str(src).expect("parity fixture must parse");
        let hex = desc
            .sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(
            hex, fixture.onnx_sha256,
            "{}: catalog sha256 and parity-fixture onnx_sha256 disagree — regenerate together",
            desc.key.model_id
        );
        assert!(
            desc.key.version.starts_with(&fixture.revision[..7]),
            "{}: catalog version {} must pin the fixture's revision {}",
            desc.key.model_id,
            desc.key.version,
            fixture.revision
        );
    }
}

#[test]
fn every_parity_fixture_has_a_catalog_entry() {
    // The reverse drift: a fixture without a catalog entry means a parity-gated
    // model whose provenance nobody pinned.
    let models = catalog();
    for (model_id, _) in PARITY_FIXTURES {
        assert!(
            models.iter().any(|d| d.key.model_id == *model_id),
            "{model_id} has a parity fixture but no catalog entry"
        );
    }
}
