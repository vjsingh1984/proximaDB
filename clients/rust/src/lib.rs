//! ProximaDB Rust SDK
//!
//! A native Rust SDK for ProximaDB, optimized for agent development.
//! Provides both client mode (HTTP/REST) and embedded mode (in-process).
//!
//! # Features
//!
//! - `client` (default) - HTTP/REST client for remote connections
//! - `embedded` - In-process database mode (requires linking to proximadb)
//! - `full` - Both client and embedded modes
//!
//! # Quick Start - Client Mode
//!
//! ```rust,ignore
//! use proximadb_sdk::{ProximaClient, StorageEngine};
//!
//! #[tokio::main]
//! async fn main() -> proximadb_sdk::Result<()> {
//!     // Connect to a ProximaDB server
//!     let client = ProximaClient::connect("http://localhost:5678")?;
//!
//!     // Create a collection
//!     client.create_collection("memories")
//!         .dimension(768)
//!         .engine(StorageEngine::Sst)
//!         .execute()
//!         .await?;
//!
//!     // Insert a vector
//!     let embedding = vec![0.1; 768];
//!     client.collection("memories")
//!         .insert()
//!         .id("mem_123")
//!         .vector(&embedding)
//!         .meta("type", "conversation")
//!         .execute()
//!         .await?;
//!
//!     // Search for similar vectors
//!     let query = vec![0.15; 768];
//!     let results = client.collection("memories")
//!         .search()
//!         .vector(&query)
//!         .top_k(10)
//!         .filter("type = 'conversation'")
//!         .execute()
//!         .await?;
//!
//!     for result in results {
//!         println!("ID: {}, Score: {}", result.id, result.score);
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! # Quick Start - Embedded Mode
//!
//! ```rust,ignore
//! use proximadb_sdk::{ProximaDB, StorageEngine};
//!
//! fn main() -> proximadb_sdk::Result<()> {
//!     // Open an embedded database
//!     let db = ProximaDB::embedded()
//!         .data_dir("/tmp/agent-memory")
//!         .cache_size_mb(512)
//!         .open()?;
//!
//!     // Create a collection
//!     db.create_collection("memories")
//!         .dimension(768)
//!         .engine(StorageEngine::Sst)
//!         .execute_sync()?;
//!
//!     // Insert vectors
//!     let embedding = vec![0.1; 768];
//!     db.collection("memories")
//!         .insert()
//!         .id("mem_123")
//!         .vector(&embedding)
//!         .meta("type", "conversation")
//!         .execute_sync()?;
//!
//!     // Search
//!     let query = vec![0.15; 768];
//!     let results = db.collection("memories")
//!         .search_embedded()
//!         .vector(&query)
//!         .top_k(10)
//!         .execute_sync()?;
//!
//!     // Cleanup
//!     db.flush()?;
//!     db.close()?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Search Modes
//!
//! ProximaDB supports multiple search modes for different recall/performance tradeoffs:
//!
//! ```rust,ignore
//! use proximadb_sdk::SearchMode;
//!
//! // Exact search (100% recall)
//! let results = collection.search()
//!     .vector(&query)
//!     .exact()
//!     .execute()
//!     .await?;
//!
//! // Approximate search (faster, ~95% recall)
//! let results = collection.search()
//!     .vector(&query)
//!     .approximate()
//!     .execute()
//!     .await?;
//!
//! // Approximate with custom nprobe
//! let results = collection.search()
//!     .vector(&query)
//!     .approximate_with_nprobe(5)
//!     .execute()
//!     .await?;
//!
//! // Adaptive (auto-selects based on dataset size)
//! let results = collection.search()
//!     .vector(&query)
//!     .adaptive(10000)
//!     .execute()
//!     .await?;
//! ```
//!
//! # Filtering
//!
//! ProximaDB provides a fluent filter builder for complex query filtering:
//!
//! ```rust,ignore
//! use proximadb_sdk::FilterBuilder;
//!
//! // Simple equality filter
//! let filter = FilterBuilder::new()
//!     .eq("category", "tech")
//!     .build();
//!
//! // Range filter
//! let filter = FilterBuilder::new()
//!     .gte("price", 100)
//!     .lte("price", 500)
//!     .build();
//!
//! // Complex filter with multiple conditions
//! let filter = FilterBuilder::new()
//!     .eq("status", "active")
//!     .in_list("category", vec!["tech", "science"])
//!     .range("rating", 3.0, 5.0)
//!     .build();
//!
//! // Use with search
//! let results = client.collection("items")
//!     .search()
//!     .vector(&query)
//!     .with_filter(filter)
//!     .execute()
//!     .await?;
//! ```
//!
//! # Graph Operations
//!
//! ProximaDB includes native graph database capabilities:
//!
//! ```rust,ignore
//! use proximadb_sdk::{ProximaClient, GraphNode, GraphEdge};
//!
//! // Create a graph
//! client.create_graph("knowledge")
//!     .execute()
//!     .await?;
//!
//! // Add nodes with fluent API
//! client.graph("knowledge")
//!     .add_node()
//!     .id("person_1")
//!     .label("Person")
//!     .property("name", "Alice")
//!     .execute()
//!     .await?;
//!
//! // Add edges
//! client.graph("knowledge")
//!     .add_edge()
//!     .from("person_1")
//!     .to("person_2")
//!     .relationship("KNOWS")
//!     .execute()
//!     .await?;
//!
//! // Traverse the graph
//! let results = client.graph("knowledge")
//!     .traverse()
//!     .start("person_1")
//!     .relationship("KNOWS")
//!     .max_depth(3)
//!     .execute()
//!     .await?;
//! ```

