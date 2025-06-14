use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerlessConfig {
    pub deployment_mode: DeploymentMode,
    pub cloud_provider: CloudProvider,
    pub auto_scaling: AutoScalingConfig,
    pub multi_tenant: MultiTenantConfig,
    pub observability: ObservabilityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeploymentMode {
    /// Pure serverless (Lambda/Functions) - best for sporadic workloads
    Functions {
        cold_start_optimization: bool,
        memory_size_mb: u32,
        timeout_seconds: u32,
    },
    /// Container serverless (Fargate/Cloud Run) - best for consistent workloads
    Containers {
        min_instances: u32,
        max_instances: u32,
        target_cpu_percent: u32,
        target_memory_percent: u32,
    },
    /// Kubernetes serverless (EKS/AKS/GKE) - best for high-throughput
    Kubernetes {
        namespace: String,
        hpa_enabled: bool,
        vpa_enabled: bool,
        keda_enabled: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CloudProvider {
    AWS {
        region: String,
        s3_bucket: String,
        dynamodb_table: Option<String>,
        rds_endpoint: Option<String>,
        elasticache_endpoint: Option<String>,
    },
    Azure {
        region: String,
        storage_account: String,
        cosmos_db_endpoint: Option<String>,
        sql_endpoint: Option<String>,
        redis_endpoint: Option<String>,
    },
    GCP {
        project_id: String,
        region: String,
        storage_bucket: String,
        firestore_database: Option<String>,
        cloud_sql_instance: Option<String>,
        memorystore_instance: Option<String>,
    },
    MultiCloud {
        primary: Box<CloudProvider>,
        secondary: Box<CloudProvider>,
        failover_enabled: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoScalingConfig {
    pub enabled: bool,
    pub scale_to_zero: bool,
    pub scale_up_threshold: f32,
    pub scale_down_threshold: f32,
    pub scale_up_delay_seconds: u32,
    pub scale_down_delay_seconds: u32,
    pub metrics: Vec<ScalingMetric>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScalingMetric {
    RequestsPerSecond { target: f32 },
    CpuUtilization { target_percent: f32 },
    MemoryUtilization { target_percent: f32 },
    QueueDepth { target_messages: u32 },
    ResponseTime { target_ms: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiTenantConfig {
    pub enabled: bool,
    pub isolation_level: IsolationLevel,
    pub namespace_header: String,
    pub default_namespace: String,
    pub resource_limits: HashMap<String, ResourceLimit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IsolationLevel {
    /// Shared infrastructure, logical separation
    Logical,
    /// Separate containers per tenant
    Container,
    /// Separate clusters per tenant  
    Cluster,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimit {
    pub max_vectors_per_collection: Option<u64>,
    pub max_collections: Option<u32>,
    pub max_requests_per_second: Option<f32>,
    pub max_storage_gb: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    pub metrics_enabled: bool,
    pub tracing_enabled: bool,
    pub logging_level: String,
    pub metrics_endpoint: String,
    pub traces_endpoint: Option<String>,
    pub health_check_path: String,
    pub readiness_check_path: String,
}

impl Default for ServerlessConfig {
    fn default() -> Self {
        Self {
            deployment_mode: DeploymentMode::Containers {
                min_instances: 1,
                max_instances: 10,
                target_cpu_percent: 70,
                target_memory_percent: 80,
            },
            cloud_provider: CloudProvider::AWS {
                region: "us-east-1".to_string(),
                s3_bucket: "vectordb-storage".to_string(),
                dynamodb_table: None,
                rds_endpoint: None,
                elasticache_endpoint: None,
            },
            auto_scaling: AutoScalingConfig {
                enabled: true,
                scale_to_zero: false,
                scale_up_threshold: 0.7,
                scale_down_threshold: 0.3,
                scale_up_delay_seconds: 30,
                scale_down_delay_seconds: 300,
                metrics: vec![
                    ScalingMetric::CpuUtilization { target_percent: 70.0 },
                    ScalingMetric::RequestsPerSecond { target: 100.0 },
                ],
            },
            multi_tenant: MultiTenantConfig {
                enabled: true,
                isolation_level: IsolationLevel::Logical,
                namespace_header: "x-tenant-id".to_string(),
                default_namespace: "default".to_string(),
                resource_limits: HashMap::new(),
            },
            observability: ObservabilityConfig {
                metrics_enabled: true,
                tracing_enabled: true,
                logging_level: "info".to_string(),
                metrics_endpoint: "/metrics".to_string(),
                traces_endpoint: None,
                health_check_path: "/health".to_string(),
                readiness_check_path: "/ready".to_string(),
            },
        }
    }
}

/// Trait for cloud storage backends
#[async_trait::async_trait]
pub trait CloudStorage: Send + Sync {
    async fn get(&self, key: &str) -> crate::Result<Option<Vec<u8>>>;
    async fn put(&self, key: &str, data: &[u8]) -> crate::Result<()>;
    async fn delete(&self, key: &str) -> crate::Result<()>;
    async fn list(&self, prefix: &str) -> crate::Result<Vec<String>>;
}

/// Trait for cloud caching backends  
#[async_trait::async_trait]
pub trait CloudCache: Send + Sync {
    async fn get(&self, key: &str) -> crate::Result<Option<Vec<u8>>>;
    async fn set(&self, key: &str, data: &[u8], ttl_seconds: Option<u64>) -> crate::Result<()>;
    async fn delete(&self, key: &str) -> crate::Result<()>;
    async fn clear(&self) -> crate::Result<()>;
}

/// Trait for cloud message queues
#[async_trait::async_trait]
pub trait CloudQueue: Send + Sync {
    async fn send(&self, queue_name: &str, message: &[u8]) -> crate::Result<String>;
    async fn receive(&self, queue_name: &str) -> crate::Result<Option<QueueMessage>>;
    async fn delete(&self, queue_name: &str, receipt_handle: &str) -> crate::Result<()>;
}

#[derive(Debug, Clone)]
pub struct QueueMessage {
    pub id: String,
    pub body: Vec<u8>,
    pub receipt_handle: String,
    pub attributes: HashMap<String, String>,
}