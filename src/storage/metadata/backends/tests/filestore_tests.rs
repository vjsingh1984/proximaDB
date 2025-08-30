// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Tests for filestore metadata backend

#[cfg(test)]
mod tests {
    use super::super::super::filestore_backend::*;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_filestore_backend_create() {
        // Basic creation test
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let config = FilestoreMetadataConfig {
            storage_url: "file:///tmp/test_metadata_info".to_string(),
            compression: true,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let backend = FilestoreMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create filestore backend");

        // Backend was created successfully - test basic operations instead
        // Since there's no health check method, we'll test a simple operation
        let collection_uuids = backend.list_collection_uuids();
        assert!(collection_uuids.is_none()); // New backend should have no collections
    }
}