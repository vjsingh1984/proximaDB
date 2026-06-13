"""
Offline unit tests for embedding_providers.providers.{local,testing}.

These are thin wrapper provider classes:
- local/{bge,e5,gte_qwen,gte_qwen_new,sentence_transformer,sfr}.py — wrap a
  sentence-transformers model via SentenceTransformerMixin (model load is lazy,
  only inside _load_model()).
- testing/{simulated,simulated_new}.py — deterministic hash/random/gaussian
  embeddings, no model at all.

EVERYTHING here is fully offline:
- `sentence_transformers` is stubbed into sys.modules BEFORE the local providers
  are imported so the import-time chain succeeds and _load_model() never touches
  the network or downloads a model.
- The simulated providers are pure NumPy + hashlib, exercised directly.
"""

import sys
import types

import numpy as np
import pytest


# ---------------------------------------------------------------------------
# Stub the heavy optional dependency BEFORE importing any local provider.
# SentenceTransformerMixin._load_model() does `from sentence_transformers import
# SentenceTransformer`. We replace the whole module with a controllable fake so
# no real model is ever downloaded or instantiated.
# ---------------------------------------------------------------------------


class _FakeSentenceTransformer:
    """Records construction args and returns deterministic fake embeddings."""

    last_kwargs = None

    def __init__(self, model_name, device=None, trust_remote_code=False, cache_folder=None):
        type(self).last_kwargs = {
            "model_name": model_name,
            "device": device,
            "trust_remote_code": trust_remote_code,
            "cache_folder": cache_folder,
        }
        self.model_name = model_name
        self.encode_calls = []

    def encode(self, texts, batch_size=32, normalize_embeddings=True,
               show_progress_bar=False, convert_to_numpy=True):
        self.encode_calls.append(
            {
                "texts": list(texts),
                "batch_size": batch_size,
                "normalize_embeddings": normalize_embeddings,
                "show_progress_bar": show_progress_bar,
                "convert_to_numpy": convert_to_numpy,
            }
        )
        # Return a deterministic (n, 4) array so shape assertions are cheap.
        n = len(texts)
        return np.arange(n * 4, dtype=np.float32).reshape(n, 4)


def _install_st_stub():
    mod = types.ModuleType("sentence_transformers")
    mod.SentenceTransformer = _FakeSentenceTransformer
    sys.modules["sentence_transformers"] = mod


_install_st_stub()


# Now import the targets (import succeeds because the stub is in place).
from proximadb_sdk.embedding_providers.core import (  # noqa: E402
    BaseEmbeddingProvider,
    ModelCache,
    ModelMetadata,
    ProviderConfig,
)
from proximadb_sdk.embedding_providers.providers.local import bge as bge_mod  # noqa: E402
from proximadb_sdk.embedding_providers.providers.local import e5 as e5_mod  # noqa: E402
from proximadb_sdk.embedding_providers.providers.local import (  # noqa: E402
    gte_qwen as gte_qwen_mod,
)
from proximadb_sdk.embedding_providers.providers.local import (  # noqa: E402
    gte_qwen_new as gte_qwen_new_mod,
)
from proximadb_sdk.embedding_providers.providers.local import (  # noqa: E402
    sentence_transformer as st_mod,
)
from proximadb_sdk.embedding_providers.providers.local import sfr as sfr_mod  # noqa: E402
from proximadb_sdk.embedding_providers.providers.testing import (  # noqa: E402
    simulated as sim_mod,
)
from proximadb_sdk.embedding_providers.providers.testing import (  # noqa: E402
    simulated_new as sim_new_mod,
)


@pytest.fixture(autouse=True)
def _clean_model_cache():
    """Each test starts and ends with an empty shared ModelCache so the fake
    SentenceTransformer is reconstructed and last_kwargs reflects this test."""
    ModelCache().clear()
    _FakeSentenceTransformer.last_kwargs = None
    yield
    ModelCache().clear()


# ===========================================================================
# Local (sentence-transformer-backed) providers
# ===========================================================================

