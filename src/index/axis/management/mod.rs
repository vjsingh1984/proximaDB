//! Index management and orchestration

pub mod manager;
pub mod adaptive_engine;
pub mod analyzer;
pub mod monitor;
pub mod strategy;
pub mod migration_engine;

// Re-export main types
pub use manager::{
    AxisManager, FilterOperator, HybridQuery, MetadataFilter, 
    MigrationStatus, QueryResult, ScoredResult, VectorQuery,
};

pub use adaptive_engine::{
    AccessFrequencyMetrics, AdaptiveIndexEngine, CollectionCharacteristics, 
    MetadataComplexity, PerformanceMetrics, QueryDistribution, 
    QueryPatternAnalysis, QueryPatternType, TemporalPattern,
};

pub use analyzer::CollectionAnalyzer;
pub use monitor::{AxisMonitor, MonitoringMetrics};
pub use strategy::{IndexStrategy, StrategySelector, StrategyRecommendation};
pub use migration_engine::{IndexMigrationEngine, MigrationPlan, MigrationPhase, MigrationDecision, MigrationComplexity};