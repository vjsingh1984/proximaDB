"""
Sliding window chunking strategy

Simple overlapping window approach for text chunking.
"""

from collections.abc import Iterable, Iterator
from typing import Any

from .base import ChunkingStrategyInterface, TextChunk


class SlidingWindowStrategy(ChunkingStrategyInterface):
    """
    Sliding window chunking with configurable overlap

    Most basic strategy - creates fixed-size chunks with overlap
    """

    #: Windows are boundary-local: a buffer holding the current window plus the
    #: overlap carry-over (<= chunk_size chars) is enough to stream.
    supports_streaming = True

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        """Create chunks using sliding window approach"""
        self.validate_config()

        if not text or not text.strip():
            return []

        base_metadata = base_metadata or {}
        chunks = []

        # Calculate step size (chunk_size - overlap)
        step_size = max(1, self.config.chunk_size - self.config.chunk_overlap)

        # Create chunks
        position = 0
        chunk_index = 0

        while position < len(text):
            # Calculate chunk boundaries
            start_pos = position
            end_pos = min(position + self.config.chunk_size, len(text))

            # Extract chunk text
            chunk_text = text[start_pos:end_pos]

            # Skip if chunk is too small (unless it's the last chunk)
            if len(chunk_text) < self.config.min_chunk_size and end_pos < len(text):
                position += step_size
                continue

            # Create chunk
            chunk_metadata = {
                **base_metadata,
                "chunk_type": "sliding_window",
                "has_overlap": chunk_index > 0 and self.config.chunk_overlap > 0,
                "overlap_size": self.config.chunk_overlap if chunk_index > 0 else 0,
            }

            chunk = TextChunk(
                text=chunk_text,
                start_pos=start_pos,
                end_pos=end_pos,
                chunk_id=f"{source_id}_chunk_{chunk_index}",
                metadata=chunk_metadata,
            )

            # Add standard metadata
            self.add_chunk_metadata(chunk, chunk_index, -1, "sliding_window")

            chunks.append(chunk)
            chunk_index += 1

            # Move to next position
            position += step_size

            # Break if we've reached the end
            if end_pos >= len(text):
                break

        # Update total chunks count
        for chunk in chunks:
            chunk.metadata["total_chunks"] = len(chunks)

        return chunks

    def chunk_stream(
        self,
        text_source: "str | Iterable[str]",
        source_id: str,
        base_metadata: dict[str, Any] | None = None,
    ) -> Iterator[TextChunk]:
        """Incrementally yield sliding-window chunks with a bounded buffer.

        Equivalent to :meth:`chunk` for every chunk's text/offsets/id/overlap
        metadata, except ``total_chunks`` is left as ``-1`` (an inherently
        global count that cannot be known without consuming the whole input).

        Overlap carry-over: the buffer retains everything from the next window's
        start position onward, so the ``chunk_overlap`` characters shared by
        consecutive windows are preserved across ``text_source`` piece
        boundaries. Memory is bounded by at most ``chunk_size`` buffered
        characters plus a single piece.
        """
        self.validate_config()

        base_metadata = base_metadata or {}
        chunk_size = self.config.chunk_size
        min_chunk_size = self.config.min_chunk_size
        step_size = max(1, chunk_size - self.config.chunk_overlap)

        pieces: Iterable[str]
        if isinstance(text_source, str):
            pieces = (text_source,) if text_source else ()
        else:
            pieces = text_source

        # `buffer` holds text starting at absolute index `buffer_origin`.
        buffer = ""
        buffer_origin = 0
        position = 0  # absolute start of the next window
        chunk_index = 0
        done = False

        def build_chunk(start_pos: int, raw: str) -> TextChunk:
            nonlocal chunk_index
            chunk_metadata = {
                **base_metadata,
                "chunk_type": "sliding_window",
                "has_overlap": chunk_index > 0 and self.config.chunk_overlap > 0,
                "overlap_size": self.config.chunk_overlap if chunk_index > 0 else 0,
            }
            chunk = TextChunk(
                text=raw,
                start_pos=start_pos,
                end_pos=start_pos + len(raw),
                chunk_id=f"{source_id}_chunk_{chunk_index}",
                metadata=chunk_metadata,
            )
            self.add_chunk_metadata(chunk, chunk_index, -1, "sliding_window")
            chunk_index += 1
            return chunk

        def trim_to(pos: int) -> None:
            """Discard buffer prefix before absolute index `pos`."""
            nonlocal buffer, buffer_origin
            drop = pos - buffer_origin
            if drop > 0:
                buffer = buffer[drop:]
                buffer_origin += drop

        def emit_ready(at_eof: bool) -> Iterator[TextChunk]:
            """Emit every window whose full extent is available (or, at EOF, the tail).

            Mirrors batch exactly: a window is *final* (loop break) when its end
            reaches the end of input. At EOF the buffer end is the input end, so
            ``position + chunk_size >= total`` marks the last window; before EOF
            a full window can never be known-final, so we wait for more input
            unless the window is already complete.
            """
            nonlocal position, done
            total_so_far = buffer_origin + len(buffer)
            while not done and position < total_so_far:
                rel = position - buffer_origin
                available = total_so_far - position  # chars from position to buffer end
                # Pre-EOF we need strictly MORE than chunk_size available so we
                # can prove this window is not the final one (batch breaks when a
                # window reaches the input end). With exactly chunk_size and no
                # EOF signal yet, the window might be the last — defer it.
                if not at_eof and available <= chunk_size:
                    break  # window not provably complete-and-non-final yet
                raw = buffer[rel : rel + chunk_size]
                # At EOF the buffer end is the true input end. A window is final
                # when its (clamped) end reaches the input end.
                is_final = at_eof and (position + chunk_size >= total_so_far)
                if len(raw) < min_chunk_size and not is_final:
                    position += step_size
                    trim_to(position)
                    continue
                yield build_chunk(position, raw)
                position += step_size
                trim_to(position)
                if is_final:
                    done = True
                    return

        for piece in pieces:
            if done:
                break
            if not piece:
                continue
            buffer += piece
            yield from emit_ready(at_eof=False)

        if not done:
            yield from emit_ready(at_eof=True)

    def __repr__(self) -> str:
        return (
            f"SlidingWindowStrategy("
            f"chunk_size={self.config.chunk_size}, "
            f"overlap={self.config.chunk_overlap})"
        )
