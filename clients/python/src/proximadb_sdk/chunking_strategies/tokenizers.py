"""Tokenizer adapters for model-agnostic chunk budget contracts."""

from __future__ import annotations

import hashlib
import re
from collections.abc import Sequence
from typing import Any


class HuggingFaceTokenCounter:
    """Adapt a fast Hugging Face tokenizer to the ``TokenCounter`` protocol."""

    def __init__(self, tokenizer: Any):
        if not getattr(tokenizer, "is_fast", False):
            raise ValueError("token-budget splitting requires a fast tokenizer")
        self._tokenizer = tokenizer
        backend = tokenizer.backend_tokenizer.to_str().encode("utf-8")
        self._fingerprint = hashlib.sha256(backend).hexdigest()

    @classmethod
    def from_pretrained(
        cls, model_id: str, *, revision: str | None = None, **kwargs: Any
    ) -> HuggingFaceTokenCounter:
        try:
            from transformers import AutoTokenizer
        except ImportError as exc:
            raise ImportError(
                "transformers is required for Hugging Face token-aware chunking; "
                "install proximadb[embeddings]"
            ) from exc
        tokenizer = AutoTokenizer.from_pretrained(model_id, revision=revision, **kwargs)
        return cls(tokenizer)

    @property
    def tokenizer(self) -> Any:
        return self._tokenizer

    @property
    def name(self) -> str:
        return str(getattr(self._tokenizer, "name_or_path", "huggingface-tokenizer"))

    @property
    def fingerprint(self) -> str:
        return self._fingerprint

    @property
    def advertised_limit(self) -> int | None:
        value = getattr(self._tokenizer, "model_max_length", None)
        if not isinstance(value, int) or value <= 0 or value >= 1_000_000_000:
            return None
        return value

    @property
    def resolved_revision(self) -> str | None:
        """Best-effort immutable Hub revision from tokenizer metadata/cache paths."""
        init_kwargs = getattr(self._tokenizer, "init_kwargs", {}) or {}
        direct = init_kwargs.get("_commit_hash") or init_kwargs.get("commit_hash")
        if direct:
            return str(direct)
        candidates = [value for value in init_kwargs.values() if isinstance(value, str)]
        for attribute in ("vocab_file", "tokenizer_file"):
            value = getattr(self._tokenizer, attribute, None)
            if isinstance(value, str):
                candidates.append(value)
        for candidate in candidates:
            match = re.search(
                r"[/\\]snapshots[/\\]([0-9a-f]{40})(?:[/\\]|$)", candidate
            )
            if match:
                return match.group(1)
        return None

    def count(self, text: str) -> int:
        encoded = self._tokenizer(
            text,
            add_special_tokens=True,
            truncation=False,
            return_attention_mask=False,
            return_token_type_ids=False,
            verbose=False,
        )
        return len(encoded["input_ids"])

    def content_offsets(self, text: str) -> Sequence[tuple[int, int]]:
        encoded = self._tokenizer(
            text,
            add_special_tokens=False,
            truncation=False,
            return_attention_mask=False,
            return_token_type_ids=False,
            return_offsets_mapping=True,
            verbose=False,
        )
        return tuple(
            (int(start), int(end))
            for start, end in encoded["offset_mapping"]
            if end > start
        )
