/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// SIMD optimization features (using stable AVX2 instead of unstable AVX-512)

// Increase recursion limit for complex types with serde
#![recursion_limit = "1024"]
// Documentation: enforce missing docs for public items.
// Modules suppress locally with #[allow(missing_docs)] until documented.
#![warn(missing_docs)]
// Suppress private item doc warnings (internal implementation details)
#![allow(clippy::missing_docs_in_private_items)]
// Suppress warnings that require significant API redesign (tracked separately)
#![allow(clippy::too_many_arguments)] // Needs config-struct refactor
#![allow(clippy::type_complexity)] // Needs type-alias extraction
#![allow(clippy::result_large_err)] // Needs error-type refactor
// Enforce error handling best practices
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]
#![warn(clippy::unimplemented)]
#![warn(clippy::todo)]
#![warn(clippy::large_enum_variant)]

//! # ProximaDB - Cloud-Native Vector Database
//!
//! **proximity at scale**
//!
//! ProximaDB is a high-performance, cloud-native vector database engineered for AI-first applications.
//! Built from the ground up for serverless deployment, intelligent data tiering, and global scale.
//!
//! ## Architecture Overview
//!
//! ProximaDB follows a modular, layered architecture optimized for vector similarity search:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Client Applications                       │
//! ├─────────────────────────────────────────────────────────────┤
//! │                  API Layer (REST + gRPC)                     │
//! │                    [api_handlers module]                     │
//! ├─────────────────────────────────────────────────────────────┤
//! │                     Service Layer                            │
//! │            [services module - business logic]                │
//! ├─────────────────────────────────────────────────────────────┤
//! │    Index Layer          │         Compute Layer              │
//! │   [AXIS engine]         │    [SIMD/GPU acceleration]         │
//! ├─────────────────────────────────────────────────────────────┤
//! │                     Storage Layer                            │
//! │    [WAL + Memtable]  →  [Storage Engines]  →  [Filesystem]  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Module Organization
//!
//! - **`api`**: Protocol definitions and API contracts
//! - **`api_handlers`**: Unified REST/gRPC request handlers with zero-copy proto-first design
//! - **`core`**: Core types, errors, configuration, and foundational components
//! - **`compute`**: Vector computation, distance metrics, quantization, and hardware acceleration
//! - **`index`**: AXIS indexing engine with multiple algorithm support (HNSW, IVF, LSH, etc.)
//! - **`services`**: Business logic layer for collections, search, and vector operations
//! - **`storage`**: Multi-tiered storage with WAL, memtable, and pluggable storage engines
//! - **`network`**: Server implementation with REST and gRPC support
//! - **`infrastructure`**: Shared infrastructure components and utilities
//! - **`metrics`**: Comprehensive metrics collection and monitoring
//!
//! ## Key Design Principles
//!
//! 1. **Proto-First Architecture**: Native protocol buffer flow without intermediate conversions
//! 2. **Zero-Copy Operations**: Minimize data copying throughout the pipeline
//! 3. **Hardware Adaptive**: Automatic detection and use of SIMD/GPU capabilities
//! 4. **Cloud-Native Storage**: Seamless integration with S3, Azure Blob, GCS
//! 5. **Pluggable Storage Engines**: Support for different workload patterns (SST, VIPER, NOVA, etc.)
//!
//! ## Key Features
//!
//! - **Proximity at Scale**: SIMD-optimized vector operations with GPU acceleration
//! - **Serverless-Native**: Scale to zero, pay per use
//! - **Intelligent Tiering**: MMAP hot data, S3 cold storage
//! - **Multi-Cloud**: AWS, Azure, GCP support
//! - **Global Distribution**: Multi-region with data residency
//! - **Enterprise Ready**: RBAC, audit logs, compliance

/// REST and gRPC API definitions and protocol contracts
// pub mod api; // Removed - using proto types directly with serde compatibility
/// Shared infrastructure components for cross-cutting concerns
#[allow(missing_docs)]
pub mod infrastructure;

/// High-performance compute layer with SIMD/GPU acceleration for vector operations
#[allow(missing_docs)]
pub mod compute;

/// Read-only collection analyzers (Entanglement Index, etc.)
pub mod analytics;

// pub mod consensus;  // Disabled - requires raft dependency

/// Core types, errors, configuration, and foundational components
#[allow(missing_docs)]
pub mod core;

