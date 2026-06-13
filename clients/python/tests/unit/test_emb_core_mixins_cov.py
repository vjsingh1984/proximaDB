"""Offline unit tests for embedding_providers core + mixins.

Covers:
- core/config.py    (ModelMetadata, ProviderConfig, merge, __str__)
- core/base.py      (BaseEmbeddingProvider lifecycle, lazy init, info, ctx mgr)
- core/registry.py  (ProviderRegistry register/resolve/list/info/clear)
- core/cache.py     (ModelCache singleton, get_or_load, stats, clear)
- mixins/normalization.py (L2 norm math, cosine sim, check_normalized)
- mixins/batching.py      (batch split, adaptive sizing, memory estimate)
- mixins/instruction.py   (instruction templating, query/passage/doc embed)
- mixins/sentence_transformer.py (model load via mocked import, embed shapes)

Fully offline: no network, no real model downloads, sentence_transformers is
stubbed in sys.modules before any import that might touch it.
"""

import sys
import types

import numpy as np
import pytest

# --- Stub heavy/optional deps BEFORE importing the target package ----------
# sentence_transformer mixin lazily imports sentence_transformers inside
# _load_model; we stub it so that path is exercisable without a download.
if "sentence_transformers" not in sys.modules:
    _st_mod = types.ModuleType("sentence_transformers")

    class _FakeSentenceTransformer:  # pragma: no cover - replaced per-test
        def __init__(self, *a, **k):
            self._args = a
            self._kwargs = k

        def encode(self, texts, **kwargs):
            return np.zeros((len(texts), 384), dtype=np.float32)

    _st_mod.SentenceTransformer = _FakeSentenceTransformer
    sys.modules["sentence_transformers"] = _st_mod

from proximadb_sdk.embedding_providers.core.base import (  # noqa: E402
    BaseEmbeddingProvider,
    EmbeddingProviderProtocol,
)
from proximadb_sdk.embedding_providers.core.cache import (  # noqa: E402
    ModelCache,
    get_model_cache,
)
from proximadb_sdk.embedding_providers.core.config import (  # noqa: E402
    ModelMetadata,
    ProviderConfig,
)
from proximadb_sdk.embedding_providers.core.registry import (  # noqa: E402
    ProviderRegistry,
)
from proximadb_sdk.embedding_providers.mixins.batching import BatchingMixin  # noqa: E402
from proximadb_sdk.embedding_providers.mixins.instruction import (  # noqa: E402
    InstructionMixin,
)
from proximadb_sdk.embedding_providers.mixins.normalization import (  # noqa: E402
    NormalizationMixin,
)
from proximadb_sdk.embedding_providers.mixins.sentence_transformer import (  # noqa: E402
    SentenceTransformerMixin,
)


# --------------------------------------------------------------------------
# Helpers / fixtures
# --------------------------------------------------------------------------
def make_model(**kw) -> ModelMetadata:
    base = dict(name="test-model", dimension=4, max_length=128)
    base.update(kw)
    return ModelMetadata(**base)


def make_config(**kw) -> ProviderConfig:
    model = kw.pop("model", make_model())
    return ProviderConfig(model=model, **kw)


class FakeModel:
    """Stand-in for a loaded encoder model."""

    def __init__(self, dim=4):
        self.dim = dim
        self.encode_calls = []

    def encode(self, texts, **kwargs):
        self.encode_calls.append((list(texts), kwargs))
        return np.ones((len(texts), self.dim), dtype=np.float32)


class DummyProvider(BaseEmbeddingProvider):
    """Concrete provider for exercising BaseEmbeddingProvider."""

    def __init__(self, config=None, fake_model=None):
        self._fake_model = fake_model or FakeModel(dim=4)
        self.load_count = 0
        super().__init__(config)

    def default_config(self) -> ProviderConfig:
        return make_config()

    def _load_model(self):
        self.load_count += 1
        return self._fake_model

    def embed(self, texts):
        self.ensure_initialized()
        return self._model.encode(texts)


class STProvider(SentenceTransformerMixin, InstructionMixin, NormalizationMixin,
                  BatchingMixin, BaseEmbeddingProvider):
    """Provider composed of all mixins for integration-ish coverage."""

    def __init__(self, config=None):
        super().__init__(config)

    def default_config(self) -> ProviderConfig:
        return make_config(model=make_model(dimension=384))


