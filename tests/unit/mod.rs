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

//! ProximaDB Unit Tests
//!
//! This module contains all unit tests organized by functional area.
//! Unit tests focus on testing individual components in isolation.

pub mod clustering_models_test;
pub mod compute;
pub mod config;
pub mod core;
pub mod handlers;
pub mod mvcc_logic_tests;
pub mod network;
pub mod query;
pub mod search;
pub mod serialization_compression_tests;
pub mod server;
pub mod services;
pub mod sst_optimization_tests;
pub mod storage;
pub mod write_buffer_recovery_stress_tests;
pub mod write_buffer_write_optimization_tests;
