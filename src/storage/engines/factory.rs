// Storage Engine Factory
// Creates the appropriate storage engine based on configuration

use anyhow::{anyhow, Result};
use std::sync::Arc;
use tracing::{info, warn};

use crate::proto::proximadb::StorageEngine as ProtoStorageEngine;
use crate::storage::traits::{
    UnifiedStorageEngine, StorageEngineStrategy,
};
use crate::metrics::collectors::EngineMetricsCollector;

use super::{
    sst::SstStorage,
    viper::ViperEngine,
    swift::SwiftEngine,
    nova::NovaEngine,
    prism::PrismEngine,
    raptor::RaptorEngine,
};

/// Storage engine factory for creating engine instances
pub struct StorageEngineFactory;

impl StorageEngineFactory {
    /// Create a storage engine from proto enum
    pub fn create_from_proto(
        engine_type: ProtoStorageEngine,
    ) -> Result<Arc<dyn UnifiedStorageEngine>> {
        match engine_type {
            ProtoStorageEngine::StorageEngineUnspecified => {
                warn!("Unspecified storage engine, defaulting to SST (VIPER not available)");
                Self::create_sst()
            }
            ProtoStorageEngine::Viper => Self::create_viper(),
            ProtoStorageEngine::Sst => Self::create_sst(),
            ProtoStorageEngine::Mmap => {
                warn!("MMAP engine not yet implemented, using SST");
                Self::create_sst()
            }
            ProtoStorageEngine::Hybrid => {
                warn!("Hybrid engine not yet implemented, using SST");
                Self::create_sst()
            }
            ProtoStorageEngine::Swift => Self::create_swift(),
            ProtoStorageEngine::Nova => Self::create_nova(),
            // Add RAPTOR for cloud-optimized workloads
            _ => {
                warn!("Unknown storage engine type, defaulting to SST");
                Self::create_sst()
            }
        }
    }
    
    /// Create a storage engine from strategy enum
    pub fn create_from_strategy(
        strategy: StorageEngineStrategy,
    ) -> Result<Arc<dyn UnifiedStorageEngine>> {
        match strategy {
            StorageEngineStrategy::Viper => Self::create_viper(),
            StorageEngineStrategy::Lsm => Self::create_sst(),
            StorageEngineStrategy::Prism => Self::create_prism(),
            StorageEngineStrategy::Hybrid => {
                // RAPTOR uses hybrid strategy (row-aligned with columnar benefits)
                info!("Creating RAPTOR engine for hybrid strategy");
                Self::create_raptor_default()
            }
            StorageEngineStrategy::Swift => {
                info!("Creating SWIFT engine");
                Self::create_swift()
            }
            StorageEngineStrategy::Nova => {
                info!("Creating NOVA engine");
                Self::create_nova()
            }
            StorageEngineStrategy::Raptor => {
                info!("Creating RAPTOR engine");
                Self::create_raptor_default()
            }
        }
    }
    
