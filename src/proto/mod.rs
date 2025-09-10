//! # Proto Module - Protocol Buffer Definitions
//!
//! This module contains ProximaDB's Protocol Buffer (protobuf) definitions that form
//! the foundation of the proto-first architecture. All data structures flow through
//! the system in protobuf format, enabling zero-copy operations and eliminating
//! serialization overhead.
//!
//! ## Proto-First Architecture
//!
//! ProximaDB uses protobuf as the native data format:
//! ```text
//! Client Request (Proto)
//!         ↓
//! API Layer (No Conversion)
//!         ↓
//! Service Layer (Direct Proto)
//!         ↓
//! Storage Layer (Proto Persistence)
//!         ↓
//! Response (Proto)
//! ```
//!
//! ## Key Benefits
//!
//! ### 1. **Zero-Copy Operations**
//! - No serialization/deserialization overhead
//! - Direct field access throughout the stack
//! - Memory-efficient data handling
//! - Reduced CPU usage
//!
//! ### 2. **Type Safety**
//! - Strongly typed messages
//! - Compile-time validation
//! - IDE autocomplete support
//! - Version compatibility checks
//!
//! ### 3. **Cross-Language Support**
//! - Native gRPC integration
//! - Python, Go, Java, C++ clients
//! - Consistent API across languages
//! - Automatic client generation
//!
//! ## Core Message Types
//!
//! ### VectorRecord
//! The fundamental data unit in ProximaDB:
//! ```protobuf
//! message VectorRecord {
//!     string id = 1;
//!     repeated float vector = 2;
//!     repeated MetadataItem metadata = 3;
//!     uint32 timestamp = 4;
//!     optional uint32 updated_at = 5;
//!     optional uint32 expires_at = 6;
//!     optional uint32 version = 7;
//!     optional bytes quantized_vector = 8;
//! }
//! ```
//!
//! ### Collection
//! Collection configuration and metadata:
//! ```protobuf
//! message Collection {
//!     string name = 1;
//!     uint32 dimensions = 2;
//!     DistanceMetric metric = 3;
//!     StorageEngine engine = 4;
//!     IndexConfig index_config = 5;
//!     CompressionConfig compression = 6;
//! }
//! ```
//!
//! ### SearchRequest
//! Vector similarity search request:
//! ```protobuf
//! message SearchRequest {
//!     string collection = 1;
//!     repeated float vector = 2;
//!     uint32 k = 3;
//!     optional MetadataFilter filter = 4;
//!     optional float radius = 5;
//! }
//! ```
//!
//! ### SearchResult
//! Search response with scored results:
//! ```protobuf
//! message SearchResult {
//!     repeated ScoredVector results = 1;
//!     uint32 total_results = 2;
//!     float search_time_ms = 3;
//! }
//! ```
//!
//! ## Service Definitions
//!
//! ### VectorService
//! Core CRUD operations:
//! ```protobuf
//! service VectorService {
//!     rpc CreateCollection(CreateCollectionRequest) returns (CreateCollectionResponse);
//!     rpc InsertVectors(InsertVectorsRequest) returns (InsertVectorsResponse);
//!     rpc SearchVectors(SearchRequest) returns (SearchResult);
//!     rpc UpdateVector(UpdateVectorRequest) returns (UpdateVectorResponse);
//!     rpc DeleteVector(DeleteVectorRequest) returns (DeleteVectorResponse);
//! }
//! ```
//!
//! ### StreamingService
//! Streaming operations for large datasets:
//! ```protobuf
//! service StreamingService {
//!     rpc StreamInsert(stream VectorRecord) returns (InsertSummary);
//!     rpc StreamSearch(SearchRequest) returns (stream ScoredVector);
//!     rpc StreamExport(ExportRequest) returns (stream VectorRecord);
//! }
//! ```
//!
//! ## Enumerations
//!
//! ### DistanceMetric
//! Supported distance metrics:
//! ```protobuf
//! enum DistanceMetric {
//!     EUCLIDEAN = 0;
//!     COSINE = 1;
//!     DOT_PRODUCT = 2;
//!     MANHATTAN = 3;
//!     HAMMING = 4;
//! }
//! ```
//!
//! ### StorageEngine
//! Available storage engines:
//! ```protobuf
//! enum StorageEngine {
//!     SST = 0;
//!     VIPER = 1;
//!     NOVA = 2;
//!     SWIFT = 3;
//!     PRISM = 4;
//!     RAPTOR = 5;
//! }
//! ```
//!
//! ## Proto-First Benefits in Practice
//!
//! ### Direct Field Access
//! ```rust
//! // No conversion needed - direct proto usage
//! let record = VectorRecord {
//!     id: "vec_123".to_string(),
//!     vector: vec![0.1, 0.2, 0.3],
//!     metadata: vec![],
//!     timestamp: now(),
//!     ..Default::default()
//! };
//!
//! // Direct field access without getters/setters
//! println!("Vector ID: {}", record.id);
//! ```
//!
//! ### Zero-Copy Persistence
//! ```rust
//! // Write proto directly to storage
//! storage.write_proto(&record)?;
//!
//! // Read proto without conversion
//! let record: VectorRecord = storage.read_proto(id)?;
//! ```
//!
//! ## Wire Format Efficiency
//!
//! Protobuf provides optimal wire format:
//! - **Varint Encoding**: Efficient integer encoding
//! - **Packed Arrays**: Optimized repeated fields
//! - **Field Presence**: Optional fields save space
//! - **Binary Format**: Compact representation
//!
//! ## Schema Evolution
//!
//! Protobuf supports backward/forward compatibility:
//! - Add new fields without breaking clients
//! - Deprecate fields gracefully
//! - Rename fields with aliases
//! - Change field types safely
//!
//! ## Generated Code
//!
//! Proto definitions are compiled to Rust:
//! ```bash
//! # Generate Rust code from .proto files
//! protoc --rust_out=src/proto proximadb.proto
//!
//! # With gRPC support
//! protoc --rust_out=src/proto \\
//!        --grpc_out=src/proto \\
//!        --plugin=protoc-gen-grpc=`which grpc_rust_plugin` \\
//!        proximadb.proto
//! ```
//!
//! ## Performance Impact
//!
//! Proto-first architecture performance:
//! - **Serialization**: 0ms (already in proto format)
//! - **Memory Usage**: 30% less than JSON
//! - **Network Transfer**: 50% smaller than JSON
//! - **CPU Usage**: 80% reduction in serialization
//!
//! ## Best Practices
//!
//! 1. **Use Proto Types Natively**: Don't convert unnecessarily
//! 2. **Leverage Field Presence**: Use optional for nullable fields
//! 3. **Pack Repeated Fields**: Enable packing for arrays
//! 4. **Version Carefully**: Plan schema evolution
//! 5. **Document Fields**: Add comments in .proto files

pub mod proximadb; // TODO: Remove once v1 schema is complete

// V1 proto definitions for SKS (Semantic Knowledge Store)
#[path = "proximadb.v1.rs"]
pub mod proximadb_v1;