# (module, ProviderClass, default model name, default batch_size, has_instruction)
LOCAL_CASES = [
    (bge_mod, bge_mod.BGEProvider, "BAAI/bge-large-en-v1.5", 32, True),
    (e5_mod, e5_mod.E5Provider, "intfloat/e5-large-v2", 32, True),
    (gte_qwen_mod, gte_qwen_mod.GTEQwenProvider,
     "Alibaba-NLP/gte-Qwen2-1.5B-instruct", 16, True),
    (gte_qwen_new_mod, gte_qwen_new_mod.GTEQwenProvider,
     "Alibaba-NLP/gte-Qwen2-1.5B-instruct", 16, True),
    (st_mod, st_mod.SentenceTransformerProvider, "all-mpnet-base-v2", 32, False),
    (sfr_mod, sfr_mod.SFRProvider, "Salesforce/SFR-Embedding-2_R", 16, True),
]


@pytest.mark.parametrize("mod,cls,default_model,batch,has_instr", LOCAL_CASES)
def test_default_config_shape(mod, cls, default_model, batch, has_instr):
    provider = cls()
    cfg = provider.default_config()
    assert isinstance(cfg, ProviderConfig)
    assert cfg.model.name == default_model
    assert cfg.batch_size == batch
    assert cfg.normalize is True
    # Instruction-based providers flip the use_query_instruction extra on.
    if has_instr:
        assert cfg.extra.get("use_query_instruction") is True
        assert cfg.model.requires_instruction is True


@pytest.mark.parametrize("mod,cls,default_model,batch,has_instr", LOCAL_CASES)
def test_provider_is_subclass_of_base(mod, cls, default_model, batch, has_instr):
    assert issubclass(cls, BaseEmbeddingProvider)


@pytest.mark.parametrize("mod,cls,default_model,batch,has_instr", LOCAL_CASES)
def test_embed_dispatches_to_sentence_transformer(mod, cls, default_model, batch, has_instr):
    provider = cls()
    out = provider.embed(["hello", "world"])
    # Lazy init must have loaded our fake model.
    assert provider._initialized is True
    assert isinstance(provider._model, _FakeSentenceTransformer)
    # The fake builds (n, 4) arrays.
    assert out.shape == (2, 4)
    # encode() received the configured batch_size + normalize flag.
    call = provider._model.encode_calls[-1]
    assert call["batch_size"] == batch
    assert call["normalize_embeddings"] is True
    assert call["show_progress_bar"] is False
    # The model was constructed with the default model name.
    assert _FakeSentenceTransformer.last_kwargs["model_name"] == default_model


@pytest.mark.parametrize("mod,cls,default_model,batch,has_instr", LOCAL_CASES)
def test_embed_empty_returns_empty_without_loading(mod, cls, default_model, batch, has_instr):
    provider = cls()
    out = provider.embed([])
    assert out.size == 0
    # Empty short-circuits before ensure_initialized.
    assert provider._initialized is False


def test_instruction_providers_apply_template_to_query():
    """embed_query prefixes the instruction template; embed_passages does not."""
    provider = bge_mod.BGEProvider()
    # embed_query -> apply_instruction -> embed([instructed])
    q_emb = provider.embed_query("machine learning")
    assert q_emb.shape == (4,)
    instructed = provider._model.encode_calls[-1]["texts"][0]
    template = provider.config.model.instruction_template
    assert instructed == template.format(query="machine learning")
    assert "machine learning" in instructed
    assert instructed != "machine learning"  # template actually applied

    # passages: no instruction prefix
    provider.embed_passages(["a passage"])
    assert provider._model.encode_calls[-1]["texts"] == ["a passage"]


def test_e5_query_prefix_is_distinct_from_bge():
    e5 = e5_mod.E5Provider()
    e5.embed_query("cats")
    e5_instructed = e5._model.encode_calls[-1]["texts"][0]
    assert e5_instructed == "query: cats"


def test_non_instruction_provider_query_is_plain():
    """SentenceTransformerProvider has no InstructionMixin, so embed exists but
    there is no embed_query; embedding text passes through unchanged."""
    provider = st_mod.SentenceTransformerProvider()
    assert not hasattr(provider, "embed_query")
    provider.embed(["plain text"])
    assert provider._model.encode_calls[-1]["texts"] == ["plain text"]


