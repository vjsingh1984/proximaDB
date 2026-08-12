// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! LLM / RAG configuration types, extracted from the root `llm` module
//! (TD-DECOMP-52).
//!
//! [`config`] carries the embedding-provider, LLM, RAG, and semantic-cache
//! configuration structs ([`config::LLMConfig`], [`config::RAGConfig`],
//! [`config::EmbeddingProvider`], [`config::SemanticCacheConfig`]). Depends only
//! on `proximadb-config`, keeping it a clean horizontal-tier leaf.

pub mod config;
