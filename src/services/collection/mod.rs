//! Collection management
//! 
//! Handles collection lifecycle, metadata, and configuration

pub mod manager;

pub use manager::{
    CollectionService as Collections,
    CollectionServiceResponse as Response,
    CollectionServiceBuilder as Builder,
};

// Rename for clarity
pub type Manager = Collections;