//! Search infrastructure shared across storage engines

pub mod progressive_search;
pub mod search_common;
pub mod search_modes;

pub use crate::storage::engines::core::search::search_common::{
    FilterProcessor as MetadataFilter, ResultManager as SearchResult, ResultManager,
    SearchConfig as SearchContext, UniversalSearchPipeline as SearchPlan,
    UniversalSearchPipeline as QueryOptimizer,
};

pub use progressive_search::{
    ProgressiveSearchExecutor as ProgressiveSearchEngine, SearchCandidate as ProgressiveRefinement,
    SearchStage,
};
