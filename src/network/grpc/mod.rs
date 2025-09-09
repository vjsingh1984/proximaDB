// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC protocol implementation with thin handlers

pub mod v1;
pub mod entity_service;

// Re-export the service from v1
pub use v1::service::ProximaDbGrpcService;
// Re-export the entity service for SKS
pub use entity_service::EntityServiceImpl;
