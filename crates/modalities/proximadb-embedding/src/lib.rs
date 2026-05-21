//! # ProximaDB Embedding Modality
//!
//! In-process embedding service for ProximaDB. Tier-aware ONNX inference with
//! two dedicated tokio runtimes for sync and async workloads, Arc-shared
//! singleton, mmap-backed read-only model weights.
//!
//! ## Public API
//!
//! ```ignore
//! use proximadb_embedding::EmbeddingService;
//!
//! // At process startup:
//! let service = EmbeddingService::global();
//!
//! // Sync ingest (Approach A — pre-WAL):
//! let result = service.embed_sync(batch).await?;
//!
//! // Async ingest (Approach B — post-WAL drainer dispatches):
//! let event_id = service.embed_async(batch).await;
//! ```
//!
//! ## Architecture
//!
//! See [`docs/architecture/EMBEDDING.md`](../../../docs/architecture/EMBEDDING.md)
//! in the AnvaiOps repo for the full ADR. Short version:
//!
//! - One process loads three BGE ONNX models once at startup. Models are
//!   read-only after init and shared across all worker threads via
//!   `Arc<EmbeddingService>`.
//! - Two tokio runtimes (sync_pool, async_pool) with reserved worker counts.
//!   Sync requests pick the next free sync worker; no async task is
//!   preempted. When sync_queue is empty, idle sync workers opportunistically
//!   steal one batch from async_queue.
//! - Tier-to-model routing resolved against the AnvaiOps tenant registry;
//!   cached per-tenant for 60s in a `DashMap`.
//! - Optional Azure OpenAI client for the Premium Embedding add-on, and a
//!   BYO endpoint client for Enterprise customers with their own model.
//!
//! ## Dependencies
//!
//! - `proximadb-kernel` — core error types
//! - `proximadb-records` — `ProximaRecord` input/output
//! - `proximadb-data-model` — `ProximaValue` for vector encoding
//! - `tokio` — runtime / scheduler
//! - `tokenizers` — server-side chunking + token counting

pub mod chunker;
pub mod config;
pub mod metrics;
pub mod models;
pub mod scheduler;
pub mod service;
pub mod tokenizer;

pub use config::{ChunkConfig, EmbedRoute, EmbeddingConfig};
pub use models::ModelRegistry;
pub use scheduler::{EmbedScheduler, IngestMode, Priority};
pub use service::{EmbedBatch, EmbedRecord, EmbedResult, EmbeddingService};

/// Stable label used by embedding records in WAL + telemetry.
pub const EMBEDDING_LABEL: &str = "embedding";

/// Error type for the embedding service.
#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("tenant_unknown: {0}")]
    TenantUnknown(String),

    #[error("model_unavailable: {0}")]
    ModelUnavailable(String),

    #[error("model_inference_failed: {0}")]
    Inference(String),

    #[error("dim_mismatch: expected {expected}, got {actual}")]
    DimMismatch { expected: usize, actual: usize },

    #[error("scheduler queue full ({queue}, depth={depth})")]
    QueueFull { queue: &'static str, depth: usize },

    #[error("byo endpoint failure: {0}")]
    ByoEndpoint(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, EmbeddingError>;
