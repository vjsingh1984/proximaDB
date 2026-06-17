//! Smoke test for the embedding service singleton + scheduler.
//!
//! Runs without requiring staged ONNX model assets. The point is to verify
//! the Arc-shared singleton, the dual-pool scheduler, and the resolve_route
//! cache all work together; model calls may either return real vectors or the
//! deliberate `ModelUnavailable` error from default builds.

use std::sync::Arc;

use proximadb_embedding::{
    EmbeddingError, EmbeddingScalarType, EmbeddingValues,
    config::{ChunkConfig, EmbedRoute, EmbeddingConfig},
    scheduler::EmbedSchedulerConfig,
    scheduler::IngestMode,
    service::{EmbedBatch, EmbedRecord, EmbeddingService},
};

fn make_batch(tenant: &str, count: usize, mode: IngestMode) -> EmbedBatch {
    EmbedBatch {
        records: (0..count)
            .map(|i| EmbedRecord {
                id: format!("rec-{}", i),
                text: format!("test record {} for tenant {}", i, tenant),
                tenant_id: tenant.to_string(),
            })
            .collect(),
        mode,
    }
}

fn initialize() -> Arc<EmbeddingService> {
    if let Some(svc) = EmbeddingService::try_global() {
        return svc;
    }
    EmbeddingService::initialize(
        EmbeddingConfig {
            route: EmbedRoute::BgeSmall,
            chunk: ChunkConfig::default(),
        },
        EmbedSchedulerConfig::default(),
    )
    .expect("initialize embedding service")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn singleton_returns_same_arc() {
    let svc1 = initialize();
    let svc2 = EmbeddingService::global();
    assert!(
        Arc::ptr_eq(&svc1, &svc2),
        "singleton must yield the same Arc"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_embed_returns_vectors_of_correct_dim() {
    let svc = initialize();
    let batch = make_batch("tenant-a", 4, IngestMode::Sync);
    let result = match svc.embed_sync(batch).await {
        Ok(result) => result,
        Err(EmbeddingError::ModelUnavailable(message)) => {
            assert!(
                message.contains("onnx") || message.contains("model"),
                "unexpected model-unavailable message: {message}"
            );
            return;
        }
        Err(error) => panic!("unexpected sync embed error: {error}"),
    };
    assert_eq!(result.vectors.len(), 4);
    for v in &result.vectors {
        assert_eq!(v.len(), 384, "bge-small dimension");
    }
    // Deterministic — same text → same vector
    let batch2 = EmbedBatch {
        records: vec![EmbedRecord {
            id: "rec-0".into(),
            text: "test record 0 for tenant tenant-a".into(),
            tenant_id: "tenant-a".into(),
        }],
        mode: IngestMode::Sync,
    };
    let result2 = svc.embed_sync(batch2).await.expect("sync embed");
    assert_eq!(result2.vectors[0], result.vectors[0]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_embed_completes_via_callback() {
    let svc = initialize();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let batch = make_batch("tenant-b", 8, IngestMode::Async);
    svc.embed_async(batch, move |result| {
        tx.send(result).ok();
    })
    .expect("submit async");
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
        .await
        .expect("async embed timed out")
        .expect("oneshot dropped");
    match result {
        Ok(result) => assert_eq!(result.vectors.len(), 8),
        Err(EmbeddingError::ModelUnavailable(message)) => {
            assert!(
                message.contains("onnx") || message.contains("model"),
                "unexpected model-unavailable message: {message}"
            );
        }
        Err(error) => panic!("unexpected async embed error: {error}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_cache_returns_default_then_explicit() {
    let svc = initialize();
    let route = svc.resolve_route("unseen-tenant");
    assert_eq!(route, EmbedRoute::BgeSmall);

    svc.update_tenant_route("premium-tenant", EmbedRoute::BgeLarge);
    let route = svc.resolve_route("premium-tenant");
    assert_eq!(route, EmbedRoute::BgeLarge);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_embed_at_precision_returns_typed_fp16() {
    let svc = initialize();
    let batch = make_batch("tenant-fp16", 3, IngestMode::Sync);
    let result = match svc
        .embed_sync_at_precision(batch, EmbeddingScalarType::Fp16)
        .await
    {
        Ok(result) => result,
        Err(EmbeddingError::ModelUnavailable(message)) => {
            assert!(
                message.contains("onnx") || message.contains("model"),
                "unexpected model-unavailable message: {message}"
            );
            return;
        }
        Err(error) => panic!("unexpected sync embed error: {error}"),
    };
    assert_eq!(result.values.len(), 3);
    for v in &result.values {
        match v {
            EmbeddingValues::Fp16(elts) => assert_eq!(elts.len(), 384, "bge-small dim, fp16"),
            other => panic!("expected Fp16 variant, got {:?}", other.scalar_type()),
        }
    }
    assert_eq!(result.summary.from, EmbeddingScalarType::Fp32);
    assert_eq!(result.summary.to, EmbeddingScalarType::Fp16);
    assert_eq!(result.summary.batch_count, 3);
    assert_eq!(result.summary.element_count, 3 * 384);
    assert!(
        result.summary.was_converted(),
        "fp32 native → fp16 canonical must be flagged as a conversion"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduler_stats_reports_worker_counts() {
    let svc = initialize();
    let stats = svc.scheduler_stats();
    assert!(stats.sync_workers >= 1);
    assert!(stats.async_workers >= 1);
}
