"""Offline unit tests for proximadb_sdk.embedding_interface.

Fully offline: sentence_transformers is stubbed in sys.modules so the BERT
provider's lazy model import is exercised without any real model download.
"""

import sys
import types

import numpy as np
import pytest

from proximadb_sdk.embedding_interface import (
    BERTEmbeddingProvider,
    CohereEmbeddingProvider,
    EmbeddingConfig,
    EmbeddingProvider,
    EmbeddingProviderFactory,
    SimulatedEmbeddingProvider,
    create_embedding_provider,
    get_default_embedding_provider,
)


# ---------------------------------------------------------------------------
# Fake sentence_transformers stub
# ---------------------------------------------------------------------------
class _FakeSentenceTransformer:
    last_init_arg = None

    def __init__(self, model_name):
        _FakeSentenceTransformer.last_init_arg = model_name
        self.model_name = model_name

    def encode(self, texts, batch_size=32, normalize_embeddings=True):
        return np.ones((len(texts), 384), dtype=np.float32)

    def get_sentence_embedding_dimension(self):
        return 384


@pytest.fixture
def stub_sentence_transformers(monkeypatch):
    """Inject a fake sentence_transformers module into sys.modules."""
    mod = types.ModuleType("sentence_transformers")
    mod.SentenceTransformer = _FakeSentenceTransformer
    monkeypatch.setitem(sys.modules, "sentence_transformers", mod)
    return mod


# ---------------------------------------------------------------------------
# EmbeddingConfig
# ---------------------------------------------------------------------------
def test_embedding_config_defaults():
    cfg = EmbeddingConfig(model_name="m", dimension=128)
    assert cfg.model_name == "m"
    assert cfg.dimension == 128
    assert cfg.batch_size == 32
    assert cfg.normalize is True
    assert cfg.cache_embeddings is True
    assert cfg.timeout_seconds == 30.0
    assert cfg.api_key is None
    assert cfg.api_url is None
    assert cfg.extra_params is None
    assert cfg.track_model_usage is True
    assert cfg.track_processing_time is True
    assert cfg.track_quality_metrics is True


# ---------------------------------------------------------------------------
# SimulatedEmbeddingProvider
# ---------------------------------------------------------------------------
def test_simulated_default_config():
    p = SimulatedEmbeddingProvider()
    assert p.is_available() is True
    assert p.model_name == "simulated"
    assert p.dimension == 384


def test_simulated_embed_texts_shape_and_determinism():
    p = SimulatedEmbeddingProvider(
        EmbeddingConfig(model_name="simulated", dimension=16)
    )
    out1 = p.embed_texts(["hello world.", "another one!"])
    out2 = p.embed_texts(["hello world.", "another one!"])
    assert out1.shape == (2, 16)
    np.testing.assert_allclose(out1, out2)
    # normalize=True -> unit norm rows
    norms = np.linalg.norm(out1, axis=1)
    np.testing.assert_allclose(norms, np.ones(2), rtol=1e-5)


def test_simulated_no_normalize():
    cfg = EmbeddingConfig(model_name="simulated", dimension=16, normalize=False)
    p = SimulatedEmbeddingProvider(cfg)
    out = p.embed_texts(["count the words here please"])
    # First dim encodes word count / 100
    assert out[0][0] == pytest.approx(5 / 100.0)


def test_simulated_embed_text_single():
    p = SimulatedEmbeddingProvider(EmbeddingConfig(model_name="simulated", dimension=8))
    vec = p.embed_text("single text")
    assert vec.shape == (8,)


def test_simulated_embed_texts_with_metadata():
    p = SimulatedEmbeddingProvider(EmbeddingConfig(model_name="simulated", dimension=8))
    embeddings, meta = p.embed_texts_with_metadata(["a", "b", "c"])
    assert embeddings.shape == (3, 8)
    assert meta["batch_size"] == 3
    assert meta["dimension"] == 8
    assert meta["model_id"] == "simulated_simulated"
    assert meta["processing_time_ms"] >= 0


def test_simulated_batch_embed_texts():
    p = SimulatedEmbeddingProvider(
        EmbeddingConfig(model_name="simulated", dimension=8, batch_size=2)
    )
    out = p.batch_embed_texts(["a", "b", "c", "d", "e"])
    assert out.shape == (5, 8)


