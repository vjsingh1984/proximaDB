// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metrics Decorator for Metadata Providers
//!
//! Provides transparent metrics collection for any MetadataProvider implementation
//! using the decorator pattern. This allows adding metrics to any backend without
//! modifying its code.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;

use crate::proto::proximadb_v1::Collection;
use crate::storage::traits::{
    InternalCollectionProvider, MetadataProvider, MetricsOperationType, UnifiedMetricsCollector,
};

/// Decorator that adds metrics collection to any MetadataProvider
pub struct MetricsDecorator<T: MetadataProvider> {
    /// Inner metadata provider
    inner: T,
    /// Metrics collector
    metrics: Arc<UnifiedMetricsCollector>,
}

impl<T: MetadataProvider> MetricsDecorator<T> {
    /// Create a new metrics decorator
    pub fn new(inner: T, metrics: Arc<UnifiedMetricsCollector>) -> Self {
        Self { inner, metrics }
    }

    /// Record operation metrics
    async fn record_operation<R>(
        &self,
        op_type: MetricsOperationType,
        operation: impl std::future::Future<Output = Result<R>>,
    ) -> Result<R> {
        let start = Instant::now();
        let result = operation.await;
        let duration = start.elapsed();

        // Record metrics (fire and forget)
        self.metrics
            .record(op_type, duration.as_millis() as u64, result.is_ok(), None);

        result
    }
}

#[async_trait]
impl<T: MetadataProvider + Send + Sync> MetadataProvider for MetricsDecorator<T> {
    async fn get_uuid(&self, collection_id: &str) -> Result<Option<String>> {
        self.record_operation(
            MetricsOperationType::Read,
            self.inner.get_uuid(collection_id),
        )
        .await
    }

    async fn collection_metadata(&self, collection_id: &str) -> Result<Option<Collection>> {
        self.record_operation(
            MetricsOperationType::Read,
            self.inner.collection_metadata(collection_id),
        )
        .await
    }

    async fn get_collection(&self, collection_id: &str) -> Result<Option<Collection>> {
        self.record_operation(
            MetricsOperationType::Read,
            self.inner.get_collection(collection_id),
        )
        .await
    }

    async fn list_collections(&self) -> Result<Vec<Collection>> {
        self.record_operation(MetricsOperationType::List, self.inner.list_collections())
            .await
    }

    async fn upsert_collection_proto(&self, collection: &Collection) -> Result<()> {
        self.record_operation(
            MetricsOperationType::Write,
            self.inner.upsert_collection_proto(collection),
        )
        .await
    }

    async fn delete_collection(&self, collection_id: &str) -> Result<()> {
        self.record_operation(
            MetricsOperationType::Delete,
            self.inner.delete_collection(collection_id),
        )
        .await
    }
}

// If the inner type implements InternalCollectionProvider, so does the decorator
#[async_trait]
impl<T: InternalCollectionProvider + Send + Sync> InternalCollectionProvider
    for MetricsDecorator<T>
{
    // Marker trait - no additional methods
}

#[cfg(test)]
mod tests {
    use super::*;
    // Mock implementation for testing
    struct MockProvider {
        fail_on_get: bool,
    }

    #[async_trait]
    impl MetadataProvider for MockProvider {
        async fn get_uuid(&self, _collection_id: &str) -> Result<Option<String>> {
            if self.fail_on_get {
                anyhow::bail!("Mock failure")
            } else {
                Ok(Some("test-uuid".to_string()))
            }
        }

        async fn collection_metadata(&self, _collection_id: &str) -> Result<Option<Collection>> {
            Ok(None)
        }

        async fn get_collection(&self, _collection_id: &str) -> Result<Option<Collection>> {
            Ok(None)
        }

        async fn list_collections(&self) -> Result<Vec<Collection>> {
            Ok(vec![])
        }

        async fn upsert_collection_proto(&self, _collection: &Collection) -> Result<()> {
            Ok(())
        }

        async fn delete_collection(&self, _collection_id: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_metrics_decorator_success() {
        let provider = MockProvider { fail_on_get: false };
        let metrics = Arc::new(UnifiedMetricsCollector::new());
        let decorated = MetricsDecorator::new(provider, metrics.clone());

        let result = decorated.get_uuid("test-collection").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some("test-uuid".to_string()));

        // Wait for async metrics recording to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Verify metrics were recorded
        let snapshot = metrics.get_snapshot().await;
        assert_eq!(snapshot.successful_operations, 1);
        assert_eq!(snapshot.failed_operations, 0);
    }

    #[tokio::test]
    async fn test_metrics_decorator_failure() {
        let provider = MockProvider { fail_on_get: true };
        let metrics = Arc::new(UnifiedMetricsCollector::new());
        let decorated = MetricsDecorator::new(provider, metrics.clone());

        let result = decorated.get_uuid("test-collection").await;
        assert!(result.is_err());

        // Wait for async metrics recording to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Verify failure metrics were recorded
        let snapshot = metrics.get_snapshot().await;
        assert_eq!(snapshot.successful_operations, 0);
        assert_eq!(snapshot.failed_operations, 1);
    }
}
