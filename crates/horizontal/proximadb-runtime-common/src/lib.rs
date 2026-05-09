//! Shared runtime utilities that are reusable across storage, query, modality,
//! platform, and application crates.
//!
//! This crate must stay horizontal: it may depend on foundation crates when
//! necessary, but it must not depend upward into domain, platform, integration,
//! root, application, or binding crates.

pub mod cache;

pub use cache::{CacheEntry, CacheError, CacheStats, LruCache, ThreadSafeLruCache};
