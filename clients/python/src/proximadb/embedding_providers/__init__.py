"""
Embedding providers for ProximaDB SDK

This package contains various embedding providers that can be used
with ProximaDB for generating vector embeddings.
"""

from .base import EmbeddingProvider, EmbeddingConfig
from .factory import (
    EmbeddingProviderFactory,
    get_embedding_provider,
    get_default_embedding_provider,
    recommend_free_providers
)

__all__ = [
    'EmbeddingProvider',
    'EmbeddingConfig',
    'EmbeddingProviderFactory',
    'get_embedding_provider',
    'get_default_embedding_provider',
    'recommend_free_providers',
]