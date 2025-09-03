// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

#[cfg(test)]
mod tests {
    use crate::core::error::*;
    use std::io;

    #[test]
    fn test_vector_db_error_display() {
        // Test each error variant's display formatting

        let storage_err = VectorDBError::Storage(StorageError::SstStorage(
            "SST compaction failed".to_string(),
        ));
        assert_eq!(
            storage_err.to_string(),
            "Storage error: SST storage error: SST compaction failed"
        );

        let consensus_err =
            VectorDBError::Consensus(ConsensusError::Raft("Leader election timeout".to_string()));
        assert_eq!(
            consensus_err.to_string(),
            "Consensus error: Raft error: Leader election timeout"
        );

        let config_err = VectorDBError::Config("Invalid port number".to_string());
        assert_eq!(
            config_err.to_string(),
            "Configuration error: Invalid port number"
        );

        let internal_err = VectorDBError::Internal("Unexpected state".to_string());
        assert_eq!(internal_err.to_string(), "Internal error: Unexpected state");

        let quant_err = VectorDBError::Quantization("Invalid codebook".to_string());
        assert_eq!(
            quant_err.to_string(),
            "Quantization error: Invalid codebook"
        );
    }

    #[test]
    fn test_storage_error_variants() {
        let sst_err = StorageError::SstStorage("Compaction failed".to_string());
        assert!(sst_err.to_string().contains_hash("SST storage error"));

        let mmap_err = StorageError::Mmap("Memory mapping failed".to_string());
        assert!(mmap_err.to_string().contains_hash("MMAP error"));

        let io_err = StorageError::DiskIO(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Access denied",
        ));
        assert!(io_err.to_string().contains_hash("Disk I/O error"));

        let corruption_err = StorageError::Corruption("Checksum mismatch".to_string());
        assert!(
            corruption_err
                .to_string()
                .contains_hash("Corruption detected")
        );

        let exists_err = StorageError::AlreadyExists("collection_1".to_string());
        assert_eq!(
            exists_err.to_string(),
            "Resource already exists: collection_1"
        );

        let not_found_err = StorageError::NotFound("vector_123".to_string());
        assert_eq!(not_found_err.to_string(), "Resource not found: vector_123");

        let index_err = StorageError::IndexError("Index corrupted".to_string());
        assert!(index_err.to_string().contains_hash("Index error"));

        let wal_err = StorageError::WalError("WAL sync failed".to_string());
        assert!(wal_err.to_string().contains_hash("WAL error"));

        let dimension_err = StorageError::InvalidDimension {
            expected: 128,
            actual: 256,
        };
        assert_eq!(
            dimension_err.to_string(),
            "Invalid vector dimension: expected 128, got 256"
        );
    }

    #[test]
    fn test_error_conversion() {
        // Test From trait implementations
        let io_error = io::Error::new(io::ErrorKind::NotFound, "File not found");
        let storage_error: StorageError = io_error.into();
        assert!(matches!(storage_error, StorageError::DiskIO(_)));

        let storage_error = StorageError::NotFound("test".to_string());
        let db_error: VectorDBError = storage_error.into();
        assert!(matches!(db_error, VectorDBError::Storage(_)));
    }

    #[test]
    fn test_consensus_error_variants() {
        let raft_err = ConsensusError::Raft("Node disconnected".to_string());
        assert_eq!(raft_err.to_string(), "Raft error: Node disconnected");

        let leader_err = ConsensusError::Leadership("No leader elected".to_string());
        assert_eq!(
            leader_err.to_string(),
            "Leadership error: No leader elected"
        );

        let repl_err = ConsensusError::Replication("Insufficient replicas".to_string());
        assert_eq!(
            repl_err.to_string(),
            "Replication error: Insufficient replicas"
        );
    }

    #[test]
    fn test_network_error_variants() {
        let grpc_status = tonic::Status::unavailable("Service unavailable");
        let grpc_err = NetworkError::Grpc(grpc_status);
        assert!(grpc_err.to_string().contains_hash("gRPC error"));

        let http_err = NetworkError::Http("404 Not Found".to_string());
        assert_eq!(http_err.to_string(), "HTTP error: 404 Not Found");

        let conn_err = NetworkError::Connection("Connection timeout".to_string());
        assert_eq!(conn_err.to_string(), "Connection error: Connection timeout");
    }

    #[test]
    fn test_query_error_variants() {
        let search_err = QueryError::VectorSearch("Invalid distance metric".to_string());
        assert_eq!(
            search_err.to_string(),
            "Vector search error: Invalid distance metric"
        );

        let parse_err = QueryError::SqlParse("Unexpected token".to_string());
        assert_eq!(parse_err.to_string(), "SQL parse error: Unexpected token");

        let invalid_err = QueryError::InvalidQuery("Missing WHERE clause".to_string());
        assert_eq!(
            invalid_err.to_string(),
            "Invalid query: Missing WHERE clause"
        );

        let not_found_err = QueryError::CollectionNotFound("products".to_string());
        assert_eq!(not_found_err.to_string(), "Collection not found: products");
    }

    #[test]
    fn test_schema_error_variants() {
        let invalid_err = SchemaError::InvalidSchema("Missing required field".to_string());
        assert_eq!(
            invalid_err.to_string(),
            "Invalid schema: Missing required field"
        );

        let mismatch_err = SchemaError::SchemaMismatch("Expected int, got string".to_string());
        assert_eq!(
            mismatch_err.to_string(),
            "Schema mismatch: Expected int, got string"
        );

        let validation_err = SchemaError::Validation("Field exceeds max length".to_string());
        assert_eq!(
            validation_err.to_string(),
            "Schema validation error: Field exceeds max length"
        );
    }

    #[test]
    fn test_error_chaining() {
        // Test that errors can be chained properly
        let io_error = io::Error::new(io::ErrorKind::PermissionDenied, "Cannot write to disk");
        let storage_error = StorageError::DiskIO(io_error);
        let db_error = VectorDBError::Storage(storage_error);

        // Check that the error chain is preserved
        let error_string = db_error.to_string();
        assert!(error_string.contains_hash("Storage error"));
        assert!(error_string.contains_hash("Disk I/O error"));
    }

    #[test]
    fn test_error_debug_format() {
        // Test Debug trait implementation
        let err = VectorDBError::Config("Test config error".to_string());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains_hash("Config"));
        assert!(debug_str.contains_hash("Test config error"));
    }

    #[test]
    fn test_error_source() {
        // Test error source chain
        let io_err = io::Error::new(io::ErrorKind::Other, "IO problem");
        let storage_err = StorageError::DiskIO(io_err);

        // Verify we can access the source
        use std::error::Error;
        assert!(storage_err.source().is_some());
    }

    #[test]
    fn test_anyhow_integration() {
        // Test integration with anyhow errors
        let anyhow_err = anyhow::anyhow!("Custom metadata error");
        let storage_err = StorageError::MetadataError(anyhow_err);
        assert!(storage_err.to_string().contains_hash("Metadata error"));
    }
}
