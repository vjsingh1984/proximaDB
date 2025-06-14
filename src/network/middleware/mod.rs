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

//! HTTP middleware for ProximaDB
//!
//! This module provides various middleware layers for HTTP request processing,
//! including authentication, rate limiting, logging, and metrics collection.

pub mod auth;
pub mod rate_limit;

pub use auth::{AuthLayer, AuthConfig};
pub use rate_limit::{RateLimitLayer, RateLimitConfig};