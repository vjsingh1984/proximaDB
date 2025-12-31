// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Protocol handlers that wrap existing server implementations
//!
//! Each handler adapts an existing server (Axum, Tonic, Arrow Flight)
//! to the `ProtocolHandler` trait interface.

mod arrow_flight;
mod grpc;
mod rest;

pub use arrow_flight::ArrowFlightHandler;
pub use grpc::GrpcHandler;
pub use rest::{RestHandler, RestHandlerConfig};
