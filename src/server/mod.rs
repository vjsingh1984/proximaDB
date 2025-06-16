// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! ProximaDB Server Module
//! 
//! This module contains the server-level abstractions and builders for ProximaDB.
//! It coordinates all subsystems while maintaining separation of concerns.

pub mod builder;

// Re-export main types for easier use
pub use builder::{
    ServerBuilder, 
    ProximaDBServer, 
    ServerConfig,
    NetworkConfig,
    ComputeConfig,
    IndexingConfig,
    MonitoringConfig,
    HardwareAcceleration,
    DistanceMetric,
    IndexingAlgorithm,
};