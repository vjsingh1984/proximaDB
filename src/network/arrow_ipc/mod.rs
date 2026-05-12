// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Arrow IPC (Flight) protocol implementation for high-throughput bulk ingestion
//!
//! This module provides an Arrow Flight server on port 5680 for:
//! - Bulk vector ingestion via DoPut (100K-200K vectors/sec)
//! - Vector search via DoGet
//! - Explicit flush/compact operations via DoAction
//!
//! Design principles:
//! - Reuse existing UnifiedHandlers for consistency with REST/gRPC
//! - Reuse existing Arrow infrastructure (arrow_ipc_scanner, unified_columnar_io)
//! - Minimal new code, maximum leverage of proven patterns

pub mod codec;
pub mod file_export;
pub mod multimodal_codec;
pub mod multimodel_codec;
pub mod server;
pub mod service;

pub use codec::ArrowProtoCodec;
pub use file_export::{
    ArrowFileExportHandler, ArrowFileInfo, ArrowFileRequest, ArrowFileTicket, ExportFileFormat,
    FlightCompression, SstArrowCache, SstArrowCacheConfig, SstArrowCacheStats,
};
pub use server::ArrowFlightServer;
pub use service::ProximaFlightService;
