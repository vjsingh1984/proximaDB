//! Index management and orchestration

pub mod adaptive_engine;
pub mod analyzer;
pub mod hnsw_param_advisor;
pub mod manager;
pub mod migration_engine;
pub mod monitor;
pub mod recall_drift;
pub mod strategy;

// Re-export main types
pub use manager::{
    AxisManager, FilterOperator, HybridQuery, MetadataFilter, MigrationStatus, QueryResult,
    ScoredResult, VectorQuery,
};

pub use adaptive_engine::{
    AccessFrequencyMetrics, AdaptiveIndexEngine, CollectionCharacteristics, MetadataComplexity,
    PerformanceMetrics, QueryDistribution, QueryPatternAnalysis, QueryPatternType, TemporalPattern,
};

pub use analyzer::CollectionAnalyzer;
pub use hnsw_param_advisor::{
    EF_SEARCH_MAX, EF_SEARCH_MIN, HnswSizingInput, HnswSizingOutput, advise_hnsw_params,
};
pub use recall_drift::{DriftKind, RecallDriftInput, RecallDriftReport, detect_recall_drift};
pub use migration_engine::{
    IndexMigrationEngine, MigrationComplexity, MigrationDecision, MigrationPhase, MigrationPlan,
};
pub use monitor::{AxisMonitor, MonitoringMetrics};
pub use strategy::{IndexStrategy, StrategyRecommendation, StrategySelector};
