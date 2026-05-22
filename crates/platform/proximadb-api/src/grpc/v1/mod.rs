//! # gRPC API v1 Services
//!
//! Version 1 gRPC services for ProximaDB.
//!
//! These services are compatibility adapters. New clients should use canonical
//! record/table services, especially `proximadb.v2.ProximaRecordService`.

use tonic::{
    Response, Status,
    metadata::{MetadataMap, MetadataValue},
};

pub mod collection;
pub mod document;
pub mod entity;
pub mod graph;
pub mod hybrid;
pub mod observability;
pub mod security;
pub mod sql;
pub mod streaming;
pub mod vector;

pub const GRPC_V1_REPLACEMENT_SURFACE: &str = "proximadb.v2.ProximaRecordService, pgwire";
pub const GRPC_V1_DEPRECATION_MESSAGE: &str =
    "gRPC v1 is compatibility-only; use canonical ProximaRecord APIs for new clients";

fn apply_v1_deprecation_metadata_map(metadata: &mut MetadataMap) {
    metadata.insert("deprecation", MetadataValue::from_static("true"));
    metadata.insert(
        "x-proximadb-api-status",
        MetadataValue::from_static("deprecated-compatibility"),
    );
    metadata.insert(
        "x-proximadb-replacement",
        MetadataValue::from_static(GRPC_V1_REPLACEMENT_SURFACE),
    );
    metadata.insert(
        "x-proximadb-deprecation-message",
        MetadataValue::from_static(GRPC_V1_DEPRECATION_MESSAGE),
    );
}

pub fn apply_v1_deprecation_metadata<T>(response: &mut Response<T>) {
    apply_v1_deprecation_metadata_map(response.metadata_mut());
}

pub fn deprecated_response<T>(body: T) -> Response<T> {
    let mut response = Response::new(body);
    apply_v1_deprecation_metadata(&mut response);
    response
}

pub fn deprecated_status(mut status: Status) -> Status {
    apply_v1_deprecation_metadata_map(status.metadata_mut());
    status
}

// Re-exports
pub use collection::CollectionServiceImpl;
pub use document::DocumentServiceImpl;
pub use entity::EntityServiceImpl;
pub use graph::GraphServiceImpl;
pub use hybrid::HybridSearchServiceImpl;
pub use observability::ObservabilityServiceImpl;
pub use security::SecurityServiceImpl;
pub use sql::SqlServiceImpl;
pub use streaming::StreamingServiceImpl;
pub use vector::VectorServiceImpl;