@pytest.fixture(autouse=True)
def _clean_cache():
    # Reset the singleton model cache between tests so counts are predictable.
    cache = ModelCache()
    cache.clear()
    cache.reset_stats()
    yield
    cache.clear()
    cache.reset_stats()


# --------------------------------------------------------------------------
# config.py
# --------------------------------------------------------------------------
def test_model_metadata_defaults_and_str():
    m = make_model()
    assert m.provider_type == "sentence-transformer"
    assert m.requires_instruction is False
    s = str(m)
    assert "test-model (4D)" in s
    # No mteb_score / english => only name+dim part
    assert "MTEB" not in s and "Lang" not in s


def test_model_metadata_str_with_score_and_lang():
    m = make_model(mteb_score=64.2, languages="multilingual")
    s = str(m)
    assert "MTEB: 64.2" in s
    assert "Lang: multilingual" in s


def test_provider_config_str():
    c = make_config(batch_size=16, normalize=False)
    s = str(c)
    assert "test-model" in s
    assert "batch_size=16" in s
    assert "normalize=False" in s
    assert "auto" in s  # device None -> 'auto'


def test_provider_config_merge_scalar_and_immutability():
    c = make_config(batch_size=32)
    c2 = c.merge(batch_size=64, normalize=False)
    assert c2.batch_size == 64
    assert c2.normalize is False
    # original unchanged
    assert c.batch_size == 32
    assert c.normalize is True
    # model preserved as ModelMetadata
    assert isinstance(c2.model, ModelMetadata)
    assert c2.model.name == "test-model"


def test_provider_config_merge_extra_merges():
    c = make_config(extra={"a": 1, "b": 2})
    c2 = c.merge(extra={"b": 99, "c": 3})
    assert c2.extra == {"a": 1, "b": 99, "c": 3}
    # original extra untouched
    assert c.extra == {"a": 1, "b": 2}


def test_provider_config_merge_model_dict_roundtrip():
    c = make_config()
    new_model_dict = {"name": "other", "dimension": 8}
    c2 = c.merge(model=new_model_dict)
    assert isinstance(c2.model, ModelMetadata)
    assert c2.model.name == "other"
    assert c2.model.dimension == 8


# --------------------------------------------------------------------------
# base.py
# --------------------------------------------------------------------------
def test_base_uses_default_config_when_none():
    p = DummyProvider()
    assert isinstance(p.config, ProviderConfig)
    assert p.config.model.name == "test-model"


def test_base_uses_supplied_config():
    cfg = make_config(batch_size=7)
    p = DummyProvider(config=cfg)
    assert p.config.batch_size == 7


def test_base_lazy_init_loads_once():
    p = DummyProvider()
    assert p._initialized is False
    p.ensure_initialized()
    p.ensure_initialized()
    assert p._initialized is True
    assert p.load_count == 1  # loaded exactly once


def test_base_dimension_and_alias_and_model_name():
    p = DummyProvider()
    assert p.get_dimension() == 4
    assert p.dimension == 4
    assert p.model_name == "test-model"


def test_base_model_name_unknown_when_no_model():
    p = DummyProvider()
    # Force missing model to hit the fallback branch
    p.config = None
    assert p.model_name == "unknown"


def test_base_embed_text_and_embed_texts():
    p = DummyProvider()
    one = p.embed_text("hello")
    assert one.shape == (4,)
    many = p.embed_texts(["a", "b"])
    assert many.shape == (2, 4)


def test_base_is_available_true():
    p = DummyProvider()
    assert p.is_available() is True


def test_base_is_available_false_on_load_error():
    p = DummyProvider()

    def boom():
        raise RuntimeError("cannot load")

    p._load_model = boom
    assert p.is_available() is False


def test_base_get_model_info():
    p = DummyProvider()
    info = p.get_model_info()
    assert info["name"] == "test-model"
    assert info["dimension"] == 4
    assert info["max_length"] == 128
    assert info["batch_size"] == 32
    assert info["normalize"] is True
    assert "device" in info


