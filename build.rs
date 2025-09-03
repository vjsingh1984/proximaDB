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
        .file_descriptor_set_path("src/proto/proximadb_descriptor.bin")
        .protoc_arg("--experimental_allow_proto3_optional") // Allow proto3 optional fields
        // Add serde derives to messages only (not enums - they get prost::Enumeration)
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Skip serde for fields containing prost_types::Value/Struct since they don't implement serde
        .field_attribute("SearchParams.filters", "#[serde(skip)]")
        .field_attribute("SearchParams.custom_hints", "#[serde(skip)]")
        .field_attribute("StructuredContent.data", "#[serde(skip)]")
        .compile(&["proto/proximadb.proto"], &["proto"])?;
    tracing::info!("✅ Protobuf compilation complete");

    debug!("cargo:rerun-if-changed=proto/proximadb.proto");
    Ok(())
}
