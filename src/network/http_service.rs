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

//! HTTP Service implementation for ProximaDB REST API
//! This is a service that creates routes, not a server that binds to ports

use axum::{
    http::{header::CONTENT_TYPE, Method},
    Router,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};

use crate::storage::StorageEngine;
use crate::api::rest::create_router;

/// HTTP Service configuration for REST API
#[derive(Debug, Clone)]
pub struct HttpServiceConfig {
    pub enable_cors: bool,
    pub enable_tracing: bool,
    pub auth_config: Option<crate::network::middleware::AuthConfig>,
    pub rate_limit_config: Option<crate::network::middleware::RateLimitConfig>,
}

impl Default for HttpServiceConfig {
    fn default() -> Self {
        Self {
            enable_cors: true,
            enable_tracing: true,
            auth_config: None,
            rate_limit_config: None,
        }
    }
}

/// HTTP Service for ProximaDB REST API
/// Creates router with REST endpoints, middleware, and configuration
pub struct HttpService {
    config: HttpServiceConfig,
    storage: Arc<RwLock<StorageEngine>>,
}

impl HttpService {
    /// Create new HTTP service
    pub fn new(config: HttpServiceConfig, storage: Arc<RwLock<StorageEngine>>) -> Self {
        Self {
            config,
            storage,
        }
    }

    /// Create the router with all REST API routes and middleware
    pub fn create_router(&self) -> Router {
        use axum::middleware;
        
        // Start with the base REST API router
        let mut app = create_router(self.storage.clone());

        // Add rate limiting middleware if configured
        if let Some(rate_limit_config) = &self.config.rate_limit_config {
            let rate_limit_state = std::sync::Arc::new(
                crate::network::middleware::rate_limit::RateLimitState::new(rate_limit_config.clone())
            );
            app = app.layer(middleware::from_fn_with_state(
                rate_limit_state,
                crate::network::middleware::rate_limit::rate_limit_middleware,
            ));
        }

        // Add authentication middleware if configured
        if let Some(auth_config) = &self.config.auth_config {
            app = app.layer(middleware::from_fn_with_state(
                auth_config.clone(),
                crate::network::middleware::auth::auth_middleware,
            ));
        }

        // Add tracing if enabled
        if self.config.enable_tracing {
            app = app.layer(TraceLayer::new_for_http());
        }

        // Add CORS if enabled
        if self.config.enable_cors {
            let cors = CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
                .allow_headers([CONTENT_TYPE]);
            app = app.layer(cors);
        }

        app
    }
}