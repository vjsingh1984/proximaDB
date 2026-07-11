//! Dependency-inversion port traits for ProximaDB storage (Slice D).
//!
//! Storage is high-level policy (engines, compaction, flush) that *drives*
//! higher-level collaborators (collection metadata, the ANN index). To let
//! `src/storage` depend *down* instead of *up* into `crate::services` /
//! `crate::index`, those collaborators are expressed here as narrow traits.
//! The concrete implementors (e.g. `CollectionService`) `impl` these traits in
//! their own modules, and the composition root injects `Arc<dyn …Port>` into
//! storage at startup. No upward edge anywhere.
//!
//! This crate is trait-only: no behavior, no concrete types beyond the stable
//! foundation proto types the contracts unavoidably name.

use anyhow::Result;
use proximadb_proto::proximadb_v1::Collection;
use proximadb_storage_common::StorageEngineType;

/// Read access to collection metadata that storage needs at flush/compaction
/// time (fetch the proto `Collection` for a name or UUID).
///
/// Inverts `crate::services::collection::CollectionService`: storage holds an
/// `Arc<dyn CollectionMetadataPort>` and never references the service crate.
/// The measured storage-driven surface is exactly one method — every call site
/// (viper engine/flush, the flush coordinator, the background-flush context)
/// only fetches the collection; richer service behavior stays in the service.
#[async_trait::async_trait]
pub trait CollectionMetadataPort: Send + Sync {
    /// Fetch the full proto collection (with all metadata) by name or UUID.
    /// Returns `Ok(None)` when the collection does not exist.
    async fn collection(&self, identifier: &str) -> Result<Option<Collection>>;
}

/// Event-log operations that storage needs when flush/compaction publishes
/// index-maintenance work.
///
/// Inverts the storage dependency on AXIS event-log concrete types. Storage
/// emits storage-native facts (collection, files, engine, quantization flags);
/// higher layers translate them into AXIS `IndexEvent`s.
#[async_trait::async_trait]
pub trait StorageEventLogPort: Send + Sync {
    /// Record a flush event and wait until the event log acknowledges it.
    async fn notify_flush(
        &self,
        collection_id: &str,
        flushed_files: Vec<String>,
        vector_count: usize,
        storage_engine: StorageEngineType,
        has_quantized: bool,
        has_fp32: bool,
    ) -> Result<()>;

    /// Record a completed compaction event.
    async fn notify_compaction(
        &self,
        collection_id: &str,
        output_files: Vec<String>,
        vector_count: usize,
        storage_engine: StorageEngineType,
    ) -> Result<()>;

    /// Return whether a file can be compacted. Implementations should fail
    /// open when the event log is unavailable.
    async fn can_compact(&self, collection_id: &str, file_path: &str) -> Result<bool>;
}

