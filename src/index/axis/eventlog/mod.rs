/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Event log queue for AXIS asynchronous indexing
//! Simple filesystem-based event tracking to coordinate between storage and indexing

pub mod event_log;
pub mod event_log_manager;
pub mod service_interface;
pub mod service_adapter;

pub use event_log::{
    EventLogQueue,
    IndexEvent,
    IndexEventBuilder,
    FileIndexingStatus,
    StorageEngineType,
    OperationType,
    EventType,
    ExtractionMode,
};

pub use event_log_manager::{
    EventLogManager,
    EventLogConfig,
};

pub use service_interface::{
    EventLogService,
    EventLogQuery,
    EventLogCommand,
    EventLogClient,
    ServiceMode,
    ServiceHealth,
    EventFilter,
    EventStatus,
};

pub use service_adapter::{
    EventLogServiceAdapter,
    EventLogServiceFactory,
};