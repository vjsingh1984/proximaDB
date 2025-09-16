#[cfg(test)]
mod base_traits_tests {
    use super::base_traits::*;
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    // Test implementation of BaseConfig
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestConfig {
        pub name: String,
        pub value: i32,
    }

    impl BaseConfig for TestConfig {
        fn validate(&self) -> Result<(), String> {
            if self.name.is_empty() {
                return Err("Name cannot be empty".to_string());
            }
            if self.value < 0 {
                return Err("Value must be non-negative".to_string());
            }
            Ok(())
        }

        fn apply_defaults(&mut self) {
            if self.name.is_empty() {
                self.name = "default".to_string();
            }
            if self.value < 0 {
                self.value = 0;
            }
        }

        fn to_map(&self) -> HashMap<String, String> {
            let mut map = HashMap::new();
            map.insert("name".to_string(), self.name.clone());
            map.insert("value".to_string(), self.value.to_string());
            map
        }
    }

    // Test implementation of BaseMetadata
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestMetadata {
        pub id: String,
        pub version: u64,
        pub timestamp: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
    }

    impl BaseMetadata for TestMetadata {
        fn version(&self) -> u64 {
            self.version
        }

        fn id(&self) -> String {
            self.id.clone()
        }

        fn created_at(&self) -> DateTime<Utc> {
            self.timestamp
        }

        fn updated_at(&self) -> DateTime<Utc> {
            self.updated_at
        }
    }

    // Test implementation of BaseStats
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestStats {
        pub count: u64,
        pub sum: f64,
        pub timestamp: DateTime<Utc>,
    }

    impl BaseStats for TestStats {
        fn aggregate(&mut self, other: &Self) {
            self.count += other.count;
            self.sum += other.sum;
            self.timestamp = Utc::now();
        }

        fn reset(&mut self) {
            self.count = 0;
            self.sum = 0.0;
            self.timestamp = Utc::now();
        }

        fn timestamp(&self) -> DateTime<Utc> {
            self.timestamp
        }
    }

    // Test implementation of BaseResult
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestResult {
        pub success: bool,
        pub data: Option<String>,
        pub error: Option<String>,
        pub processing_time_us: Option<u64>,
    }

    impl BaseResult<String> for TestResult {
        fn is_success(&self) -> bool {
            self.success
        }

        fn data(&self) -> Option<&String> {
            self.data.as_ref()
        }

        fn error(&self) -> Option<&str> {
            self.error.as_deref()
        }

        fn processing_time_us(&self) -> Option<u64> {
            self.processing_time_us
        }
    }

    // Test implementation of BaseService
    struct TestService {
        pub name: &'static str,
        pub running: bool,
        pub healthy: bool,
    }

    #[async_trait]
    impl BaseService for TestService {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            if self.running {
                return Err("Service already running".into());
            }
            self.running = true;
            self.healthy = true;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            if !self.running {
                return Err("Service not running".into());
            }
            self.running = false;
            self.healthy = false;
            Ok(())
        }

        async fn health_check(&self) -> bool {
            self.healthy && self.running
        }

