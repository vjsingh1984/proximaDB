//! Compatibility facade for the multi-model SQL router.
//!
//! New implementation lives in `query::multimodel_router`; this module preserves
//! the historical `multimodal_router` path used by protocol code.

pub use crate::query::multimodel_router::{
    DataModel, MultiModelResult as MultiModalResult, ObservabilityResult, StorageOptions,
    detect_storage_options_from_create, detect_store_type_from_create,
    detect_store_type_from_query,
};
