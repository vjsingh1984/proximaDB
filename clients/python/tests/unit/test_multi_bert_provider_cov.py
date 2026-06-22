"""
Offline unit tests for proximadb_sdk.embedding_providers.multi_bert_provider.

TD-126 System-B collapse: MultiBERTProvider / AdaptiveBERTProvider were ported
onto ``core.BaseEmbeddingProvider`` and registered (``multi-bert`` /
``adaptive-bert``). The module now imports cleanly (no ``.base`` shim, no heavy
module-level imports — torch/transformers/sentence_transformers are imported
lazily inside the model-loading / encoding paths), so this test imports it
directly and monkeypatches the heavy deps via ``sys.modules`` + the lazily
imported names. No model is ever downloaded and no GPU is touched.
"""

import contextlib
import sys
import types

import numpy as np
import pytest

# ---------------------------------------------------------------------------
# Stub the heavy optional deps in sys.modules BEFORE the provider imports them
# (it imports them lazily inside methods, so installing the stubs here suffices).
# ---------------------------------------------------------------------------


class _FakeTensor:
    """Tiny tensor stand-in supporting the ops the encoder uses."""

    def __init__(self, arr):
        self.arr = np.asarray(arr, dtype=np.float32)

    def __mul__(self, other):
        return _FakeTensor(self.arr * other.arr)

    def __truediv__(self, other):
        return _FakeTensor(self.arr / other.arr)

    def sum(self, dim=None):
        return _FakeTensor(self.arr.sum(axis=dim))

    def max(self, dim=None):
        return (_FakeTensor(self.arr.max(axis=dim)),)

    def unsqueeze(self, axis):
        return _FakeTensor(np.expand_dims(self.arr, axis))

    def cpu(self):
        return self

    def numpy(self):
        return self.arr

    def __getitem__(self, idx):
        return _FakeTensor(self.arr[idx])


class _FakeOutputs:
    def __init__(self, last_hidden_state):
        self.last_hidden_state = last_hidden_state


# torch / torch.nn / torch.nn.functional
_torch = types.ModuleType("torch")


class _OOM(Exception):
    pass


_torch.cuda = types.SimpleNamespace(
    is_available=lambda: False,
    empty_cache=lambda: None,
    get_device_properties=lambda idx: types.SimpleNamespace(total_memory=8e9),
    OutOfMemoryError=_OOM,
)
_torch.no_grad = lambda: contextlib.nullcontext()
_torch.backends = types.SimpleNamespace(
    mps=types.SimpleNamespace(is_available=lambda: False)
)
_nn = types.ModuleType("torch.nn")
_F = types.ModuleType("torch.nn.functional")
_F.normalize = lambda t, p=2, dim=1: _FakeTensor(
    t.arr / (np.linalg.norm(t.arr, axis=dim, keepdims=True) + 1e-12)
)
_nn.functional = _F
_torch.nn = _nn
sys.modules["torch"] = _torch
sys.modules["torch.nn"] = _nn
sys.modules["torch.nn.functional"] = _F


# sentence_transformers + transformers (dimension-aware fakes installed below
# once we can read the provider's MODELS registry).
_st = types.ModuleType("sentence_transformers")
_tf = types.ModuleType("transformers")
sys.modules["sentence_transformers"] = _st
sys.modules["transformers"] = _tf

from proximadb_sdk.embedding_providers import get_provider  # noqa: E402
from proximadb_sdk.embedding_providers.multi_bert_provider import (  # noqa: E402
    AdaptiveBERTProvider,
    ModelSize,
    MultiBERTProvider,
)

_PATH_TO_DIM = {
    cfg["name"]: cfg["dimension"] for cfg in MultiBERTProvider.MODELS.values()
}


class FakeSentenceTransformer:
    last_init = None

    def __init__(self, model_path, device=None, cache_folder=None):
        FakeSentenceTransformer.last_init = {
            "model_path": model_path,
            "device": device,
            "cache_folder": cache_folder,
        }
        self.model_path = model_path
        self._dim = _PATH_TO_DIM.get(model_path, 768)

    def encode(
        self,
        texts,
        batch_size=32,
        normalize_embeddings=True,
        show_progress_bar=False,
        device=None,
        convert_to_numpy=True,
    ):
        return np.array(
            [[float(len(t)) + j for j in range(self._dim)] for t in texts],
            dtype=np.float32,
        )


