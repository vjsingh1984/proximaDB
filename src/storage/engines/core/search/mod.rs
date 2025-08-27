//! Search infrastructure shared across storage engines

pub mod search_common;
pub mod progressive_search;
pub mod search_modes;

pub use crate::storage::engines::core::search::search_common::{
    SearchConfig as SearchContext, 
    UniversalSearchPipeline as SearchPlan,
    ResultManager as SearchResult,
    FilterProcessor as MetadataFilter,
    ResultManager,
    UniversalSearchPipeline as QueryOptimizer,
};

pub use progressive_search::{
    ProgressiveSearchExecutor as ProgressiveSearchEngine, 
    SearchCandidate as ProgressiveRefinement,
    SearchStage,
};