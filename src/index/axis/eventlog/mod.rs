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
pub mod service_adapter;
pub mod service_interface;

pub use event_log::{
    EventLogQueue, EventType, ExtractionMode, FileIndexingStatus, IndexEvent, IndexEventBuilder,
    OperationType, StorageEngineType,
};

pub use event_log_manager::{EventLogConfig, EventLogManager};

pub use service_interface::{
    EventFilter, EventLogClient, EventLogCommand, EventLogQuery, EventLogService, EventStatus,
    ServiceHealth, ServiceMode,
};

pub use service_adapter::{EventLogServiceAdapter, EventLogServiceFactory};
