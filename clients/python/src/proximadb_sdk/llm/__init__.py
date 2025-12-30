# Copyright 2025 ProximaDB
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.

"""LLM Integration Module for ProximaDB Python SDK.

This module provides integration with Victor (codingagent) for:
- Embedding generation (local and cloud models)
- RAG (Retrieval-Augmented Generation) pipelines
- Semantic caching for LLM responses

Usage:
    from proximadb_sdk.llm import RAGPipeline, EmbeddingProvider

    # Create RAG pipeline with ProximaDB
    rag = RAGPipeline(
        client=proximadb_client,
        embedding_provider="sentence-transformers",
        embedding_model="all-MiniLM-L12-v2",
    )

    # Index documents
    await rag.index_documents("knowledge_base", documents)

    # Query
    response = await rag.query("What is ProximaDB?", "knowledge_base")
    print(response.answer)

Requirements:
    pip install proximadb-python[llm]

    This installs:
    - victor-ai: Victor AI framework for embeddings and LLM
    - sentence-transformers: Local embedding models
"""

from proximadb_sdk.llm.config import (
    EmbeddingConfig,
    EmbeddingProvider,
    LLMConfig,
    RAGConfig,
    SemanticCacheConfig,
)
from proximadb_sdk.llm.embedding import EmbeddingService
from proximadb_sdk.llm.rag import Document, RAGPipeline, RAGResponse, Source
from proximadb_sdk.llm.semantic_cache import CachedResponse, SemanticCache

__all__ = [
    # Config
    "LLMConfig",
    "EmbeddingConfig",
    "EmbeddingProvider",
    "RAGConfig",
    "SemanticCacheConfig",
    # Embedding
    "EmbeddingService",
    # RAG
    "RAGPipeline",
    "RAGResponse",
    "Document",
    "Source",
    # Cache
    "SemanticCache",
    "CachedResponse",
]
