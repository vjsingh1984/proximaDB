//! Shared foundational contracts for ProximaDB.
//!
//! This crate is intentionally small. It holds leaf-level types that many
//! higher-level crates depend on and that should not pull in storage, query,
//! transport, or binding implementations.

pub mod base62;
pub mod checksum;
pub mod config_error;
pub mod encoding;
pub mod error;
pub mod foundation;
pub mod hash;
pub mod metadata_error;
pub mod service_error;
pub mod stream_error;
pub mod uuid;

pub use base62::*;
pub use checksum::*;
pub use encoding::*;
pub use error::*;
pub use foundation::*;
pub use hash::*;
pub use config_error::ConfigError;
pub use metadata_error::MetadataError;
pub use service_error::ServiceError;
pub use stream_error::{StreamError, StreamResult};
pub use uuid::*;
