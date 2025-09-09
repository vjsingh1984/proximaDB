// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC protocol implementation with thin handlers

pub mod v1;
pub mod entity_service;
pub mod graph_service;
pub mod vector_service;
pub mod sql_service;
pub mod collection_service;

// Re-export the entity service for SKS
pub use entity_service::EntityServiceImpl;
// Re-export the graph service
pub use graph_service::GraphServiceImpl;
