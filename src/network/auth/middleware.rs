/*
 * Copyright 2025 ProximaDB
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

//! Unified Authentication Middleware for ProximaDB REST API

use crate::network::auth::{AuthError, AuthResult, AuthService, Permission, PermissionContext, ResourceType};
use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{Json, Response},
};
use serde::Serialize;
use std::sync::Arc;
use tracing::debug;

/// Authentication middleware state
pub struct AuthMiddlewareState {
    pub auth_service: Arc<AuthService>,
}

/// Authentication error response
#[derive(Debug, Serialize)]
pub struct AuthErrorResponse {
    pub error: String,
    pub message: String,
    pub code: u16,
}

/// Middleware to authenticate and authorize requests
pub async fn auth_middleware<B>(
    State(auth_state): State<AuthMiddlewareState>,
    mut request: Request<B>,
    next: Next<B>,
) -> Result<Response, (StatusCode, Json<AuthErrorResponse>)> {
    let uri = request.uri();
    let method = request.method();
    let path = uri.path();
    
    debug!("Processing auth for {} {}", method, path);
    
    // Skip authentication for health endpoints (configurable)
    if should_skip_auth(path) {
        debug!("Skipping auth for health endpoint: {}", path);
        return Ok(next.run(request).await);
    }
    
    // Extract authorization header
    let auth_header = extract_auth_header(&request)?;
    
    // Authenticate the request
    let auth_result = auth_state
        .auth_service
        .authenticate(&auth_header)
        .await
        .map_err(|e| auth_error_to_response(e))?;
    
    // Check basic permissions for the endpoint
    let required_permission = determine_required_permission(method.as_str(), path);
    if let Some(permission) = required_permission {
        let context = build_permission_context(path, &auth_result);
        
        auth_state
            .auth_service
            .rbac
            .check_permission_with_context(&auth_result.user_id, permission, &context)
            .await
            .map_err(|e| auth_error_to_response(e))?;
    }
    
    // Add auth result to request extensions for use by handlers
    request.extensions_mut().insert(auth_result);
    
    Ok(next.run(request).await)
}

/// Extract authorization header from request
fn extract_auth_header<B>(request: &Request<B>) -> Result<String, (StatusCode, Json<AuthErrorResponse>)> {
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(AuthErrorResponse {
                    error: "missing_authorization".to_string(),
                    message: "Authorization header is required".to_string(),
                    code: 401,
                }),
            )
        })?;
    
    let auth_str = auth_header.to_str().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(AuthErrorResponse {
                error: "invalid_authorization_header".to_string(),
                message: "Authorization header contains invalid characters".to_string(),
                code: 400,
            }),
        )
    })?;
    
    Ok(auth_str.to_string())
}

/// Determine if authentication should be skipped for this path
fn should_skip_auth(path: &str) -> bool {
    // Health endpoints
    if path.starts_with("/health") {
        return true;
    }
    
    // Auth endpoints (login, etc.)
    if path.starts_with("/auth/") {
        return true;
    }
    
    // Public documentation endpoints
    if path.starts_with("/docs") || path.starts_with("/openapi") {
        return true;
    }
    
    false
}

/// Determine required permission based on HTTP method and path
fn determine_required_permission(method: &str, path: &str) -> Option<Permission> {
    match path {
        // Collection endpoints
        path if path.starts_with("/collections") => {
            match method {
                "GET" => {
                    if path.ends_with("/collections") {
                        Some(Permission::ListCollections)
                    } else {
                        Some(Permission::ReadCollectionMetadata)
                    }
                }
                "POST" => Some(Permission::CreateCollection),
                "PUT" | "PATCH" => Some(Permission::UpdateCollectionMetadata),
                "DELETE" => Some(Permission::DeleteCollection),
                _ => None,
            }
        }
        
        // Vector endpoints
        path if path.contains("/vectors") => {
            match method {
                "GET" => Some(Permission::ReadVectors),
                "POST" => {
                    if path.contains("/search") {
                        Some(Permission::SearchVectors)
                    } else {
                        Some(Permission::InsertVectors)
                    }
                }
                "PUT" | "PATCH" => Some(Permission::UpdateVectors),
                "DELETE" => Some(Permission::DeleteVectors),
                _ => None,
            }
        }
        
        // Graph endpoints
        path if path.contains("/graph") => {
            match method {
                "GET" => Some(Permission::ReadGraphRelations),
                "POST" => Some(Permission::CreateGraphRelations),
                "DELETE" => Some(Permission::DeleteGraphRelations),
                _ => None,
            }
        }
        
        // SQL query endpoints
        path if path.contains("/sql") || path.contains("/query") => {
            Some(Permission::ExecuteSqlQueries)
        }
        
        // System endpoints
        path if path.starts_with("/system") => {
            match method {
                "GET" => {
                    if path.contains("/metrics") {
                        Some(Permission::ViewSystemMetrics)
                    } else {
                        Some(Permission::ViewSystemHealth)
                    }
                }
                "POST" | "PUT" | "PATCH" => Some(Permission::ConfigureSystem),
                _ => None,
            }
        }
        
        // Admin endpoints
        path if path.starts_with("/admin") => {
            if path.contains("/users") {
                Some(Permission::ManageUsers)
            } else if path.contains("/roles") {
                Some(Permission::ManageRoles)
            } else if path.contains("/api-keys") {
                Some(Permission::ManageApiKeys)
            } else if path.contains("/audit") {
                Some(Permission::ViewAuditLogs)
            } else {
                Some(Permission::ConfigureSystem)
            }
        }
        
        _ => None, // No specific permission required
    }
}

/// Build permission context from request path and auth result
fn build_permission_context(path: &str, auth_result: &AuthResult) -> PermissionContext {
    let resource_type = if path.starts_with("/collections") {
        ResourceType::Collection
    } else if path.contains("/vectors") {
        ResourceType::Vector
    } else if path.contains("/graph") {
        ResourceType::Graph
    } else if path.starts_with("/system") || path.starts_with("/admin") {
        ResourceType::System
    } else if path.contains("/users") {
        ResourceType::User
    } else if path.contains("/roles") {
        ResourceType::Role
    } else {
        ResourceType::System
    };
    
    // Extract resource ID from path if possible
    let resource_id = extract_resource_id(path);
    
    PermissionContext {
        resource_type,
        resource_id,
        tenant_id: auth_result.tenant_id.clone(),
        metadata: std::collections::HashMap::new(),
    }
}

/// Extract resource ID from URL path
fn extract_resource_id(path: &str) -> Option<String> {
    let path_segments: Vec<&str> = path.split('/').collect();
    
    // Look for patterns like /collections/{id}, /vectors/{id}, etc.
    for (i, segment) in path_segments.iter().enumerate() {
        if matches!(*segment, "collections" | "vectors" | "users" | "roles") {
            if let Some(id) = path_segments.get(i + 1) {
                if !id.is_empty() && *id != "search" && *id != "bulk" {
                    return Some(id.to_string());
                }
            }
        }
    }
    
    None
}

/// Convert auth error to HTTP response
fn auth_error_to_response(error: AuthError) -> (StatusCode, Json<AuthErrorResponse>) {
    match error {
        AuthError::AuthenticationFailed(msg) => (
            StatusCode::UNAUTHORIZED,
            Json(AuthErrorResponse {
                error: "authentication_failed".to_string(),
                message: msg,
                code: 401,
            }),
        ),
        AuthError::AuthorizationDenied(permission) => (
            StatusCode::FORBIDDEN,
            Json(AuthErrorResponse {
                error: "authorization_denied".to_string(),
                message: format!("Permission required: {:?}", permission),
                code: 403,
            }),
        ),
        AuthError::InvalidToken(msg) => (
            StatusCode::UNAUTHORIZED,
            Json(AuthErrorResponse {
                error: "invalid_token".to_string(),
                message: msg,
                code: 401,
            }),
        ),
        AuthError::TokenExpired => (
            StatusCode::UNAUTHORIZED,
            Json(AuthErrorResponse {
                error: "token_expired".to_string(),
                message: "Token has expired".to_string(),
                code: 401,
            }),
        ),
        AuthError::InvalidCredentials => (
            StatusCode::UNAUTHORIZED,
            Json(AuthErrorResponse {
                error: "invalid_credentials".to_string(),
                message: "Invalid credentials provided".to_string(),
                code: 401,
            }),
        ),
        AuthError::UserNotFound(user_id) => (
            StatusCode::NOT_FOUND,
            Json(AuthErrorResponse {
                error: "user_not_found".to_string(),
                message: format!("User not found: {}", user_id),
                code: 404,
            }),
        ),
        AuthError::RoleNotFound(role) => (
            StatusCode::NOT_FOUND,
            Json(AuthErrorResponse {
                error: "role_not_found".to_string(),
                message: format!("Role not found: {}", role),
                code: 404,
            }),
        ),
    }
}

/// Extension trait to extract auth result from request
pub trait RequestAuthExt {
    fn auth_result(&self) -> Option<&AuthResult>;
    fn user_id(&self) -> Option<&str>;
    fn tenant_id(&self) -> Option<&str>;
}

impl<B> RequestAuthExt for Request<B> {
    fn auth_result(&self) -> Option<&AuthResult> {
        self.extensions().get::<AuthResult>()
    }
    
    fn user_id(&self) -> Option<&str> {
        self.auth_result().map(|auth| auth.user_id.as_str())
    }
    
    fn tenant_id(&self) -> Option<&str> {
        self.auth_result().and_then(|auth| auth.tenant_id.as_deref())
    }
}

/// Optional authorization check for specific operations within handlers
pub async fn check_permission(
    auth_service: &AuthService,
    auth_result: &AuthResult,
    permission: Permission,
    resource_type: ResourceType,
    resource_id: Option<String>,
) -> Result<(), AuthError> {
    let context = PermissionContext {
        resource_type,
        resource_id,
        tenant_id: auth_result.tenant_id.clone(),
        metadata: std::collections::HashMap::new(),
    };
    
    auth_service
        .rbac
        .check_permission_with_context(&auth_result.user_id, permission, &context)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Method, Uri};
    
    #[test]
    fn test_should_skip_auth() {
        assert!(should_skip_auth("/health"));
        assert!(should_skip_auth("/health/ready"));
        assert!(should_skip_auth("/auth/login"));
        assert!(should_skip_auth("/docs"));
        assert!(!should_skip_auth("/collections"));
        assert!(!should_skip_auth("/api/collections"));
    }
    
    #[test]
    fn test_determine_required_permission() {
        // Collection endpoints
        assert_eq!(
            determine_required_permission("GET", "/collections"),
            Some(Permission::ListCollections)
        );
        assert_eq!(
            determine_required_permission("POST", "/collections"),
            Some(Permission::CreateCollection)
        );
        assert_eq!(
            determine_required_permission("DELETE", "/collections/test"),
            Some(Permission::DeleteCollection)
        );
        
        // Vector endpoints
        assert_eq!(
            determine_required_permission("POST", "/collections/test/vectors/search"),
            Some(Permission::SearchVectors)
        );
        assert_eq!(
            determine_required_permission("POST", "/collections/test/vectors"),
            Some(Permission::InsertVectors)
        );
        
        // System endpoints
        assert_eq!(
            determine_required_permission("GET", "/system/metrics"),
            Some(Permission::ViewSystemMetrics)
        );
        assert_eq!(
            determine_required_permission("POST", "/system/config"),
            Some(Permission::ConfigureSystem)
        );
    }
    
    #[test]
    fn test_extract_resource_id() {
        assert_eq!(
            extract_resource_id("/collections/test-collection"),
            Some("test-collection".to_string())
        );
        assert_eq!(
            extract_resource_id("/collections/test-collection/vectors"),
            Some("test-collection".to_string())
        );
        assert_eq!(extract_resource_id("/collections"), None);
        assert_eq!(
            extract_resource_id("/collections/test/vectors/search"),
            Some("test".to_string())
        );
    }
}