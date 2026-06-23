"""
Sentence-based chunking strategy

Chunks text at sentence boundaries while respecting size constraints.
"""

import re
from collections.abc import Iterable, Iterator
from typing import Any

from .base import ChunkingConfig, ChunkingStrategyInterface, TextChunk


class SentenceStrategy(ChunkingStrategyInterface):
    """
    Sentence-based chunking that preserves complete sentences

    Groups sentences together until reaching size limit
    """

    #: Sentence boundaries are local; a buffer holding the current in-progress
    #: sentence plus the current group is enough to stream.
    supports_streaming = True

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

    def _split_into_sentences(self, text: str) -> list[str]:
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

    def _split_with_carry(self, buffer: str) -> tuple[list[str], int]:
        """Split a raw buffer into finalized sentences + the raw-carry offset.

        Returns ``(finalized_sentences, carry_start)`` where ``carry_start`` is
        the index in ``buffer`` at which the still-open (unfinalized) sentence
        group begins — i.e. the RAW text ``buffer[carry_start:]`` must be carried
        forward unprocessed so inter-piece whitespace is preserved exactly. If
        the buffer ends on a finalized sentence boundary, ``carry_start`` is
        ``len(buffer)``.

        The finalization logic mirrors :meth:`_split_into_sentences` exactly; we
        additionally track the raw span of each regex part so the carry is a
        verbatim slice of ``buffer`` (not a normalized rejoin).
        """
        # Raw part spans: text between successive sentence-boundary matches.
        spans: list[tuple[int, int]] = []
        prev = 0
        for m in self.sentence_pattern.finditer(buffer):
            spans.append((prev, m.start()))
            prev = m.end()
        spans.append((prev, len(buffer)))

        sentences: list[str] = []
        current = ""
        group_start: int | None = None  # raw start of the open group
        for start, end in spans:
            part = buffer[start:end].strip()
            if not part:
                continue
            if group_start is None:
                group_start = start
            current += (" " if current else "") + part
            if self._is_sentence_end(current):
                sentences.append(current)
                current = ""
                group_start = None

        carry_start = group_start if group_start is not None else len(buffer)
        return sentences, carry_start

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
        sentences: Iterable[str],
        source_id: str,
        base_metadata: dict[str, Any],
    ) -> Iterator[TextChunk]:
        """Group a stream of sentences into chunks (shared by batch + stream).

        Yields chunks with ``total_chunks`` left as ``-1`` — the batch path
        back-fills the real count, the streaming path cannot know it.
        """
        current_chunk: list[str] = []
        current_length = 0
        current_start = 0
        chunk_index = 0

        def build(
            chunk_text: str, start: int, index: int, sentences_in_chunk: list[str]
        ) -> TextChunk:
            first = sentences_in_chunk[0]
            chunk_metadata = {
                **base_metadata,
                "chunk_type": "sentence",
                "sentence_count": len(sentences_in_chunk),
                "first_sentence": (first[:50] + "..." if len(first) > 50 else first),
            }
            chunk = TextChunk(
                text=chunk_text,
                start_pos=start,
                end_pos=start + len(chunk_text),
                chunk_id=f"{source_id}_chunk_{index}",
                metadata=chunk_metadata,
            )
            self.add_chunk_metadata(chunk, index, -1, "sentence")
            return chunk

        for sentence in sentences:
            sentence_length = len(sentence)

            if (
                current_chunk
                and current_length + sentence_length + 1 > self.config.chunk_size
            ):
                chunk_text = " ".join(current_chunk)
                if len(chunk_text) >= self.config.min_chunk_size:
                    yield build(chunk_text, current_start, chunk_index, current_chunk)
                    chunk_index += 1

                current_chunk = []
                current_length = 0
                current_start += len(chunk_text) + 1

            current_chunk.append(sentence)
            current_length += sentence_length + (1 if current_chunk else 0)

        if current_chunk:
            chunk_text = " ".join(current_chunk)
            if len(chunk_text) >= self.config.min_chunk_size or chunk_index == 0:
                yield build(chunk_text, current_start, chunk_index, current_chunk)

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        """Create chunks at sentence boundaries"""
        self.validate_config()

        if not text:
            return []

        base_metadata = base_metadata or {}

        sentences = self._split_into_sentences(text)
        if not sentences:
            return []

        chunks = list(self._group_sentences(sentences, source_id, base_metadata))

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
        """Incrementally yield sentence chunks with a bounded buffer.

        Equivalent to :meth:`chunk` for every chunk's text/offsets/id, except
        ``total_chunks`` is left as ``-1`` (an inherently global count).

        The raw input buffer is split into sentences as text arrives; every
        sentence except the last (which may still grow) is committed to the
        grouping engine, and the trailing partial sentence is carried over to
        the next ``text_source`` piece. Memory is bounded by the current group
        plus the in-progress sentence.
        """
        self.validate_config()

        base_metadata = base_metadata or {}

        pieces: Iterable[str]
        if isinstance(text_source, str):
            pieces = (text_source,) if text_source else ()
        else:
            pieces = text_source

        def sentence_stream() -> Iterator[str]:
            buffer = ""
            for piece in pieces:
                if not piece:
                    continue
                buffer += piece
                finalized, carry_start = self._split_with_carry(buffer)
                yield from finalized
                # Carry the RAW remainder verbatim so inter-piece whitespace is
                # preserved exactly (matches feeding the whole text at once).
                buffer = buffer[carry_start:]
            # Flush the final carried fragment(s) at EOF (matches batch's
            # trailing-`current` append for text that never closed a sentence).
            yield from self._split_into_sentences(buffer)

        yield from self._group_sentences(sentence_stream(), source_id, base_metadata)

    def __repr__(self) -> str:
        return f"SentenceStrategy(chunk_size={self.config.chunk_size})"
