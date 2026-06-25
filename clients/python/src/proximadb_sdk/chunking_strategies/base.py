"""
Base interfaces and data structures for chunking strategies

Defines the core abstractions for text chunking without any embedding concerns.
"""

from abc import ABC, abstractmethod
from collections.abc import Iterable, Iterator
from dataclasses import dataclass, field
from enum import Enum
from typing import Any


def _coalesce_text_source(text_source: "str | Iterable[str]") -> str:
    """Materialize a text source (str or iterable of str pieces) into one string.

    Accepts either a single ``str`` or any iterable yielding string pieces
    (e.g. successive ``file.read(n)`` reads). This is the materializing path used
    by the default (non-streaming) ``chunk_stream`` and by strategies that cannot
    stream — it intentionally builds the whole input in memory.
    """
    if isinstance(text_source, str):
        return text_source
    return "".join(text_source)


class ChunkingStrategy(Enum):
    """Available chunking strategies"""

    SLIDING_WINDOW = "sliding_window"
    SENTENCE = "sentence"
    PARAGRAPH = "paragraph"
    SEMANTIC = "semantic"  # structural/regex semantic boundaries (no embeddings)
    SEMANTIC_EMBEDDING = "semantic_embedding"  # embedding-breakpoint semantic chunking (injected provider)
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
    metadata: dict[str, Any] = field(default_factory=dict)

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
    sentence_endings: list[str] = field(
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
    section_patterns: list[str] = field(default_factory=list)
    topic_indicators: list[str] = field(default_factory=list)

    # Embedding-based semantic chunking (SEMANTIC_EMBEDDING strategy only).
    # The provider is INJECTED — a core.BaseEmbeddingProvider-style object with
    # a batch embed/encode method, OR a Callable[[list[str]], list[list[float]]].
    # Typed as Any so the base module pulls NO heavy embedding deps (the lazy
    # boundary that keeps `import proximadb_sdk` light stays intact).
    embedding_provider: Any | None = None
    # Window of context sentences blended into each side of a breakpoint test.
    buffer_size: int = 1
    # Percentile of the consecutive-group distance distribution above which a
    # breakpoint is placed (LlamaIndex default: 95th percentile).
    breakpoint_percentile_threshold: float = 95.0


class ChunkingStrategyInterface(ABC):
    """
    Abstract interface for chunking strategies

    Focuses purely on text chunking - no embedding operations
    """

    #: Whether this strategy can chunk incrementally with a bounded buffer
    #: (i.e. boundary-local). Strategies that need the whole input (tree-sitter
    #: parse, all-sentence embeddings, recursive cascade, whole-doc structure)
    #: leave this ``False`` and fall back to materialize-then-chunk in
    #: :meth:`chunk_stream`. Streamable strategies override it to ``True``.
    supports_streaming: bool = False

    def __init__(self, config: ChunkingConfig):
        self.config = config

    @abstractmethod
    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
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

    def chunk_stream(
        self,
        text_source: "str | Iterable[str]",
        source_id: str,
        base_metadata: dict[str, Any] | None = None,
    ) -> Iterator[TextChunk]:
        """Yield chunks for ``text_source`` one at a time.

        ``text_source`` may be a single ``str`` or an iterable of text pieces
        (e.g. successive file reads).

        The default implementation is the **honest fallback** for strategies
        that cannot stream: it materializes ``text_source`` into one string,
        runs the batch :meth:`chunk`, and ``yield from`` the resulting list.
        Memory is therefore bounded by the input size plus the full chunk list,
        *not* constant — but chunks are still produced via an iterator so
        callers consume them incrementally.

        Streamable strategies (``supports_streaming == True``) override this to
        maintain only a bounded local buffer and emit chunks as boundaries are
        crossed, never accumulating the full input or the full output list.
        """
        text = _coalesce_text_source(text_source)
        yield from self.chunk(text, source_id, base_metadata)

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
