// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Vector Operations Module
//!
//! Optimized vector operations with intelligent routing, validation, and performance monitoring.
//! Provides unified interfaces for insert and search operations across different storage engines.

pub mod insert;
pub mod search;

pub use insert::{
    VectorInsertHandler, InsertConfig, InsertValidator, InsertOptimizer, 
    ValidationResult, InsertOperation, InsertOptions, OptimizedInsertOperation,
    InsertBatch, BatchPriority, ProcessingHint, InsertResult, InsertPerformanceInfo,
    DimensionValidator, DuplicateValidator, VectorQualityValidator,
    BatchOptimizer, VectorizedOptimizer
};

pub use search::{
    VectorSearchHandler, SearchConfig, QueryPlanner, ResultProcessor,
    QueryPlan, ExecutionStep, StepType, QueryCost, SearchResult,
    SearchPerformanceInfo, SearchPerformanceRecord, SearchStats,
    QueryType, CostBasedPlanner, HeuristicPlanner,
    ScoreNormalizer, ResultDeduplicator, QualityEnhancer
};