// Module declarations
pub mod error;
pub mod filter;

/// Serde compatibility shims for tolerant response deserialization.
///
/// The ProximaDB REST surface models several response fields as
/// nullable (`Option<T>` server-side, `nullable: true` in the OpenAPI
/// spec) — e.g. `CollectionV2Summary.record_count` is `null` when
/// `include_stats=false`, and `RecordV2Response.vector` / `.text_fields`
/// are `null` when not requested. The hand-written facade DTOs keep the
/// ergonomic non-`Option` Rust types (`u64`, `Vec<_>`) for callers, so
/// they need a deserializer that treats an explicit JSON `null` the same
/// as an absent field: fall back to `Default`. Plain `#[serde(default)]`
/// only covers the *absent* case and errors on explicit `null`.
pub(crate) mod serde_compat {
    use serde::{Deserialize, Deserializer};

    /// Deserialize `T`, mapping an explicit JSON `null` (and an absent
    /// field, via `#[serde(default)]` on the field) to `T::default()`.
    pub fn null_as_default<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: Deserialize<'de> + Default,
        D: Deserializer<'de>,
    {
        Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
    }
}

// Generated, do-not-edit REST transport (TD-126 Phase 4). progenitor emits the
// typed low-level client + models from the published OpenAPI spec; the
// hand-written `client` facade below wraps it. Regenerate with `make
// gen-rust-sdk`; CI gate `rust-sdk-codegen-drift` enforces it stays in sync.
#[cfg(feature = "client")]
pub mod genrest;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "client")]
pub mod graph;

pub mod collection;
pub mod search;

#[cfg(feature = "embedded")]
pub mod embedded;

// Re-exports for convenient access
pub use error::{
    CollectionError, ConfigError, EmbeddedError, NetworkError, ProximaError, Result, SearchError,
    VectorError,
};

#[cfg(feature = "client")]
pub use client::{
    ClientBuilder, ClientConfig, CollectionInfo, ColumnDefinition, ExplainQueryRequest, GraphInfo,
    HealthStatus, ProbeStatus, ProximaClient, QueryRequest, SchemaDefinition, SchemaResponse,
    UpdateSchemaRequest, UpdateSchemaResponse,
};

#[allow(deprecated)]
pub use collection::{
    CollectionBuilder, CollectionHandle, DistanceMetric, EmbeddingPrecision, IndexType,
    InsertBuilder, InsertBuilderBatch, InsertBuilderWithId, ProximaRecord, StorageEngine,
    UpdateBuilder, VectorRecord,
};

pub use search::{SearchBuilder, SearchMode, SearchResult};

// Filter builder exports
pub use filter::{
    Filter, FilterBuilder, FilterCondition, FilterGroup, FilterNode, FilterOp, LogicalOp,
    and_filters, eq, in_list, ne, or_filters, range,
};

// Graph exports (client mode only)
#[cfg(feature = "client")]
pub use graph::{
    EdgeBuilder, GraphBuilder, GraphEdge, GraphHandle, GraphNode, NodeBuilder, TraversalBuilder,
    TraversalDirection, TraversalResult,
};

#[cfg(feature = "embedded")]
pub use embedded::{EmbeddedBuilder, EmbeddedConfig, ProximaDB, StorageLocation, StorageStats};

/// Version of the ProximaDB SDK
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Convenience function to connect to a ProximaDB server
#[cfg(feature = "client")]
pub fn connect(url: impl Into<String>) -> Result<ProximaClient> {
    ProximaClient::connect(url)
}

/// Convenience function to create an embedded database builder
#[cfg(feature = "embedded")]
pub fn embedded() -> EmbeddedBuilder {
    ProximaDB::embedded()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }

    #[cfg(feature = "client")]
    #[test]
    fn test_connect_function() {
        let result = if std::env::var_os("LLVM_PROFILE_FILE").is_some() {
            Ok(ProximaClient::for_tests("http://localhost:5678"))
        } else {
            connect("http://localhost:5678")
        };
        assert!(result.is_ok());
    }
}
