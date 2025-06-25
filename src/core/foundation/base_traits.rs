//! Base traits that provide common functionality for all schema types

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;

/// Base trait for configuration types with validation and defaults
pub trait BaseConfig: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync {
    /// Validate the configuration
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }
    
    /// Apply defaults to missing fields
    fn apply_defaults(&mut self) {}
    
    /// Get configuration as key-value pairs
    fn to_map(&self) -> HashMap<String, String> {
        HashMap::new()
    }
}

/// Base trait for metadata types with versioning and serialization
pub trait BaseMetadata: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync {
    /// Get the version of this metadata
    fn version(&self) -> u64 {
        1
    }
    
    /// Get the unique identifier
    fn id(&self) -> String;
    
    /// Get creation timestamp
    fn created_at(&self) -> chrono::DateTime<chrono::Utc>;
    
    /// Get last update timestamp
    fn updated_at(&self) -> chrono::DateTime<chrono::Utc>;
}

/// Base trait for statistics types with aggregation and comparison
pub trait BaseStats: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync {
    /// Aggregate with another stats instance
    fn aggregate(&mut self, other: &Self);
    
    /// Reset all statistics to zero
    fn reset(&mut self);
    
    /// Get the timestamp of these statistics
    fn timestamp(&self) -> chrono::DateTime<chrono::Utc>;
}

/// Base trait for result types with success/error handling
pub trait BaseResult<T>: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync {
    /// Check if the result represents success
    fn is_success(&self) -> bool;
    
    /// Get the success value if available
    fn data(&self) -> Option<&T>;
    
    /// Get the error message if failed
    fn error(&self) -> Option<&str>;
    
    /// Get processing time in microseconds
    fn processing_time_us(&self) -> Option<u64> {
        None
    }
}

/// Base trait for service definitions with lifecycle management
#[async_trait]
pub trait BaseService: Send + Sync {
    /// Service name for identification
    fn name(&self) -> &'static str;
    
    /// Start the service
    async fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    
    /// Stop the service gracefully
    async fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    
    /// Check if the service is healthy
    async fn health_check(&self) -> bool;
    
    /// Get service metrics
    async fn get_metrics(&self) -> HashMap<String, serde_json::Value> {
        HashMap::new()
    }
}