/// Level-specific vector quantization + Hamming distance that storage's
/// search/compaction paths need.
///
/// Inverts `crate::compute::quantization::quantization_engine::UnifiedQuantizationEngine`
/// (modality-tier `proximadb-quantization-kernel`, re-exported up through
/// `compute::quantization`): storage holds an `Arc<dyn QuantizationEnginePort>`
/// and never names the concrete engine, so the storage→modality up-edge dissolves.
///
/// The measured storage-driven surface is the level-specific `quantize_to_*`
/// encoders plus Hamming distance — every signature is primitive in/out
/// (`&[f32]` → `Vec<u8>`/`Vec<u16>`/tuples/`u32`), so NO modality type crosses
/// the seam (no facade DTOs needed). The generic level-based `quantize(level)`
/// is intentionally NOT here: it would drag the modality `UnifiedQuantizationLevel`
/// type across the boundary. Callers use the level-specific method instead.
///
/// The concrete `UnifiedQuantizationEngine` `impl`s this in its own crate — a
/// downward dep (modality→storage-ports is layering-allowed; the forbidden
/// direction is storage→modality). The composition root injects the impl.
pub trait QuantizationEnginePort: Send + Sync {
    /// 1-bit sign/threshold quantization → packed bits.
    fn quantize_to_binary(&self, vector: &[f32]) -> Result<Vec<u8>>;
    /// 8-bit signed scalar quantization.
    fn quantize_to_int8(&self, vector: &[f32]) -> Result<Vec<u8>>;
    /// 4-bit scalar quantization → (codes, min, max, len).
    fn quantize_to_u4(&self, vector: &[f32]) -> Result<(Vec<u8>, f32, f32, usize)>;
    /// 6-bit scalar quantization → (codes, min, max, len).
    fn quantize_to_u6(&self, vector: &[f32]) -> Result<(Vec<u8>, f32, f32, usize)>;
    /// 8-bit unsigned scalar quantization → (codes, min, max).
    fn quantize_to_u8(&self, vector: &[f32]) -> Result<(Vec<u8>, f32, f32)>;
    /// 16-bit unsigned scalar quantization → (codes, min, max).
    fn quantize_to_u16(&self, vector: &[f32]) -> Result<(Vec<u16>, f32, f32)>;
    /// Product quantization → packed codes.
    fn quantize_to_pq(
        &self,
        vector: &[f32],
        num_subvectors: usize,
        bits_per_code: u32,
    ) -> Result<Vec<u8>>;
    /// Hamming distance between two bit-packed binary codes.
    fn calculate_hamming_distance(&self, a: &[u8], b: &[u8]) -> u32;
}

/// Graph collection operations that observability's telemetry-graph-linker needs.
///
/// Inverts `GraphOperationsService` (root `src/graph/service.rs`): observability
/// holds `Arc<dyn GraphOperationsPort>` instead of `Arc<GraphService>`. The
/// measured surface is 2 methods, both foundation-typed (proto `CreateGraphRequest`
/// + `GraphCollection`, `Vec<String>`). The concrete `GraphOperationsService`
/// `impl`s this in the root (it defines the type → can impl any trait for it).
#[async_trait::async_trait]
pub trait GraphOperationsPort: Send + Sync {
    /// List all graph collection IDs.
    async fn list_graphs(&self) -> Result<Vec<String>>;

    /// Create a new graph collection. Returns the created collection.
    async fn create_graph_collection(
        &self,
        request: proximadb_proto::proximadb_v1::CreateGraphRequest,
    ) -> Result<std::sync::Arc<proximadb_proto::proximadb_v1::GraphCollection>>;
}

use proximadb_quantization_model::{
    QuantizedVector, StorageQuantizedData, UnifiedQuantizationLevel,
};

/// Collection-aware batch quantization + dequantization that storage's
/// write/compaction/search paths need.
///
/// Inverts `StorageQuantizationEngine` (modality-tier kernel): storage holds an
/// `Arc<dyn StorageQuantizationEnginePort>` and never names the concrete engine.
/// The return types (`Vec<StorageQuantizedData>`, `Vec<f32>`) and params
/// (`UnifiedQuantizationLevel`, `&QuantizedVector`) are all foundation types
/// (moved by #842) → **facade-FREE** (no DTO/adaptor layer).
///
/// The concrete `StorageQuantizationEngine` `impl`s this in its own crate (a
/// downward dep — modality→storage-ports is layering-allowed). The composition
/// root injects the impl. Training (`train(&mut self)`) is NOT on the port
/// (it's `&mut self`, not object-safe) — training happens before injection.
#[async_trait::async_trait]
pub trait StorageQuantizationEnginePort: Send + Sync {
    /// Quantize a batch of vectors using the engine's configured levels.
    async fn quantize_batch(
        &self,
        vectors: &[Vec<f32>],
        ids: Option<&[String]>,
    ) -> Result<Vec<StorageQuantizedData>>;

    /// Quantize a batch with a specific level override.
    async fn quantize_batch_with_level(
        &self,
        vectors: &[Vec<f32>],
        level: UnifiedQuantizationLevel,
    ) -> Result<Vec<StorageQuantizedData>>;

    /// Dequantize a vector back to approximate float values.
    async fn dequantize(&self, quantized: &QuantizedVector) -> Result<Vec<f32>>;
}
