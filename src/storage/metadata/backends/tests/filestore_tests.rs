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
        let filesystem_factory = Arc::new(FilesystemFactory::new());
        let config = FilestoreMetadataConfig {
            storage_url: "file:///tmp/test_metadata".to_string(),
            enable_compression: true,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: None,
        };

        let backend = FilestoreMetadataBackend::new(config, filesystem_factory)
            .await
            .expect("Failed to create filestore backend");

        assert!(backend.internal_health_check().await.is_ok());
    }
}