//! Index management and orchestration

pub mod adaptive_engine;
pub mod analyzer;
pub mod filtered_search; // Filtered search implementation (Issue #40, SB-10)
pub mod manager;
pub mod migration_engine;
pub mod monitor;
pub mod strategy;

// Re-export main types
pub use filtered_search::{AxisMetadataLookup, FilteredSearchResult};
pub use manager::{
    AxisManager, FilterOperator, HybridQuery, MetadataFilter, MigrationStatus, QueryResult,
    ScoredResult, VectorQuery,
};

pub use adaptive_engine::{
    AccessFrequencyMetrics, AdaptiveIndexEngine, CollectionCharacteristics, MetadataComplexity,
    PerformanceMetrics, QueryDistribution, QueryPatternAnalysis, QueryPatternType, TemporalPattern,
};

pub use analyzer::CollectionAnalyzer;
pub use migration_engine::{
    IndexMigrationEngine, MigrationComplexity, MigrationDecision, MigrationPhase, MigrationPlan,
};
pub use monitor::{AxisMonitor, MonitoringMetrics};
pub use strategy::{IndexStrategy, StrategyRecommendation, StrategySelector};
