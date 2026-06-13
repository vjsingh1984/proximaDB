"""Compatibility shim for the provider overlays in this package.

Several provider modules here (``bge``, ``sfr``, ``e5``,
``sentence_transformer``, ``finbert_provider``, ``multi_bert_provider``) do
``from .base import EmbeddingConfig, EmbeddingProvider``, but the canonical
definitions live in :mod:`proximadb_sdk.embedding_interface` (the
``embedding_providers/base.py`` module was dropped in a refactor while those
overlays kept the old import path, leaving them un-importable as shipped).

Re-export the canonical symbols so the overlays import correctly.
"""

from ..embedding_interface import EmbeddingConfig, EmbeddingProvider

__all__ = ["EmbeddingConfig", "EmbeddingProvider"]
