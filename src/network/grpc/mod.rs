// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC protocol implementation with thin handlers

pub mod service;

pub use service::ProximaDbGrpcService;