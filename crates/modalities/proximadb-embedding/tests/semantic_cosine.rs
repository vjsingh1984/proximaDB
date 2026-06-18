//! BGE semantic-clustering proof. Embeds real text via the ONNX model and
//! asserts that semantically-related texts (two about cats) are closer in
//! cosine space than an unrelated text (finance). This is the gold-standard
//! check that the BGE embeddings are real and meaningful — not the synthetic
//! fallback. Requires `--features onnx` and a staged model:
//!   PROXIMADB_EMBED_MODEL_DIR=/path/with/bge-small-en-v1.5.onnx+tokenizer.json
#![cfg(feature = "onnx")]

use proximadb_embedding::{
    EmbeddingError,
    config::{ChunkConfig, EmbedRoute, EmbeddingConfig},
    scheduler::{EmbedSchedulerConfig, IngestMode},
    service::{EmbedBatch, EmbedRecord, EmbeddingService},
};

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bge_clusters_cats_apart_from_finance() {
    let svc = EmbeddingService::try_global().unwrap_or_else(|| {
        EmbeddingService::initialize(
            EmbeddingConfig {
                route: EmbedRoute::BgeSmall,
                chunk: ChunkConfig::default(),
            },
            EmbedSchedulerConfig::default(),
        )
        .expect("initialize embedding service")
    });

    let texts = [
        (
            "cat",
            "A cat is sleeping peacefully on the warm sunny windowsill",
        ),
        (
            "kitten",
            "Kittens are baby cats that love to play and chase balls of yarn",
        ),
        (
            "finance",
            "The stock market crashed today amid recession fears and rising inflation",
        ),
    ];
    let batch = EmbedBatch {
        records: texts
            .iter()
            .map(|(id, t)| EmbedRecord {
                id: (*id).to_string(),
                text: (*t).to_string(),
                tenant_id: "semantic-test".to_string(),
            })
            .collect(),
        mode: IngestMode::Sync,
    };

    let result = match svc.embed_sync(batch).await {
        Ok(r) => r,
        Err(EmbeddingError::ModelUnavailable(m)) => {
            panic!(
                "BGE model unavailable — run with --features onnx and PROXIMADB_EMBED_MODEL_DIR pointing at the staged model: {m}"
            );
        }
        Err(e) => panic!("embed error: {e}"),
    };

    let v = &result.vectors;
    assert_eq!(v.len(), 3, "three embeddings");
    assert_eq!(v[0].len(), 384, "bge-small is 384-dim");

    let cat_kitten = cosine(&v[0], &v[1]);
    let cat_finance = cosine(&v[0], &v[2]);
    let kitten_finance = cosine(&v[1], &v[2]);
    eprintln!(
        "SEMANTIC COSINE: cos(cat,kitten)={cat_kitten:.4}  cos(cat,finance)={cat_finance:.4}  cos(kitten,finance)={kitten_finance:.4}"
    );

    assert!(
        cat_kitten > cat_finance,
        "cat~kitten ({cat_kitten:.4}) must exceed cat~finance ({cat_finance:.4}) — BGE is not clustering semantically"
    );
    assert!(
        cat_kitten > kitten_finance,
        "cat~kitten ({cat_kitten:.4}) must exceed kitten~finance ({kitten_finance:.4})"
    );
}