class FakeAutoModelInstance:
    def __init__(self, dim):
        self.dim = dim

    def to(self, device):
        return self

    def eval(self):
        return self

    def __call__(self, **inputs):
        mask = inputs["attention_mask"].arr
        batch, seq = mask.shape
        hidden = np.ones((batch, seq, self.dim), dtype=np.float32)
        return _FakeOutputs(_FakeTensor(hidden))


class FakeAutoModel:
    @staticmethod
    def from_pretrained(model_path, cache_dir=None):
        return FakeAutoModelInstance(_PATH_TO_DIM.get(model_path, 768))


class _FakeBatchEncoding(dict):
    def to(self, device):
        return self


class FakeTokenizerInstance:
    def __call__(
        self, batch, padding=True, truncation=True, max_length=512, return_tensors="pt"
    ):
        mask = np.ones((len(batch), 4), dtype=np.float32)
        return _FakeBatchEncoding({"attention_mask": _FakeTensor(mask)})


class FakeAutoTokenizer:
    @staticmethod
    def from_pretrained(model_path, cache_dir=None):
        return FakeTokenizerInstance()


_st.SentenceTransformer = FakeSentenceTransformer
_tf.AutoModel = FakeAutoModel
_tf.AutoTokenizer = FakeAutoTokenizer


@pytest.fixture(autouse=True)
def offline_env(monkeypatch, tmp_path):
    """Force CPU + redirect the default cache dir into a tmp dir."""
    import proximadb_sdk.embedding_providers.multi_bert_provider as mbp

    monkeypatch.setattr(mbp.Path, "home", classmethod(lambda cls: tmp_path))
    yield


def make_st_provider(model_name="mpnet-base", **kw):
    return MultiBERTProvider(model_name=model_name, device="cpu", **kw)


# -- registration / config ---------------------------------------------------
def test_registered_resolves():
    assert type(get_provider("multi-bert")) is MultiBERTProvider
    assert type(get_provider("adaptive-bert")) is AdaptiveBERTProvider
    assert type(get_provider("bert")) is MultiBERTProvider  # alias


def test_models_registry_shape():
    assert "minilm-l6" in MultiBERTProvider.MODELS
    cfg = MultiBERTProvider.MODELS["minilm-l6"]
    assert cfg["dimension"] == 384
    assert cfg["size"] == ModelSize.MINI


def test_default_config():
    cfg = MultiBERTProvider(model_name="mpnet-base", device="cpu").default_config()
    assert cfg.model.name == "sentence-transformers/all-mpnet-base-v2"


# -- construction ------------------------------------------------------------
def test_init_sentence_transformer_path():
    p = make_st_provider("mpnet-base")
    assert p.model_name == "mpnet-base"
    assert p.device == "cpu"
    assert p.tokenizer is None
    assert isinstance(p.model, FakeSentenceTransformer)


def test_init_transformers_path():
    p = make_st_provider("bert-base")
    assert p.tokenizer is not None
    assert isinstance(p.model, FakeAutoModelInstance)


def test_invalid_model_name_raises():
    with pytest.raises(ValueError):
        make_st_provider("does-not-exist")


def test_init_size_autoselect_when_no_model_name():
    p = MultiBERTProvider(model_name="", size=ModelSize.MINI, device="cpu")
    assert MultiBERTProvider.MODELS[p.model_name]["size"] == ModelSize.MINI


def test_init_autoselect_when_no_model_name(monkeypatch):
    import psutil

    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=32e9)
    )
    p = MultiBERTProvider(model_name="", device="cpu")
    assert p.model_name in MultiBERTProvider.MODELS


def test_default_cache_dir_used():
    p = make_st_provider("mpnet-base")
    assert "proximadb" in str(p.cache_dir)


def test_custom_cache_dir(tmp_path):
    p = make_st_provider("mpnet-base", cache_dir=str(tmp_path / "models"))
    assert str(tmp_path) in str(p.cache_dir)