def test_embed_query_with_config_disabling_instruction():
    """If the configured model does not require instruction, query text is
    passed through verbatim (apply_instruction early-returns)."""
    plain_model = ModelMetadata(
        name="BAAI/bge-small-en-v1.5", dimension=4, requires_instruction=False
    )
    cfg = ProviderConfig(model=plain_model, batch_size=8, normalize=False)
    provider = bge_mod.BGEProvider(cfg)
    provider.embed_query("verbatim")
    assert provider._model.encode_calls[-1]["texts"] == ["verbatim"]
    assert provider._model.encode_calls[-1]["batch_size"] == 8
    assert provider._model.encode_calls[-1]["normalize_embeddings"] is False


def test_st_load_model_passes_config_through():
    """_load_model forwards device/trust_remote_code/cache_dir to the model ctor."""
    model = ModelMetadata(name="custom-model", dimension=4)
    cfg = ProviderConfig(
        model=model, device="cpu", trust_remote_code=True, cache_dir="/tmp/x"
    )
    provider = st_mod.SentenceTransformerProvider(cfg)
    provider.embed(["x"])
    kw = _FakeSentenceTransformer.last_kwargs
    assert kw["model_name"] == "custom-model"
    assert kw["device"] == "cpu"
    assert kw["trust_remote_code"] is True
    assert kw["cache_folder"] == "/tmp/x"


def test_model_catalogs_are_populated():
    assert "BAAI/bge-large-en-v1.5" in bge_mod.BGE_MODELS
    assert "intfloat/e5-large-v2" in e5_mod.E5_MODELS
    assert "Alibaba-NLP/gte-Qwen2-7B-instruct" in gte_qwen_mod.GTE_QWEN_MODELS
    assert "Alibaba-NLP/gte-Qwen2-7B-instruct" in gte_qwen_new_mod.GTE_QWEN_MODELS
    assert "all-mpnet-base-v2" in st_mod.SENTENCE_TRANSFORMER_MODELS
    assert "Salesforce/SFR-Embedding-2_R" in sfr_mod.SFR_MODELS


def test_gte_qwen_new_v2_alias_class_is_subclass():
    """gte_qwen_new defines GTEQwenProvider then re-defines it subclassing itself
    for backward-compat registration; both forms must work end-to-end."""
    provider = gte_qwen_new_mod.GTEQwenProvider()
    out = provider.embed(["t"])
    assert out.shape == (1, 4)
    cfg = provider.default_config()
    assert cfg.trust_remote_code is False


# ===========================================================================
# Testing (simulated) providers — deterministic, no model
# ===========================================================================


def test_simulated_default_config_and_load():
    p = sim_mod.SimulatedEmbeddingProvider()
    cfg = p.default_config()
    assert cfg.model.dimension == 384
    assert cfg.extra["seed"] == 42
    assert cfg.extra["method"] == "hash"
    # _load_model returns True (ready sentinel) — exercise it directly.
    assert p._load_model() is True


def test_simulated_embed_shape_and_determinism():
    p = sim_mod.SimulatedEmbeddingProvider()
    a = p.embed(["alpha", "beta", "gamma"])
    assert a.shape == (3, 384)
    assert a.dtype == np.float32
    # Same input -> identical output (deterministic).
    b = p.embed(["alpha", "beta", "gamma"])
    np.testing.assert_array_equal(a, b)
    # Different texts -> different rows.
    assert not np.array_equal(a[0], a[1])


def test_simulated_normalized_rows_are_unit_length():
    p = sim_mod.SimulatedEmbeddingProvider()
    out = p.embed(["x", "y"])
    norms = np.linalg.norm(out, axis=1)
    np.testing.assert_allclose(norms, np.ones(2), rtol=1e-5, atol=1e-5)


def test_simulated_no_normalize_keeps_raw_magnitudes():
    model = ModelMetadata(name="simulated-embeddings", dimension=16)
    cfg = ProviderConfig(model=model, normalize=False, extra={"seed": 7, "method": "hash"})
    p = sim_mod.SimulatedEmbeddingProvider(cfg)
    out = p.embed(["raw"])
    assert out.shape == (1, 16)
    # Not necessarily unit norm when normalization is off.
    assert abs(np.linalg.norm(out[0]) - 1.0) > 1e-6 or np.linalg.norm(out[0]) != 1.0


