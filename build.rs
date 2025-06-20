fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize basic logging for build scripts
    tracing_subscriber::fmt::init();
    
    tracing::info!("🔨 Building ProximaDB protobuf schemas");
    
    // Compile optimized ProximaDB proto with zero-copy support
    tracing::debug!("Compiling protobuf schemas from proto/proximadb.proto");
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/proto")
        .file_descriptor_set_path("src/proto/proximadb_descriptor.bin")
        .compile(&["proto/proximadb.proto"], &["proto"])?;
    tracing::info!("✅ Protobuf compilation complete");

    println!("cargo:rerun-if-changed=proto/proximadb.proto");
    Ok(())
}
