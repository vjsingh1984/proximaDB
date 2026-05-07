# Copyright 2025 ProximaDB
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.

"""LLM Configuration for ProximaDB Python SDK."""

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, Optional


class EmbeddingProvider(str, Enum):
    """Embedding provider types.

    Matches Victor's embedding model types:
    - sentence-transformers: Local models (air-gapped compatible)
    - openai: OpenAI API embeddings
    - cohere: Cohere API embeddings
    - ollama: Ollama local models (high-performance)
    """

    SENTENCE_TRANSFORMERS = "sentence-transformers"
    OPENAI = "openai"
    COHERE = "cohere"
    OLLAMA = "ollama"


@dataclass
class EmbeddingConfig:
    """Configuration for embedding generation.

    Attributes:
        provider: Embedding provider type
        model_name: Model name (e.g., "BAAI/bge-small-en-v1.5")
        dimension: Embedding dimension (auto-detected if not specified)
        api_key: API key for cloud providers
        batch_size: Batch size for embedding generation
        base_url: Base URL for local providers (Ollama)
    """

    provider: EmbeddingProvider = EmbeddingProvider.SENTENCE_TRANSFORMERS
    model_name: str = "BAAI/bge-small-en-v1.5"
    dimension: Optional[int] = None
    api_key: Optional[str] = None
    batch_size: int = 32
    base_url: str = "http://localhost:11434"  # For Ollama

    def get_dimension(self) -> int:
        """Get embedding dimension, inferring from model if not specified."""
        if self.dimension is not None:
            return self.dimension

        # Infer from model name
        dimension_map = {
            # Sentence-transformers
            "all-MiniLM-L6-v2": 384,
            "all-MiniLM-L12-v2": 384,
            "all-mpnet-base-v2": 768,
            "BAAI/bge-small-en-v1.5": 384,
            "BAAI/bge-base-en-v1.5": 768,
            "BAAI/bge-large-en-v1.5": 1024,
            # OpenAI
            "text-embedding-3-small": 1536,
            "text-embedding-3-large": 3072,
            "text-embedding-ada-002": 1536,
            # Cohere
            "embed-english-v3.0": 1024,
            "embed-multilingual-v3.0": 1024,
            "embed-english-light-v3.0": 384,
            # Ollama
            "qwen3-embedding:8b": 4096,
            "qwen3-embedding:4b": 2560,
            "nomic-embed-text": 768,
            "mxbai-embed-large": 1024,
        }
        return dimension_map.get(self.model_name, 384)


@dataclass
class RAGConfig:
    """Configuration for RAG pipeline.

    Attributes:
        retrieval_top_k: Number of documents to retrieve
        context_top_k: Number of documents to include in context
        max_context_tokens: Maximum tokens in context
        similarity_threshold: Minimum similarity for retrieval (0-1)
        semantic_cache_enabled: Whether to cache RAG responses
        llm_provider: LLM provider for generation (e.g., "ollama", "openai")
        llm_model: LLM model for generation
        temperature: Generation temperature
        max_tokens: Maximum response tokens
        system_prompt: Custom system prompt
    """

    retrieval_top_k: int = 10
    context_top_k: int = 5
    max_context_tokens: int = 2000
    similarity_threshold: float = 0.5
    semantic_cache_enabled: bool = True
    llm_provider: str = "ollama"
    llm_model: str = "llama3.1:8b"
    temperature: float = 0.7
    max_tokens: int = 1024
    system_prompt: Optional[str] = None


@dataclass
class SemanticCacheConfig:
    """Configuration for semantic caching.

    Attributes:
        enabled: Whether caching is enabled
        collection_name: Collection for cache storage
        similarity_threshold: Similarity threshold for cache hits (0-1)
        ttl_hours: Cache entry TTL in hours
        max_entries: Maximum cache entries
        min_query_length: Minimum query length to cache
    """

    enabled: bool = True
    collection_name: str = "_rag_cache"
    similarity_threshold: float = 0.95
    ttl_hours: int = 24
    max_entries: int = 10000
    min_query_length: int = 10


@dataclass
class LLMConfig:
    """Main LLM configuration.

    Attributes:
        enabled: Whether LLM integration is enabled
        embedding: Embedding configuration
        rag: RAG pipeline configuration
        cache: Semantic cache configuration
        default_collection: Default collection for embeddings
    """

    enabled: bool = True
    embedding: EmbeddingConfig = field(default_factory=EmbeddingConfig)
    rag: RAGConfig = field(default_factory=RAGConfig)
    cache: SemanticCacheConfig = field(default_factory=SemanticCacheConfig)
    default_collection: str = "embeddings"

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "LLMConfig":
        """Create config from dictionary."""
        embedding_data = data.get("embedding", {})
        if "provider" in embedding_data and isinstance(embedding_data["provider"], str):
            embedding_data["provider"] = EmbeddingProvider(embedding_data["provider"])

        return cls(
            enabled=data.get("enabled", True),
            embedding=(
                EmbeddingConfig(**embedding_data)
                if embedding_data
                else EmbeddingConfig()
            ),
            rag=RAGConfig(**data.get("rag", {})),
            cache=SemanticCacheConfig(**data.get("cache", {})),
            default_collection=data.get("default_collection", "embeddings"),
        )