def test_init_options_propagate():
    p = make_st_provider(
        "mpnet-base", batch_size=8, normalize=False, pooling_strategy="cls"
    )
    assert p.batch_size == 8
    assert p.normalize is False
    assert p.pooling_strategy == "cls"


# -- auto-selection helpers (CPU branch) -------------------------------------
def test_auto_select_high_ram(monkeypatch):
    import psutil

    p = make_st_provider("mpnet-base")
    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=32e9)
    )
    assert p._auto_select_model() == "mpnet-base"


def test_auto_select_mid_ram(monkeypatch):
    import psutil

    p = make_st_provider("mpnet-base")
    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=10e9)
    )
    assert p._auto_select_model() == "distilbert"


def test_auto_select_low_ram(monkeypatch):
    import psutil

    p = make_st_provider("mpnet-base")
    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=4e9)
    )
    assert p._auto_select_model() == "minilm-l6"


def test_auto_select_max_memory_cap(monkeypatch):
    import psutil

    p = make_st_provider("mpnet-base")
    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=64e9)
    )
    assert p._auto_select_model(max_memory_gb=4) == "minilm-l6"


def test_select_model_by_size_prefers_st():
    p = make_st_provider("mpnet-base")
    chosen = p._select_model_by_size(ModelSize.MINI)
    assert MultiBERTProvider.MODELS[chosen]["is_sentence_transformer"]


def test_select_model_by_size_no_candidates():
    p = make_st_provider("mpnet-base")

    class _NoSize:
        pass

    assert p._select_model_by_size(_NoSize()) == "mpnet-base"


# -- encoding dispatch -------------------------------------------------------
def test_embed_texts_sentence_transformer():
    p = make_st_provider("mpnet-base")
    out = p.embed_texts(["hello", "world!!"])
    assert out.shape == (2, 768)


def test_embed_core_entrypoint():
    p = make_st_provider("mpnet-base")
    assert p.embed(["hi"]).shape == (1, 768)
    assert p.embed([]).size == 0


def test_embed_texts_caching_reuses():
    p = make_st_provider("mpnet-base")
    p.embed_texts(["abc"])
    assert len(p._cache) == 1
    p.embed_texts(["abc", "different"])
    assert len(p._cache) == 2


def test_embed_texts_transformers_mean_pool():
    p = make_st_provider("bert-base", pooling_strategy="mean")
    out = p.embed_texts(["one", "two"])
    assert out.shape == (2, 768)


def test_embed_texts_transformers_cls_pool():
    p = make_st_provider("bert-base", pooling_strategy="cls", normalize=False)
    out = p.embed_texts(["one"])
    assert out.shape == (1, 768)


def test_embed_texts_transformers_max_pool():
    p = make_st_provider("bert-base", pooling_strategy="max", normalize=True)
    out = p.embed_texts(["alpha", "beta"])
    assert out.shape == (2, 768)


def test_embed_texts_transformers_batching():
    p = make_st_provider("bert-base", batch_size=2)
    out = p.embed_texts(["a", "b", "c", "d", "e"])
    assert out.shape == (5, 768)


def test_embed_documents():
    p = make_st_provider("mpnet-base")
    docs = [{"text": "doc one"}, {"text": "doc two"}, {"nope": "x"}]
    out = p.embed_documents(docs)
    assert out.shape == (3, 768)


def test_embed_documents_custom_field():
    p = make_st_provider("mpnet-base")
    out = p.embed_documents([{"body": "hi"}], text_field="body")
    assert out.shape == (1, 768)


# -- info / dimension / benchmark --------------------------------------------
def test_get_dimension():
    p = make_st_provider("e5-large")
    assert p.get_dimension() == 1024


def test_get_model_info():
    p = make_st_provider("mpnet-base")
    info = p.get_model_info()
    assert info["provider"] == "MultiBERT"
    assert info["model"] == "mpnet-base"
    assert info["dimension"] == 768
    assert info["size"] == "base"
    assert info["device"] == "cpu"


def test_benchmark_default_texts():
    p = make_st_provider("mpnet-base")
    res = p.benchmark()
    assert res["dimension"] == 768
    assert res["batch_size"] == p.batch_size
    assert res["device"] == "cpu"
    assert res["texts_per_second"] > 0


