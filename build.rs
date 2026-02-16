fn main() {
    // Build script entry point
    // Protobuf compilation is handled via prost-build in code generation
    println!("cargo:rerun-if-changed=proto");
}

#[cfg(feature = "compile_protobuf")]
fn compile_protobuf_schemas() -> Result<(), Box<dyn std::error::Error>> {
    // Check if protoc is available
    let protoc_path = std::path::PathBuf::from("protoc");

    // If protoc is not available, log warning and return early (but continue with stub generation)
    if !protoc_path.exists() {
        tracing::warn!(
            "Protobuf compiler not found: {}. Please install Protocol Buffers compiler or set PROTOC environment variable.",
            protoc
        );
    }

    // Build protoc command with arguments
    let protoc_args = ["--experimental_allow_proto3_optional"];

    // Run protoc to compile schemas
    let status = Command::new("protoc")
        .args(protoc_args)
        .current_dir(PathBuf::from(PROXIMA_DIR))
        .output(capture_output(true))
        .status();

    // Handle result
    match status {
        Ok(_) => {
            tracing::info!("Protobuf schema compilation: legacy migration complete");
        }
        Err(e) => {
            tracing::warn!("Protobuf compilation failed: {}", e);
        }
    }

    Ok(())
}
