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

//! Authentication middleware for ProximaDB HTTP API

use axum::{
    extract::State,
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{Json, Response},
};
use serde::Serialize;
use std::collections::HashMap;

/// Authentication configuration
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Enable authentication (if false, all requests pass through)
    pub enabled: bool,
    /// API keys mapped to user/tenant information
    pub api_keys: HashMap<String, UserInfo>,
    /// Whether to require authentication for health endpoints
    pub require_auth_for_health: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_keys: HashMap::new(),
            require_auth_for_health: false,
        }
    }
}

/// User information associated with an API key
#[derive(Debug, Clone)]
pub struct UserInfo {
    pub user_id: String,
    pub tenant_id: Option<String>,
    pub permissions: Vec<String>,
}

/// Authentication error response
#[derive(Debug, Serialize)]
pub struct AuthErrorResponse {
    error: String,
    message: String,
}

/// Authentication layer for Axum
pub struct AuthLayer {
    #[allow(dead_code)]
    config: AuthConfig,
}

impl AuthLayer {
    pub fn new(config: AuthConfig) -> Self {
        Self { config }
    }

    /// Create a disabled authentication layer (all requests pass through)
    pub fn disabled() -> Self {
        Self {
            config: AuthConfig {
                enabled: false,
                ..Default::default()
            },
        }
    }

    /// Create an authentication layer with basic API key support
    pub fn with_api_keys(api_keys: HashMap<String, UserInfo>) -> Self {
        Self {
            config: AuthConfig {
                enabled: true,
                api_keys,
                require_auth_for_health: false,
            },
        }
    }
}

/// Authentication middleware function
pub async fn auth_middleware<B>(
    State(auth_config): State<AuthConfig>,
    mut request: Request<B>,
    next: Next<B>,
) -> Result<Response, (StatusCode, Json<AuthErrorResponse>)> {
    // Skip authentication if disabled
    if !auth_config.enabled {
        return Ok(next.run(request).await);
    }

    let path = request.uri().path();

    // Skip authentication for health endpoints (if configured)
    if !auth_config.require_auth_for_health && is_health_endpoint(path) {
        return Ok(next.run(request).await);
    }

    // Extract Authorization header
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok());

    let api_key = match auth_header {
        Some(header_value) => {
            if header_value.starts_with("Bearer ") {
                &header_value[7..] // Remove "Bearer " prefix
            } else if header_value.starts_with("API-Key ") {
                &header_value[8..] // Remove "API-Key " prefix
            } else {
                header_value // Use as-is for simple API key
            }
        }
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(AuthErrorResponse {
                    error: "missing_authorization".to_string(),
                    message: "Authorization header is required".to_string(),
                }),
            ));
        }
    };

    // Validate API key
    let user_info = match auth_config.api_keys.get(api_key) {
        Some(user_info) => user_info.clone(),
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(AuthErrorResponse {
                    error: "invalid_api_key".to_string(),
                    message: "Invalid API key".to_string(),
                }),
            ));
        }
    };

    // Add user information to request extensions for use by handlers
    request.extensions_mut().insert(user_info);

    Ok(next.run(request).await)
}

/// Check if the path is a health endpoint
fn is_health_endpoint(path: &str) -> bool {
    path.starts_with("/health")
}

/// Extension trait to extract user info from requests
pub trait RequestUserInfo {
    fn user_info(&self) -> Option<&UserInfo>;
}

impl<T> RequestUserInfo for Request<T> {
    fn user_info(&self) -> Option<&UserInfo> {
        self.extensions().get::<UserInfo>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_health_endpoint() {
        assert!(is_health_endpoint("/health"));
        assert!(is_health_endpoint("/health/ready"));
        assert!(is_health_endpoint("/health/live"));
        assert!(!is_health_endpoint("/collections"));
        assert!(!is_health_endpoint("/api/health"));
    }

    #[test]
    fn test_auth_config_default() {
        let config = AuthConfig::default();
        assert!(!config.enabled);
        assert!(config.api_keys.is_empty());
        assert!(!config.require_auth_for_health);
    }
}