def test_base_cleanup_resets_state_and_calls_hook():
    calls = []

    class P(DummyProvider):
        def _cleanup_model(self):
            calls.append("cleanup")

    p = P()
    p.ensure_initialized()
    assert p._model is not None
    p.cleanup()
    assert p._model is None
    assert p._initialized is False
    assert calls == ["cleanup"]
    # cleanup when no model is a no-op
    p.cleanup()


def test_base_context_manager():
    p = DummyProvider()
    with p as ctx:
        assert ctx is p
        assert p._initialized is True
    # exit triggers cleanup
    assert p._model is None
    assert p._initialized is False


def test_base_repr_reflects_state():
    p = DummyProvider()
    assert "not initialized" in repr(p)
    p.ensure_initialized()
    assert "initialized" in repr(p)
    assert "test-model" in repr(p)


def test_base_protocol_runtime_checkable():
    p = DummyProvider()
    p.ensure_initialized()
    assert isinstance(p, EmbeddingProviderProtocol)


# --------------------------------------------------------------------------
# registry.py
# --------------------------------------------------------------------------
@pytest.fixture
def fresh_registry():
    # Snapshot + clear so we don't clobber real registrations permanently.
    snap = (
        dict(ProviderRegistry._providers),
        dict(ProviderRegistry._metadata),
        dict(ProviderRegistry._aliases),
        dict(ProviderRegistry._descriptions),
    )
    ProviderRegistry.clear()
    yield ProviderRegistry
    (ProviderRegistry._providers,
     ProviderRegistry._metadata,
     ProviderRegistry._aliases,
     ProviderRegistry._descriptions) = (
        dict(snap[0]), dict(snap[1]), dict(snap[2]), dict(snap[3])
    )


def _register(reg, name, aliases=None, desc="", models=None):
    models = models or {"m1": make_model(name="m1"), "m2": make_model(name="m2")}

    @reg.register(name=name, models=models, aliases=aliases, description=desc)
    class _P(DummyProvider):
        pass

    return _P


def test_registry_register_and_get(fresh_registry):
    cls = _register(fresh_registry, "prov", aliases=["alias1", "alias2"], desc="d")
    assert fresh_registry.get_provider("prov") is cls
    # alias resolution + case-insensitive
    assert fresh_registry.get_provider("ALIAS1") is cls


def test_registry_register_rejects_non_subclass(fresh_registry):
    with pytest.raises(TypeError):
        @fresh_registry.register(name="bad", models={})
        class NotAProvider:
            pass


def test_registry_get_provider_unknown_raises(fresh_registry):
    with pytest.raises(ValueError) as exc:
        fresh_registry.get_provider("nope")
    assert "Unknown embedding provider" in str(exc.value)


def test_registry_alias_override_warns(fresh_registry, caplog):
    _register(fresh_registry, "first", aliases=["shared"])
    with caplog.at_level("WARNING"):
        _register(fresh_registry, "second", aliases=["shared"])
    assert fresh_registry._aliases["shared"] == "second"


def test_registry_get_models_and_default(fresh_registry):
    models = {"x": make_model(name="x"), "y": make_model(name="y")}
    _register(fresh_registry, "p", models=models)
    got = fresh_registry.get_models("p")
    assert set(got.keys()) == {"x", "y"}
    default = fresh_registry.get_default_model("p")
    assert default.name == "x"  # first inserted
    # unknown provider -> empty / None
    assert fresh_registry.get_models("missing") == {}
    assert fresh_registry.get_default_model("missing") is None


def test_registry_list_providers(fresh_registry):
    _register(fresh_registry, "b", aliases=["ba"])
    _register(fresh_registry, "a")
    assert fresh_registry.list_providers() == ["a", "b"]
    with_aliases = fresh_registry.list_providers(include_aliases=True)
    assert "ba" in with_aliases


def test_registry_provider_info(fresh_registry):
    _register(fresh_registry, "p", desc="hello")
    info = fresh_registry.get_provider_info("p")
    assert info["name"] == "p"
    assert info["description"] == "hello"
    assert info["num_models"] == 2
    assert info["default_model"] == "m1"


def test_registry_provider_info_unknown_raises(fresh_registry):
    with pytest.raises(ValueError):
        fresh_registry.get_provider_info("ghost")


def test_registry_clear(fresh_registry):
    _register(fresh_registry, "p")
    fresh_registry.clear()
    assert fresh_registry.list_providers() == []


