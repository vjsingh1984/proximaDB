//! Collection management
//!
//! Handles collection lifecycle, metadata, and configuration

pub mod manager;
pub mod security;

pub use manager::{
    CollectionService as Collections, CollectionServiceBuilder as Builder,
    CollectionServiceResponse as Response,
};
pub use security::SecureCollectionService;

// Rename for clarity
pub type Manager = Collections;
