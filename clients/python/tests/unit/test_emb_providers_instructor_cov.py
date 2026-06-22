"""
Offline unit tests for the legacy InstructorProvider.

InstructorProvider still subclasses the System-B ``EmbeddingProvider`` base
(via the ``embedding_providers.base`` shim), so we stub the optional
``InstructorEmbedding`` dep through ``sys.modules`` and exercise it without any
real model download. (The cloud providers were ported onto core and are covered
in ``test_emb_providers_cloud_cov.py``.)
"""

import sys
import types
from dataclasses import dataclass, field
from typing import Any

import numpy as np
import pytest


def _install_base_stub() -> types.ModuleType:
    """Inject the permissive legacy base the overlay was written against.

    The canonical ``embedding_interface.EmbeddingProvider`` is abstract
    (``__init__`` + ``embed_text`` are abstractmethods), so we provide the
    lazy, concrete base the InstructorProvider overlay expects.
    """
    mod = types.ModuleType("proximadb_sdk.embedding_providers.base")

    @dataclass
    class EmbeddingConfig:
        model_name: str
        dimension: int
        batch_size: int = 32
        normalize: bool = True
        cache_embeddings: bool = True
        timeout_seconds: float = 30.0
        device: Any = None
        api_key: str | None = None
        api_url: str | None = None
        extra_params: dict[str, Any] = field(default_factory=dict)

    class EmbeddingProvider:
        def __init__(self, config: "EmbeddingConfig | None" = None):
            self.config = config if config is not None else self._get_default_config()
            self._available = None
            self.client = None
            self.model = None
            self._token_count = 0

        def _get_default_config(self):  # pragma: no cover - overridden
            raise NotImplementedError

        def _initialize(self):  # pragma: no cover - overridden
            raise NotImplementedError

    mod.EmbeddingConfig = EmbeddingConfig
    mod.EmbeddingProvider = EmbeddingProvider
    sys.modules["proximadb_sdk.embedding_providers.base"] = mod
    return mod


_install_base_stub()


def _install_instructor_stub():
    instructor_mod = types.ModuleType("InstructorEmbedding")

    class _INSTRUCTOR:
        def __init__(self, model_name=None, device=None):
            self.model_name = model_name
            self.device = device

        def encode(
            self,
            instruction_pairs,
            batch_size=None,
            show_progress_bar=None,
            normalize_embeddings=None,
            convert_to_numpy=None,
        ):
            return np.array([[0.1, 0.2, 0.3, 0.4] for _ in instruction_pairs])

    instructor_mod.INSTRUCTOR = _INSTRUCTOR
    sys.modules["InstructorEmbedding"] = instructor_mod


_install_instructor_stub()

from proximadb_sdk.embedding_providers.instructor import (  # noqa: E402
    InstructorProvider,
)


class TestInstructorProvider:
    def test_default_config(self):
        p = InstructorProvider()
        cfg = p._get_default_config()
        assert cfg.model_name == "hkunlp/instructor-base"
        assert cfg.dimension == 768
        assert "instruction" in cfg.extra_params

    def test_initialize_success(self):
        p = InstructorProvider()
        p._initialize()
        assert p._available is True
        assert p.config.dimension == 768
        assert p.is_available() is True

    def test_initialize_import_error(self, monkeypatch):
        monkeypatch.setitem(sys.modules, "InstructorEmbedding", None)
        p = InstructorProvider()
        p._initialize()
        assert p._available is False

    def test_embed_texts_dispatch(self):
        p = InstructorProvider()
        p._initialize()
        out = p.embed_texts(["foo", "bar"])
        assert out.shape == (2, 4)

    def test_embed_texts_unavailable(self):
        p = InstructorProvider()
        p._available = False
        with pytest.raises(RuntimeError):
            p.embed_texts(["a"])

    def test_props(self):
        p = InstructorProvider()
        assert p.dimension == p.config.dimension
        assert p.model_name == p.config.model_name
