"""
Recursive chunking strategy

Applies multiple strategies recursively to achieve optimal chunk sizes.
"""

from typing import Any

from .base import ChunkingConfig, ChunkingStrategyInterface, TextChunk
from .paragraph import ParagraphStrategy
from .sentence import SentenceStrategy
from .sliding_window import SlidingWindowStrategy


class RecursiveStrategy(ChunkingStrategyInterface):
    """
    Recursive chunking that tries different strategies in order

    Attempts to chunk using increasingly granular methods:
    1. Paragraph-based chunking
    2. Sentence-based chunking for large paragraphs
    3. Sliding window as last resort
    """

    def __init__(self, config: ChunkingConfig):
        super().__init__(config)

        # Initialize sub-strategies
        self.paragraph_strategy = ParagraphStrategy(config)
        self.sentence_strategy = SentenceStrategy(config)
        self.sliding_window_strategy = SlidingWindowStrategy(config)

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        """Create chunks using recursive strategy selection"""
        self.validate_config()

        if not text:
            return []

        base_metadata = base_metadata or {}

        # First attempt: paragraph-based chunking
        para_chunks = self.paragraph_strategy.chunk(text, source_id, base_metadata)

        # Check if any chunks are too large
        final_chunks = []
        chunk_index = 0

        for para_chunk in para_chunks:
            if len(para_chunk.text) > self.config.max_chunk_size:
                # Chunk is too large - apply sentence chunking
                sub_chunks = self._recursive_split(
                    para_chunk.text,
                    para_chunk.start_pos,
                    source_id,
                    chunk_index,
                    base_metadata,
                    parent_chunk_id=para_chunk.chunk_id,
                )
                final_chunks.extend(sub_chunks)
                chunk_index += len(sub_chunks)
            else:
                # Chunk is acceptable - update metadata
                para_chunk.metadata.update(
                    {
                        "chunk_type": "recursive",
                        "recursive_level": 1,
                        "strategy_used": "paragraph",
                    }
                )
                para_chunk.chunk_id = f"{source_id}_chunk_{chunk_index}"
                self.add_chunk_metadata(para_chunk, chunk_index, -1, "recursive")
                final_chunks.append(para_chunk)
                chunk_index += 1

        # Update total chunks count
        for chunk in final_chunks:
            chunk.metadata["total_chunks"] = len(final_chunks)

        return final_chunks

    def _recursive_split(
        self,
        text: str,
        start_pos: int,
        source_id: str,
        chunk_index: int,
        base_metadata: dict[str, Any],
        parent_chunk_id: str,
        level: int = 2,
    ) -> list[TextChunk]:
        """Recursively split text using finer-grained strategies"""
        chunks = []

        # Try sentence-based chunking
        if level == 2:
            # Create temporary config for sentence chunking
            sentence_chunks = self.sentence_strategy.chunk(
                text, f"{source_id}_temp", base_metadata
            )

            # Check if sentence chunks are still too large
            for i, sent_chunk in enumerate(sentence_chunks):
                if len(sent_chunk.text) > self.config.max_chunk_size:
                    # Still too large - use sliding window
                    sub_chunks = self._sliding_window_split(
                        sent_chunk.text,
                        start_pos + sent_chunk.start_pos,
                        source_id,
                        chunk_index + i,
                        base_metadata,
                        parent_chunk_id,
                    )
                    chunks.extend(sub_chunks)
                else:
                    # Sentence chunk is acceptable
                    chunk_metadata = {
                        **sent_chunk.metadata,
                        "chunk_type": "recursive",
                        "recursive_level": level,
                        "strategy_used": "sentence",
                        "parent_chunk": parent_chunk_id,
                    }

                    chunk = TextChunk(
                        text=sent_chunk.text,
                        start_pos=start_pos + sent_chunk.start_pos,
                        end_pos=start_pos + sent_chunk.end_pos,
                        chunk_id=f"{source_id}_chunk_{chunk_index + i}",
                        metadata=chunk_metadata,
                    )

                    self.add_chunk_metadata(chunk, chunk_index + i, -1, "recursive")
                    chunks.append(chunk)

        return chunks

    def _sliding_window_split(
        self,
        text: str,
        start_pos: int,
        source_id: str,
        chunk_index: int,
        base_metadata: dict[str, Any],
        parent_chunk_id: str,
    ) -> list[TextChunk]:
        """Final resort: sliding window split"""
        chunks = []

        # Use sliding window strategy
        window_chunks = self.sliding_window_strategy.chunk(
            text, f"{source_id}_temp", base_metadata
        )

        # Update chunk metadata
        for i, window_chunk in enumerate(window_chunks):
            chunk_metadata = {
                **window_chunk.metadata,
                "chunk_type": "recursive",
                "recursive_level": 3,
                "strategy_used": "sliding_window",
                "parent_chunk": parent_chunk_id,
                "forced_split": True,
            }

            chunk = TextChunk(
                text=window_chunk.text,
                start_pos=start_pos + window_chunk.start_pos,
                end_pos=start_pos + window_chunk.end_pos,
                chunk_id=f"{source_id}_chunk_{chunk_index}_{i}",
                metadata=chunk_metadata,
            )

            self.add_chunk_metadata(chunk, chunk_index, -1, "recursive")
            chunks.append(chunk)

        return chunks

    def __repr__(self) -> str:
        return (
            f"RecursiveStrategy("
            f"chunk_size={self.config.chunk_size}, "
            f"max_chunk_size={self.config.max_chunk_size})"
        )
