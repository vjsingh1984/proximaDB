use tracing::debug;

// Only import these when actually compiling GPU kernels
#[cfg(any(
    all(feature = "gpu", target_os = "linux", target_arch = "x86_64"),
    all(feature = "gpu", target_os = "macos", target_arch = "aarch64")
))]
use std::env;
#[cfg(any(
    all(feature = "gpu", target_os = "linux", target_arch = "x86_64"),
    all(feature = "gpu", target_os = "macos", target_arch = "aarch64")
))]
use std::path::PathBuf;
#[cfg(any(
    all(feature = "gpu", target_os = "linux", target_arch = "x86_64"),
    all(feature = "gpu", target_os = "macos", target_arch = "aarch64")
))]
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize basic logging for build scripts
    tracing_subscriber::fmt::init();

    tracing::info!("🔨 Building ProximaDB protobuf schemas");

    // Compile CUDA kernels if feature is enabled
    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    compile_cuda_kernels()?;

    // Compile Metal shaders if feature is enabled
    #[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
    compile_metal_shaders()?;

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
        // IMPORTANT: Graph types (Node, Edge) have PropertyValue which uses custom serde
        // These will be handled in serde_impls.rs, NOT here

        // Graph collection message types - most can use auto-generated serde
        // Only PropertyConstraint has oneof and needs custom impl
        .type_attribute("GraphSchema", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("NodeLabelSchema", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("EdgeTypeSchema", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("PropertySchema", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("StringConstraint", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("NumericConstraint", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("ArrayConstraint", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("RegexConstraint", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("UniqueConstraint", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("GraphStorageConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("GraphEngineConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("AccessControl", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Permission", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("UpdateSchemaRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("SchemaValidationResult", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("ValidationError", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("ValidationWarning", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("GraphIndex", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("GraphCollection", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CreateGraphRequest", "#[derive(serde::Serialize, serde::Deserialize)]")

        // Enum types for graph collection
        .type_attribute("PropertyType", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("Cardinality", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("PermissionType", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add serde ONLY to simple request types (responses have custom implementations in serde_impls.rs)
        .type_attribute("VectorSearchRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("VectorBatchRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CollectionRequest", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Make map fields in CollectionRequest optional (default to empty map)
        .field_attribute("CollectionRequest.query_params", "#[serde(default)]")
        .field_attribute("CollectionRequest.options", "#[serde(default)]")
        .field_attribute("CollectionRequest.migration_config", "#[serde(default)]")
        // Now add the simple response types that were removed from serde_impls.rs
        .type_attribute("VectorOperationResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CollectionResponse", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("VectorRecord", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Make map field in VectorRecord optional (default to empty map)
        .field_attribute("VectorRecord.metadata", "#[serde(default)]")
        .type_attribute("Collection", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute("CollectionConfig", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Make repeated fields in CollectionConfig optional (default to empty vec)
        .field_attribute("CollectionConfig.tags", "#[serde(default)]")
        .field_attribute("CollectionConfig.filterable_columns", "#[serde(default)]")
        .field_attribute("CollectionConfig.index_configs", "#[serde(default)]")
        .field_attribute("CollectionConfig.embedding_models", "#[serde(default)]")
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
        // Make map field in SearchQuery optional (default to empty map)
        .field_attribute("SearchQuery.filters", "#[serde(default)]")
        .type_attribute("IncludeFields", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Make optional fields in IncludeFields use defaults
        .field_attribute("IncludeFields.source", "#[serde(default)]")
        .field_attribute("IncludeFields.source_options", "#[serde(default)]")
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
                "proto/proximadb/v1/graph_collection.proto",
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

#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
fn compile_cuda_kernels() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;

    tracing::info!("🚀 Compiling CUDA kernels for GPU acceleration");

    // Check if nvcc is available
    let nvcc_check = Command::new("nvcc")
        .arg("--version")
        .output();

    if nvcc_check.is_err() {
        tracing::warn!("⚠️  nvcc not found - CUDA kernels will not be compiled");
        tracing::warn!("   GPU feature will fall back to CPU implementation");
        tracing::warn!("   Install CUDA Toolkit 11.0+ to enable GPU acceleration");
        return Ok(());
    }

    tracing::info!("✅ Found nvcc compiler");

    // CUDA source files
    let cuda_sources = vec![
        "src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/cuda/kernels.cu",
    ];

    // Output directory
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let cuda_build_dir = out_dir.join("cuda");
    fs::create_dir_all(&cuda_build_dir)?;

    // Compile each CUDA source file
    for source in &cuda_sources {
        tracing::info!("   Compiling CUDA kernel: {}", source);

        let source_path = PathBuf::from(source);
        let obj_file = cuda_build_dir.join(
            source_path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
                + ".o",
        );

        // Compile with nvcc
        let status = Command::new("nvcc")
            .args(&[
                "-c",                           // Compile only (don't link)
                "-O3",                          // Optimization level 3
                "--compiler-options",           // Pass options to host compiler
                "-fPIC",                        // Position-independent code
                "-arch=sm_60",                  // Target compute capability 6.0+ (Pascal and newer)
                "-gencode=arch=compute_60,code=sm_60",  // Pascal (GTX 10xx)
                "-gencode=arch=compute_70,code=sm_70",  // Volta (V100)
                "-gencode=arch=compute_75,code=sm_75",  // Turing (RTX 20xx)
                "-gencode=arch=compute_80,code=sm_80",  // Ampere (RTX 30xx, A100)
                "-gencode=arch=compute_86,code=sm_86",  // Ampere (RTX 30xx mobile)
                "-gencode=arch=compute_89,code=sm_89",  // Ada Lovelace (RTX 40xx)
                "-gencode=arch=compute_90,code=sm_90",  // Hopper (H100)
                "--use_fast_math",              // Use fast math optimizations
                "-Xptxas=-v",                   // Verbose PTX assembly (for debugging)
                source,
                "-o",
                obj_file.to_str().unwrap(),
            ])
            .status()?;

        if !status.success() {
            return Err(format!("Failed to compile CUDA kernel: {}", source).into());
        }

        tracing::info!("   ✅ Compiled: {}", obj_file.display());

        // Tell cargo to rerun if CUDA source changes
        println!("cargo:rerun-if-changed={}", source);
    }

    // Link CUDA object files into static library
    let lib_file = cuda_build_dir.join("libproximadb_cuda.a");

    tracing::info!("   Creating static library: {}", lib_file.display());

    let obj_files: Vec<PathBuf> = cuda_sources
        .iter()
        .map(|source| {
            let source_path = PathBuf::from(source);
            cuda_build_dir.join(
                source_path
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
                    + ".o",
            )
        })
        .collect();

    let status = Command::new("ar")
        .arg("rcs")
        .arg(&lib_file)
        .args(&obj_files)
        .status()?;

    if !status.success() {
        return Err("Failed to create CUDA static library".into());
    }

    tracing::info!("   ✅ Created: {}", lib_file.display());

    // Tell cargo to link against CUDA runtime and our library
    println!("cargo:rustc-link-search=native={}", cuda_build_dir.display());
    println!("cargo:rustc-link-lib=static=proximadb_cuda");
    println!("cargo:rustc-link-lib=cudart");

    // Add CUDA library path
    if let Ok(cuda_path) = env::var("CUDA_PATH") {
        println!("cargo:rustc-link-search=native={}/lib64", cuda_path);
    } else {
        // Default CUDA installation paths
        println!("cargo:rustc-link-search=native=/usr/local/cuda/lib64");
        println!("cargo:rustc-link-search=native=/usr/local/cuda/lib");
    }

    tracing::info!("✅ CUDA kernel compilation complete");

    Ok(())
}

#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
fn compile_metal_shaders() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;

    tracing::info!("🚀 Compiling Metal shaders for GPU acceleration on Apple Silicon");

    // Check if xcrun is available
    let xcrun_check = Command::new("xcrun")
        .arg("--version")
        .output();

    if xcrun_check.is_err() {
        tracing::warn!("⚠️  xcrun not found - Metal shaders will not be precompiled");
        tracing::warn!("   GPU feature will fall back to runtime shader compilation");
        tracing::warn!("   Install Xcode Command Line Tools to enable precompiled shaders");
        return Ok(());
    }

    tracing::info!("✅ Found xcrun compiler");

    // Metal shader source files
    let metal_sources = vec![
        "src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/kernels.metal",
    ];

    // Output directory
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let metal_build_dir = out_dir.join("metal");
    fs::create_dir_all(&metal_build_dir)?;

    // Also create target/metal for the precompiled library
    let target_metal_dir = PathBuf::from("target/metal");
    fs::create_dir_all(&target_metal_dir)?;

    // Compile each Metal shader source file
    for source in &metal_sources {
        tracing::info!("   Compiling Metal shader: {}", source);

        let source_path = PathBuf::from(source);
        let air_file = metal_build_dir.join(
            source_path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
                + ".air",
        );

        // Step 1: Compile .metal to .air (Apple Intermediate Representation)
        let status = Command::new("xcrun")
            .args(&[
                "-sdk", "macosx",
                "metal",
                "-c",                           // Compile only
                "-O3",                          // Optimization level 3
                "-std=metal3.1",                // Metal 3.1 standard (macOS 14+)
                "-mmacosx-version-min=12.0",    // Minimum macOS 12 (M1 launch)
                source,
                "-o",
                air_file.to_str().unwrap(),
            ])
            .status();

        match status {
            Ok(status) if status.success() => {
                // Compilation successful
            }
            Ok(_) | Err(_) => {
                // Metal toolchain not available - fall back to runtime compilation
                tracing::warn!("⚠️  Metal shader compilation failed - falling back to runtime compilation");
                tracing::warn!("   To enable precompiled shaders, run:");
                tracing::warn!("   xcodebuild -downloadPlatform iOS");
                tracing::warn!("   or install full Xcode from App Store");
                return Ok(());
            }
        }

        tracing::info!("   ✅ Compiled to AIR: {}", air_file.display());

        // Tell cargo to rerun if Metal source changes
        println!("cargo:rerun-if-changed={}", source);
    }

    // Step 2: Link all .air files into .metallib
    let lib_file = metal_build_dir.join("libproximadb_metal.metallib");
    let target_lib_file = target_metal_dir.join("libproximadb_metal.metallib");

    tracing::info!("   Creating Metal library: {}", lib_file.display());

    let air_files: Vec<PathBuf> = metal_sources
        .iter()
        .map(|source| {
            let source_path = PathBuf::from(source);
            metal_build_dir.join(
                source_path
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
                    + ".air",
            )
        })
        .collect();

    let status = Command::new("xcrun")
        .args(&["-sdk", "macosx", "metallib"])
        .args(&air_files)
        .arg("-o")
        .arg(&lib_file)
        .status()?;

    if !status.success() {
        return Err("Failed to create Metal library".into());
    }

    tracing::info!("   ✅ Created: {}", lib_file.display());

    // Copy to target/metal for runtime loading
    fs::copy(&lib_file, &target_lib_file)?;
    tracing::info!("   ✅ Copied to: {}", target_lib_file.display());

    // Set environment variable for runtime to find the library
    println!("cargo:rustc-env=METAL_LIBRARY_PATH={}", target_lib_file.display());

    tracing::info!("✅ Metal shader compilation complete");

    Ok(())
}
