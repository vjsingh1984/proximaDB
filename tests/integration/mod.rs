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

//! ProximaDB Integration Tests
//!
//! This module organizes all integration tests that test multiple components
//! working together and end-to-end functionality.

// Integration test modules - these are organized by functional area
// Each subdirectory contains integration tests for that area

// gRPC integration tests
pub mod grpc;

// REST API integration tests  
pub mod rest;

// Storage system integration tests
pub mod storage;

// Vector operations integration tests
pub mod vector;

// Middleware integration tests are at this level since they cross-cut concerns