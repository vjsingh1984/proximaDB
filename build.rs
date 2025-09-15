use tracing::debug;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize basic logging for build scripts
    tracing_subscriber::fmt::init();

    tracing::info!("🔨 Building ProximaDB protobuf schemas");

    // Compile v1 protobuf schemas with zero-copy support and serde derives
    tracing::debug!("Compiling v1 protobuf schemas - legacy migration complete!");
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/proto")
        .file_descriptor_set_path("src/proto/proximadb_v1_descriptor.bin")
        .protoc_arg("--experimental_allow_proto3_optional") // Allow proto3 optional fields
        // ULTRA-MINIMAL: Only add serde to simple enum types that need REST API serialization
        // Custom serde implementations handle oneof types and their nested components
        .type_attribute("DistanceMetric", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("StorageEngine", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("IndexingAlgorithm", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CompressionAlgorithm", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CollectionOperation", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("PropertyFilterOperator", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("QuantizationLevel", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde to array/object types needed by custom serde implementations
        .type_attribute("SqlArray", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("SqlObject", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("PropertyArray", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("PropertyObject", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde to config types needed for JSON serialization
        .type_attribute("IndexConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("QuantizationConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde to specific config types used by IndexConfig
        .type_attribute("HnswConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("IvfConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("LshConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde to embedding types needed for graph serialization
        .type_attribute("EmbeddingVersion", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde to graph types needed for JSON serialization
        .type_attribute("Node", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Edge", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde ONLY to simple request types (responses have custom implementations in serde_impls.rs)
        .type_attribute("VectorSearchRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("VectorBatchRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CollectionRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Now add the simple response types that were removed from serde_impls.rs
        .type_attribute("VectorOperationResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CollectionResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("VectorRecord", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Collection", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CollectionConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CollectionStats", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Entity", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("EntityResult", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde to missing simple types used by Entity and other complex types
        .type_attribute("TypedMetadata", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Provenance", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Relation", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("TemporalInfo", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("MetadataFilter", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde to SQL request/response types for gRPC/REST API
        .type_attribute("ExecuteSqlRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("ExecuteSqlResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("SqlRow", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("SqlRowField", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde to graph request/response types for gRPC/REST API
        .type_attribute("TraversalRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("TraversalResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("HybridSearchRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("HybridSearchResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CreateNodeRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CreateEdgeRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("NodeQuery", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("EdgeQuery", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add missing simple types found in error analysis
        .type_attribute("SearchQuery", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("IncludeFields", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("SearchParams", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("SearchOptimization", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("OperationMetrics", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("SearchResult", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("SearchVectorRecord", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("PropertyFilter", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("GraphPath", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("TraversalStats", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("HybridSearchStats", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("FilterableColumnSpec", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("StorageConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("StorageAssignment", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("TemporalVersion", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("VectorData", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("StringArray", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("TimeRange", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("PageInfo", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("ProgressInfo", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Component", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("LabelStats", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("EdgeTypeStats", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("GraphStats", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CompressionConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("FilterCondition", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde to oneof variant enums (needed by custom serde implementations)
        .type_attribute("sql_value::Value", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("filter_clause::Value", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("property_value::Value", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("metadata_item::Value", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("metadata_value::Value", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("source_content::Data", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("typed_field::Value", "#[derive(serde::Serialize, serde::Deserialize)]")
        // oneof types (SqlValue, PropertyValue, etc.) get custom serde from serde_impls.rs
        // TODO(migration): Remove "proto/proximadb.proto" once v1 schema is complete
        .compile(
            &[
                "proto/proximadb/v1/entity.proto",
                "proto/proximadb/v1/relations.proto",
                "proto/proximadb/v1/context.proto",
                "proto/proximadb/v1/graph.proto",
                "proto/proximadb/v1/vector.proto",
                "proto/proximadb/v1/types.proto",
                "proto/proximadb/v1/vector_types.proto",
                "proto/proximadb/v1/collection_types.proto",
                "proto/proximadb/v1/collection.proto",
                "proto/proximadb/v1/sql.proto",
            ],
            &["proto"],
        )?;
    tracing::info!("✅ Protobuf compilation complete");

    debug!("cargo:rerun-if-changed=proto/proximadb/v1/graph.proto");
    debug!("cargo:rerun-if-changed=proto/proximadb/v1/vector.proto");
    debug!("cargo:rerun-if-changed=proto/proximadb/v1/types.proto");
    debug!("cargo:rerun-if-changed=proto/proximadb/v1/sql.proto");
    debug!("cargo:rerun-if-changed=proto/proximadb/v1/vector_types.proto");
    debug!("cargo:rerun-if-changed=proto/proximadb/v1/collection_types.proto");
    debug!("cargo:rerun-if-changed=proto/proximadb/v1/collection.proto");
    Ok(())
}
