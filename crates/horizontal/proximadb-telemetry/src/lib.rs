//! Shared telemetry and request-correlation helpers for ProximaDB.
//!
//! This crate owns lightweight observability primitives that must be reusable by
//! API, network, query, modality, and runtime crates without depending on the
//! root application crate.

pub mod request_context;

pub use request_context::{
    REQUEST_ID_HEADER, REQUEST_ID_METADATA_KEY, RequestContext, create_request_context,
    extract_or_generate_request_id,
};
