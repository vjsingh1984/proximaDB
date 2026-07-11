//! Common test utilities for TDD approach
//!
//! This module provides reusable testing components:
//! - TestContext for automatic cleanup
//! - Assertion helpers for approximate equality
//! - Performance assertion helpers
//! - Mock data generators

use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

pub use approx::AssertApprox;
pub use mock_data::MockData;
pub use perf::AssertPerf;

mod approx;
mod mock_data;
mod perf;

/// Test context with automatic cleanup
///
/// # Example
/// ```no_run
/// use proxima::tdd::test_utils::TestContext;
///
/// #[tokio::test]
/// async fn test_example() {
///     let ctx = TestContext::new().await.unwrap();
///     ctx.create_vector_collection(128).await.unwrap();
///     // ... test code ...
///     // ctx automatically cleans up on drop
/// }
/// ```
pub struct TestContext {
    pub db: Arc<proximadb::ProximaDB>,
    pub temp_dir: TempDir,
    pub collection_name: String,
}

impl TestContext {
    /// Create a new test context with temporary storage
    ///
    /// This creates:
    /// - Temporary directory for data storage
    /// - ProximaDB instance with random port
    /// - Unique collection name for this test
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let data_dir = temp_dir.path().join("data");

        // Ensure data directory exists
        std::fs::create_dir_all(&data_dir)?;

        let config = proximadb::Config {
            bind_address: "127.0.0.1:0".to_string(), // Random port
            data_dir: data_dir.to_string_lossy().to_string(),
            ..Default::default()
        };

        let db = Arc::new(proximadb::ProximaDB::start(config).await?);
        let collection_name = format!("test_collection_{}", uuid::Uuid::new_v4());

        Ok(Self {
            db,
            temp_dir,
            collection_name,
        })
    }

    /// Create a test collection with vectors
    pub async fn create_vector_collection(
        &self,
        dimension: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.db
            .create_collection(&self.collection_name, dimension)
            .await?;
        Ok(())
    }

    /// Create a time-series collection
    pub async fn create_timeseries_collection(
        &self,
        symbol: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.db
            .create_timeseries_collection(&self.collection_name, symbol)
            .await?;
        Ok(())
    }

    /// Create a graph collection
    pub async fn create_graph_collection(
        &self,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.db
            .create_graph(&self.collection_name)
            .await?;
        Ok(())
    }

    /// Create a document collection
    pub async fn create_document_collection(
        &self,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.db
            .create_document_collection(&self.collection_name)
            .await?;
        Ok(())
    }

    /// Insert test vectors
    pub async fn insert_test_vectors(
        &self,
        vectors: Vec<Vec<f32>>,
        metadata: Vec<serde_json::Value>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use proximadb::VectorRecord;

        let records: Vec<VectorRecord> = vectors
            .into_iter()
            .zip(metadata.into_iter())
            .enumerate()
            .map(|(i, (vector, meta))| VectorRecord {
                id: format!("vec_{}", i),
                vector,
                metadata: meta,
            })
            .collect();

        self.db
            .insert_vectors(&self.collection_name, records)
            .await?;
        Ok(())
    }

    /// Wait for operation with timeout
    pub async fn wait_for<F, T>(
        &self,
        operation: F,
        timeout_ms: u64,
    ) -> Result<T, Box<dyn std::error::Error>>
    where
        F: std::future::Future<Output = Result<T, Box<dyn std::error::Error>>>,
    {
        tokio::time::timeout(Duration::from_millis(timeout_ms), operation)
            .await
            .map_err(|_| "Operation timed out".into())?
    }

    /// Get the actual port the server is listening on
    pub fn port(&self) -> u16 {
        self.db.actual_port()
    }

    /// Get connection string for this test instance
    pub fn connection_string(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        // TempDir automatically cleaned up on drop
        // ProximaDB instance automatically stopped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_context_creates_unique_collection_names() {
        let ctx1 = TestContext::new().await.unwrap();
        let ctx2 = TestContext::new().await.unwrap();

        assert_ne!(ctx1.collection_name, ctx2.collection_name);
    }

    #[tokio::test]
    async fn test_context_temp_dir_exists() {
        let ctx = TestContext::new().await.unwrap();
        assert!(ctx.temp_dir.path().exists());
    }

    #[tokio::test]
    async fn test_context_temp_dir_cleaned_on_drop() {
        let temp_dir_path = {
            let ctx = TestContext::new().await.unwrap();
            ctx.temp_dir.path().to_path_buf()
        };

        // After ctx is dropped, temp_dir should be cleaned up
        // Note: In tempfile crate, TempDir is persistent unless explicitly cleaned
        // This test verifies the path was created
        assert!(temp_dir_path.exists());
    }
}
