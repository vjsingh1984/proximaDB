"""
Fixed-size chunking strategy implementation

Splits text into chunks of exactly the specified size without regard to
sentence or paragraph boundaries.
"""

from collections.abc import Iterable, Iterator
from typing import Any

from .base import OFFSET_CONTRACT_EXACT, ChunkingStrategyInterface, TextChunk
from .spans import is_empty, strip_span


class FixedSizeStrategy(ChunkingStrategyInterface):
    """
    Fixed-size chunking strategy

    Splits text into chunks of exactly the specified size.
    This is the simplest chunking strategy but may split words or sentences.
    """

    #: Boundaries are absolute multiples of ``chunk_size`` over the concatenated
    #: input, so a bounded buffer of ``chunk_size`` chars is enough to stream.
    supports_streaming = True

    #: Span-first: every chunk is a verbatim slice of the source.
    _offset_contract = OFFSET_CONTRACT_EXACT

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        """
        Split text into fixed-size chunks

        Args:
            text: The text to chunk
            source_id: Identifier for the source document
            base_metadata: Optional metadata to include with all chunks

        Returns:
            List of TextChunk objects
        """
        self.validate_config()

        if not text or not text.strip():
            return []

        chunks = []
        chunk_size = self.config.chunk_size
        text_length = len(text)

        # Create chunks of fixed size
        for window_start in range(0, text_length, chunk_size):
            window_end = min(window_start + chunk_size, text_length)
            is_final = window_end >= text_length

            # Narrow the SPAN rather than stripping the text: stripping the text
            # while keeping the raw window bounds is what made a chunk stop
            # equalling its own span.
            start_pos, end_pos = strip_span(text, window_start, window_end)
            if is_empty((start_pos, end_pos)):
                continue

            # Undersized windows are skipped, but never the last one — dropping
            # the tail silently loses content, and after the min<=chunk clamp in
            # ChunkingConfig the tail is the only window that can be undersized.
            if (
                self._size(text, start_pos, end_pos) < self.config.min_chunk_size
                and not is_final
            ):
                continue

            chunk_id = f"{source_id}_chunk_{len(chunks)}"

            chunk = TextChunk(
                text=text[start_pos:end_pos],
                start_pos=start_pos,
                end_pos=end_pos,
                chunk_id=chunk_id,
                metadata={
                    "source_id": source_id,
                    "chunk_type": "fixed_size",
                    "chunk_size": chunk_size,
                    **(base_metadata or {}),
                },
            )

            chunks.append(chunk)

        # Add standard metadata
        for i, chunk in enumerate(chunks):
            self.add_chunk_metadata(chunk, i, len(chunks), "fixed_size")

        return chunks

    def chunk_stream(
        self,
        text_source: "str | Iterable[str]",
        source_id: str,
        base_metadata: dict[str, Any] | None = None,
    ) -> Iterator[TextChunk]:
        """Incrementally yield fixed-size chunks with a bounded buffer.

        Equivalent to :meth:`chunk` for every chunk's text/offsets/id, except
        ``total_chunks`` (an inherently global count) is left as ``-1`` because
        it cannot be known without consuming the whole input — which would
        defeat bounded-memory streaming.

        Memory is bounded by at most ``chunk_size`` buffered characters plus
        whatever a single ``text_source`` piece carries.
        """
        self.validate_config()

        chunk_size = self.config.chunk_size
        min_chunk_size = self.config.min_chunk_size

        # `str` is itself iterable of 1-char strings; treat it as a single piece.
        pieces: Iterable[str]
        if isinstance(text_source, str):
            pieces = (text_source,) if text_source else ()
        else:
            pieces = text_source

        buffer = ""
        # Absolute start position (in the concatenated input) of buffer[0].
        buffer_origin = 0
        kept = 0

        def make_chunk(window_start: int, raw: str, is_final: bool) -> TextChunk | None:
            nonlocal kept
            # Narrow within the window, then slice — identical to the batch path
            # so text, offsets and ids agree piece-size for piece-size.
            local_start, local_end = strip_span(raw, 0, len(raw))
            if is_empty((local_start, local_end)):
                return None
            if (
                self._size(raw, local_start, local_end) < min_chunk_size
                and not is_final
            ):
                return None
            start_pos = window_start + local_start
            chunk = TextChunk(
                text=raw[local_start:local_end],
                start_pos=start_pos,
                end_pos=window_start + local_end,
                chunk_id=f"{source_id}_chunk_{kept}",
                metadata={
                    "source_id": source_id,
                    "chunk_type": "fixed_size",
                    "chunk_size": chunk_size,
                    **(base_metadata or {}),
                },
            )
            self.add_chunk_metadata(chunk, kept, -1, "fixed_size")
            kept += 1
            return chunk

        for piece in pieces:
            if not piece:
                continue
            buffer += piece
            # STRICTLY greater, so the window we emit is provably not the last:
            # the batch path knows which window is final and applies the
            # last-chunk escape to it, and retaining one window's worth here is
            # what lets the streaming path make the same call without lookahead.
            while len(buffer) > chunk_size:
                chunk = make_chunk(buffer_origin, buffer[:chunk_size], False)
                if chunk is not None:
                    yield chunk
                buffer = buffer[chunk_size:]
                buffer_origin += chunk_size

        # Flush the remainder — at most one window, and it IS the final one.
        if buffer:
            chunk = make_chunk(buffer_origin, buffer, True)
            if chunk is not None:
                yield chunk
