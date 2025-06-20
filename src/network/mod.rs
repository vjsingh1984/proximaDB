// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Network layer implementation for ProximaDB

pub mod grpc;
pub mod metrics_service;
pub mod middleware;
pub mod multi_server;
pub mod server_builder;

use serde::{Deserialize, Serialize};

pub use metrics_service::*;
pub use multi_server::{
    GrpcHttpServerConfig, MultiServer, MultiServerConfig, RestHttpServerConfig,
};
pub use server_builder::{GrpcHttpServerBuilder, MultiServerBuilder, RestHttpServerBuilder};
pub use middleware::*;

/// Network server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Server bind address
    pub bind_address: String,

    /// Server port
    pub port: u16,

    /// Enable gRPC server
    pub enable_grpc: bool,

    /// Enable REST server
    pub enable_rest: bool,

    /// Enable web dashboard
    pub enable_dashboard: bool,

    /// Authentication configuration
    pub auth: AuthConfig,

    /// Rate limiting configuration
    pub rate_limit: RateLimitConfig,

    /// Request timeout in seconds
    pub request_timeout_secs: u64,

    /// Maximum request size in bytes
    pub max_request_size: usize,

    /// Keep-alive timeout in seconds
    pub keep_alive_timeout_secs: u64,

    /// TCP nodelay setting
    pub tcp_nodelay: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0".to_string(),
            port: 5678,
            enable_grpc: true,
            enable_rest: true,
            enable_dashboard: true,
            auth: AuthConfig::default(),
            rate_limit: RateLimitConfig::default(),
            request_timeout_secs: 30,
            max_request_size: 32 * 1024 * 1024, // 32MB
            keep_alive_timeout_secs: 60,
            tcp_nodelay: true,
        }
    }
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Enable authentication
    pub enabled: bool,

    /// JWT secret key
    pub jwt_secret: Option<String>,

    /// JWT expiration time in seconds
    pub jwt_expiration_secs: u64,

    /// API key validation
    pub api_keys: Vec<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            jwt_secret: None,
            jwt_expiration_secs: 3600, // 1 hour
            api_keys: Vec::new(),
        }
    }
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting
    pub enabled: bool,

    /// Requests per minute per client
    pub requests_per_minute: u32,

    /// Burst allowance
    pub burst_size: u32,

    /// Rate limit by IP address
    pub by_ip: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_minute: 1000,
            burst_size: 100,
            by_ip: true,
        }
    }
}

