// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Service layer modules

pub mod collection_service;
pub mod migration;
pub mod storage_path_service;
pub mod vector_service;

pub use collection_service::CollectionService;
pub use vector_service::VectorService;