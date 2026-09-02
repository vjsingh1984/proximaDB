"""
Base interfaces and data structures for chunking strategies

Defines the core abstractions for text chunking without any embedding concerns.
"""

import functools
from abc import ABC, abstractmethod
from collections.abc import Iterable, Iterator
from dataclasses import dataclass, field, fields
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
    # Optional exact model input after deterministic per-source context
    # propagation. ``text`` remains the source slice named by start/end so
    # citation offsets never become synthetic.
    model_input_text: str | None = None

    @property
    def start(self) -> int:
        """Backward compatibility alias for start_pos"""
        return self.start_pos

    @property
    def end(self) -> int:
        """Backward compatibility alias for end_pos"""
        return self.end_pos

    @property
    def input_text(self) -> str:
        """Text counted and sent to the model, preserving legacy behavior."""
        return self.model_input_text if self.model_input_text is not None else self.text

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
        # A declarative `sizing` policy is resolved to absolutes FIRST, so that
        # everything below it -- the clamps, validate_config, and every strategy
        # reading self.config.chunk_size -- keeps seeing plain integers and does
        # not learn that fractions exist. The clamps then still apply as a
        # backstop, so a policy cannot smuggle in a state a literal config
        # could not reach.
        if self.sizing is not None:
            if self.token_budget is not None:
                raise ValueError(
                    "supply either `sizing` or the legacy `token_budget`, not "
                    "both: they are two spellings of one budget and there is no "
                    "correct precedence between them"
                )
            resolved = self.sizing.resolve()
            self.chunk_size = resolved.window
            self.chunk_overlap = resolved.overlap
            self.min_chunk_size = resolved.minimum
            self.max_chunk_size = resolved.maximum
            if self.measure is None:
                self.measure = resolved.measure

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
    # How sizes are COUNTED. ``None`` means the character measure, which is
    # both the default and the fast path (no prefix array, no materialisation),
    # so existing behaviour is byte-for-byte unchanged. Typed ``Any`` for the
    # same reason as the fields below: the base module must pull no heavy deps.
    measure: Any | None = None

    # Declarative budget (sizing.SizingPolicy). When present it is resolved to
    # the absolute fields above at construction; when absent those fields ARE
    # the budget, so there is exactly one internal path and legacy callers are
    # byte-for-byte unchanged.
    sizing: Any | None = None

    token_budget: Any | None = None
    input_contract: Any | None = None
    input_role: Any | None = None
    # Optional contracts.ChunkContextRenderer. Kept dependency-light like
    # token_budget/input_contract because base.py must not import tokenizers.
    context_renderer: Any | None = None


