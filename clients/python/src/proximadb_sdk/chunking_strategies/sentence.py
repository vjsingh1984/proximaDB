"""
Sentence-based chunking strategy

Chunks text at sentence boundaries while respecting size constraints.
"""

import re
from typing import Any, Dict, List, Optional

from .base import ChunkingConfig, ChunkingStrategyInterface, TextChunk


class SentenceStrategy(ChunkingStrategyInterface):
    """
    Sentence-based chunking that preserves complete sentences

    Groups sentences together until reaching size limit
    """

    def __init__(self, config: ChunkingConfig):
        super().__init__(config)
        self._compile_patterns()

    def _compile_patterns(self):
        """Compile regex patterns for sentence detection"""
        # Basic sentence ending pattern
        endings = "|".join(re.escape(e) for e in self.config.sentence_endings)

        # Pattern for sentence boundaries
        # Handles abbreviations, decimals, etc.
        self.sentence_pattern = re.compile(
            rf"(?<=[{endings}])\s+(?=[A-Z])|"  # Standard sentence end
            rf"(?<=[{endings}])\s*\n+|"  # Sentence end with newline
            rf"\n\n+"  # Paragraph breaks
        )

        # Pattern for abbreviations to avoid false splits
        self.abbrev_pattern = re.compile(
            r"\b(?:Mr|Mrs|Ms|Dr|Prof|Sr|Jr|Inc|Ltd|Co|Corp|vs|etc|eg|ie|cf)\.$",
            re.IGNORECASE,
        )

    def _split_into_sentences(self, text: str) -> List[str]:
        """Split text into sentences"""
        # Initial split
        parts = self.sentence_pattern.split(text)

        # Clean up and merge incorrectly split sentences
        sentences = []
        current = ""

        for part in parts:
            part = part.strip()
            if not part:
                continue

            current += (" " if current else "") + part

            # Check if this is a complete sentence
            if self._is_sentence_end(current):
                sentences.append(current)
                current = ""

        # Add any remaining text
        if current:
            sentences.append(current)

        return sentences

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

    def chunk(
        self, text: str, source_id: str, base_metadata: Optional[Dict[str, Any]] = None
    ) -> List[TextChunk]:
        """Create chunks at sentence boundaries"""
        self.validate_config()

        if not text:
            return []

        base_metadata = base_metadata or {}
        chunks = []

        # Split into sentences
        sentences = self._split_into_sentences(text)
        if not sentences:
            return []

        # Group sentences into chunks
        current_chunk = []
        current_length = 0
        current_start = 0
        chunk_index = 0

        for i, sentence in enumerate(sentences):
            sentence_length = len(sentence)

            # Check if adding this sentence would exceed chunk size
            if (
                current_chunk
                and current_length + sentence_length + 1 > self.config.chunk_size
            ):
                # Create chunk from current sentences
                chunk_text = " ".join(current_chunk)

                if len(chunk_text) >= self.config.min_chunk_size:
                    chunk_metadata = {
                        **base_metadata,
                        "chunk_type": "sentence",
                        "sentence_count": len(current_chunk),
                        "first_sentence": (
                            current_chunk[0][:50] + "..."
                            if len(current_chunk[0]) > 50
                            else current_chunk[0]
                        ),
                    }

                    chunk = TextChunk(
                        text=chunk_text,
                        start_pos=current_start,
                        end_pos=current_start + len(chunk_text),
                        chunk_id=f"{source_id}_chunk_{chunk_index}",
                        metadata=chunk_metadata,
                    )

                    self.add_chunk_metadata(chunk, chunk_index, -1, "sentence")
                    chunks.append(chunk)
                    chunk_index += 1

                # Reset for next chunk
                current_chunk = []
                current_length = 0
                current_start += len(chunk_text) + 1

            # Add sentence to current chunk
            current_chunk.append(sentence)
            current_length += sentence_length + (1 if current_chunk else 0)

        # Handle remaining sentences
        if current_chunk:
            chunk_text = " ".join(current_chunk)

            if len(chunk_text) >= self.config.min_chunk_size or not chunks:
                chunk_metadata = {
                    **base_metadata,
                    "chunk_type": "sentence",
                    "sentence_count": len(current_chunk),
                    "first_sentence": (
                        current_chunk[0][:50] + "..."
                        if len(current_chunk[0]) > 50
                        else current_chunk[0]
                    ),
                }

                chunk = TextChunk(
                    text=chunk_text,
                    start_pos=current_start,
                    end_pos=current_start + len(chunk_text),
                    chunk_id=f"{source_id}_chunk_{chunk_index}",
                    metadata=chunk_metadata,
                )

                self.add_chunk_metadata(chunk, chunk_index, -1, "sentence")
                chunks.append(chunk)

        # Update total chunks count
        for chunk in chunks:
            chunk.metadata["total_chunks"] = len(chunks)

        return chunks

    def __repr__(self) -> str:
        return f"SentenceStrategy(chunk_size={self.config.chunk_size})"