# --------------------------------------------------------------------------
# cache.py
# --------------------------------------------------------------------------
def test_cache_singleton():
    assert ModelCache() is ModelCache()
    assert get_model_cache() is ModelCache()


def test_cache_get_or_load_miss_then_hit():
    cache = ModelCache()
    loads = []

    def loader():
        loads.append(1)
        return "MODEL"

    v1 = cache.get_or_load("k", loader)
    v2 = cache.get_or_load("k", loader)
    assert v1 == "MODEL" and v2 == "MODEL"
    assert len(loads) == 1  # loaded once
    stats = cache.stats()
    assert stats["loads"] == 1
    assert stats["misses"] == 1
    assert stats["hits"] >= 1


def test_cache_force_reload():
    cache = ModelCache()
    seq = iter(["A", "B"])
    cache.get_or_load("k", lambda: next(seq))
    v = cache.get_or_load("k", lambda: next(seq), force_reload=True)
    assert v == "B"


def test_cache_get_and_keys_and_size():
    cache = ModelCache()
    assert cache.get("absent") is None
    cache.get_or_load("k1", lambda: 1)
    cache.get_or_load("k2", lambda: 2)
    assert cache.get("k1") == 1
    assert set(cache.keys()) == {"k1", "k2"}
    assert cache.size() == 2


def test_cache_clear_specific_and_all():
    cache = ModelCache()
    cache.get_or_load("k1", lambda: 1)
    cache.get_or_load("k2", lambda: 2)
    cache.clear("k1")
    assert "k1" not in cache.keys()
    # clearing missing key -> warning path, no error
    cache.clear("nope")
    cache.clear()
    assert cache.size() == 0


def test_cache_loader_exception_propagates():
    cache = ModelCache()

    def bad():
        raise ValueError("load failed")

    with pytest.raises(ValueError):
        cache.get_or_load("badk", bad)
    assert "badk" not in cache.keys()


def test_cache_reset_stats_and_repr():
    cache = ModelCache()
    cache.get_or_load("k", lambda: 1)
    cache.reset_stats()
    assert cache.stats() == {"hits": 0, "misses": 0, "loads": 0}
    assert "ModelCache(" in repr(cache)


# --------------------------------------------------------------------------
# normalization.py
# --------------------------------------------------------------------------
def test_normalize_1d():
    out = NormalizationMixin.normalize_embeddings(np.array([3.0, 4.0]))
    assert np.isclose(np.linalg.norm(out), 1.0)
    assert np.allclose(out, [0.6, 0.8])


def test_normalize_1d_zero_norm_warns():
    z = np.array([0.0, 0.0])
    out = NormalizationMixin.normalize_embeddings(z)
    assert np.allclose(out, z)  # returned unchanged


def test_normalize_2d_with_zero_row():
    embs = np.array([[3.0, 4.0], [0.0, 0.0]])
    out = NormalizationMixin.normalize_embeddings(embs)
    assert np.isclose(np.linalg.norm(out[0]), 1.0)
    assert np.allclose(out[1], [0.0, 0.0])  # zero row stays zero (div by 1.0)


def test_normalize_empty():
    e = np.array([])
    assert NormalizationMixin.normalize_embeddings(e).size == 0


def test_check_normalized_variants():
    assert NormalizationMixin.check_normalized(np.array([])) is True
    assert bool(NormalizationMixin.check_normalized(np.array([0.6, 0.8]))) is True
    assert bool(NormalizationMixin.check_normalized(np.array([3.0, 4.0]))) is False
    assert bool(NormalizationMixin.check_normalized(np.array([[0.6, 0.8], [1.0, 0.0]]))) is True
    assert bool(NormalizationMixin.check_normalized(np.array([[3.0, 4.0]]))) is False


def test_cosine_similarity():
    sim = NormalizationMixin.get_cosine_similarity(
        np.array([1.0, 0.0]), np.array([0.0, 1.0])
    )
    assert np.isclose(sim, 0.0)
    same = NormalizationMixin.get_cosine_similarity(
        np.array([1.0, 1.0]), np.array([2.0, 2.0])
    )
    assert np.isclose(same, 1.0)