/// Unified error handling for REST and gRPC APIs
#[allow(missing_docs)]
pub mod errors;

/// Native graph database engine with CSR format and Arc-based memory sharing
#[allow(missing_docs)]
pub mod graph;

// pub mod distributed;  // Temporarily disabled for single-node optimization

/// Unified API handlers for REST and gRPC with proto-first zero-copy design
#[allow(missing_docs)]
pub mod api_handlers;

/// Enhanced authentication and authorization for multi-tenant enterprise
#[allow(missing_docs)]
pub mod auth;

/// Comprehensive audit system for enterprise compliance
#[allow(missing_docs)]
pub mod audit;

/// Unified security architecture consolidating auth, RBAC, and audit
#[allow(missing_docs)]
pub mod security;

/// AI-powered intelligence for Release 2 enterprise platform
#[allow(missing_docs)]
pub mod ai;

/// Enterprise deployment automation for one-click setup
#[allow(missing_docs)]
pub mod deployment;

/// Enterprise revenue engine for billing and customer success (opt-in via feature `revenue_surface`)
#[cfg(feature = "revenue_surface")]
#[allow(missing_docs)]
pub mod revenue;

/// Sales enablement platform for customer-facing sales automation (opt-in via feature `sales_endpoints`)
#[cfg(feature = "sales_endpoints")]
#[allow(missing_docs)]
pub mod sales_enablement;

/// License management and tier enforcement for all deployment models (opt-in via feature `licensing_surface`)
#[cfg(feature = "licensing_surface")]
#[allow(missing_docs)]
pub mod licensing;

/// Executive intelligence platform for C-level strategic analytics (opt-in)
#[cfg(feature = "executive_intel")]
pub mod executive;

/// AXIS indexing engine with support for multiple algorithms (HNSW, IVF, LSH, etc.)
#[allow(missing_docs)]
pub mod index;

/// AutoML Framework for automated optimization and tuning
#[allow(missing_docs)]
pub mod automl;

/// LLM Integration for embeddings, RAG, and semantic caching
/// Leverages Victor (codingagent) framework for embedding generation
#[allow(missing_docs)]
pub mod llm;

/// Unified metrics module — combines advanced persistent metrics with real-time monitoring
#[allow(missing_docs)]
pub mod metrics;
/// Real-time monitoring and health check infrastructure
#[allow(missing_docs)]
pub mod monitoring;
/// Network transport layer — REST, gRPC, Arrow Flight, PostgreSQL wire protocol
#[allow(missing_docs)]
pub mod network;
/// Protocol Buffer definitions generated by prost-build (auto-generated, not manually documented)
#[allow(missing_docs)]
pub mod proto;
/// Multi-model query engine — federated SQL, RL planner, materialized views
#[allow(missing_docs)]
pub mod query;

/// DataFusion integration for compute engine compatibility.
/// Provides TableProvider implementations for SQL queries over ProximaDB collections.
/// NOTE: Feature-gated due to Arrow version mismatch (DataFusion 45.x uses Arrow 54.x,
/// ProximaDB uses Arrow 57.x). Enable with `--features datafusion-integration`.
#[cfg(feature = "datafusion-integration")]
pub mod datafusion;

/// Schema management — Avro, versioning, and metadata schema definitions
#[allow(missing_docs)]
pub mod schema;
// NOTE: schema_constants module removed - using hardcoded schema_types.rs instead
// schema_types removed - use core::avro_unified instead
/// Search primitives — hybrid search, BM25, fusion strategies
#[allow(missing_docs)]
pub mod search;
/// Server lifecycle — startup, shutdown, configuration, health probes
#[allow(missing_docs)]
pub mod server;
/// Service layer — collection management, vector operations, event log
#[allow(missing_docs)]
pub mod services;
/// Storage engine layer — 6 engines, WAL, filesystem, cache, memtable, metadata
#[allow(missing_docs)]
pub mod storage;
/// Common utilities — UUID, checksum, encoding, bitmap, cache
#[allow(missing_docs)]
pub mod utils;

/// DataSource Connector interface (Spark DataSource V2-style)
/// Provides pluggable connectors for external storage systems and pushdown negotiation
#[allow(missing_docs)]
pub mod connectors;

/// Observability module for Cloud SIEM / Datadog-like capabilities
/// Provides high-throughput ingestion and querying for logs, metrics, and traces
#[allow(missing_docs)]
pub mod observability;

