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

// Module declarations
pub mod error;

#[cfg(feature = "client")]
pub mod client;

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
pub use client::{ClientBuilder, ClientConfig, CollectionInfo, HealthStatus, ProximaClient};

pub use collection::{
    CollectionBuilder, CollectionHandle, DistanceMetric, IndexType, InsertBuilder,
    InsertBuilderBatch, InsertBuilderWithId, StorageEngine,
};

pub use search::{SearchBuilder, SearchMode, SearchResult};

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
        let result = connect("http://localhost:5678");
        assert!(result.is_ok());
    }
}
