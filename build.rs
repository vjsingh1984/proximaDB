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
        // Add serde to graph types needed for JSON serialization
        .type_attribute("Node", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Edge", "#[derive(serde::Serialize, serde::Deserialize)]")
        // oneof types (SqlValue, PropertyValue) get custom serde from serde_impls.rs but PartialEq works fine
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