def config_kwargs(config: Any) -> dict[str, Any]:
    """Every ``ChunkingConfig`` field carried by ``config``, minus ``strategy``.

    Exists to replace hand-written forwarding lists. Rebuilding a config by
    naming its fields one at a time means every field added later is silently
    dropped by every list that was not updated -- and "silently" is the whole
    problem: a dropped ``measure`` does not raise, it just chunks in characters
    while the caller believes it asked for tokens. The same shape already cost
    this codebase the ``embedding_provider`` and ``min/max_chunk_size`` fields
    (``document_processor.py`` carries a comment recording that bug shipping).

    Deriving from :func:`dataclasses.fields` makes the forwarding total by
    construction, so a new field is propagated the moment it is declared.
    """
    return {
        f.name: getattr(config, f.name)
        for f in fields(ChunkingConfig)
        if f.name != "strategy" and hasattr(config, f.name)
    }


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

    def __init_subclass__(cls, **kwargs: Any) -> None:
        """Wrap each subclass's ``chunk`` so the size cap cannot be opted out of.

        TD-CHUNK-2 S1 asks for ``max_chunk_size`` as a hard post-condition "so no
        source can violate it". A helper the strategy must remember to call is
        exactly the shape that gets forgotten -- and the whole point of
        TD-CHUNK-2 is that boundary sources become PLURAL and composable, so the
        number of places that could forget is about to grow. Wrapping at class
        definition makes the guarantee structural: a new source inherits it
        without knowing it exists.
        """
        super().__init_subclass__(**kwargs)
        implementation = cls.__dict__.get("chunk")
        if implementation is None or getattr(implementation, "_cap_guarded", False):
            return

        @functools.wraps(implementation)
        def guarded(
            self: "ChunkingStrategyInterface", *args: Any, **kwargs: Any
        ) -> list[TextChunk]:
            # Fully signature-transparent. Restating the common
            # (text, source_id, base_metadata) shape here would silently NARROW
            # any implementation that differs -- `CodeChunkingStrategy.chunk`
            # takes `metadata=`, and hardcoding the usual signature dropped it
            # with a TypeError. A guard that changes the API it guards is worse
            # than no guard.
            text = args[0] if args else kwargs.get("text", "")
            return self._enforce_cap(
                implementation(self, *args, **kwargs),
                text if isinstance(text, str) else "",
            )

        guarded._cap_guarded = True  # type: ignore[attr-defined]
        cls.chunk = guarded  # type: ignore[method-assign]

    def _enforce_cap(self, chunks: list[TextChunk], text: str) -> list[TextChunk]:
        """Split any emitted chunk that exceeds ``max_chunk_size``.

        Splitting, not raising, and not passing it through:

        * passing it through is the worst option -- the provider either rejects
          the call or SILENTLY TRUNCATES, and truncation loses the tail with no
          signal, which is the exact defect class ADR-091 exists to remove;
        * raising would make a legal document permanently unindexable over a
          boundary the user did not choose and cannot fix.

        This mirrors the reasoning already written on ``spans.hard_split``; what
        is new is that it now applies to chunks a strategy has already emitted,
        so no source can route around it.

        Two deliberate limits:

        * It is skipped when the strategy's offsets are not ``exact``. Re-cutting
          requires indexing the source with the chunk's own offsets, and a
          ``legacy`` strategy's offsets do not index the source -- splitting on
          them would produce confidently wrong text. Refusing to act on
          unreliable offsets is the same rule the offset contract exists to
          state.
        * It cuts with :meth:`_fit_end` rather than ``hard_split`` because the
          latter's ``start + cap`` arithmetic is characters by construction,
          which would silently mis-cut under a token measure.
        """
        cap = self.config.max_chunk_size
        if cap <= 0 or self._offset_contract != OFFSET_CONTRACT_EXACT:
            return chunks

        rebuilt: list[TextChunk] = []
        split_any = False
        for chunk in chunks:
            start, end = chunk.start_pos, chunk.end_pos
            if end <= start or self._size(text, start, end) <= cap:
                rebuilt.append(chunk)
                continue
            split_any = True
            cursor = start
            while cursor < end:
                cut = self._fit_end(text, cursor, cap, end)
                if cut <= cursor:  # a cap that fits nothing must still advance
                    cut = min(cursor + 1, end)
                piece = TextChunk(
                    text=text[cursor:cut],
                    start_pos=cursor,
                    end_pos=cut,
                    chunk_id=chunk.chunk_id,
                    metadata={**chunk.metadata, "cap_enforced": True},
                )
                rebuilt.append(piece)
                cursor = cut

        if not split_any:
            # The overwhelmingly common path: nothing violated the cap, so the
            # list is returned untouched -- same objects, same ids, same order.
            return chunks

        # Ids and indices encode POSITION, so they have to be restated once the
        # count changes; leaving them would emit duplicate ids, which is the
        # collision bug this program already found in anvaiops.
        for index, chunk in enumerate(rebuilt):
            base_id = chunk.chunk_id.rsplit("_chunk_", 1)[0]
            chunk.chunk_id = f"{base_id}_chunk_{index}"
            chunk.metadata["chunk_id"] = chunk.chunk_id
            chunk.metadata["chunk_index"] = index
            chunk.metadata["chunk_length"] = len(chunk.text)
        for chunk in rebuilt:
            if chunk.metadata.get("total_chunks", -1) != -1:
                chunk.metadata["total_chunks"] = len(rebuilt)
        return rebuilt

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
        measure = getattr(self.config, "measure", None)
        if measure is None:
            # Default: the character measure, inlined. Deliberately NOT a
            # delegation to CharMeasure — this runs once per size comparison in
            # every grouping loop, and the decoupling must not make the default
            # path pay for an abstraction it does not use.
            return end - start
        return measure.size(source, start, end)

    def _advance(self, source: Any, start: int, units: int) -> int:
        """Offset ``units`` units after ``start``, in this strategy's measure.

        The companion to :meth:`_size`: sizing compares, windowing advances. For
        a non-additive measure the result is a candidate to be verified, not a
        guarantee.
        """
        measure = getattr(self.config, "measure", None)
        if measure is None:
            return start + units
        return measure.advance(source, start, units)

    def _fit_end(self, source: Any, start: int, units: int, limit: int) -> int:
        """Greatest end at or before ``limit`` whose span really fits ``units``.

        This is the one place a non-additive measure is made safe. :meth:`_advance`
        answers "where would ``units`` units land", which for an additive measure
        *is* the answer and for a non-additive one is only a candidate — the
        rendered size of the resulting span can exceed the budget because of
        overhead no span owns (role prefixes, tokenizer special tokens).

        Verifying unconditionally would tax the default path for a problem it
        does not have, so the check is gated on the measure's own declaration.
        The verified branch bisects, which requires that size be non-decreasing
        in ``end`` — true of any measure that counts units in a span, and the
        weakest assumption under which the search is meaningful at all.
        """
        end = min(self._advance(source, start, units), limit)
        measure = getattr(self.config, "measure", None)
        if measure is None or getattr(measure, "is_additive", True):
            return end
        if end <= start or measure.size(source, start, end) <= units:
            return end
        # Overshoot: the largest end in (start, end) that fits.
        low, high, best = start + 1, end, start
        while low <= high:
            middle = (low + high) // 2
            if measure.size(source, start, middle) <= units:
                best = middle
                low = middle + 1
            else:
                high = middle - 1
        return best

    def _require_streamable_measure(self) -> None:
        """Refuse to stream under a measure that needs the whole document.

        Streaming holds a bounded buffer by construction, so a whole-document
        measure has nothing correct to say about it. The failure mode without
        this guard is the dangerous kind: the buffer gets measured *as if* it
        were the document, every number looks reasonable, and chunks silently
        overflow the model budget. Refusing is the only honest answer, and it
        is the same answer ``TokenBudgetStrategy`` already gives by declaring
        ``supports_streaming = False``.
        """
        measure = getattr(self.config, "measure", None)
        if measure is not None and getattr(measure, "needs_document", False):
            raise ValueError(
                f"measure {getattr(measure, 'name', measure)!r} needs the whole "
                "document and cannot be used on the streaming path, which holds "
                "only a bounded buffer. Use chunk() instead, or configure the "
                "character measure for streaming."
            )

    def _measure_name(self) -> str:
        """Identity of the active measure, for metadata and pool keys."""
        measure = getattr(self.config, "measure", None)
        return "char" if measure is None else str(getattr(measure, "name", "custom"))

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