def test_simulated_batch_embed_override_batch_size():
    p = SimulatedEmbeddingProvider(EmbeddingConfig(model_name="simulated", dimension=8))
    out = p.batch_embed_texts(["a", "b", "c"], batch_size=1)
    assert out.shape == (3, 8)


def test_batch_embed_empty():
    p = SimulatedEmbeddingProvider(EmbeddingConfig(model_name="simulated", dimension=8))
    out = p.batch_embed_texts([])
    assert out.size == 0


def test_get_model_id_strips_class_suffix():
    p = SimulatedEmbeddingProvider(EmbeddingConfig(model_name="foo", dimension=8))
    # class name SimulatedEmbeddingProvider -> "simulated" prefix after replace
    assert p.get_model_id() == "simulated_foo"


# ---------------------------------------------------------------------------
# BERTEmbeddingProvider (with stub)
# ---------------------------------------------------------------------------
def test_bert_default_config_available(stub_sentence_transformers):
    p = BERTEmbeddingProvider()
    assert p.is_available() is True
    assert p.model_name == "BAAI/bge-small-en-v1.5"
    assert p.dimension == 384
    assert _FakeSentenceTransformer.last_init_arg == "BAAI/bge-small-en-v1.5"


def test_bert_embed_texts(stub_sentence_transformers):
    p = BERTEmbeddingProvider()
    out = p.embed_texts(["one", "two"])
    assert out.shape == (2, 384)


def test_bert_embed_text_single(stub_sentence_transformers):
    p = BERTEmbeddingProvider()
    vec = p.embed_text("hello")
    assert vec.shape == (384,)


def test_bert_dimension_without_model_falls_back_to_config(stub_sentence_transformers):
    p = BERTEmbeddingProvider(EmbeddingConfig(model_name="x", dimension=99))
    p._model = None
    assert p.dimension == 99


def test_bert_import_error_makes_unavailable(monkeypatch):
    # Ensure no sentence_transformers present -> ImportError branch
    monkeypatch.setitem(sys.modules, "sentence_transformers", None)
    p = BERTEmbeddingProvider()
    assert p.is_available() is False
    with pytest.raises(RuntimeError):
        p.embed_texts(["x"])


def test_bert_generic_exception_makes_unavailable(monkeypatch, capsys):
    bad = types.ModuleType("sentence_transformers")

    def _boom(name):
        raise RuntimeError("model load fail")

    bad.SentenceTransformer = _boom
    monkeypatch.setitem(sys.modules, "sentence_transformers", bad)
    p = BERTEmbeddingProvider()
    assert p.is_available() is False
    captured = capsys.readouterr()
    assert "Failed to initialize BERT model" in captured.out


# ---------------------------------------------------------------------------
# CohereEmbeddingProvider
# ---------------------------------------------------------------------------
def test_cohere_requires_api_key():
    with pytest.raises(ValueError, match="Cohere API key required"):
        CohereEmbeddingProvider(EmbeddingConfig(model_name="c", dimension=4096))


def test_cohere_not_available_and_methods_raise():
    cfg = EmbeddingConfig(model_name="embed-english-v2.0", dimension=4096, api_key="k")
    p = CohereEmbeddingProvider(cfg)
    assert p.is_available() is False
    assert p.model_name == "embed-english-v2.0"
    assert p.dimension == 4096
    with pytest.raises(RuntimeError):
        p.embed_texts(["x"])
    with pytest.raises(RuntimeError):
        p.embed_text("x")


# ---------------------------------------------------------------------------
# EmbeddingProviderFactory
# ---------------------------------------------------------------------------
def test_factory_list_providers():
    providers = EmbeddingProviderFactory.list_providers()
    assert "bert" in providers
    assert "simulated" in providers
    assert "cohere" in providers


def test_factory_create_simulated():
    p = EmbeddingProviderFactory.create_provider(
        "simulated", EmbeddingConfig(model_name="simulated", dimension=8)
    )
    assert isinstance(p, SimulatedEmbeddingProvider)