        async fn get_metrics(&self) -> HashMap<String, serde_json::Value> {
            let mut metrics = HashMap::new();
            metrics.insert("running".to_string(), serde_json::Value::Bool(self.running));
            metrics.insert("healthy".to_string(), serde_json::Value::Bool(self.healthy));
            metrics
        }
    }

    #[test]
    fn test_base_config_default_validation() {
        let config = TestConfig {
            name: "test".to_string(),
            value: 42,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_base_config_validation_failure() {
        let config = TestConfig {
            name: "".to_string(),
            value: -1,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Name cannot be empty"));
    }

    #[test]
    fn test_base_config_apply_defaults() {
        let mut config = TestConfig {
            name: "".to_string(),
            value: -5,
        };
        config.apply_defaults();
        assert_eq!(config.name, "default");
        assert_eq!(config.value, 0);
    }

    #[test]
    fn test_base_config_to_map() {
        let config = TestConfig {
            name: "test_config".to_string(),
            value: 123,
        };
        let map = config.to_map();
        assert_eq!(map.get("name").unwrap(), "test_config");
        assert_eq!(map.get("value").unwrap(), "123");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_base_metadata_version() {
        let now = Utc::now();
        let metadata = TestMetadata {
            id: "test_id".to_string(),
            version: 5,
            timestamp: now,
            updated_at: now,
        };
        assert_eq!(metadata.version(), 5);
        assert_eq!(metadata.id(), "test_id");
        assert_eq!(metadata.created_at(), now);
        assert_eq!(metadata.updated_at(), now);
    }

    #[test]
    fn test_base_stats_aggregation() {
        let timestamp = Utc::now();
        let mut stats1 = TestStats {
            count: 10,
            sum: 100.0,
            timestamp,
        };
        let stats2 = TestStats {
            count: 5,
            sum: 50.0,
            timestamp,
        };

        stats1.aggregate(&stats2);
        assert_eq!(stats1.count, 15);
        assert_eq!(stats1.sum, 150.0);
        // Timestamp should be updated
        assert!(stats1.timestamp >= timestamp);
    }

    #[test]
    fn test_base_stats_reset() {
        let mut stats = TestStats {
            count: 100,
            sum: 500.0,
            timestamp: Utc::now(),
        };
        let old_timestamp = stats.timestamp;

        // Small delay to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(1));
        
        stats.reset();
        assert_eq!(stats.count, 0);
        assert_eq!(stats.sum, 0.0);
        assert!(stats.timestamp > old_timestamp);
    }

    #[test]
    fn test_base_result_success() {
        let result = TestResult {
            success: true,
            data: Some("success_data".to_string()),
            error: None,
            processing_time_us: Some(1000),
        };

        assert!(result.is_success());
        assert_eq!(result.data().unwrap(), "success_data");
        assert!(result.error().is_none());
        assert_eq!(result.processing_time_us().unwrap(), 1000);
    }

    #[test]
    fn test_base_result_error() {
        let result = TestResult {
            success: false,
            data: None,
            error: Some("test error".to_string()),
            processing_time_us: Some(500),
        };

        assert!(!result.is_success());
        assert!(result.data().is_none());
        assert_eq!(result.error().unwrap(), "test error");
        assert_eq!(result.processing_time_us().unwrap(), 500);
    }

    #[tokio::test]
    async fn test_base_service_lifecycle() {
        let mut service = TestService {
            name: "test_service",
            running: false,
            healthy: false,
        };

        // Test initial state
        assert_eq!(service.name(), "test_service");
        assert!(!service.health_check().await);

        // Test start
        assert!(service.start().await.is_ok());
        assert!(service.running);
        assert!(service.health_check().await);

        // Test double start (should fail)
        assert!(service.start().await.is_err());

        // Test stop
        assert!(service.stop().await.is_ok());
        assert!(!service.running);
        assert!(!service.health_check().await);

        // Test double stop (should fail)
        assert!(service.stop().await.is_err());
    }

    #[tokio::test]
    async fn test_base_service_metrics() {
        let mut service = TestService {
            name: "metrics_test",
            running: false,
            healthy: false,
        };

        let metrics = service.get_metrics().await;
        assert_eq!(metrics.get("running").unwrap(), &serde_json::Value::Bool(false));
        assert_eq!(metrics.get("healthy").unwrap(), &serde_json::Value::Bool(false));

        service.start().await.unwrap();
        let metrics = service.get_metrics().await;
        assert_eq!(metrics.get("running").unwrap(), &serde_json::Value::Bool(true));
        assert_eq!(metrics.get("healthy").unwrap(), &serde_json::Value::Bool(true));
    }
}

#[cfg(test)]
mod conversion_tests {
    use super::conversion::*;

    // Test types for conversion
    #[derive(Debug, PartialEq, Clone)]
    struct SourceType {
        value: i32,
    }

    #[derive(Debug, PartialEq, Clone)]
    struct TargetType {
        value: i32,
        extra: String,
    }

    impl ToUnified<TargetType> for SourceType {
        fn to_unified(self) -> TargetType {
            TargetType {
                value: self.value,
                extra: "converted".to_string(),
            }
        }
    }

    impl FromUnified<SourceType> for TargetType {
        fn from_unified(unified: SourceType) -> Self {
            TargetType {
                value: unified.value,
                extra: "from_unified".to_string(),
            }
        }
    }

    // Error-prone conversion for testing try_* methods
    struct ErrorProneType {
        value: i32,
    }

    impl ToUnified<TargetType> for ErrorProneType {
        fn to_unified(self) -> TargetType {
            TargetType {
                value: self.value,
                extra: "error_prone".to_string(),
            }
        }

        fn try_to_unified(self) -> Result<TargetType, String> {
            if self.value < 0 {
                Err("Negative values not allowed".to_string())
            } else {
                Ok(self.to_unified())
            }
        }
    }

    #[test]
    fn test_to_unified_conversion() {
        let source = SourceType { value: 42 };
        let target = source.to_unified();
        
        assert_eq!(target.value, 42);
        assert_eq!(target.extra, "converted");
    }

    #[test]
    fn test_try_to_unified_success() {
        let source = SourceType { value: 100 };
        let result = source.try_to_unified();
        
        assert!(result.is_ok());
        let target = result.unwrap();
        assert_eq!(target.value, 100);
        assert_eq!(target.extra, "converted");
    }

    #[test]
    fn test_try_to_unified_error() {
        let error_prone = ErrorProneType { value: -1 };
        let result = error_prone.try_to_unified();
        
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Negative values not allowed");
    }

    #[test]
    fn test_try_to_unified_success_with_error_prone() {
        let error_prone = ErrorProneType { value: 5 };
        let result = error_prone.try_to_unified();
        
        assert!(result.is_ok());
        let target = result.unwrap();
        assert_eq!(target.value, 5);
        assert_eq!(target.extra, "error_prone");
    }

    #[test]
    fn test_from_unified_conversion() {
        let source = SourceType { value: 24 };
        let target = TargetType::from_unified(source);
        
        assert_eq!(target.value, 24);
        assert_eq!(target.extra, "from_unified");
    }

    #[test]
    fn test_try_from_unified_success() {
        let source = SourceType { value: 77 };
        let result = TargetType::try_from_unified(source);
        
        assert!(result.is_ok());
        let target = result.unwrap();
        assert_eq!(target.value, 77);
        assert_eq!(target.extra, "from_unified");
    }

    #[test]
    fn test_convert_vec() {
        let sources = vec![
            SourceType { value: 1 },
            SourceType { value: 2 },
            SourceType { value: 3 },
        ];
        
        let targets: Vec<TargetType> = convert_vec(sources);
        
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].value, 1);
        assert_eq!(targets[1].value, 2);
        assert_eq!(targets[2].value, 3);
        for target in &targets {
            assert_eq!(target.extra, "converted");
        }
    }

    #[test]
    fn test_convert_vec_empty() {
        let sources: Vec<SourceType> = vec![];
        let targets: Vec<TargetType> = convert_vec(sources);
        
        assert_eq!(targets.len(), 0);
    }

    #[test]
    fn test_convert_option_some() {
        let source = Some(SourceType { value: 99 });
        let target: Option<TargetType> = convert_option(source);
        
        assert!(target.is_some());
        let target = target.unwrap();
        assert_eq!(target.value, 99);
        assert_eq!(target.extra, "converted");
    }

    #[test]
    fn test_convert_option_none() {
        let source: Option<SourceType> = None;
        let target: Option<TargetType> = convert_option(source);
        
        assert!(target.is_none());
    }

    #[test]
    fn test_convert_result_ok() {
        let source: Result<SourceType, String> = Ok(SourceType { value: 88 });
        let target: Result<TargetType, String> = convert_result(source);
        
        assert!(target.is_ok());
        let target = target.unwrap();
        assert_eq!(target.value, 88);
        assert_eq!(target.extra, "converted");
    }

    #[test]
    fn test_convert_result_err() {
        let source: Result<SourceType, String> = Err("test error".to_string());
        let target: Result<TargetType, String> = convert_result(source);
        
        assert!(target.is_err());
        assert_eq!(target.unwrap_err(), "test error");
    }

    #[test]
    fn test_conversion_type_aliases() {
        let result: ConversionResult<i32> = Ok(42);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);

        let error: ConversionError = "test error".to_string();
        assert_eq!(error, "test error");
    }
}

