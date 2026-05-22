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
pub trait BaseMetadata:
    Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync
{
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
pub trait BaseResult<T>:
    Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync
{
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct DemoConfig {
        name: String,
    }

    impl BaseConfig for DemoConfig {}

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct DemoMetadata {
        id: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    impl BaseMetadata for DemoMetadata {
        fn id(&self) -> String {
            self.id.clone()
        }

        fn created_at(&self) -> DateTime<Utc> {
            self.created_at
        }

        fn updated_at(&self) -> DateTime<Utc> {
            self.updated_at
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct DemoStats {
        count: u64,
        timestamp: DateTime<Utc>,
    }

    impl BaseStats for DemoStats {
        fn aggregate(&mut self, other: &Self) {
            self.count += other.count;
        }

        fn reset(&mut self) {
            self.count = 0;
        }

        fn timestamp(&self) -> DateTime<Utc> {
            self.timestamp
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct DemoResult {
        success: bool,
        data: Option<u32>,
        error: Option<String>,
    }

    impl BaseResult<u32> for DemoResult {
        fn is_success(&self) -> bool {
            self.success
        }

        fn data(&self) -> Option<&u32> {
            self.data.as_ref()
        }

        fn error(&self) -> Option<&str> {
            self.error.as_deref()
        }
    }

    #[test]
    fn base_config_metadata_stats_and_result_defaults_are_usable() {
        let mut config = DemoConfig {
            name: "demo".to_string(),
        };
        config.apply_defaults();
        assert!(config.validate().is_ok());
        assert!(config.to_map().is_empty());

        let at = Utc.timestamp_opt(1_700_000_000, 0).single().unwrap();
        let metadata = DemoMetadata {
            id: "m1".to_string(),
            created_at: at,
            updated_at: at,
        };
        assert_eq!(metadata.version(), 1);
        assert_eq!(metadata.id(), "m1");
        assert_eq!(metadata.created_at(), at);
        assert_eq!(metadata.updated_at(), at);

        let mut stats = DemoStats {
            count: 2,
            timestamp: at,
        };
        stats.aggregate(&DemoStats {
            count: 3,
            timestamp: at,
        });
        assert_eq!(stats.count, 5);
        stats.reset();
        assert_eq!(stats.count, 0);
        assert_eq!(stats.timestamp(), at);

        let success = DemoResult {
            success: true,
            data: Some(7),
            error: None,
        };
        assert!(success.is_success());
        assert_eq!(success.data(), Some(&7));
        assert_eq!(success.error(), None);
        assert_eq!(success.processing_time_us(), None);
    }
}
