// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Service layer modules

pub mod collection_service;
pub mod direct_vector_service;
pub mod migration;
pub mod storage_path_service;
pub mod streaming_search;

pub use collection_service::CollectionService;
pub use direct_vector_service::DirectVectorService;
pub use streaming_search::{StreamingSearchService, StreamingSearchConfig, StreamingSearchResult};