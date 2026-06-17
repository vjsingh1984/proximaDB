//! Compatibility shim for ProximaDB protobuf contracts.
//!
//! The generated protobuf and gRPC surface now compiles in the dedicated
//! `proximadb-proto` workspace crate. The root crate keeps this module so
//! existing imports like `crate::proto::proximadb_v1` continue to work during
//! the workspace migration.

pub use proximadb_proto::cluster;
pub use proximadb_proto::explain;
pub use proximadb_proto::proximadb_cluster_v1;
pub use proximadb_proto::proximadb_explain_v1;
pub use proximadb_proto::proximadb_streaming_v1;
pub use proximadb_proto::proximadb_v1;
pub use proximadb_proto::proximadb_v2;
pub use proximadb_proto::serde_impls;
pub use proximadb_proto::streaming;
pub use proximadb_proto::v1;
pub use proximadb_proto::v2;

pub mod defaults;
