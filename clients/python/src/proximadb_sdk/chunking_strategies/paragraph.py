"""
Paragraph-based chunking strategy

Chunks text at paragraph boundaries while respecting size constraints.
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


class ParagraphStrategy(ChunkingStrategyInterface):
    """
    Paragraph-based chunking that preserves paragraph structure

    Keeps paragraphs together when possible, splits large paragraphs if needed
    """

    #: Paragraph boundaries (blank lines) are local; a buffer holding the
    #: current paragraph group plus the in-progress paragraph is enough.
    supports_streaming = True

    #: Span-first: every chunk is a verbatim slice of the source.
    _offset_contract = OFFSET_CONTRACT_EXACT

    def __init__(self, config: ChunkingConfig):
        super().__init__(config)
        self._compile_patterns()

    def _compile_patterns(self):
        """Compile regex patterns for paragraph detection"""
        # Pattern for paragraph boundaries
        self.paragraph_pattern = re.compile(r"\n\s*\n+")

        # Pattern for list items
        self.list_pattern = re.compile(r"^\s*[\-\*\+•]\s+", re.MULTILINE)

        # Pattern for numbered lists
        self.numbered_list_pattern = re.compile(r"^\s*\d+[\.\)]\s+", re.MULTILINE)

    def _paragraph_spans(self, text: str, origin: int = 0) -> list[Span]:
        """Paragraph spans, derived from the separator matches themselves.

        Replaces the old ``text.find(stripped_part, cursor)`` search: `find` is
        O(n*m) over a large document — one of the two quadratic paths the
        ``whitespace_heavy`` corpus entry exists to catch — and it can only
        recover a position for text it already reconstructed. Deriving the span
        directly from ``finditer`` is both linear and exact.
        """
        spans: list[Span] = []
        cursor = 0
        for match in self.paragraph_pattern.finditer(text):
            span = strip_span(text, cursor, match.start())
            if not is_empty(span):
                spans.append((span[0] + origin, span[1] + origin))
            cursor = match.end()
        tail = strip_span(text, cursor, len(text))
        if not is_empty(tail):
            spans.append((tail[0] + origin, tail[1] + origin))
        return spans

    def _split_into_paragraphs(self, text: str) -> list[tuple[str, int]]:
        """Split text into paragraphs with positions.

        Retained as the ``(text, start)`` view over :meth:`_paragraph_spans` for
        callers and tests that predate the span-first migration.
        """
        return [(text[start:end], start) for start, end in self._paragraph_spans(text)]

    def _is_list_paragraph(self, text: str) -> bool:
        """Check if paragraph is a list"""
        lines = text.strip().split("\n")
        if len(lines) < 2:
            return False

        # Check if most lines are list items
        list_lines = sum(
            1
            for line in lines
            if self.list_pattern.match(line) or self.numbered_list_pattern.match(line)
        )

        return list_lines >= len(lines) * 0.7

    def _split_large_paragraph_spans(
        self, slicer: Slicer, start: int, end: int, max_size: int
    ) -> list[tuple[Span, bool]]:
        """Split one oversized paragraph into (span, forced) of at most ``max_size``.

        Sentence boundaries first, then :func:`hard_split` as the terminal
        backstop so a paragraph with no sentence boundary at all (minified
        content, CJK without terminators) still cannot exit over the cap. The
        old version grouped to ``chunk_size`` with no cap check and rebuilt text
        with ``" ".join``, then guessed offsets by advancing a cursor by the
        rejoined length — the single worst offset site in this file.
        """
        text = slicer(start, end)
        if self._size(slicer, start, end) <= max_size:
            return [((start, end), False)]

        endings = "".join(re.escape(e) for e in self.config.sentence_endings)
        sentence_pattern = re.compile(rf"(?<=[{endings}])\s+")

        units: list[Span] = []
        cursor = 0
        for match in sentence_pattern.finditer(text):
            span = strip_span(text, cursor, match.start())
            if not is_empty(span):
                units.append(span)
            cursor = match.end()
        tail = strip_span(text, cursor, len(text))
        if not is_empty(tail):
            units.append(tail)
        if not units:
            units = [(0, len(text))]

        out: list[Span] = []
        group: list[Span] = []
        for unit in units:
            if group and self._size(text, group[0][0], unit[1]) > max_size:
                out.append(merge_spans(group))
                group = []
            group.append(unit)
        if group:
            out.append(merge_spans(group))

        # Absolutize, enforcing the cap on anything a sentence split left over.
        # `forced` marks a cut made with no boundary to honour, so the trace's
        # boundary histogram shows how often the backstop actually fires.
        capped: list[tuple[Span, bool]] = []
        for local_start, local_end in out:
            if self._size(text, local_start, local_end) <= max_size:
                capped.append(((start + local_start, start + local_end), False))
                continue
            for piece_start, piece_end in hard_split(
                text, local_start, local_end, max_size
            ):
                capped.append(((start + piece_start, start + piece_end), True))
        return capped

    def _split_large_paragraph(self, text: str, max_size: int) -> list[str]:
        """Text view over :meth:`_split_large_paragraph_spans` (compat)."""
        return [
            text[span[0] : span[1]]
            for span, _forced in self._split_large_paragraph_spans(
                lambda a, b: text[a:b], 0, len(text), max_size
            )
        ]

    def _group_paragraphs(
        self,
        paragraphs: Iterable[Span],
        slicer: Slicer,
        source_id: str,
        base_metadata: dict[str, Any],
        release: Callable[[int], None] = lambda _pos: None,
    ) -> Iterator[TextChunk]:
        """Group a stream of (paragraph, abs_start) into chunks.

        Shared by batch :meth:`chunk` and streaming :meth:`chunk_stream`. Yields
        chunks with ``total_chunks`` left as ``-1``; the batch path back-fills
        the count, the streaming path cannot know it.
        """
        chunk_index = 0
        group: list[Span] = []

        def emit(
            spans: list[Span], is_list: bool = False, forced: bool = False
        ) -> TextChunk:
            nonlocal chunk_index
            start, end = merge_spans(spans)
            chunk = self._create_chunk(
                slicer(start, end),
                start,
                chunk_index,
                source_id,
                base_metadata,
                len(spans),
                is_list,
                forced,
            )
            chunk_index += 1
            release(end)
            return chunk

        for span in paragraphs:
            para_start, para_end = span
            para_length = self._size(slicer, para_start, para_end)

            # A paragraph larger than the cap cannot join a group at all.
            if para_length > self.config.max_chunk_size:
                if group:
                    yield emit(group)
                    group = []
                is_list = self._is_list_paragraph(slicer(para_start, para_end))
                for sub, forced in self._split_large_paragraph_spans(
                    slicer, para_start, para_end, self.config.chunk_size
                ):
                    yield emit([sub], is_list, forced)
                continue

            # Flush when adding this paragraph would exceed chunk_size — but
            # only if what we already have clears the floor. Otherwise keep
            # accumulating: an undersized group MERGES FORWARD rather than being
            # dropped, which is what previously deleted interior content
            # outright (only the final group had an escape hatch).
            if group:
                group_start = group[0][0]
                if self._size(slicer, group_start, para_end) > self.config.chunk_size:
                    current_length = self._size(slicer, group_start, group[-1][1])
                    fits_cap = (
                        self._size(slicer, group_start, para_end)
                        <= self.config.max_chunk_size
                    )
                    if current_length >= self.config.min_chunk_size or not fits_cap:
                        yield emit(group)
                        group = []

            group.append(span)

        # Never drop the remainder — emit it short rather than lose it.
        if group:
            yield emit(group)

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        """Create chunks at paragraph boundaries"""
        self.validate_config()

        if not text:
            return []

        base_metadata = base_metadata or {}

        # Split into paragraphs
        spans = self._paragraph_spans(text)
        if not spans:
            return []

        chunks = list(
            self._group_paragraphs(
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
        """Expose every paragraph end without legacy character grouping."""
        return [
            *(match.start() for match in self.paragraph_pattern.finditer(text)),
            len(text),
        ]

    def chunk_stream(
        self,
        text_source: "str | Iterable[str]",
        source_id: str,
        base_metadata: dict[str, Any] | None = None,
    ) -> Iterator[TextChunk]:
        """Incrementally yield paragraph chunks with a bounded buffer.

        Equivalent to :meth:`chunk` for every chunk's text/offsets/id, except
        ``total_chunks`` is left as ``-1`` (an inherently global count).

        Paragraph boundaries (blank lines) are detected in a growing buffer;
        each completed paragraph (with its absolute start offset in the
        concatenated input) is committed to the grouping engine, and the
        trailing partial paragraph is carried over. Memory is bounded by the
        current paragraph group plus the in-progress paragraph.
        """
        self.validate_config()

        base_metadata = base_metadata or {}
        buffer = SpanBuffer()
        # The grouper releases only up to the end of a chunk it has just
        # emitted, so the buffer always still holds the current group. That is
        # what lets streaming slice `source[group_span]` — the same operation
        # batch performs — instead of rejoining paragraph texts.
        yield from self._group_paragraphs(
            self._paragraph_span_stream(text_source, buffer),
            buffer.slice,
            source_id,
            base_metadata,
            buffer.trim_to,
        )

    def _paragraph_span_stream(
        self, text_source: "str | Iterable[str]", buffer: SpanBuffer
    ) -> Iterator[Span]:
        """Yield absolute paragraph spans as boundaries are confirmed.

        Feeds the SAME grouping engine the batch path uses, so the two cannot
        disagree. Only the trailing paragraph is held back — a separator match
        that ends exactly at the buffer end may still grow with the next piece,
        so it is not yet a decided boundary.
        """
        pieces: Iterable[str]
        if isinstance(text_source, str):
            pieces = (text_source,) if text_source else ()
        else:
            pieces = text_source

        # Absolute offset up to which spans have already been emitted.
        emitted_to = 0

        def drain(final: bool) -> Iterator[Span]:
            nonlocal emitted_to
            text = buffer.buffer
            origin = buffer.origin
            last_end = 0
            for match in self.paragraph_pattern.finditer(text):
                if not final and match.end() == len(text):
                    # The separator may extend with the next piece; not decided.
                    break
                span = strip_span(text, last_end, match.start())
                last_end = match.end()
                if is_empty(span):
                    continue
                absolute = (span[0] + origin, span[1] + origin)
                if absolute[0] >= emitted_to:
                    emitted_to = absolute[1]
                    yield absolute
            if final:
                tail = strip_span(text, last_end, len(text))
                if not is_empty(tail):
                    absolute = (tail[0] + origin, tail[1] + origin)
                    if absolute[0] >= emitted_to:
                        emitted_to = absolute[1]
                        yield absolute

        for piece in pieces:
            if not piece:
                continue
            buffer.append(piece)
            yield from drain(final=False)

        yield from drain(final=True)

    def _create_chunk(
        self,
        text: str,
        start_pos: int,
        chunk_index: int,
        source_id: str,
        base_metadata: dict[str, Any],
        paragraph_count: int,
        is_list: bool = False,
        forced_split: bool = False,
    ) -> TextChunk:
        """Create a chunk with metadata"""
        chunk_metadata = {
            **base_metadata,
            "chunk_type": "paragraph",
            "paragraph_count": paragraph_count,
            "is_list": is_list,
            "forced_split": forced_split,
            "first_line": (
                text.split("\n")[0][:50] + "..."
                if len(text.split("\n")[0]) > 50
                else text.split("\n")[0]
            ),
        }

        chunk = TextChunk(
            text=text,
            start_pos=start_pos,
            end_pos=start_pos + len(text),
            chunk_id=f"{source_id}_chunk_{chunk_index}",
            metadata=chunk_metadata,
        )

        self.add_chunk_metadata(chunk, chunk_index, -1, "paragraph")
        return chunk

    def __repr__(self) -> str:
        return f"ParagraphStrategy(chunk_size={self.config.chunk_size})"
