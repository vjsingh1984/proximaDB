"""Token-budget decorator for all boundary-selection strategies."""

from __future__ import annotations

import bisect
from collections.abc import Iterable, Iterator, Sequence
from typing import Any

from .base import ChunkingStrategyInterface, TextChunk
from .contracts import (
    CompositeInputContract,
    InputRole,
    OverflowPolicy,
    ResolvedInputContract,
    ShortChunkPolicy,
    TokenBudget,
    as_composite_contract,
)


class TokenBudgetStrategy(ChunkingStrategyInterface):
    """Apply an exact model-input budget over preferred structural boundaries.

    The wrapped strategy proposes useful end positions (sentences, paragraphs,
    code symbols, semantic spans). This decorator owns final coverage, overlap,
    splitting, and exact validation. It intentionally materializes the source;
    tokenizer-safe incremental streaming requires a separate boundary-state
    contract and is not claimed here.
    """

    supports_streaming = False

    def __init__(
        self,
        boundary_strategy: ChunkingStrategyInterface,
        budget: TokenBudget,
        input_contract: ResolvedInputContract | CompositeInputContract,
        *,
        role: InputRole = InputRole.DOCUMENT,
    ):
        super().__init__(boundary_strategy.config)
        self.boundary_strategy = boundary_strategy
        self.budget = budget
        self.input_contract = as_composite_contract(input_contract)
        self.role = role
        if budget.target_tokens > self.input_contract.minimum_context_limit:
            raise ValueError(
                f"target_tokens={budget.target_tokens} exceeds the smallest model "
                f"context {self.input_contract.minimum_context_limit}"
            )

    def _preferred_token_ends(
        self,
        text: str,
        source_id: str,
        base_metadata: dict[str, Any] | None,
        token_ends: Sequence[int],
    ) -> list[int]:
        preferred: set[int] = set()
        for end_pos in self.boundary_strategy.preferred_boundaries(
            text, source_id, base_metadata
        ):
            if not 0 < end_pos <= len(text):
                continue
            token_end = bisect.bisect_right(token_ends, end_pos)
            if token_end > 0:
                preferred.add(token_end)
        preferred.add(len(token_ends))
        return sorted(preferred)

    @staticmethod
    def _char_bounds(
        offsets: Sequence[tuple[int, int]],
        start_token: int,
        end_token: int,
        text_length: int,
    ) -> tuple[int, int]:
        start_char = 0 if start_token == 0 else offsets[start_token][0]
        end_char = (
            text_length if end_token == len(offsets) else offsets[end_token - 1][1]
        )
        return start_char, end_char

    def _greatest_fitting_end(
        self,
        text: str,
        offsets: Sequence[tuple[int, int]],
        start_token: int,
    ) -> int:
        low = start_token + 1
        high = min(len(offsets), start_token + self.budget.target_tokens)
        best: int | None = None
        while low <= high:
            middle = (low + high) // 2
            start_char, end_char = self._char_bounds(
                offsets, start_token, middle, len(text)
            )
            candidate = text[start_char:end_char]
            if self.input_contract.fits(
                candidate, self.role, self.budget.target_tokens
            ):
                best = middle
                low = middle + 1
            else:
                high = middle - 1
        if best is None:
            raise ValueError(
                "role prefix and tokenizer overhead leave no room for one source token "
                f"inside target_tokens={self.budget.target_tokens}"
            )
        return best

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        if not text or not text.strip():
            return []

        primary_counter = self.input_contract.primary.counter
        offsets = primary_counter.content_offsets(text)
        if offsets is None:
            raise ValueError(
                f"token counter {primary_counter.name} cannot provide source offsets"
            )
        offsets = tuple(offsets)
        if not offsets:
            return []

        whole_counts = self.input_contract.counts(text, self.role)
        if not self.input_contract.fits(text, self.role, self.budget.target_tokens):
            if self.budget.overflow_policy == OverflowPolicy.ERROR:
                raise ValueError(
                    f"source input token counts {whole_counts} exceed target "
                    f"{self.budget.target_tokens} or a model context limit"
                )
            if self.budget.overflow_policy == OverflowPolicy.DROP:
                return []

        token_ends = [end for _, end in offsets]
        preferred = self._preferred_token_ends(
            text, source_id, base_metadata, token_ends
        )
        chunks: list[TextChunk] = []
        start_token = 0
        while start_token < len(offsets):
            maximum_end = self._greatest_fitting_end(text, offsets, start_token)
            minimum_end = min(
                len(offsets), start_token + self.budget.min_content_tokens
            )
            candidate_boundaries = [
                end for end in preferred if minimum_end <= end <= maximum_end
            ]
            end_token = (
                candidate_boundaries[-1] if candidate_boundaries else maximum_end
            )
            content_tokens = end_token - start_token
            is_tail = end_token == len(offsets)
            if is_tail and content_tokens < self.budget.min_content_tokens:
                if self.budget.short_chunk_policy == ShortChunkPolicy.DROP:
                    break
                if self.budget.short_chunk_policy == ShortChunkPolicy.ERROR:
                    raise ValueError(
                        f"final source span has only {content_tokens} primary tokens"
                    )

            start_char, end_char = self._char_bounds(
                offsets, start_token, end_token, len(text)
            )
            chunk_text = text[start_char:end_char]
            counts = self.input_contract.validate(chunk_text, self.role)
            if any(count > self.budget.target_tokens for count in counts.values()):
                raise AssertionError(
                    "internal error: emitted chunk exceeds target budget"
                )

            index = len(chunks)
            metadata = {
                **(base_metadata or {}),
                "source_id": source_id,
                "chunk_type": "token_budget",
                "boundary_strategy": self.config.strategy.value,
                "length_unit": "tokens",
                "token_counts": counts,
                "primary_tokenizer": primary_counter.name,
                "primary_content_tokens": content_tokens,
                "token_budget": self.budget.to_manifest(),
                "input_role": self.role.value,
                "has_overlap": index > 0 and self.budget.overlap_tokens > 0,
                "overlap_tokens": self.budget.overlap_tokens if index > 0 else 0,
            }
            chunk = TextChunk(
                text=chunk_text,
                start_pos=start_char,
                end_pos=end_char,
                chunk_id=f"{source_id}_chunk_{index}",
                metadata=metadata,
            )
            self.add_chunk_metadata(chunk, index, -1, "token_budget")
            chunks.append(chunk)

            if is_tail:
                break
            next_start = end_token - self.budget.overlap_tokens
            start_token = max(start_token + 1, next_start)

        for chunk in chunks:
            chunk.metadata["total_chunks"] = len(chunks)
        return chunks

    def chunk_stream(
        self,
        text_source: str | Iterable[str],
        source_id: str,
        base_metadata: dict[str, Any] | None = None,
    ) -> Iterator[TextChunk]:
        text = text_source if isinstance(text_source, str) else "".join(text_source)
        yield from self.chunk(text, source_id, base_metadata)
