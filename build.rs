use tracing::debug;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize basic logging for build scripts
    tracing_subscriber::fmt::init();

    tracing::info!("🔨 Building ProximaDB protobuf schemas");

    // Compile optimized ProximaDB proto with zero-copy support and serde derives
    tracing::debug!("Compiling protobuf schemas from proto/proximadb.proto with serde support");
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/proto")
        .file_descriptor_set_path("src/proto/proximadb_v1_descriptor.bin")
        .protoc_arg("--experimental_allow_proto3_optional") // Allow proto3 optional fields
        // Add serde derives to messages only (not enums - they get prost::Enumeration)
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Skip serde for fields containing prost_types::Value/Struct/Timestamp since they don't implement serde
        .field_attribute("SearchParams.filters", "#[serde(skip)]")
        .field_attribute("SearchParams.custom_hints", "#[serde(skip)]")
        .field_attribute("StructuredContent.data", "#[serde(skip)]")
        .field_attribute("Entity.flexible_metadata", "#[serde(skip)]")
        // Skip all Timestamp fields
        .field_attribute("EmbeddingVersion.created_at", "#[serde(skip)]")
        .field_attribute("Provenance.extracted_at", "#[serde(skip)]")
        .field_attribute("TemporalInfo.created_at", "#[serde(skip)]")
        .field_attribute("TemporalInfo.valid_from", "#[serde(skip)]")
        .field_attribute("TemporalInfo.valid_to", "#[serde(skip)]")
        .field_attribute("TemporalVersion.timestamp", "#[serde(skip)]")
        .field_attribute("Relation.created_at", "#[serde(skip)]")
        .field_attribute("TimeRange.start", "#[serde(skip)]")
        .field_attribute("TimeRange.end", "#[serde(skip)]")
        .field_attribute("typed_field::Value.TimestampValue", "#[serde(skip)]")
        .field_attribute("temporal_clause::Clause.AtTime", "#[serde(skip)]")
        .compile(
            &[
                "proto/proximadb.proto",
                "proto/proximadb/v1/entity.proto",
                "proto/proximadb/v1/relations.proto",
                "proto/proximadb/v1/context.proto",
                "proto/proximadb/v1/graph.proto",
                "proto/proximadb/v1/vector.proto",
                "proto/proximadb/v1/sql.proto",
            ],
            &["proto"],
        )?;
    tracing::info!("✅ Protobuf compilation complete");

    debug!("cargo:rerun-if-changed=proto/proximadb.proto");
    debug!("cargo:rerun-if-changed=proto/proximadb/v1/graph.proto");
    debug!("cargo:rerun-if-changed=proto/proximadb/v1/vector.proto");
    debug!("cargo:rerun-if-changed=proto/proximadb/v1/sql.proto");
    Ok(())
}