# --------------------------------------------------------------------------
# batching.py
# --------------------------------------------------------------------------
class BatchProvider(BatchingMixin):
    def __init__(self, batch_size=32, dimension=768):
        self.config = make_config(batch_size=batch_size,
                                  model=make_model(dimension=dimension))


def test_create_batches_default_and_custom():
    bp = BatchProvider(batch_size=2)
    texts = ["t1", "t2", "t3", "t4", "t5"]
    custom = list(bp.create_batches(texts, batch_size=2))
    assert len(custom) == 3
    assert custom[0] == ["t1", "t2"]
    assert custom[-1] == ["t5"]
    # default from config.batch_size
    default = list(bp.create_batches(texts))
    assert len(default) == 3


def test_adaptive_batch_size_buckets():
    bp = BatchProvider()
    assert bp.adaptive_batch_size([]) == 32  # empty -> config default
    assert bp.adaptive_batch_size(["hi"] * 5) == 64           # <100
    assert bp.adaptive_batch_size(["x" * 200]) == 32          # <500
    assert bp.adaptive_batch_size(["x" * 1000]) == 16         # <2000
    assert bp.adaptive_batch_size(["x" * 3000]) == 8          # <5000
    assert bp.adaptive_batch_size(["x" * 6000]) == 4          # >=5000


def test_estimate_memory_usage_positive():
    bp = BatchProvider(dimension=768)
    mb = bp.estimate_memory_usage(100, 500)
    assert mb > 0
    # larger inputs => more memory
    assert bp.estimate_memory_usage(200, 500) > mb


def test_should_use_batching():
    bp = BatchProvider(batch_size=32)
    assert bp.should_use_batching(10) is False
    assert bp.should_use_batching(100) is True


# --------------------------------------------------------------------------
# instruction.py
# --------------------------------------------------------------------------
class InstrProvider(InstructionMixin):
    def __init__(self, model, fake_model=None):
        self.config = make_config(model=model)
        self._fm = fake_model or FakeModel(dim=model.dimension)

    def embed(self, texts):
        return self._fm.encode(texts)


def test_instruction_passage_passthrough():
    m = make_model(requires_instruction=True, instruction_template="Query: {query}")
    p = InstrProvider(m)
    assert p.apply_instruction("foo", is_query=False) == "foo"


def test_instruction_no_requirement_passthrough():
    m = make_model(requires_instruction=False)
    p = InstrProvider(m)
    assert p.apply_instruction("foo", is_query=True) == "foo"


def test_instruction_applies_template():
    m = make_model(requires_instruction=True, instruction_template="Query: {query}")
    p = InstrProvider(m)
    assert p.apply_instruction("ml", is_query=True) == "Query: ml"


def test_instruction_missing_template_warns():
    m = make_model(requires_instruction=True, instruction_template=None)
    p = InstrProvider(m)
    assert p.apply_instruction("ml", is_query=True) == "ml"


def test_instruction_bad_template_keyerror():
    m = make_model(requires_instruction=True, instruction_template="{missing}")
    p = InstrProvider(m)
    # {missing} is not 'query' -> KeyError path -> returns original text
    assert p.apply_instruction("ml", is_query=True) == "ml"


def test_instruction_embed_query_and_queries():
    m = make_model(dimension=4, requires_instruction=True,
                   instruction_template="Q: {query}")
    fm = FakeModel(dim=4)
    p = InstrProvider(m, fake_model=fm)
    q = p.embed_query("hi")
    assert q.shape == (4,)
    # the embedded text was instruction-wrapped
    assert fm.encode_calls[-1][0] == ["Q: hi"]
    qs = p.embed_queries(["a", "b"])
    assert qs.shape == (2, 4)
    assert fm.encode_calls[-1][0] == ["Q: a", "Q: b"]


def test_instruction_embed_passages_and_documents():
    m = make_model(dimension=4, requires_instruction=True,
                   instruction_template="Q: {query}")
    fm = FakeModel(dim=4)
    p = InstrProvider(m, fake_model=fm)
    passages = p.embed_passages(["p1", "p2"])
    assert passages.shape == (2, 4)
    assert fm.encode_calls[-1][0] == ["p1", "p2"]  # no instruction on passages
    docs = p.embed_documents([{"text": "d1"}, {"id": "x"}, "raw"])
    assert docs.shape == (3, 4)
    assert fm.encode_calls[-1][0] == ["d1", "", "raw"]


