// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Storage types used across the storage module
//!
//! Re-exports proto-generated enums as single source of truth

// Use proto-generated enums directly - no more duplicates!
pub use crate::proto::proximadb_v1::DistanceMetric;
pub use crate::proto::proximadb_v1::IndexingAlgorithm;
pub use crate::proto::proximadb_v1::StorageEngine as StorageEngineType;