    /// Create VIPER engine
    fn create_viper() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating VIPER storage engine");
        // VIPER needs async initialization, block on it for now
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(async {
            let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
            let filesystem = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config).await?);
            let viper_config = crate::core::config::ViperConfig::default();
            let distance_compute = Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default());
            ViperEngine::new(
                "default".to_string(),  // Default collection ID
                viper_config,
                filesystem,
                distance_compute,
            ).await
        })?;
        Ok(Arc::new(engine))
    }
    
    /// Create SST engine
    fn create_sst() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating SST storage engine");
        // SST needs async initialization, block on it for now
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(async {
            let sst_config = crate::core::config::SstConfig::default();
            let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
            let filesystem = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config).await?);
            let distance_compute = Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default());
            SstStorage::new(sst_config, filesystem, distance_compute).await
        })?;
        Ok(Arc::new(engine))
    }
    
    /// Create SWIFT engine (Storage With Instant Fast Traversal)
    fn create_swift() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating SWIFT (Storage With Instant Fast Traversal) storage engine");
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(SwiftEngine::new())?;
        Ok(Arc::new(engine))
    }
    
    /// Create NOVA engine (Next-gen Optimized Vector Analytics)
    fn create_nova() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating NOVA (Next-gen Optimized Vector Analytics) storage engine");
        let runtime = tokio::runtime::Runtime::new()?;
        let engine = runtime.block_on(NovaEngine::new())?;
        Ok(Arc::new(engine))
    }
    
    /// Create RAPTOR engine (Row-Aligned Predicated Tensor Optimized Repository)
    fn create_raptor_default() -> Result<Arc<dyn UnifiedStorageEngine>> {
        warn!("RAPTOR engine requires async initialization with collection info");
        // For now, return SST as fallback
        Self::create_sst()
    }
    
    /// Create RAPTOR engine with specific configuration (async)
    pub async fn create_raptor(
        collection_id: String,
        base_path: String,
        config: Option<super::raptor::RaptorConfig>,
    ) -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating RAPTOR (Row-Aligned Predicated Tensor Optimized Repository) storage engine");
        
        let config = config.unwrap_or_else(super::raptor::RaptorConfig::default);
        // Create shared cache for RAPTOR
        use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
        let cache = Arc::new(CrossCacheOrchestrator::new());
        let engine = RaptorEngine::new(collection_id, base_path, config, cache).await?;
        Ok(Arc::new(engine))
    }
    
    /// Create PRISM engine (Progressive Retrieval through Indexed Storage Management)
    fn create_prism() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating PRISM (Progressive Retrieval through Indexed Storage Management) storage engine");
        
        // Use default configuration for now
        let config = super::prism::config::Config::default();
        
        // TODO: This needs to be updated when the PRISM engine constructor is fixed
        // For now, return an error indicating PRISM needs additional setup
        Err(anyhow!("PRISM engine requires async initialization - use create_prism_async()"))
    }
    
    /// Create PRISM engine (async version)
    pub async fn create_prism_async() -> Result<Arc<dyn UnifiedStorageEngine>> {
        info!("Creating PRISM (Progressive Retrieval through Indexed Storage Management) storage engine");
        
        // Use default configuration
        let config = super::prism::config::Config::default();
        
        // Create PRISM engine with async initialization
        let engine = PrismEngine::new(config).await?;
        Ok(Arc::new(engine))
    }
    
    /// Create a storage engine with metrics integration
    pub fn create_with_metrics(
        engine_type: ProtoStorageEngine,
        metrics_collector: Arc<EngineMetricsCollector>,
    ) -> Result<Arc<dyn UnifiedStorageEngine>> {
        let engine = Self::create_from_proto(engine_type)?;
        
        // Set up metrics for SWIFT and NOVA engines
        match engine_type {
            ProtoStorageEngine::Swift => {
                if let Ok(mut swift) = Arc::try_unwrap(engine).and_then(|e| e.downcast::<SwiftEngine>()) {
                    swift.set_metrics_collector(metrics_collector.clone());
                    // Register engine with collector
                    let weak_ref = Arc::downgrade(&(Arc::new(swift) as Arc<dyn UnifiedStorageEngine>));
                    tokio::spawn(async move {
                        metrics_collector.register_engine("SWIFT".to_string(), weak_ref).await;
                    });
                    return Ok(Arc::new(swift) as Arc<dyn UnifiedStorageEngine>);
                }
            }
            ProtoStorageEngine::Nova => {
                // NOVA engine already created, just register it
                let weak_ref = Arc::downgrade(&engine);
                let collector = metrics_collector.clone();
                tokio::spawn(async move {
                    collector.register_engine("NOVA".to_string(), weak_ref).await;
                });
            }
            _ => {
                // For other engines, just register without metrics modification
                let weak_ref = Arc::downgrade(&engine);
                let engine_name = format!("{:?}", engine_type);
                tokio::spawn(async move {
                    metrics_collector.register_engine(engine_name, weak_ref).await;
                });
            }
        }
        
        Ok(engine)
    }
    
    /// Create the best engine for a given workload
    pub fn create_for_workload(workload: WorkloadType) -> Result<Arc<dyn UnifiedStorageEngine>> {
        match workload {
            WorkloadType::Analytics => {
                info!("Analytics workload detected, using NOVA for advanced columnar analytics");
                Self::create_nova()
            }
            WorkloadType::Transactional => {
                info!("Transactional workload detected, using SWIFT for fast ID lookups");
                Self::create_swift()
            }
            WorkloadType::Mixed => {
                info!("Mixed workload detected, using VIPER for balanced performance");
                Self::create_viper()
            }
            WorkloadType::Experimental => {
                info!("Experimental workload, using RAPTOR for cloud-optimized features");
                Self::create_raptor_default()
            }
        }
    }
    
    /// Get engine recommendations based on requirements
    pub fn recommend_engine(requirements: &EngineRequirements) -> ProtoStorageEngine {
        // Score each engine based on requirements
        let mut scores = vec![
            (ProtoStorageEngine::Viper, 0),
            (ProtoStorageEngine::Sst, 0),
            (ProtoStorageEngine::Swift, 0),
            (ProtoStorageEngine::Nova, 0),
        ];
        
        for (engine, score) in &mut scores {
            *score = Self::score_engine(*engine, requirements);
        }
        
        // Sort by score (highest first)
        scores.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        
        let (best_engine, best_score) = scores[0];
        info!(
            "Recommended engine: {:?} (similarity: {})",
            best_engine, best_score
        );
        
        best_engine
    }
    
    /// Score an engine based on requirements
    fn score_engine(engine: ProtoStorageEngine, req: &EngineRequirements) -> i32 {
        let mut score = 0;
        
        match engine {
            ProtoStorageEngine::Viper => {
                // VIPER: Good for general use, columnar storage
                if req.needs_columnar { score += 20; }
                if req.needs_compression { score += 15; }
                if req.needs_batch_operations { score += 10; }
                score += 10; // Base score for maturity
            }
            ProtoStorageEngine::Sst => {
                // SST: Good for write-heavy, row-based
                if req.needs_fast_writes { score += 20; }
                if req.needs_transactions { score += 15; }
                if req.needs_id_lookup { score += 10; }
                score += 10; // Base score for maturity
            }
            ProtoStorageEngine::Swift => {
                // SWIFT: Storage With Instant Fast Traversal - optimized for AXIS integration
                if req.needs_id_lookup { score += 25; }
                if req.needs_progressive_search { score += 20; }
                if req.needs_quantization { score += 15; }
                if req.needs_zero_overhead { score += 20; }
                score += 5; // Lower base score (newer)
            }
            ProtoStorageEngine::Nova => {
                // NOVA: Next-gen Optimized Vector Analytics - advanced columnar with dual-mode
                if req.needs_columnar { score += 25; }
                if req.needs_predicate_pushdown { score += 20; }
                if req.needs_projection { score += 15; }
                if req.needs_progressive_search { score += 15; }
                if req.needs_zero_overhead { score += 20; }
                score += 5; // Lower base score (newer)
            }
            _ => {}
        }
        
        score
    }
}

