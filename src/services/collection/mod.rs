//! Collection management
//!
//! Handles collection lifecycle, metadata, and configuration

pub mod manager;

pub use manager::{
    CollectionService as Collections, CollectionServiceBuilder as Builder,
    CollectionServiceResponse as Response,
};

// Rename for clarity
pub type Manager = Collections;