def test_factory_create_bert_available(stub_sentence_transformers):
    p = EmbeddingProviderFactory.create_provider(
        "bert", EmbeddingConfig(model_name="m", dimension=384)
    )
    assert isinstance(p, BERTEmbeddingProvider)


def test_factory_create_model_name_as_type(stub_sentence_transformers):
    p = EmbeddingProviderFactory.create_provider("all-MiniLM-L6-v2")
    assert isinstance(p, BERTEmbeddingProvider)


def test_factory_unavailable_falls_back_to_simulated(monkeypatch, capsys):
    monkeypatch.setitem(sys.modules, "sentence_transformers", None)
    p = EmbeddingProviderFactory.create_provider(
        "bert", EmbeddingConfig(model_name="m", dimension=384)
    )
    assert isinstance(p, SimulatedEmbeddingProvider)
    assert "unavailable, using simulated" in capsys.readouterr().out


def test_factory_unknown_provider():
    with pytest.raises(ValueError, match="Unknown embedding provider"):
        EmbeddingProviderFactory.create_provider(
            "nonexistent", EmbeddingConfig(model_name="m", dimension=8)
        )


def test_factory_register_provider():
    class CustomProvider(SimulatedEmbeddingProvider):
        pass

    EmbeddingProviderFactory.register_provider("custom_test", CustomProvider)
    assert "custom_test" in EmbeddingProviderFactory.list_providers()
    p = EmbeddingProviderFactory.create_provider(
        "custom_test", EmbeddingConfig(model_name="x", dimension=8)
    )
    assert isinstance(p, CustomProvider)
    # cleanup
    del EmbeddingProviderFactory._providers["custom_test"]


def test_factory_register_invalid_provider():
    class NotAProvider:
        pass

    with pytest.raises(ValueError, match="must inherit from EmbeddingProvider"):
        EmbeddingProviderFactory.register_provider("bad", NotAProvider)


# ---------------------------------------------------------------------------
# Convenience functions
# ---------------------------------------------------------------------------
def test_create_embedding_provider_simulated_explicit():
    p = create_embedding_provider("simulated", model_name="simulated", dimension=8)
    assert isinstance(p, SimulatedEmbeddingProvider)
    assert p.dimension == 8


def test_create_embedding_provider_kwargs_only_uses_defaults():
    p = create_embedding_provider("simulated", batch_size=16)
    assert isinstance(p, SimulatedEmbeddingProvider)
    assert p.config.batch_size == 16
    assert p.config.model_name == "simulated"
    assert p.config.dimension == 384


def test_create_embedding_provider_kwargs_unknown_type_defaults():
    # unknown provider type but in fallback default branch -> still builds config,
    # then create_provider raises Unknown for the type
    with pytest.raises(ValueError, match="Unknown embedding provider"):
        create_embedding_provider("weird", batch_size=8)


def test_create_embedding_provider_no_config_branch(monkeypatch):
    # No kwargs, no model_name/dimension -> config stays None
    monkeypatch.setitem(sys.modules, "sentence_transformers", None)
    p = create_embedding_provider("bert")
    # bert unavailable -> simulated fallback, config None passed through
    assert isinstance(p, SimulatedEmbeddingProvider)


def test_create_embedding_provider_cohere_defaults():
    # kwargs path picks cohere defaults; api_key provided so it constructs
    p = create_embedding_provider("cohere", api_key="k")
    assert isinstance(p, SimulatedEmbeddingProvider)  # cohere unavailable -> fallback


def test_get_default_embedding_provider(monkeypatch):
    monkeypatch.setitem(sys.modules, "sentence_transformers", None)
    p = get_default_embedding_provider()
    assert isinstance(p, SimulatedEmbeddingProvider)


def test_get_default_embedding_provider_with_stub(stub_sentence_transformers):
    p = get_default_embedding_provider()
    assert isinstance(p, BERTEmbeddingProvider)


# ---------------------------------------------------------------------------
# Abstract base cannot be instantiated directly
# ---------------------------------------------------------------------------
def test_embedding_provider_is_abstract():
    with pytest.raises(TypeError):
        EmbeddingProvider(EmbeddingConfig(model_name="m", dimension=8))
