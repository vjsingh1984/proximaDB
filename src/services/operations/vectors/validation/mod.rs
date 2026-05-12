//! Metadata validation and generation utilities for vector operations.

pub mod metadata;

pub use metadata::{
    DefaultPseudoQueryGenerator, PROXIMADB_PSEUDO_QUERY_FIELD, PROXIMADB_PSEUDO_QUERY_SOURCE_FIELD,
    PseudoQueryGenerator, apply_pseudo_query_metadata,
};
