// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC V2 API implementation for ProximaRecord and schema management
//!
//! This module provides gRPC handlers for the V2 API, which introduces:
//! - ProximaRecord with typed fields (TEXT, INTEGER, FLOAT, DECIMAL, UUID, etc.)
//! - Schema enforcement (STRICT, FLEXIBLE, HYBRID modes)
//! - Dedicated TEXT column storage with chunking support
//! - Typed filtering with range, equality, and CONTAINS operators
//!
//! ## Endpoints
//!
//! ### Record Operations
//! - `InsertRecords` - Batch insert ProximaRecords
//! - `UpsertRecords` - Insert or update records
//! - `UpdateRecords` - Update existing records
//! - `DeleteRecords` - Delete records by ID
//!
//! ### Search Operations
//! - `Search` - Search with typed filters
//! - `SearchStream` - Streaming search results
//!
//! ### Schema Operations
//! - `CreateSchema` - Create a new schema for a collection
//! - `GetSchema` - Get schema for a collection
//! - `ListSchemas` - List all schemas for a collection
//! - `EvolveSchema` - Evolve schema with compatibility checks

pub mod document_service;
pub mod graph_service;
pub mod record_service;

pub use document_service::ProximaDocumentServiceImpl;
pub use graph_service::ProximaGraphServiceImpl;
pub use record_service::ProximaRecordServiceImpl;
