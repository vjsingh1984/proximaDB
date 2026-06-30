"""
Fixed-size chunking strategy implementation

Splits text into chunks of exactly the specified size without regard to
sentence or paragraph boundaries.
"""

from collections.abc import Iterable, Iterator
from typing import Any

from .base import ChunkingStrategyInterface, TextChunk, _coalesce_text_source


class FixedSizeStrategy(ChunkingStrategyInterface):
    """
    Fixed-size chunking strategy

    Splits text into chunks of exactly the specified size.
    This is the simplest chunking strategy but may split words or sentences.
    """

    #: Boundaries are absolute multiples of ``chunk_size`` over the concatenated
    #: input, so a bounded buffer of ``chunk_size`` chars is enough to stream.
    supports_streaming = True

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
        for start_pos in range(0, text_length, chunk_size):
            end_pos = min(start_pos + chunk_size, text_length)
            chunk_text = text[start_pos:end_pos].strip()

            # Skip chunks that are too small
            if len(chunk_text) < self.config.min_chunk_size:
                continue

            # Create chunk
            chunk_id = f"{source_id}_chunk_{len(chunks)}"

            chunk = TextChunk(
                text=chunk_text,
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

        def make_chunk(start_pos: int, raw: str) -> TextChunk | None:
            nonlocal kept
            end_pos = start_pos + len(raw)
            chunk_text = raw.strip()
            if len(chunk_text) < min_chunk_size:
                return None
            chunk = TextChunk(
                text=chunk_text,
                start_pos=start_pos,
                end_pos=end_pos,
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
            # Emit every full window currently available in the buffer.
            while len(buffer) >= chunk_size:
                raw = buffer[:chunk_size]
                chunk = make_chunk(buffer_origin, raw)
                if chunk is not None:
                    yield chunk
                buffer = buffer[chunk_size:]
                buffer_origin += chunk_size

        # Flush the final partial window (batch slices the tail too).
        if buffer:
            chunk = make_chunk(buffer_origin, buffer)
            if chunk is not None:
                yield chunk