# --------------------------------------------------------------------------
# sentence_transformer.py
# --------------------------------------------------------------------------
def test_st_embed_empty_returns_empty():
    p = STProvider()
    out = p.embed([])
    assert out.size == 0


def test_st_load_model_uses_mocked_sentence_transformers(monkeypatch):
    captured = {}

    class FakeST:
        def __init__(self, name, device=None, trust_remote_code=False,
                     cache_folder=None):
            captured["name"] = name
            captured["device"] = device
            captured["trust_remote_code"] = trust_remote_code
            captured["cache_folder"] = cache_folder

        def encode(self, texts, **kwargs):
            captured["encode_kwargs"] = kwargs
            return np.ones((len(texts), 384), dtype=np.float32)

    monkeypatch.setattr(
        sys.modules["sentence_transformers"], "SentenceTransformer", FakeST
    )

    p = STProvider(config=make_config(
        model=make_model(dimension=384, name="my-st"),
        device="cpu", normalize=True, batch_size=8, trust_remote_code=True,
        cache_dir="/tmp/cache",
    ))
    out = p.embed(["hello", "world"])
    assert out.shape == (2, 384)
    assert captured["name"] == "my-st"
    assert captured["device"] == "cpu"
    assert captured["trust_remote_code"] is True
    assert captured["cache_folder"] == "/tmp/cache"
    # embed passes config-driven kwargs through to encode
    assert captured["encode_kwargs"]["batch_size"] == 8
    assert captured["encode_kwargs"]["normalize_embeddings"] is True
    assert captured["encode_kwargs"]["show_progress_bar"] is False


def test_st_load_model_caches_across_instances(monkeypatch):
    load_count = {"n": 0}

    class FakeST:
        def __init__(self, *a, **k):
            load_count["n"] += 1

        def encode(self, texts, **kwargs):
            return np.ones((len(texts), 384), dtype=np.float32)

    monkeypatch.setattr(
        sys.modules["sentence_transformers"], "SentenceTransformer", FakeST
    )
    cfg = make_config(model=make_model(dimension=384, name="shared"))
    p1 = STProvider(config=cfg)
    p2 = STProvider(config=cfg)
    p1.embed(["a"])
    p2.embed(["b"])
    assert load_count["n"] == 1  # ModelCache shared the instance


def test_st_load_model_import_error(monkeypatch):
    # Simulate sentence_transformers not installed: a stand-in module whose
    # SentenceTransformer attribute access raises ImportError, so the
    # `from sentence_transformers import SentenceTransformer` inside
    # _load_model hits its `except ImportError` branch. (Setting the
    # sys.modules entry to None can make coverage's import machinery hang.)
    broken = types.ModuleType("sentence_transformers")

    def _raise(name):
        raise ImportError("not installed")

    broken.__getattr__ = _raise  # PEP 562 module-level __getattr__
    monkeypatch.setitem(sys.modules, "sentence_transformers", broken)
    p = STProvider()
    with pytest.raises(ImportError):
        p.ensure_initialized()


def test_st_embed_batch_custom_size_restores(monkeypatch):
    class FakeST:
        def __init__(self, *a, **k):
            pass

        def encode(self, texts, **kwargs):
            FakeST.last_bs = kwargs["batch_size"]
            return np.ones((len(texts), 384), dtype=np.float32)

    monkeypatch.setattr(
        sys.modules["sentence_transformers"], "SentenceTransformer", FakeST
    )
    p = STProvider(config=make_config(model=make_model(dimension=384), batch_size=16))
    p.embed_batch(["x", "y"], batch_size=99)
    assert FakeST.last_bs == 99
    # original batch_size restored
    assert p.config.batch_size == 16


def test_st_embed_batch_no_override(monkeypatch):
    class FakeST:
        def __init__(self, *a, **k):
            pass

        def encode(self, texts, **kwargs):
            FakeST.last_bs = kwargs["batch_size"]
            return np.ones((len(texts), 384), dtype=np.float32)

    monkeypatch.setattr(
        sys.modules["sentence_transformers"], "SentenceTransformer", FakeST
    )
    p = STProvider(config=make_config(model=make_model(dimension=384), batch_size=16))
    p.embed_batch(["x"])
    assert FakeST.last_bs == 16
