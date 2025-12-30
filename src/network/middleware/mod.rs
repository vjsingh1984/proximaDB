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
//! including authentication, rate limiting, CORS, timeout, and metrics collection.
//!
//! # Security Middleware
//!
//! All security middleware is designed with defense-in-depth principles:
//! - **CORS**: Whitelist-based origin control (default: deny all cross-origin)
//! - **Rate Limiting**: Token bucket with per-IP and global limits
//! - **Authentication**: JWT and API key validation
//! - **Timeout**: Request timeout enforcement
//!
//! # Usage
//!
//! ```rust,ignore
//! use proximadb::network::middleware::{CorsConfig, RateLimitConfig, TimeoutConfig};
//!
//! // Production security configuration
//! let cors = CorsConfig::production()
//!     .allow_origin("https://app.example.com");
//! let rate_limit = RateLimitConfig::default(); // Enabled by default
//! let timeout = TimeoutConfig::default(); // 30 second timeout
//! ```

pub mod auth;
pub mod backpressure;
pub mod cors;
pub mod rate_limit;
pub mod request_id;
pub mod tenant;
pub mod timeout;
pub mod tls;

#[cfg(test)]
mod tests;

pub use auth::{AuthConfig, AuthLayer, UserInfo};
pub use backpressure::{BackpressureConfig, create_concurrency_limit_layer};
pub use cors::{CorsConfig, CorsConfigError, create_cors_layer};
pub use rate_limit::{RateLimitConfig, RateLimitLayer};
pub use request_id::{request_id_middleware, RequestId, RequestIdExt, RequestIdLayer, X_REQUEST_ID};
pub use tenant::{
    TenantContext, TenantContextExt, TenantExtractor, TenantExtractorConfig,
    TenantIdSource, X_TENANT_ID, tenant_middleware, create_tenant_extractor,
};
pub use timeout::{TimeoutConfig, create_timeout_layer};
pub use tls::{
    TlsAuthenticatedUser, TlsAuthenticatedUserExt, TlsClientCertConfig, TlsClientCertLayer,
    TlsClientCertState, TlsCertErrorResponse, matches_cn_pattern, tls_client_cert_middleware,
};
