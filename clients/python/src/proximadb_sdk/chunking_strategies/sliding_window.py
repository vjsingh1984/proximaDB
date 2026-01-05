"""
Sliding window chunking strategy

Simple overlapping window approach for text chunking.
"""

from typing import List, Dict, Any, Optional
from .base import ChunkingStrategyInterface, TextChunk, ChunkingConfig


class SlidingWindowStrategy(ChunkingStrategyInterface):
    """
    Sliding window chunking with configurable overlap

    Most basic strategy - creates fixed-size chunks with overlap
    """

    def chunk(
        self, text: str, source_id: str, base_metadata: Optional[Dict[str, Any]] = None
    ) -> List[TextChunk]:
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

    def __repr__(self) -> str:
        return (
            f"SlidingWindowStrategy("
            f"chunk_size={self.config.chunk_size}, "
            f"overlap={self.config.chunk_overlap})"
        )