def test_simulated_empty_input():
    p = sim_mod.SimulatedEmbeddingProvider()
    assert p.embed([]).size == 0


def test_simulated_large_dimension_rehashes():
    """Dimension > 8 forces the rehash branch (i*4 >= len(hash_bytes))."""
    model = ModelMetadata(name="simulated-embeddings", dimension=100)
    cfg = ProviderConfig(model=model, normalize=False, extra={"seed": 1, "method": "hash"})
    p = sim_mod.SimulatedEmbeddingProvider(cfg)
    out = p.embed(["needs-rehash"])
    assert out.shape == (1, 100)
    # values stay in [-1, 1]
    assert out.min() >= -1.0001 and out.max() <= 1.0001


# --- simulated_new (v2) covers three generation methods -------------------


def test_simulated_new_hash_method():
    p = sim_new_mod.SimulatedEmbeddingProvider()
    out = p.embed(["one", "two"])
    assert out.shape == (2, 384)
    np.testing.assert_array_equal(out, p.embed(["one", "two"]))
    # _load_model returns None in the v2 implementation
    assert p._load_model() is None


def test_simulated_new_random_method():
    model = ModelMetadata(name="simulated-embeddings", dimension=32)
    cfg = ProviderConfig(model=model, normalize=False, extra={"seed": 9, "method": "random"})
    p = sim_new_mod.SimulatedEmbeddingProvider(cfg)
    a = p.embed(["r1", "r2"])
    assert a.shape == (2, 32)
    # deterministic via per-text RandomState seed
    np.testing.assert_array_equal(a, p.embed(["r1", "r2"]))


def test_simulated_new_gaussian_method():
    model = ModelMetadata(name="simulated-embeddings", dimension=24)
    cfg = ProviderConfig(model=model, normalize=True, extra={"seed": 3, "method": "gaussian"})
    p = sim_new_mod.SimulatedEmbeddingProvider(cfg)
    out = p.embed(["g"])
    assert out.shape == (1, 24)
    np.testing.assert_allclose(np.linalg.norm(out[0]), 1.0, rtol=1e-5, atol=1e-5)


def test_simulated_new_unknown_method_raises():
    model = ModelMetadata(name="simulated-embeddings", dimension=8)
    cfg = ProviderConfig(model=model, extra={"seed": 1, "method": "nope"})
    p = sim_new_mod.SimulatedEmbeddingProvider(cfg)
    with pytest.raises(ValueError, match="Unknown method"):
        p.embed(["boom"])


def test_simulated_new_empty_input():
    p = sim_new_mod.SimulatedEmbeddingProvider()
    assert p.embed([]).size == 0


def test_simulated_new_v2_alias_class():
    p = sim_new_mod.SimulatedEmbeddingProviderV2()
    assert isinstance(p, sim_new_mod.SimulatedEmbeddingProvider)
    out = p.embed(["alias"])
    assert out.shape == (1, 384)


def test_simulated_new_default_config():
    p = sim_new_mod.SimulatedEmbeddingProvider()
    cfg = p.default_config()
    assert cfg.batch_size == 1000
    assert cfg.extra["method"] == "hash"
    assert cfg.model.dimension == 384


def test_simulated_zero_vector_normalization_guard():
    """If a generated row is all-zero, normalization must not divide by zero.
    We force this by mocking the per-text generator to emit zeros."""
    model = ModelMetadata(name="simulated-embeddings", dimension=4)
    cfg = ProviderConfig(model=model, normalize=True, extra={"seed": 0, "method": "hash"})
    p = sim_new_mod.SimulatedEmbeddingProvider(cfg)
    p._hash_based_embedding = lambda text, dimension, seed: np.zeros(dimension, dtype=np.float32)
    out = p.embed(["zero"])
    # norm guard set zero-norm rows divisor to 1.0 -> output stays zeros, no NaN
    assert not np.isnan(out).any()
    np.testing.assert_array_equal(out[0], np.zeros(4, dtype=np.float32))
