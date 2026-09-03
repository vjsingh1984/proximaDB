"""Token-budget decorator for all boundary-selection strategies."""

from __future__ import annotations

import bisect
from collections.abc import Iterable, Iterator, Sequence
from typing import Any

from .base import OFFSET_CONTRACT_EXACT, ChunkingStrategyInterface, TextChunk
from .boundaries import StrategyBoundarySource
from .contracts import (
    ChunkContextRenderer,
    CompositeInputContract,
    InputRole,
    OverflowPolicy,
    ResolvedInputContract,
    ShortChunkPolicy,
    TokenBudget,
    as_composite_contract,
)
from .measures import TokenMeasure


class TokenBudgetStrategy(ChunkingStrategyInterface):
    """Apply an exact model-input budget over preferred structural boundaries.

    The wrapped strategy proposes useful end positions (sentences, paragraphs,
    code symbols, semantic spans). This decorator owns final coverage, overlap,
    splitting, and exact validation. It intentionally materializes the source;
    tokenizer-safe incremental streaming requires a separate boundary-state
    contract and is not claimed here.
    """

    supports_streaming = False

    #: It slices the ORIGINAL text (`text[start_char:end_char]`), so its offsets
    #: are exact; it was inheriting the ``legacy`` default and under-promising.
    _offset_contract = OFFSET_CONTRACT_EXACT

    def __init__(
        self,
        boundary_strategy: ChunkingStrategyInterface,
        budget: TokenBudget,
        input_contract: ResolvedInputContract | CompositeInputContract,
        *,
        role: InputRole = InputRole.DOCUMENT,
        boundary_source: Any | None = None,
        context_renderer: ChunkContextRenderer | None = None,
    ):
        super().__init__(boundary_strategy.config)
        self.boundary_strategy = boundary_strategy
        # ADR-091 D2: this class IS the segmenter, and a segmenter consumes
        # boundary *candidates* -- it should not care which component produced
        # them or how many did. `boundary_source` is that seam; when it is None
        # the wrapped strategy is adapted into a source, so the existing
        # single-strategy path is the degenerate case of the general one rather
        # than a parallel code path.
        self.boundary_source = boundary_source or StrategyBoundarySource(
            boundary_strategy
        )
        self.budget = budget
        self.input_contract = as_composite_contract(input_contract)
        self.role = role
        self.context_renderer = context_renderer
        self._boundary_meaning: dict[int, tuple[int, str, dict]] = {}
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
        self._boundary_meaning = {}
        for boundary in self.boundary_source.boundaries(
            text, source_id=source_id, base_metadata=base_metadata
        ):
            end_pos = boundary.end
            if not 0 < end_pos <= len(text):
                continue
            token_end = bisect.bisect_right(token_ends, end_pos)
            if token_end > 0:
                preferred.add(token_end)
                # Keep the strongest meaning that maps to this token boundary:
                # several character offsets can collapse onto one token index,
                # and the informative one should survive the collapse.
                previous = self._boundary_meaning.get(token_end)
                if previous is None or boundary.strength > previous[0]:
                    self._boundary_meaning[token_end] = (
                        boundary.strength,
                        boundary.kind.value,
                        dict(boundary.meaning),
                    )
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
        base_metadata: dict[str, Any] | None,
    ) -> int:
        low = start_token + 1
        high = min(len(offsets), start_token + self.budget.target_tokens)
        best: int | None = None
        while low <= high:
            middle = (low + high) // 2
            start_char, end_char = self._char_bounds(
                offsets, start_token, middle, len(text)
            )
            candidate = self._render_input(text[start_char:end_char], base_metadata)
            if self.input_contract.fits(
                candidate, self.role, self.budget.target_tokens
            ):
                best = middle
                low = middle + 1
            else:
                high = middle - 1
        if best is None:
            raise ValueError(
                "propagated context, role prefix and tokenizer overhead leave no room "
                "for one source token "
                f"inside target_tokens={self.budget.target_tokens}"
            )
        return best

    def _render_input(self, text: str, base_metadata: dict[str, Any] | None) -> str:
        if self.context_renderer is None:
            return text
        return self.context_renderer.render(text, base_metadata)

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        self.validate_config()

        if not text or not text.strip():
            return []

        primary_counter = self.input_contract.primary.counter
        # The content-token grid comes from TokenMeasure rather than being
        # rebuilt here. Two copies of "turn content_offsets into a usable grid"
        # is exactly the duplication ADR-091 exists to remove, and the shared
        # one additionally rejects non-monotone offsets -- which bisect would
        # otherwise consume silently, mis-cutting with no error. What stays
        # local is the part that is genuinely different: the RENDERED budget
        # search below, which is non-additive and must ask the composite
        # contract, not a measure.
        offsets = TokenMeasure(primary_counter).unit_spans(text)
        if not offsets:
            return []

        whole_input = self._render_input(text, base_metadata)
        whole_counts = self.input_contract.counts(whole_input, self.role)
        if not self.input_contract.fits(
            whole_input, self.role, self.budget.target_tokens
        ):
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
        previous_end_token: int | None = None
        while start_token < len(offsets):
            maximum_end = self._greatest_fitting_end(
                text, offsets, start_token, base_metadata
            )
            minimum_end = min(
                len(offsets), start_token + self.budget.min_content_tokens
            )
            if previous_end_token is not None:
                minimum_end = min(
                    len(offsets),
                    max(
                        minimum_end,
                        previous_end_token + max(1, self.budget.min_content_tokens),
                    ),
                )
            candidate_boundaries = [
                end for end in preferred if minimum_end <= end <= maximum_end
            ]
            end_token = (
                candidate_boundaries[-1] if candidate_boundaries else maximum_end
            )
            if previous_end_token is not None and end_token <= previous_end_token:
                # The configured overlap consumed the usable content capacity.
                # Drop overlap for this step rather than re-emitting an old
                # boundary with zero new source coverage.
                start_token = previous_end_token
                continue
            content_tokens = end_token - start_token
            actual_overlap_tokens = (
                max(0, previous_end_token - start_token)
                if previous_end_token is not None
                else 0
            )
            new_content_tokens = (
                end_token - previous_end_token
                if previous_end_token is not None
                else content_tokens
            )
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
            model_input_text = self._render_input(chunk_text, base_metadata)
            counts = self.input_contract.validate(model_input_text, self.role)
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
                "boundary_kind": (
                    self._boundary_meaning.get(end_token, (0, None, {}))[1]
                ),
                "boundary_meaning": (
                    self._boundary_meaning.get(end_token, (0, None, {}))[2]
                ),
                "has_overlap": actual_overlap_tokens > 0,
                "overlap_tokens": actual_overlap_tokens,
                "new_content_tokens": new_content_tokens,
                "context_propagated": model_input_text != chunk_text,
            }
            chunk = TextChunk(
                text=chunk_text,
                start_pos=start_char,
                end_pos=end_char,
                chunk_id=f"{source_id}_chunk_{index}",
                metadata=metadata,
                model_input_text=(
                    model_input_text if model_input_text != chunk_text else None
                ),
            )
            self.add_chunk_metadata(chunk, index, -1, "token_budget")
            chunks.append(chunk)

            if is_tail:
                break
            previous_end_token = end_token
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