def test_benchmark_custom_texts():
    p = make_st_provider("mpnet-base")
    res = p.benchmark(["only this one"])
    assert res["dimension"] == 768


# -- compare_models ----------------------------------------------------------
def test_compare_models_returns_dataframe():
    import pandas as pd

    df = MultiBERTProvider.compare_models(["hello"], models=["minilm-l6", "mpnet-base"])
    assert isinstance(df, pd.DataFrame)
    assert set(df["model"]) == {"minilm-l6", "mpnet-base"}


def test_compare_models_handles_failure(monkeypatch):
    real_init = MultiBERTProvider.__init__

    def flaky_init(self, *a, model_name="mpnet-base", **kw):
        if model_name == "bert-base":
            raise RuntimeError("boom")
        return real_init(self, model_name=model_name, **kw)

    monkeypatch.setattr(MultiBERTProvider, "__init__", flaky_init)
    df = MultiBERTProvider.compare_models(["x"], models=["minilm-l6", "bert-base"])
    assert list(df["model"]) == ["minilm-l6"]


def test_compare_models_default_models():
    import pandas as pd

    df = MultiBERTProvider.compare_models(["hi"], models=None)
    assert isinstance(df, pd.DataFrame)
    assert len(df) == 4


# -- AdaptiveBERTProvider ----------------------------------------------------
def test_adaptive_default_autoselect(monkeypatch):
    import psutil

    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=32e9)
    )
    p = AdaptiveBERTProvider(device="cpu")
    assert p.prefer_speed is False
    assert p.prefer_accuracy is False
    assert p.performance_stats["total_texts"] == 0


def test_adaptive_prefer_speed():
    p = AdaptiveBERTProvider(prefer_speed=True, device="cpu")
    assert p.model_name == "minilm-l12"


def test_adaptive_prefer_accuracy():
    p = AdaptiveBERTProvider(prefer_accuracy=True, device="cpu")
    assert p.model_name == "e5-large"


def test_adaptive_embed_updates_stats():
    p = AdaptiveBERTProvider(prefer_speed=True, device="cpu")
    out = p.embed_texts(["short text", "another"])
    assert out.shape[0] == 2
    assert p.performance_stats["total_texts"] == 2


def test_adaptive_switch_for_long_texts_speed():
    p = AdaptiveBERTProvider(prefer_speed=True, device="cpu")
    assert p.model_name == "minilm-l12"
    p.embed_texts(["x" * 1500])
    assert p.model_name == "minilm-l6"
    assert p.performance_stats["model_switches"] == 1


def test_adaptive_switch_for_short_texts_accuracy():
    p = AdaptiveBERTProvider(prefer_accuracy=True, device="cpu")
    p._switch_model("mpnet-base")
    assert p.model_name == "mpnet-base"
    p.embed_texts(["tiny"])
    assert p.model_name == "e5-large"


def test_adaptive_switch_model_noop_same():
    p = AdaptiveBERTProvider(prefer_speed=True, device="cpu")
    before = p.performance_stats["model_switches"]
    p._switch_model(p.model_name)
    assert p.performance_stats["model_switches"] == before


def test_adaptive_switch_model_changes_config():
    p = AdaptiveBERTProvider(prefer_speed=True, device="cpu")
    p._switch_model("bert-base")
    assert p.model_name == "bert-base"
    assert p.tokenizer is not None
    assert p.get_dimension() == 768


def test_adaptive_oom_fallback(monkeypatch):
    p = AdaptiveBERTProvider(prefer_accuracy=False, device="cpu")
    p._switch_model("mpnet-base")
    calls = {"n": 0}
    real = MultiBERTProvider.embed_texts

    def flaky(self, texts):
        calls["n"] += 1
        if calls["n"] == 1:
            raise _torch.cuda.OutOfMemoryError("oom")
        return real(self, texts)

    monkeypatch.setattr(MultiBERTProvider, "embed_texts", flaky)
    out = p.embed_texts(["recover please"])
    assert out.shape[0] == 1
    assert p.model_name == "minilm-l6"
