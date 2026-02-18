// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Protocol detectors for identifying incoming request protocols
//!
//! This module provides detectors for gRPC, Arrow Flight, and REST protocols.
//! Detectors are ordered by priority (higher = checked first).

mod arrow_flight;
mod grpc;
mod rest;

pub use arrow_flight::ArrowFlightDetector;
pub use grpc::GrpcDetector;
pub use rest::RestDetector;

#[cfg(test)]
mod tests;