#[cfg(test)]
mod generic_types_tests {
    use super::generic_types::*;
    use super::base_traits::*;
    use chrono::Utc;
    use std::collections::HashMap;

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
    struct TestData {
        name: String,
        value: i32,
    }

    #[test]
    fn test_generic_config_creation() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };
        let config = GenericConfig::new(data.clone());
        
        assert_eq!(config.data, data);
        assert!(config.validation_rules.is_empty());
    }

    #[test]
    fn test_generic_config_with_validation() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };
        
        let mut rules = HashMap::new();
        rules.insert("max_value".to_string(), "100".to_string());
        
        let config = GenericConfig::new(data).with_validation(rules.clone());
        assert_eq!(config.validation_rules, rules);
    }

    #[test]
    fn test_generic_config_base_trait_validation_success() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };
        let config = GenericConfig::new(data);
        
        // Should pass validation with no rules
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_generic_config_base_trait_validation_failure() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };
        
        let mut rules = HashMap::new();
        rules.insert("required".to_string(), "".to_string()); // Empty message triggers failure
        
        let config = GenericConfig::new(data).with_validation(rules);
        let result = config.validate();
        
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Validation failed"));
    }

    #[test]
    fn test_generic_metadata_creation() {
        let data = TestData {
            name: "metadata_test".to_string(),
            value: 123,
        };
        let metadata = GenericMetadata::new("test_id".to_string(), data.clone());
        
        assert_eq!(metadata.id, "test_id");
        assert_eq!(metadata.data, data);
        assert_eq!(metadata.version, 1);
        assert!(metadata.tags.is_empty());
        assert!(metadata.properties.is_empty());
        
        // Timestamps should be recent
        let now = Utc::now();
        assert!((now - metadata.created_at()).num_seconds() < 1);
        assert!((now - metadata.updated_at).num_seconds() < 1);
    }

    #[test]
    fn test_generic_metadata_with_tags() {
        let data = TestData {
            name: "tagged".to_string(),
            value: 456,
        };
        let tags = vec!["tag1".to_string(), "tag2".to_string()];
        let metadata = GenericMetadata::new("tagged_id".to_string(), data)
            .with_tags(tags.clone());
        
        assert_eq!(metadata.tags, tags);
    }

    #[test]
    fn test_generic_metadata_with_properties() {
        let data = TestData {
            name: "props".to_string(),
            value: 789,
        };
        
        let mut properties = HashMap::new();
        properties.insert("priority".to_string(), serde_json::Value::String("high".to_string()));
        properties.insert("count".to_string(), serde_json::Value::Number(serde_json::Number::from(10)));
        
        let metadata = GenericMetadata::new("props_id".to_string(), data)
            .with_properties(properties.clone());
        
        assert_eq!(metadata.properties, properties);
    }

    #[test]
    fn test_generic_metadata_base_trait() {
        let data = TestData {
            name: "trait_test".to_string(),
            value: 999,
        };
        let metadata = GenericMetadata::new("trait_id".to_string(), data);
        
        // Test BaseMetadata trait methods
        assert_eq!(metadata.version(), 1);
        assert_eq!(metadata.id(), "trait_id");
        
        let created = metadata.created_at();
        let updated = metadata.updated_at();
        assert_eq!(created, updated); // Should be same on creation
    }

    #[test]
    fn test_generic_stats_creation() {
        let data = TestData {
            name: "stats".to_string(),
            value: 111,
        };
        let stats = GenericStats::new(data.clone());
        
        assert_eq!(stats.data, data);
        assert_eq!(stats.collection_count, 1);
        assert_eq!(stats.reset_count, 0);
        
        let now = Utc::now();
        assert!((now - stats.timestamp).num_seconds() < 1);
    }

    #[test]
    fn test_generic_stats_update_data() {
        let initial_data = TestData {
            name: "initial".to_string(),
            value: 1,
        };
        let mut stats = GenericStats::new(initial_data);
        let initial_timestamp = stats.timestamp;
        
        // Small delay to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(1));
        
        let new_data = TestData {
            name: "updated".to_string(),
            value: 2,
        };
        stats.update_data(new_data.clone());
        
        assert_eq!(stats.data, new_data);
        assert!(stats.timestamp > initial_timestamp);
    }

    #[test]
    fn test_generic_stats_base_trait_aggregate() {
        let data1 = TestData {
            name: "stats1".to_string(),
            value: 1,
        };
        let data2 = TestData {
            name: "stats2".to_string(),
            value: 2,
        };
        
        let mut stats1 = GenericStats::new(data1);
        stats1.collection_count = 5;
        let initial_timestamp = stats1.timestamp;
        
        let mut stats2 = GenericStats::new(data2);
        stats2.collection_count = 3;
        
        // Small delay to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(1));
        
        stats1.aggregate(&stats2);
        
        assert_eq!(stats1.collection_count, 8); // 5 + 3
        assert!(stats1.timestamp > initial_timestamp);
    }

    #[test]
    fn test_generic_stats_base_trait_reset() {
        let data = TestData {
            name: "reset_test".to_string(),
            value: 42,
        };
        let mut stats = GenericStats::new(data);
        stats.collection_count = 100;
        let initial_timestamp = stats.timestamp;
        
        // Small delay to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(1));
        
        stats.reset();
        
        assert_eq!(stats.collection_count, 0);
        assert_eq!(stats.reset_count, 1);
        assert!(stats.timestamp > initial_timestamp);
    }

    #[test]
    fn test_generic_result_success() {
        let data = TestData {
            name: "success".to_string(),
            value: 200,
        };
        let result = GenericResult::success(data.clone());
        
        assert!(result.success);
        assert_eq!(result.data, Some(data));
        assert!(result.error_code.is_none());
        assert!(result.error_code.is_none());
        assert!(result.processing_time_us.is_none());
        assert!(result.metadata.is_empty());
    }

    #[test]
    fn test_generic_result_error() {
        let mut result: GenericResult<TestData> = GenericResult::error();
        result.error_code = Some("Test error".to_string());
        
        assert!(!result.success);
        assert!(result.data.is_none());
        assert_eq!(result.error_code, Some("Test error".to_string()));
        assert!(result.processing_time_us.is_none());
        assert!(result.metadata.is_empty());
    }

    #[test]
    fn test_generic_result_with_processing_time() {
        let data = TestData {
            name: "timed".to_string(),
            value: 123,
        };
        let result = GenericResult::success(data).with_processing_time(5000);
        
        assert_eq!(result.processing_time_us, Some(5000));
    }

    #[test]
    fn test_generic_result_with_error_code() {
        let mut result: GenericResult<TestData> = GenericResult::error();
        result.error_code = Some("ERR_001".to_string());
        
        assert_eq!(result.error_code, Some("ERR_001".to_string()));
    }

    #[test]
    fn test_generic_result_base_trait_success() {
        let data = TestData {
            name: "trait_success".to_string(),
            value: 456,
        };
        let result = GenericResult::success(data.clone()).with_processing_time(1000);
        
        // Test BaseResult trait methods
        assert!(result.is_success());
        assert_eq!(result.data().unwrap(), &data);
        assert!(result.error().is_none());
        assert_eq!(result.processing_time_us().unwrap(), 1000);
    }

    #[test]
    fn test_generic_result_base_trait_error() {
        let mut result: GenericResult<TestData> = GenericResult::error()
            .with_processing_time(500);
        result.error_code = Some("Trait error".to_string());
        
        // Test BaseResult trait methods
        assert!(!result.is_success());
        assert!(result.data().is_none());
        assert_eq!(result.error().unwrap(), "Trait error");
        assert_eq!(result.processing_time_us().unwrap(), 500);
    }

    #[test]
    fn test_generic_result_chaining() {
        let data = TestData {
            name: "chained".to_string(),
            value: 999,
        };
        let result = GenericResult::success(data)
            .with_processing_time(2000)
            .with_error_code("SUCCESS".to_string()); // Can have error code even on success
        
        assert!(result.is_success());
        assert_eq!(result.processing_time_us().unwrap(), 2000);
        assert_eq!(result.error_code, Some("SUCCESS".to_string()));
    }

    #[test]
    fn test_generic_result_error_chaining() {
        let mut result: GenericResult<TestData> = GenericResult::error()
            .with_processing_time(100);
        result.error_code = Some("ERR_CHAIN".to_string());
        
        assert!(!result.is_success());
        assert_eq!(result.error().unwrap(), "ERR_CHAIN");
        assert_eq!(result.error_code.as_ref().unwrap(), "ERR_CHAIN");
        assert_eq!(result.processing_time_us().unwrap(), 100);
    }
}
