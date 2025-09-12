/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Integration between storage flush operations and AXIS EventLog
//!
//! NOTE: This module is deprecated. Storage engines (SST and VIPER) now
//! directly notify the EventLog service through their flush_eventlog_integration
//! modules. The EventLog consumer in eventlog_consumer.rs handles async
//! processing of flush events for AXIS indexing.

// This file is kept as a placeholder for compatibility but is no longer used.
// All flush-to-AXIS integration now happens through:
// - src/storage/engines/sst/flush_eventlog_integration.rs
// - src/storage/engines/viper/flush_eventlog_integration.rs
// - src/index/axis/eventlog_consumer.rs

/// Placeholder for compatibility
pub struct FlushIntegration;

/// Placeholder for compatibility
#[derive(Debug, Clone)]
pub struct FlushConfig {
    pub enabled: bool,
}

/// Placeholder for compatibility
#[derive(Debug, Clone)]
pub struct FlushStats {
    pub total_flushes: u64,
}
