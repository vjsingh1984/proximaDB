"""
Fixed-size chunking strategy implementation

Splits text into chunks of exactly the specified size without regard to
sentence or paragraph boundaries.
"""

from typing import List, Dict, Any, Optional

from .base import ChunkingStrategyInterface, ChunkingConfig, TextChunk


class FixedSizeStrategy(ChunkingStrategyInterface):
    """
    Fixed-size chunking strategy

    Splits text into chunks of exactly the specified size.
    This is the simplest chunking strategy but may split words or sentences.
    """

    def chunk(
        self, text: str, source_id: str, base_metadata: Optional[Dict[str, Any]] = None
    ) -> List[TextChunk]:
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
