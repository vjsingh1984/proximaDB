"""
Sentence-based chunking strategy

Chunks text at sentence boundaries while respecting size constraints.
"""

import re
from collections.abc import Callable, Iterable, Iterator
from typing import Any

from .base import (
    OFFSET_CONTRACT_EXACT,
    ChunkingConfig,
    ChunkingStrategyInterface,
    TextChunk,
)
from .spans import (
    Slicer,
    Span,
    SpanBuffer,
    hard_split,
    is_empty,
    merge_spans,
    strip_span,
)


class SentenceStrategy(ChunkingStrategyInterface):
    """
    Sentence-based chunking that preserves complete sentences

    Groups sentences together until reaching size limit
    """

    #: Sentence boundaries are local; a buffer holding the current in-progress
    #: sentence plus the current group is enough to stream.
    supports_streaming = True

    #: Span-first: every chunk is a verbatim slice of the source.
    _offset_contract = OFFSET_CONTRACT_EXACT

    def __init__(self, config: ChunkingConfig):
        super().__init__(config)
        self._compile_patterns()

    def _compile_patterns(self):
        """Compile the sentence-boundary pattern.

        Two bugs lived in the old one-liner, both from interpolating
        ``"|".join(endings)`` into a CHARACTER CLASS:

        * ``|`` itself became a sentence terminator, and any multi-character
          ending the caller configured decomposed into its individual
          characters.
        * The ASCII branch required ``(?=[A-Z])`` after the terminator, so the
          ``。！？`` endings that ship in ``ChunkingConfig.sentence_endings``
          could never fire — 41 KB of Chinese came back as one chunk — and
          lowercase-initial sentences were missed in every script.

        Built instead as an alternation of per-ending lookbehinds (Python allows
        differing widths *across* alternatives, just not within one), split by
        script because the two need different right-hand context:

        * ASCII terminators must be followed by whitespace. That is what keeps
          ``1.5`` and ``0.90`` intact, and it is what ``(?=[A-Z])`` was actually
          buying — abbreviations are handled by :meth:`_is_sentence_end`.
        * Non-ASCII terminators are self-delimiting: CJK is not space-separated,
          so requiring whitespace would mean never splitting at all.
        """
        ascii_endings = [e for e in self.config.sentence_endings if e.isascii()]
        wide_endings = [e for e in self.config.sentence_endings if not e.isascii()]
        # Closing punctuation may sit between the terminator and the space.
        closers = r"[\"'\u201d\u2019)\]]*"

        alternatives: list[str] = []
        if ascii_endings:
            lookbehind = "|".join(f"(?<={re.escape(e)})" for e in ascii_endings)
            alternatives.append(rf"(?:{lookbehind}){closers}\s+")
        if wide_endings:
            lookbehind = "|".join(f"(?<={re.escape(e)})" for e in wide_endings)
            alternatives.append(rf"(?:{lookbehind}){closers}\s*")
        # A blank line ends a sentence whatever its punctuation — named so the
        # splitter can finalise on it even when _is_sentence_end says no.
        alternatives.append(r"(?P<para>\n\s*\n+)")

        self.sentence_pattern = re.compile("|".join(alternatives))

        # Pattern for abbreviations to avoid false splits
        self.abbrev_pattern = re.compile(
            r"\b(?:Mr|Mrs|Ms|Dr|Prof|Sr|Jr|Inc|Ltd|Co|Corp|vs|etc|eg|ie|cf)\.$",
            re.IGNORECASE,
        )

    def _sentence_spans(
        self,
        text: str,
        origin: int = 0,
        *,
        only_closed: bool = False,
        scan_from: int = 0,
    ) -> list[Span]:
        """Sentence spans over ``text``, offset by ``origin``.

        The span-first replacement for the old string accumulation
        (``current += (" " if current else "") + part``), which both destroyed
        positions — the root of every offset failure in this strategy — and was
        quadratic in the number of parts.

        ``only_closed`` drops a trailing group that no separator match closed.
        Streaming needs this: a span's stripped end sits BEFORE any trailing
        whitespace, so position alone cannot tell "sentence finished" from
        "sentence still accumulating" — ``"Alpha "`` would otherwise look like a
        complete sentence because its content ends at 5 while the buffer ends at
        6. Decidedness comes from having consumed a boundary, never from an
        offset comparison.

        ``scan_from`` restricts the scan to a suffix without slicing the string,
        so lookbehind assertions can still see the character before it. Streaming
        needs this: re-scanning the whole retained buffer on every piece is
        O(n^2) in the number of pieces, which on a document with no boundaries to
        release (nothing emitted, nothing trimmed) is quadratic in the document.
        """
        spans: list[Span] = []
        pending: list[Span] = []
        cursor = scan_from

        def flush() -> None:
            nonlocal pending
            if pending:
                merged = merge_spans(pending)
                spans.append((merged[0] + origin, merged[1] + origin))
                pending = []

        for match in self.sentence_pattern.finditer(text, scan_from):
            unit = strip_span(text, cursor, match.start())
            cursor = match.end()
            if not is_empty(unit):
                pending.append(unit)
            if not pending:
                continue
            # Slice ONCE per candidate, not once per part.
            candidate = text[pending[0][0] : pending[-1][1]]
            if match.group("para") is not None or self._is_sentence_end(candidate):
                flush()

        tail = strip_span(text, cursor, len(text))
        if not is_empty(tail):
            pending.append(tail)
        if only_closed:
            # No boundary closed this group; it may still grow.
            pending = []
        flush()
        return spans

    def _split_into_sentences(self, text: str) -> list[str]:
        """Text view over :meth:`_sentence_spans` (compat surface)."""
        return [text[start:end] for start, end in self._sentence_spans(text)]

    def _is_sentence_end(self, text: str) -> bool:
        """Check if text ends with a sentence ending"""
        text = text.rstrip()

        # Check for abbreviation
        if self.abbrev_pattern.search(text):
            return False

        # Check for sentence ending
        for ending in self.config.sentence_endings:
            if text.endswith(ending):
                return True

        return False

    def _group_sentences(
        self,
        sentences: Iterable[Span],
        slicer: Slicer,
        source_id: str,
        base_metadata: dict[str, Any],
        release: Callable[[int], None] = lambda _pos: None,
    ) -> Iterator[TextChunk]:
        """Group sentence spans into chunks (shared by batch + stream).

        Yields chunks with ``total_chunks`` left as ``-1`` — the batch path
        back-fills the real count, the streaming path cannot know it.
        """
        chunk_index = 0
        group: list[Span] = []

        def emit(spans: list[Span], forced: bool = False) -> TextChunk:
            nonlocal chunk_index
            start, end = merge_spans(spans)
            text = slicer(start, end)
            first = slicer(*spans[0])
            chunk = TextChunk(
                text=text,
                start_pos=start,
                end_pos=end,
                chunk_id=f"{source_id}_chunk_{chunk_index}",
                metadata={
                    **base_metadata,
                    "chunk_type": "sentence",
                    "sentence_count": len(spans),
                    "forced_split": forced,
                    "first_sentence": (
                        first[:50] + "..." if len(first) > 50 else first
                    ),
                },
            )
            self.add_chunk_metadata(chunk, chunk_index, -1, "sentence")
            chunk_index += 1
            release(end)
            return chunk

        for span in sentences:
            # A single sentence over the cap cannot join a group at all; split it
            # at the backstop first. The old guard consulted only the ACCUMULATED
            # group and then appended the incoming sentence unconditionally, so
            # one oversized sentence always shipped over the cap.
            if self._size_of_span(slicer, span) > self.config.max_chunk_size:
                if group:
                    yield emit(group)
                    group = []
                unit_text = slicer(span[0], span[1])
                for piece_start, piece_end in hard_split(
                    unit_text, 0, len(unit_text), self.config.max_chunk_size
                ):
                    yield emit([(span[0] + piece_start, span[0] + piece_end)], True)
                continue

            if group:
                group_start = group[0][0]
                if self._size(slicer, group_start, span[1]) > self.config.chunk_size:
                    current_length = self._size(slicer, group_start, group[-1][1])
                    fits_cap = (
                        self._size(slicer, group_start, span[1])
                        <= self.config.max_chunk_size
                    )
                    # Merge forward rather than drop: previously only the FINAL
                    # group had an escape hatch, so an undersized interior group
                    # was deleted outright.
                    if current_length >= self.config.min_chunk_size or not fits_cap:
                        yield emit(group)
                        group = []

            group.append(span)

        if group:
            yield emit(group)

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        """Create chunks at sentence boundaries"""
        self.validate_config()

        if not text:
            return []

        base_metadata = base_metadata or {}

        spans = self._sentence_spans(text)
        if not spans:
            return []

        chunks = list(
            self._group_sentences(
                spans, lambda a, b: text[a:b], source_id, base_metadata
            )
        )

        # Update total chunks count
        for chunk in chunks:
            chunk.metadata["total_chunks"] = len(chunks)

        return chunks

    def preferred_boundaries(
        self,
        text: str,
        source_id: str,
        base_metadata: dict[str, Any] | None = None,
    ) -> list[int]:
        """Expose every real sentence boundary without character grouping."""
        return [end for _start, end in self._sentence_spans(text)] + [len(text)]

    def chunk_stream(
        self,
        text_source: "str | Iterable[str]",
        source_id: str,
        base_metadata: dict[str, Any] | None = None,
    ) -> Iterator[TextChunk]:
        """Incrementally yield sentence chunks with a bounded buffer.

        Equivalent to :meth:`chunk` for every chunk's text/offsets/id, except
        ``total_chunks`` is left as ``-1`` (an inherently global count).
        """
        self.validate_config()

        base_metadata = base_metadata or {}
        buffer = SpanBuffer()
        yield from self._group_sentences(
            self._sentence_span_stream(text_source, buffer),
            buffer.slice,
            source_id,
            base_metadata,
            buffer.trim_to,
        )

    def _sentence_span_stream(
        self, text_source: "str | Iterable[str]", buffer: SpanBuffer
    ) -> Iterator[Span]:
        r"""Yield absolute sentence spans as boundaries become decided.

        The old ``_split_with_carry`` treated the END OF THE BUFFER as a decided
        sentence boundary, so chunk content depended on read granularity — it
        injected a space inside ``1.5`` and could emit ``end_pos`` past the end
        of the input. A separator that ends exactly at the buffer end may still
        grow with the next piece (``\s+`` is greedy, and the ASCII branch needs a
        character after the terminator to exist at all), so it is held back.
        """
        pieces: Iterable[str]
        if isinstance(text_source, str):
            pieces = (text_source,) if text_source else ()
        else:
            pieces = text_source

        emitted_to = 0
        # Scanning costs O(retained buffer), so scanning on EVERY piece is
        # quadratic in the number of pieces when there is no boundary to release
        # (nothing emitted => nothing trimmed => the buffer only grows). Coalesce
        # scans instead. Output is unaffected: the final drain is unconditional,
        # so a boundary is never lost, only noticed slightly later.
        scan_stride = max(64, self.config.chunk_size // 4)
        scanned_len = 0

        def drain(final: bool) -> Iterator[Span]:
            nonlocal emitted_to
            text = buffer.buffer
            origin = buffer.origin
            # Everything before `emitted_to` is already decided; re-scanning it
            # on every piece is what makes naive streaming quadratic.
            scan_from = max(0, emitted_to - origin)
            for span in self._sentence_spans(
                text, origin, only_closed=not final, scan_from=scan_from
            ):
                if span[0] >= emitted_to:
                    emitted_to = span[1]
                    yield span

        for piece in pieces:
            if not piece:
                continue
            buffer.append(piece)
            if len(buffer.buffer) - scanned_len < scan_stride:
                continue
            scanned_len = len(buffer.buffer)
            yield from drain(final=False)

        yield from drain(final=True)

    def __repr__(self) -> str:
        return f"SentenceStrategy(chunk_size={self.config.chunk_size})"
