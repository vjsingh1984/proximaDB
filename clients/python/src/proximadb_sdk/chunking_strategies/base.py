"""
Base interfaces and data structures for chunking strategies

Defines the core abstractions for text chunking without any embedding concerns.
"""

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional


class ChunkingStrategy(Enum):
    """Available chunking strategies"""

    SLIDING_WINDOW = "sliding_window"
    SENTENCE = "sentence"
    PARAGRAPH = "paragraph"
    SEMANTIC = "semantic"
    RECURSIVE = "recursive"
    FIXED_SIZE = "fixed_size"
    CODE = "code"  # AST-aware code chunking using tree-sitter


@dataclass
class TextChunk:
    """
    Represents a text chunk with metadata

    Pure data structure - no network operations or embeddings
    """

    text: str
    start_pos: int
    end_pos: int
    chunk_id: str
    metadata: Dict[str, Any] = field(default_factory=dict)

    @property
    def start(self) -> int:
        """Backward compatibility alias for start_pos"""
        return self.start_pos

    @property
    def end(self) -> int:
        """Backward compatibility alias for end_pos"""
        return self.end_pos

    def __post_init__(self):
        """Add chunk-specific metadata"""
        if "chunk_length" not in self.metadata:
            self.metadata["chunk_length"] = len(self.text)
        if "chunk_id" not in self.metadata:
            self.metadata["chunk_id"] = self.chunk_id


@dataclass
class ChunkingConfig:
    """
    Configuration for chunking strategies

    Pure configuration - no embedding-related settings
    """

    strategy: ChunkingStrategy = ChunkingStrategy.SLIDING_WINDOW
    chunk_size: int = 512
    chunk_overlap: int = 50
    min_chunk_size: int = 100
    max_chunk_size: int = 2048

    def __post_init__(self):
        """Validate and adjust configuration values"""
        # Auto-adjust chunk_overlap if it's too large for chunk_size
        if self.chunk_overlap >= self.chunk_size:
            # Set overlap to 20% of chunk_size as a reasonable default
            self.chunk_overlap = min(int(self.chunk_size * 0.2), self.chunk_size - 1)

        # Ensure chunk_overlap is never negative
        if self.chunk_overlap < 0:
            self.chunk_overlap = 0

        # Ensure max_chunk_size is at least chunk_size
        if self.max_chunk_size < self.chunk_size:
            self.max_chunk_size = self.chunk_size

    # Strategy-specific settings
    sentence_endings: List[str] = field(
        default_factory=lambda: [".", "!", "?", "。", "！", "？"]
    )
    preserve_sentences: bool = True
    preserve_paragraphs: bool = True
    preserve_code_blocks: bool = True
    preserve_tables: bool = True

    # Context settings
    add_context: bool = False
    context_size: int = 50

    # Semantic settings (no embeddings)
    section_patterns: List[str] = field(default_factory=list)
    topic_indicators: List[str] = field(default_factory=list)


class ChunkingStrategyInterface(ABC):
    """
    Abstract interface for chunking strategies

    Focuses purely on text chunking - no embedding operations
    """

    def __init__(self, config: ChunkingConfig):
        self.config = config

    @abstractmethod
    def chunk(
        self, text: str, source_id: str, base_metadata: Optional[Dict[str, Any]] = None
    ) -> List[TextChunk]:
        """
        Chunk text into smaller pieces

        Args:
            text: The text to chunk
            source_id: Identifier for the source document
            base_metadata: Optional metadata to include with all chunks

        Returns:
            List of TextChunk objects
        """
        pass

    def validate_config(self) -> None:
        """Validate configuration for this strategy"""
        if self.config.chunk_size <= 0:
            raise ValueError("chunk_size must be positive")
        if self.config.chunk_overlap < 0:
            raise ValueError("chunk_overlap cannot be negative")
        if self.config.chunk_overlap >= self.config.chunk_size:
            raise ValueError("chunk_overlap must be less than chunk_size")
        if self.config.min_chunk_size < 0:
            raise ValueError("min_chunk_size cannot be negative")
        if self.config.max_chunk_size < self.config.chunk_size:
            raise ValueError("max_chunk_size must be >= chunk_size")

    def add_chunk_metadata(
        self, chunk: TextChunk, chunk_index: int, total_chunks: int, strategy_name: str
    ) -> None:
        """Add standard metadata to a chunk"""
        chunk.metadata.update(
            {
                "chunk_index": chunk_index,
                "total_chunks": total_chunks,
                "chunking_strategy": strategy_name,
                "chunk_size_config": self.config.chunk_size,
                "chunk_overlap_config": self.config.chunk_overlap,
            }
        )

    def normalize_text(self, text: str) -> str:
        """Basic text normalization"""
        # Replace multiple spaces with single space
        text = " ".join(text.split())
        # Preserve paragraph breaks
        text = text.replace("\n\n", "\n<<PARA_BREAK>>\n")
        text = text.replace("\n", " ")
        text = text.replace("\n<<PARA_BREAK>>\n", "\n\n")
        return text.strip()
