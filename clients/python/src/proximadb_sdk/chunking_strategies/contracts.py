"""Model-input contracts used by token-budgeted chunking.

The chunker depends on these small protocols instead of an embedding provider.
This keeps chunking usable without loading a model while still letting a loaded
provider supply the exact tokenizer, role prefixes, and effective context cap.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Sequence
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Protocol, runtime_checkable


class InputRole(str, Enum):
    """How text will be presented to a retrieval embedding model."""

    DOCUMENT = "document"
    QUERY = "query"


class OverflowPolicy(str, Enum):
    """Policy for a source span that is larger than the token budget."""

    SPLIT = "split"
    ERROR = "error"
    DROP = "drop"


class ShortChunkPolicy(str, Enum):
    """Policy for a final span below ``TokenBudget.min_content_tokens``."""

    KEEP = "keep"
    DROP = "drop"
    ERROR = "error"


@dataclass(frozen=True)
class TokenBudget:
    """Desired rendered-input budget and source-token overlap.

    ``target_tokens`` counts the exact rendered model input, including role
    prefixes and tokenizer special tokens. ``overlap_tokens`` and
    ``min_content_tokens`` are measured by the primary tokenizer over raw source
    text, before role rendering.
    """

    target_tokens: int
    overlap_tokens: int = 0
    min_content_tokens: int = 1
    overflow_policy: OverflowPolicy = OverflowPolicy.SPLIT
    short_chunk_policy: ShortChunkPolicy = ShortChunkPolicy.KEEP

    def __post_init__(self) -> None:
        if self.target_tokens <= 0:
            raise ValueError("target_tokens must be positive")
        if self.overlap_tokens < 0:
            raise ValueError("overlap_tokens cannot be negative")
        if self.overlap_tokens >= self.target_tokens:
            raise ValueError("overlap_tokens must be less than target_tokens")
        if self.min_content_tokens < 0:
            raise ValueError("min_content_tokens cannot be negative")

    def to_manifest(self) -> dict[str, Any]:
        return {
            "target_tokens": self.target_tokens,
            "overlap_tokens": self.overlap_tokens,
            "min_content_tokens": self.min_content_tokens,
            "overflow_policy": self.overflow_policy.value,
            "short_chunk_policy": self.short_chunk_policy.value,
        }


@runtime_checkable
class TokenCounter(Protocol):
    """Exact token measurement plus optional raw-text offset mapping."""

    @property
    def name(self) -> str: ...

    @property
    def fingerprint(self) -> str: ...

    @property
    def advertised_limit(self) -> int | None: ...

    def count(self, text: str) -> int:
        """Count fully rendered text, including model special tokens."""
        ...

    def content_offsets(self, text: str) -> Sequence[tuple[int, int]] | None:
        """Return raw-token character offsets without special tokens."""
        ...


@dataclass(frozen=True)
class InputRenderer:
    """Role-specific, deterministic text rendering."""

    document_template: str = "{text}"
    query_template: str = "{text}"

    def __post_init__(self) -> None:
        for name, template in (
            ("document_template", self.document_template),
            ("query_template", self.query_template),
        ):
            if template.count("{text}") != 1:
                raise ValueError(
                    f"{name} must contain exactly one '{{text}}' placeholder"
                )

    def render(self, text: str, role: InputRole) -> str:
        template = (
            self.document_template
            if role == InputRole.DOCUMENT
            else self.query_template
        )
        return template.format(text=text)

    @property
    def fingerprint(self) -> str:
        payload = json.dumps(
            {
                "document_template": self.document_template,
                "query_template": self.query_template,
            },
            sort_keys=True,
            separators=(",", ":"),
        )
        return hashlib.sha256(payload.encode("utf-8")).hexdigest()


@dataclass(frozen=True)
class ResolvedInputContract:
    """Runtime-verified model input contract."""

    model_id: str
    model_revision: str
    counter: TokenCounter = field(compare=False, repr=False)
    effective_context_limit: int
    renderer: InputRenderer = field(default_factory=InputRenderer)
    native_dimension: int | None = None
    output_dimension: int | None = None
    supported_output_dimensions: tuple[int, ...] = ()
    minimum_output_dimension: int | None = None
    document_encode_parameters: tuple[tuple[str, str], ...] = ()
    query_encode_parameters: tuple[tuple[str, str], ...] = ()

    def __post_init__(self) -> None:
        if self.effective_context_limit <= 0:
            raise ValueError("effective_context_limit must be positive")
        advertised = self.counter.advertised_limit
        if advertised is not None and self.effective_context_limit > advertised:
            raise ValueError(
                f"effective context {self.effective_context_limit} exceeds "
                f"tokenizer limit {advertised} for {self.model_id}"
            )
        if self.native_dimension is not None and self.native_dimension <= 0:
            raise ValueError("native_dimension must be positive")
        if self.output_dimension is not None and self.output_dimension <= 0:
            raise ValueError("output_dimension must be positive")
        if (
            self.native_dimension is not None
            and self.output_dimension is not None
            and self.output_dimension > self.native_dimension
        ):
            raise ValueError("output_dimension cannot exceed native_dimension")
        if any(dimension <= 0 for dimension in self.supported_output_dimensions):
            raise ValueError("supported_output_dimensions must all be positive")
        if self.native_dimension is not None and any(
            dimension > self.native_dimension
            for dimension in self.supported_output_dimensions
        ):
            raise ValueError(
                "supported_output_dimensions cannot exceed native_dimension"
            )
        if self.minimum_output_dimension is not None:
            if self.minimum_output_dimension <= 0:
                raise ValueError("minimum_output_dimension must be positive")
            if (
                self.native_dimension is not None
                and self.minimum_output_dimension > self.native_dimension
            ):
                raise ValueError(
                    "minimum_output_dimension cannot exceed native_dimension"
                )

    def render(self, text: str, role: InputRole) -> str:
        return self.renderer.render(text, role)

    def count(self, text: str, role: InputRole) -> int:
        return self.counter.count(self.render(text, role))

    def validate(self, text: str, role: InputRole) -> int:
        count = self.count(text, role)
        if count > self.effective_context_limit:
            raise ValueError(
                f"{self.model_id} {role.value} input has {count} tokens, exceeding "
                f"the effective limit {self.effective_context_limit}"
            )
        return count

    def _manifest_payload(self) -> dict[str, Any]:
        return {
            "model_id": self.model_id,
            "model_revision": self.model_revision,
            "tokenizer": self.counter.name,
            "tokenizer_fingerprint": self.counter.fingerprint,
            "effective_context_limit": self.effective_context_limit,
            "renderer_fingerprint": self.renderer.fingerprint,
            "document_template": self.renderer.document_template,
            "query_template": self.renderer.query_template,
            "native_dimension": self.native_dimension,
            "output_dimension": self.output_dimension,
            "supported_output_dimensions": list(self.supported_output_dimensions),
            "minimum_output_dimension": self.minimum_output_dimension,
            "document_encode_parameters": dict(self.document_encode_parameters),
            "query_encode_parameters": dict(self.query_encode_parameters),
        }

    @property
    def fingerprint(self) -> str:
        payload = json.dumps(
            self._manifest_payload(), sort_keys=True, separators=(",", ":")
        )
        return hashlib.sha256(payload.encode("utf-8")).hexdigest()

    def to_manifest(self) -> dict[str, Any]:
        return {**self._manifest_payload(), "contract_fingerprint": self.fingerprint}


@dataclass(frozen=True)
class CompositeInputContract:
    """Compatibility contract for a byte-identical multi-model corpus."""

    contracts: tuple[ResolvedInputContract, ...]
    primary_index: int = 0

    def __post_init__(self) -> None:
        if not self.contracts:
            raise ValueError("at least one input contract is required")
        if not 0 <= self.primary_index < len(self.contracts):
            raise ValueError("primary_index is outside the contracts tuple")
        model_ids = [contract.model_id for contract in self.contracts]
        if len(set(model_ids)) != len(model_ids):
            raise ValueError("composite contract model_id values must be unique")

    @property
    def primary(self) -> ResolvedInputContract:
        return self.contracts[self.primary_index]

    @property
    def minimum_context_limit(self) -> int:
        return min(contract.effective_context_limit for contract in self.contracts)

    def counts(self, text: str, role: InputRole) -> dict[str, int]:
        return {
            contract.model_id: contract.count(text, role) for contract in self.contracts
        }

    def fits(self, text: str, role: InputRole, target_tokens: int) -> bool:
        return all(
            contract.count(text, role)
            <= min(target_tokens, contract.effective_context_limit)
            for contract in self.contracts
        )

    def validate(self, text: str, role: InputRole) -> dict[str, int]:
        return {
            contract.model_id: contract.validate(text, role)
            for contract in self.contracts
        }

    def to_manifest(self) -> dict[str, Any]:
        payload = {
            "primary_model_id": self.primary.model_id,
            "contracts": [contract.to_manifest() for contract in self.contracts],
        }
        serialized = json.dumps(payload, sort_keys=True, separators=(",", ":"))
        return {
            **payload,
            "contract_fingerprint": hashlib.sha256(
                serialized.encode("utf-8")
            ).hexdigest(),
        }


def as_composite_contract(
    contract: ResolvedInputContract | CompositeInputContract,
) -> CompositeInputContract:
    if isinstance(contract, CompositeInputContract):
        return contract
    return CompositeInputContract((contract,))
