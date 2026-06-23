"""
Offline unit tests for the (now core-based, registered) InstructorProvider.

TD-126 System-B collapse: InstructorProvider was ported onto
``core.BaseEmbeddingProvider`` and registered under ``instructor``. It no longer
subclasses the legacy ``embedding_providers.base`` shim, so this test resolves it
through the real registry (``get_provider("instructor")``) and stubs the optional
``InstructorEmbedding`` dependency via ``sys.modules`` — no model is downloaded.
"""

import sys
import types

import numpy as np
import pytest


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
            # 768-dim deterministic vectors (all hkunlp/instructor-* are 768d).
            return np.ones((len(instruction_pairs), 768), dtype=np.float32)

    instructor_mod.INSTRUCTOR = _INSTRUCTOR
    sys.modules["InstructorEmbedding"] = instructor_mod


_install_instructor_stub()

from proximadb_sdk.embedding_providers import get_provider  # noqa: E402
from proximadb_sdk.embedding_providers.instructor import (  # noqa: E402
    DEFAULT_INSTRUCTIONS,
    InstructorProvider,
)


class TestInstructorProvider:
    def test_registered_resolves(self):
        cls = type(get_provider("instructor"))
        assert cls is InstructorProvider
        # aliases resolve too
        assert type(get_provider("hkunlp")) is InstructorProvider

    def test_default_config(self):
        p = InstructorProvider()
        cfg = p.default_config()
        assert cfg.model.name == "hkunlp/instructor-base"
        assert cfg.model.dimension == 768
        assert cfg.extra["instruction"] == DEFAULT_INSTRUCTIONS["retrieval"]

    def test_dimension_and_instruction(self):
        p = InstructorProvider()
        assert p.get_dimension() == 768
        assert p.instruction == DEFAULT_INSTRUCTIONS["retrieval"]

    def test_embed_dispatch(self):
        p = InstructorProvider()
        out = p.embed(["foo", "bar"])
        assert out.shape == (2, 768)

    def test_embed_empty(self):
        p = InstructorProvider()
        assert p.embed([]).size == 0

    def test_embed_with_instructions_single(self):
        p = InstructorProvider()
        out = p.embed_texts_with_instructions(["a", "b"], "Represent X:")
        assert out.shape == (2, 768)

    def test_embed_with_instructions_per_text(self):
        p = InstructorProvider()
        out = p.embed_texts_with_instructions(["a", "b"], ["i1:", "i2:"])
        assert out.shape == (2, 768)

    def test_embed_with_instructions_length_mismatch(self):
        p = InstructorProvider()
        with pytest.raises(ValueError):
            p.embed_texts_with_instructions(["a", "b"], ["only-one:"])

    def test_load_model_import_error(self, monkeypatch):
        monkeypatch.setitem(sys.modules, "InstructorEmbedding", None)
        p = InstructorProvider()
        with pytest.raises(ImportError):
            p.ensure_initialized()

    def test_create_with_instruction(self):
        p = InstructorProvider.create_with_instruction(
            "Custom instruction:", batch_size=8
        )
        assert p.instruction == "Custom instruction:"
        assert p.config.batch_size == 8
        assert p.get_dimension() == 768

    def test_create_with_instruction_unknown_model(self):
        p = InstructorProvider.create_with_instruction(
            "Instr:", model_name="hkunlp/instructor-xl"
        )
        assert p.config.model.name == "hkunlp/instructor-xl"
