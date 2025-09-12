// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Error types for the SST storage engine.

use thiserror::Error;

/// The main error type for the SST module.
#[derive(Error, Debug)]
pub enum SstError {
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization or deserialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),

    /// An error that occurred during compaction.
    #[error("Compaction error: {0}")]
    Compaction(String),

    /// An error that occurred during flushing.
    #[error("Flush error: {0}")]
    Flush(String),

    /// An error that occurred during searching.
    #[error("Search error: {0}")]
    Search(String),

    /// An error that occurred in the underlying storage.
    #[error("Storage error: {0}")]
    Storage(String),

    /// An invalid argument was provided.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    /// A collection was not found.
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    /// An internal error occurred.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// A specialized `Result` type for SST operations.
pub type Result<T> = std::result::Result<T, SstError>;
