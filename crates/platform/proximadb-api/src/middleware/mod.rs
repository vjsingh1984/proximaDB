//! # API Middleware
//!
//! Cross-protocol middleware for authentication, rate limiting, CORS, etc.

pub mod auth;
pub mod cors;
pub mod rate_limit;
pub mod request_id;

// Re-exports
pub use auth::AuthMiddleware;
pub use cors::CorsMiddleware;
pub use rate_limit::RateLimitMiddleware;
pub use request_id::RequestIdMiddleware;
