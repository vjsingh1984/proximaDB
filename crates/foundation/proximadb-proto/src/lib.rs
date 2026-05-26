//! Shared protobuf and gRPC contract crate for ProximaDB.
//!
//! This is the first workspace extraction slice. It preserves the existing
//! generated sources as the single source of truth while compiling them in an
//! isolated crate so unrelated root-crate changes do not force recompilation of
//! the full protocol surface.

#![allow(missing_docs)]
#![allow(clippy::large_enum_variant)]

pub mod utils {
    pub use proximadb_kernel::encoding;
}

/// Generated v1 protobuf and gRPC definitions.
pub mod proximadb_v1 {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/proto/proximadb.v1.rs"
    ));
}

/// Hand-written ranking-pipeline wire types (R-7c.4a).
///
/// Source of truth lives in `proto/proximadb/v1/ranking.proto`; this
/// module ships the prost-derived Rust mirrors until the next proto
/// regeneration pass folds them into `src/proto/`.
pub mod ranking;

/// Compatibility alias used by the generated streaming protobuf code.
pub mod v1 {
    pub use super::proximadb_v1::*;
}

/// Generated streaming protobuf definitions.
pub mod streaming {
    pub mod v1 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/proto/proximadb.streaming.v1.rs"
        ));
    }
}

pub mod proximadb_streaming_v1 {
    pub use super::streaming::v1::*;
}

pub mod cluster {
    pub mod v1 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/proto/proximadb.cluster.v1.rs"
        ));
    }
}

pub mod proximadb_cluster_v1 {
    pub use super::cluster::v1::*;
}

pub mod v2 {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/proto/proximadb.v2.rs"
    ));
}

pub mod proximadb_v2 {
    pub use super::v2::*;
}

pub mod explain {
    pub mod v1 {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/proto/proximadb.explain.v1.rs"
        ));
    }
}

pub mod proximadb_explain_v1 {
    pub use super::explain::v1::*;
}

impl proximadb_v1::StorageEngine {
    /// Get all storage engines exposed by the protocol contract.
    pub fn all() -> &'static [Self] {
        &[
            Self::Viper,
            Self::Sst,
            Self::Helix,
            Self::Swift,
            Self::Nova,
            Self::Raptor,
            Self::Mmap,
            Self::Hybrid,
            Self::Tst,
            Self::Cedar,
            Self::Titan,
            Self::Chrono,
        ]
    }

    /// Whether the engine supports compression-aware persistence.
    pub fn supports_compression(&self) -> bool {
        matches!(self, Self::Viper | Self::Sst)
    }

    /// Whether the engine supports transactional workflows.
    pub fn supports_transactions(&self) -> bool {
        matches!(self, Self::Sst | Self::Hybrid)
    }

    /// Whether the engine persists data across process restarts.
    pub fn is_persistent(&self) -> bool {
        true
    }
}

pub mod proto {
    pub use super::proximadb_v1;
    pub use super::v1;
}

#[path = "proto/serde_impls.rs"]
pub mod serde_impls;

#[cfg(test)]
mod tests {
    use super::proximadb_v1::StorageEngine;

    #[test]
    fn storage_engine_helper_contracts_cover_all_protocol_engines() {
        let all = StorageEngine::all();
        assert_eq!(all.len(), 12);
        assert!(all.contains(&StorageEngine::Viper));
        assert!(all.contains(&StorageEngine::Sst));
        assert!(all.contains(&StorageEngine::Titan));

        for engine in all {
            assert!(engine.is_persistent());
        }

        assert!(StorageEngine::Viper.supports_compression());
        assert!(StorageEngine::Sst.supports_compression());
        assert!(!StorageEngine::Helix.supports_compression());

        assert!(StorageEngine::Sst.supports_transactions());
        assert!(StorageEngine::Hybrid.supports_transactions());
        assert!(!StorageEngine::Nova.supports_transactions());
    }
}