/// Workload type for engine selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadType {
    /// Analytics workload (read-heavy, aggregations)
    Analytics,
    /// Transactional workload (write-heavy, point queries)
    Transactional,
    /// Mixed workload (balanced read/write)
    Mixed,
    /// Experimental workload (testing new features)
    Experimental,
}

/// Engine requirements for recommendation
#[derive(Debug, Clone)]
pub struct EngineRequirements {
    pub needs_columnar: bool,
    pub needs_compression: bool,
    pub needs_batch_operations: bool,
    pub needs_fast_writes: bool,
    pub needs_transactions: bool,
    pub needs_id_lookup: bool,
    pub needs_progressive_search: bool,
    pub needs_quantization: bool,
    pub needs_predicate_pushdown: bool,
    pub needs_projection: bool,
    pub needs_zero_overhead: bool,
}

impl Default for EngineRequirements {
    fn default() -> Self {
        Self {
            needs_columnar: false,
            needs_compression: true,
            needs_batch_operations: true,
            needs_fast_writes: false,
            needs_transactions: false,
            needs_id_lookup: true,
            needs_progressive_search: false,
            needs_quantization: true,
            needs_predicate_pushdown: false,
            needs_projection: false,
            needs_zero_overhead: false,
        }
    }
}

/// Engine comparison result
#[derive(Debug)]
pub struct EngineComparison {
    pub engine_name: String,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
    pub best_for: Vec<String>,
    pub performance_score: i32,
    pub maturity_score: i32,
}

