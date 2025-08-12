"""
Embedding providers for ProximaDB SDK

This package contains various embedding providers that can be used
with ProximaDB for generating vector embeddings.
"""

from .base import EmbeddingProvider, EmbeddingConfig

__all__ = [
    'EmbeddingProvider',
    'EmbeddingConfig',
]