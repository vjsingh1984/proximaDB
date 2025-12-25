//! Progressive Search Trait-Based Architecture (ISP Compliant)
//!
//! This module provides a trait-based abstraction for progressive multi-stage search,
//! following the Interface Segregation Principle (ISP) from SOLID.
//!
//! ## Design Goals:
//!
//! 1. **Interface Segregation**: Each stage implements only what it needs
//! 2. **Pluggability**: Engines can compose custom stage pipelines
//! 3. **Testability**: Individual stages can be tested in isolation
//! 4. **Extensibility**: New quantization stages can be added without modifying existing code
//!
//! ## Architecture:
//!
//! ```text
//! Query → [BinaryStage] → [Int8Stage] → [PqStage] → [Fp32Stage] → Results
//!              ↓               ↓            ↓             ↓
//!           Filter          Rank         Rank         Rerank
//!          (fast)        (medium)      (medium)      (precise)
//! ```
//!
//! ## Usage:
//!
//! ```rust,ignore
//! use crate::storage::engines::core::progressive::*;
//!
//! let coordinator = ProgressiveSearchCoordinator::new()
//!     .add_stage(Box::new(BinaryStage::new(0.7)))
//!     .add_stage(Box::new(Int8Stage::default()))
//!     .add_stage(Box::new(Fp32Stage));
//!
//! let results = coordinator.search(query, candidates, top_k, expansion_factor).await?;
//! ```

mod stage;
mod coordinator;
mod factory;

pub use stage::{
    ProgressiveSearchStage,
    StageResult,
    ScoredCandidate,
    QuantizationLevel,
    BinaryStage,
    Int8Stage,
    PqStage,
    Fp32Stage,
};

pub use coordinator::{
    ProgressiveSearchCoordinator,
    CoordinatorConfig,
    StageStats,
};

pub use factory::{
    ProgressivePipelineFactory,
    ProgressiveEngineType,
    PipelineStage,
};
