# `network` Module Review Report - Feature Gaps

## Identified Feature Gaps

The following items indicate areas for future work or missing features:

*   **File:** `multi_server.rs`
    *   **Line 334:** `// TODO: Use cloud_config from TOML for S3/GCS/Azure credentials`
    *   **Line 436:** `// TODO: Add assignment service recovery after StorageEngine starts`
    *   **Line 584:** `// TODO: Implement actual vector recovery from write buffer`
*   **File:** `metrics_service.rs`
    *   **Line 145:** `let _since = params.since; // TODO: Use for historical data`
    *   **Line 171:** `collection_interval_ms: 5000, // TODO: Get from config`
*   **File:** `rest/progressive_search_handler.rs`
    *   **Line 166:** `filter_expression: None, // TODO: Convert params.filter to FilterExpression`
*   **File:** `rest/handlers.rs`
    *   **Line 940:** `None, // TODO: Convert MetadataFilter to serde_json::Map`
    *   **Line 1137:** `// TODO: Delegate to metrics query service when integrated`
    *   **Line 1157:** `// TODO: Delegate to metrics query service when integrated`
    *   **Line 1312:** `custom_levels: vec![], // TODO: Convert REST levels to proto levels`
    *   **Line 1500:** `// TODO: Restore when QuantizationLevel and LevelType are available`
*   **File:** `grpc/service.rs`
    *   **Line 66:** `// TODO: Configure cloud-specific filesystem settings`
    *   **Line 548:** `Err(Status::unimplemented("Operation not yet implemented"))`
    *   **Line 1341:** `// TODO: Refactor to use trait abstractions or integration tests.`