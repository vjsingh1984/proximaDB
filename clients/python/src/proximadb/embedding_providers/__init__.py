"""
Embedding providers for ProximaDB Python SDK

This package provides various embedding providers with a pluggable interface.
Includes both free/local providers and paid API providers.
"""

from .base import EmbeddingProvider, EmbeddingConfig
from .sentence_transformer import SentenceTransformerProvider
from .instructor import InstructorProvider
from .fastembed import FastEmbedProvider
from .openai_compatible import OpenAICompatibleProvider
from .openai_provider import OpenAIProvider
from .cohere import CohereProvider
from .simulated import SimulatedEmbeddingProvider
from .factory import (
    EmbeddingProviderFactory,
    get_embedding_provider,
    get_default_embedding_provider,
    recommend_free_providers
)

__all__ = [
    # Base classes
    'EmbeddingProvider',
    'EmbeddingConfig',
    
    # Free providers
    'SentenceTransformerProvider',
    'InstructorProvider', 
    'FastEmbedProvider',
    'OpenAICompatibleProvider',
    'SimulatedEmbeddingProvider',
    
    # Paid providers
    'OpenAIProvider',
    'CohereProvider',
    
    # Factory and helpers
    'EmbeddingProviderFactory',
    'get_embedding_provider',
    'get_default_embedding_provider',
    'recommend_free_providers',
]