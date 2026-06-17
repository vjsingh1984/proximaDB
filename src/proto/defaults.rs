//! Compatibility re-export for runtime proto defaults.
//!
//! Runtime defaults inspect host capacity and emit diagnostics, so the behavior
//! lives in the platform runtime crate rather than generated proto contracts.

pub use proximadb_runtime::proto_defaults::*;

/// Convert a v1 wire `VectorRecord` into the canonical record envelope.
///
/// ADR-009 keeps v1 vector payloads as protocol compatibility inputs only.
/// Callers below the protocol edge should receive `ProximaRecord` with the
/// same defaulting behavior regardless of REST, gRPC, or streaming transport.
pub fn vector_record_to_proxima_record(
    mut record: crate::proto::proximadb_v1::VectorRecord,
) -> proximadb_records::ProximaRecord {
    apply_vector_record_defaults(&mut record);
    proximadb_records::ProximaRecord::from(record)
}
