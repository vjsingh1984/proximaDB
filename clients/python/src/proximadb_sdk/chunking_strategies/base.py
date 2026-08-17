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


#: Unit in which ``TextChunk.start_pos``/``end_pos`` are expressed. Text
#: strategies use character offsets; the deprecated code path publishes UTF-8
#: byte offsets through the same field (TD-CG2), which is why the basis must be
#: declared rather than assumed.
OFFSET_BASIS_CHAR = "char"
OFFSET_BASIS_BYTE = "byte"

#: Strength of the offset guarantee.
#: ``exact``  — ``source[start_pos:end_pos] == chunk.text``, safe to slice.
#: ``legacy`` — offsets are approximate/derived; NEVER slice the source with them.
OFFSET_CONTRACT_EXACT = "exact"
OFFSET_CONTRACT_LEGACY = "legacy"


def offsets_are_exact(metadata: dict[str, Any] | None) -> bool:
    """True only when a chunk's metadata explicitly promises sliceable offsets.

    Absence means legacy. This is the whole mixed-read rule: chunks persisted
    before the span-first migration carry no marker, and a reader that slices
    the source with them gets shifted or unrelated text.
    """
    if not metadata:
        return False
    return metadata.get("offset_contract") == OFFSET_CONTRACT_EXACT


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

        # Ensure min_chunk_size never exceeds chunk_size. The pair is otherwise
        # self-contradictory — every full window is already below the floor, so
        # a correct implementation would have to either drop everything or
        # ignore the floor. Clamping (rather than raising) keeps the public
        # convenience helpers working: `chunk_by_sentences(text, chunk_size=30)`
        # inherits the default min_chunk_size=100 and must not blow up.
        # Once clamped, the only under-minimum span a strategy can produce is
        # the final partial one, which is what makes the last-chunk escape
        # provably sufficient rather than a special case.
        if self.min_chunk_size > self.chunk_size:
            self.min_chunk_size = self.chunk_size

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

    # Optional model-input budget. When present, the selected strategy proposes
    # preferred structural boundaries and TokenBudgetStrategy owns final token
    # segmentation. These are typed as Any so importing the base module remains
    # dependency-light and existing character-based configurations are unchanged.
    token_budget: Any | None = None
    input_contract: Any | None = None
    input_role: Any | None = None


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

    #: Whether this strategy's spans can be sliced against the source. Defaults
    #: to ``legacy`` and is raised to ``exact`` per strategy as each is migrated
    #: to span-first construction, so the marker never over-promises for a
    #: strategy that has not been converted yet.
    _offset_contract: str = OFFSET_CONTRACT_LEGACY

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

    def preferred_boundaries(
        self,
        text: str,
        source_id: str,
        base_metadata: dict[str, Any] | None = None,
    ) -> list[int]:
        """Return preferred raw-text end offsets for an external size budget.

        The compatibility default derives boundaries from this strategy's
        ordinary chunks. Structural strategies override it to expose every
        atomic boundary, independent of their legacy character-size grouping.
        """
        return [
            chunk.end_pos
            for chunk in self.chunk(text, source_id, base_metadata)
            if 0 < chunk.end_pos <= len(text)
        ]

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

    def _size(self, source: Any, start: int, end: int) -> int:
        """Size of the span ``[start, end)`` in this strategy's measure.

        THE chokepoint. Every size decision in every strategy routes through
        here, so a measure is injected in exactly one place per strategy rather
        than at the ~41 sites that used to inline ``span[1] - span[0]``.

        ``source`` is the document text, or a ``Slicer`` for it — the grouping
        loops are shared between the batch and streaming paths, and streaming
        has only a bounded buffer, never the whole document. The character
        measure ignores it entirely (span extent IS the size), which keeps the
        default path allocation-free; a non-character measure materialises the
        span through it to count. That identity holds ONLY for characters: under
        a token measure a span's extent and its count are unrelated numbers.
        """
        return end - start

    def _size_of_span(self, source: Any, span: tuple[int, int]) -> int:
        """Convenience for the common ``self._size(source, *span)`` shape."""
        return self._size(source, span[0], span[1])

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
        if self.config.min_chunk_size > self.config.chunk_size:
            # Self-contradictory: every full window is already below the floor,
            # so a correct implementation must either drop everything or ignore
            # the floor. Rejecting it is also what makes the last-chunk escape
            # provably sufficient — the only under-minimum span becomes the
            # final partial one.
            raise ValueError(
                f"min_chunk_size ({self.config.min_chunk_size}) must be <= "
                f"chunk_size ({self.config.chunk_size})"
            )

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
                # Offset contract (ADR-091 axiom 2 / TD-CG2). `start_pos` and
                # `end_pos` are persisted by document_processor.py, so their
                # MEANING is a stored contract. These markers are additive and
                # readers must treat their ABSENCE as legacy — i.e. offsets that
                # cannot be sliced against the source. Mixed-read-safe, no flag
                # day, no backfill: the old offsets were never usable, so there
                # is no reader to preserve, only one to stop misleading.
                "offset_basis": OFFSET_BASIS_CHAR,
                "offset_contract": self._offset_contract,
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
