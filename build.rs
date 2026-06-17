fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Note: Proto files are currently pre-generated in the extracted
    // `proximadb-proto` workspace crate. The root crate no longer rebuilds
    // directly on proto-tree changes.
    //
    // The build system that generated them is not currently active in build.rs.
    // To regenerate proto files, use: ./scripts/regenerate-proto.sh
    //
    // TODO: Integrate tonic-build directly into build.rs for automatic regeneration
    // Blocked on: Need to investigate correct tonic-build 0.14 API usage

    // Add cdylib crate-type when pylib feature is enabled
    // This allows Python/FFI builds to generate shared libraries
    // while avoiding test exclusion issues in default builds (see ADR-006)
    if std::env::var("CARGO_FEATURE_PYLIB").is_ok() {
        println!("cargo:rust-cdylib=proximadb");
    }

    Ok(())
}

// NOTE: Proto regeneration is currently manual. See docs/proto-regeneration-workflow.md
// The previous attempt to use tonic_build::configure() failed due to API mismatch.
// The pre-generated proto files now live in the extracted `proximadb-proto` crate
// and should be used as-is until we can investigate the correct tonic-build 0.14
// API or update to a newer version.

// Legacy protobuf compilation function (feature-gated, no longer used)
// Kept for reference but not called - tonic-build handles all proto compilation
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
