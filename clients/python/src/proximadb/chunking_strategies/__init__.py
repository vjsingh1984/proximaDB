"""
Pluggable chunking strategies for ProximaDB Python SDK

This package provides a clean interface for text chunking with proper separation of concerns:
- Each chunking strategy in its own module
- No embedding logic mixed with chunking
- Metadata generation is purely about chunk properties
- Extensible interface for custom strategies
"""

from .base import ChunkingStrategy, ChunkingStrategyInterface, TextChunk, ChunkingConfig
from .sliding_window import SlidingWindowStrategy
from .sentence import SentenceStrategy
from .paragraph import ParagraphStrategy
from .semantic import SemanticStrategy
from .recursive import RecursiveStrategy
from .factory import ChunkingStrategyFactory, get_chunking_strategy

__all__ = [
    'ChunkingStrategy',
    'ChunkingStrategyInterface',
    'TextChunk',
    'ChunkingConfig',
    'SlidingWindowStrategy',
    'SentenceStrategy',
    'ParagraphStrategy',
    'SemanticStrategy',
    'RecursiveStrategy',
    'ChunkingStrategyFactory',
    'get_chunking_strategy',
]