impl StorageEngineFactory {
    /// Compare all available engines
    pub fn compare_engines() -> Vec<EngineComparison> {
        vec![
            EngineComparison {
                engine_name: "VIPER".to_string(),
                pros: vec![
                    "Columnar storage for analytics".to_string(),
                    "Excellent compression".to_string(),
                    "Parquet integration".to_string(),
                ],
                cons: vec![
                    "Higher write latency".to_string(),
                    "Complex compaction_info".to_string(),
                ],
                best_for: vec![
                    "Analytics workloads".to_string(),
                    "Read-heavy applications".to_string(),
                ],
                performance_score: 80,
                maturity_score: 90,
            },
            EngineComparison {
                engine_name: "SST".to_string(),
                pros: vec![
                    "Fast writes".to_string(),
                    "Simple design".to_string(),
                    "Good for streaming".to_string(),
                ],
                cons: vec![
                    "Less compression".to_string(),
                    "Row-based storage".to_string(),
                ],
                best_for: vec![
                    "Write-heavy workloads".to_string(),
                    "Real-time ingestion".to_string(),
                ],
                performance_score: 75,
                maturity_score: 90,
            },
            EngineComparison {
                engine_name: "SWIFT".to_string(),
                pros: vec![
                    "Zero-overhead with AXIS".to_string(),
                    "B+ tree ID indexing".to_string(),
                    "Progressive search".to_string(),
                    "Dual-mode operation".to_string(),
                ],
                cons: vec![
                    "Newer, less tested".to_string(),
                    "Complex architecture".to_string(),
                ],
                best_for: vec![
                    "AXIS integration".to_string(),
                    "ID-based lookups".to_string(),
                    "Memory-constrained environments".to_string(),
                ],
                performance_score: 85,
                maturity_score: 60,
            },
            EngineComparison {
                engine_name: "NOVA".to_string(),
                pros: vec![
                    "Columnar with dual-mode".to_string(),
                    "Predicate pushdown".to_string(),
                    "Projection support".to_string(),
                    "Best compression".to_string(),
                ],
                cons: vec![
                    "Newest engine".to_string(),
                    "Complex configuration".to_string(),
                ],
                best_for: vec![
                    "Advanced analytics".to_string(),
                    "Large-scale deployments".to_string(),
                    "R&D experiments".to_string(),
                ],
                performance_score: 90,
                maturity_score: 50,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_engine_recommendation() {
        // Test for analytics workload
        let req = EngineRequirements {
            needs_columnar: true,
            needs_compression: true,
            needs_predicate_pushdown: true,
            needs_projection: true,
            ..Default::default()
        };
        
        let engine = StorageEngineFactory::recommend_engine(&req);
        assert_eq!(engine, ProtoStorageEngine::Nova);
        
        // Test for transactional workload
        let req = EngineRequirements {
            needs_fast_writes: true,
            needs_transactions: true,
            needs_id_lookup: true,
            ..Default::default()
        };
        
        let engine = StorageEngineFactory::recommend_engine(&req);
        assert!(matches!(
            engine, 
            ProtoStorageEngine::Sst | ProtoStorageEngine::Swift
        ));
    }
    
    #[test]
    fn test_workload_based_selection() {
        // Analytics should prefer NOVA
        let engine = StorageEngineFactory::create_for_workload(
            WorkloadType::Analytics
        );
        assert!(engine.is_ok());
        
        // Transactional should prefer SWIFT
        let engine = StorageEngineFactory::create_for_workload(
            WorkloadType::Transactional
        );
        assert!(engine.is_ok());
    }
    
    #[test]
    fn test_engine_comparison() {
        let comparisons = StorageEngineFactory::compare_engines();
        
        assert_eq!(comparisons.len(), 4);
        
        // NOVA should have highest performance score
        let nova = comparisons.iter()
            .find(|c| c.engine_name == "NOVA")
            .unwrap();
        assert_eq!(nova.performance_score, 90);
        
        // VIPER and SST should have highest maturity
        let viper = comparisons.iter()
            .find(|c| c.engine_name == "VIPER")
            .unwrap();
        assert_eq!(viper.maturity_score, 90);
    }
}