/// Real-time streaming module for continuous vector ingestion
/// Provides lock-free ring buffers, backpressure handling, and live queries
#[allow(missing_docs)]
pub mod streaming;

/// Change Data Capture (CDC) module for database synchronization
/// Captures changes from PostgreSQL, MySQL, MongoDB and streams to Kafka, webhooks
#[allow(missing_docs)]
pub mod cdc;

/// Cross-model ACID transactions with two-phase commit protocol
/// Provides atomicity, consistency, isolation, and durability across vector, document, graph, and time-series models
#[allow(missing_docs)]
pub mod transaction;

/// Database operations for backup, restore, and maintenance
/// Provides incremental snapshots, WAL checkpointing, and disaster recovery
#[allow(missing_docs)]
pub mod operations;

/// Benchmark suite for performance validation
/// Provides ANN-benchmarks integration and competitor comparisons
#[allow(missing_docs)]
pub mod bench;

pub mod version;

/// Embedded mode for in-process database usage without network layer
/// Enable with feature flag: --features python
#[allow(missing_docs)]
pub mod embedded;

/// Core database instance and lifecycle management
/// Moved from lib.rs to improve modularity
pub mod database;

/// Distributed cluster coordination for multi-node deployments
/// Provides consensus, metadata management, shard routing, and node registry
#[allow(missing_docs)]
pub mod cluster;

/// Unified Catalog System with pluggable backends
/// Supports: Native, AWS Glue, Databricks Unity, Apache Polaris, Hive, Iceberg
#[allow(missing_docs)]
pub mod catalog;

// NOTE: Compiled Avro schemas disabled - using hardcoded schema_types.rs instead
// pub mod compiled_schemas {
//     include!(concat!(env!("OUT_DIR"), "/compiled_schemas.rs"));
// }

// Re-export commonly used types from core
pub use core::{Config, VectorRecord, error::ProximaDBError as Error};

// Re-export catalog types for unified schema management
pub use catalog::{
    CatalogCache,
    // Catalog management
    CatalogManager,
    TableIdentifier,
    // Catalog federation for unified view across internal and external catalogs
    federation::{
        ConstraintSupport, ExternalCatalog, ExternalCatalogConfig, ExternalCatalogType,
        FederatedCatalog, FederatedCatalogConfig, FederatedTableInfo,
    },
    // Internal schema registry
    internal::{
        // Object model
        CatalogObject,
        // Enforcement
        ConstraintEnforcer,
        ConstraintType,
        ConstraintViolation,
        DocumentProperties,
        EnforcementResult,
        ForeignKeyReference,
        GraphProperties,
        // Information schema
        InformationSchema,
        InformationSchemaView,
        InternalSchemaRegistry,
        // Model properties
        ModelProperties,
        ObjectSchema,
        ObjectType,
        ObservabilityProperties,
        RdbmsProperties,
        ReferentialAction,
        SchemaEnforcementMode,
        // Constraints
        TableConstraint,
        VectorProperties,
    },
};

// ============================================================================
// Storage-Compute Separation Re-exports (Hadoop-style architecture)
// ============================================================================

// Re-export key compute types for the pluggable compute layer
pub use compute::{
    ComputeCapabilities,
    // Compute plan types
    ComputePlan,
    // Compute provider interface
    ComputeProvider,
    // Compute scheduler
    ComputeScheduler,
    CostEstimate,
    Expr as ComputeExpr,
    LocalComputeProvider,
    PlanNode,
    SchedulingPolicy,
};

// Re-export key connector types for external system integration
pub use connectors::{
    DataReader,
    // Core connector traits
    DataSourceConnector,
    DataWriter,
    // Pushdown types
    PushdownRequest,
    PushdownResponse,
    // Context types
    ReadContext,
    TableInfo,
    TableStatistics,
    WriteContext,
    // Result types
    WriteResult,
};

// Re-export key storage format types for format abstraction
pub use storage::formats::{
    // Format registry
    FormatRegistry,
    FormatType,
    // Context types
    ReadContext as FormatReadContext,
    // Core format traits
    StorageFormat,
    WriteContext as FormatWriteContext,
};

/// Convenience result type using a boxed dynamic error for cross-layer propagation.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

// Re-export the main database instance from the database module
pub use database::ProximaDB;
