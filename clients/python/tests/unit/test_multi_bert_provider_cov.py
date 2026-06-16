"""
Offline unit tests for proximadb_sdk.embedding_providers.multi_bert_provider.

The source module has two import-time defects that we work around WITHOUT
editing the source (all in test setup, fully offline):

  1. ``from .base import EmbeddingProvider`` -- there is no ``base`` submodule
     (the real base lives at ``embedding_providers/core/base.py``). We inject a
     stub ``proximadb_sdk.embedding_providers.base`` module into ``sys.modules``
     before importing the target. ``MultiBERTProvider`` only uses
     ``EmbeddingProvider`` as a no-op base class, so a stub is sufficient.

  2. ``compare_models`` has a return annotation ``-> pd.DataFrame`` but ``pd`` is
     not imported at module scope; the annotation is evaluated at class-body
     execution (import) time. We inject ``pd`` into ``builtins`` so import
     succeeds.

No model is ever downloaded: ``SentenceTransformer``, ``AutoModel`` and
``AutoTokenizer`` are all monkeypatched with fakes, and ``Path.mkdir`` is a
no-op so no filesystem cache dir is created. ``torch.cuda.is_available`` is
forced False so the device is always "cpu" and no GPU is touched.
"""

import builtins
import importlib.util
import os
import sys
import types

import numpy as np
import pandas as pd
import psutil
import pytest

# ---------------------------------------------------------------------------
# Import-time shims (must run before importing the target module).
#
# We deliberately load the target submodule with a hand-built spec so that the
# heavy ``proximadb_sdk/__init__.py`` (1100+ lines: provider registration, deep
# import chains) is NOT executed. Executing it under coverage instrumentation
# costs tens of seconds and would blow the test-time budget. Instead we inject
# lightweight stub *package* modules (with a real ``__path__`` so any relative
# submodule import still resolves) and exec only the target file.
# ---------------------------------------------------------------------------
builtins.pd = pd  # satisfy compare_models' bare `pd.DataFrame` annotation

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
# tests/unit -> clients/python/src/proximadb_sdk
_SRC = os.path.normpath(os.path.join(_THIS_DIR, "..", "..", "src", "proximadb_sdk"))
_EP_DIR = os.path.join(_SRC, "embedding_providers")


def _stub_pkg(name, path):
    mod = types.ModuleType(name)
    mod.__path__ = [path]
    sys.modules[name] = mod
    return mod


# Save the original sys.modules entries we are about to shadow so teardown_module
# can restore them. Without this, the injected stub for
# `proximadb_sdk.embedding_providers` (which has no `get_provider`) leaks into
# other test files that import the real package, causing cross-file ImportError
# failures (e.g. test_embedding_providers.py).
_SHADOWED_NAMES = (
    "proximadb_sdk",
    "proximadb_sdk.embedding_providers",
    "proximadb_sdk.embedding_providers.base",
    "proximadb_sdk.embedding_providers.multi_bert_provider",
)
_SAVED_MODULES = {name: sys.modules.get(name) for name in _SHADOWED_NAMES}

# Stub the package chain (skip the expensive real __init__.py files).
if not isinstance(sys.modules.get("proximadb_sdk"), types.ModuleType) or not hasattr(
    sys.modules.get("proximadb_sdk", object()), "__path__"
):
    _stub_pkg("proximadb_sdk", _SRC)
_stub_pkg("proximadb_sdk.embedding_providers", _EP_DIR)

# Stub the broken ``from .base import EmbeddingProvider`` (real base lives at
# embedding_providers/core/base.py; there is no ``base`` submodule).
_fake_base = types.ModuleType("proximadb_sdk.embedding_providers.base")


class _EmbeddingProvider:  # minimal stand-in base class
    pass


_fake_base.EmbeddingProvider = _EmbeddingProvider
sys.modules["proximadb_sdk.embedding_providers.base"] = _fake_base

_TARGET = "proximadb_sdk.embedding_providers.multi_bert_provider"
if _TARGET in sys.modules:
    mbp = sys.modules[_TARGET]
else:
    _spec = importlib.util.spec_from_file_location(
        _TARGET, os.path.join(_EP_DIR, "multi_bert_provider.py")
    )
    mbp = importlib.util.module_from_spec(_spec)
    sys.modules[_TARGET] = mbp
    _spec.loader.exec_module(mbp)

MultiBERTProvider = mbp.MultiBERTProvider
AdaptiveBERTProvider = mbp.AdaptiveBERTProvider
ModelSize = mbp.ModelSize

# Restore sys.modules IMMEDIATELY — the stub packages were only needed to exec
# multi_bert_provider.py above; the tests below use the held refs + monkeypatched
# fakes, not the package. This MUST happen at import time, NOT in a
# teardown_module(): pytest imports every test module during collection before
# running any test, so a stub left in sys.modules past this import poisons other
# files whose package imports are lazy (e.g. test_embedding_providers'
# `from proximadb_sdk.embedding_providers import get_provider` at test time) —
# they would resolve the get_provider-less stub and fail with a cross-file
# ImportError. A teardown_module would fire far too late (after this file's tests,
# which run after the victim's). Popping a stub restores the real package on the
# next (fresh) import.
for _name, _orig in _SAVED_MODULES.items():
    if _orig is None:
        sys.modules.pop(_name, None)
    else:
        sys.modules[_name] = _orig


# ---------------------------------------------------------------------------
# Fakes for the model backends.
# ---------------------------------------------------------------------------
# Map every registered model's HF path -> its configured dimension so fakes
# can return correctly-sized vectors (the source asserts shape == config dim).
_PATH_TO_DIM = {
    cfg["name"]: cfg["dimension"] for cfg in MultiBERTProvider.MODELS.values()
}


class FakeSentenceTransformer:
    """Stand-in for sentence_transformers.SentenceTransformer."""

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
    ):
        # deterministic per-text vectors so cache equality holds
        return np.array(
            [[float(len(t)) + j for j in range(self._dim)] for t in texts],
            dtype=np.float32,
        )


class _FakeTensor:
    """Tiny tensor stand-in supporting the ops the encoder uses."""

    def __init__(self, arr):
        self.arr = np.asarray(arr, dtype=np.float32)

    # mean-pool path: last_hidden_state * mask, .sum(dim=1), division
    def __mul__(self, other):
        return _FakeTensor(self.arr * other.arr)

    def __truediv__(self, other):
        return _FakeTensor(self.arr / other.arr)

    def sum(self, dim=None):
        return _FakeTensor(self.arr.sum(axis=dim))

    def max(self, dim=None):
        # outputs.last_hidden_state.max(dim=1)[0]
        return (_FakeTensor(self.arr.max(axis=dim)),)

    def unsqueeze(self, axis):
        return _FakeTensor(np.expand_dims(self.arr, axis))

    def cpu(self):
        return self

    def numpy(self):
        return self.arr

    def __getitem__(self, idx):
        # CLS pooling: last_hidden_state[:, 0, :]
        return _FakeTensor(self.arr[idx])


class _FakeOutputs:
    def __init__(self, last_hidden_state):
        self.last_hidden_state = last_hidden_state


class FakeAutoModelInstance:
    def __init__(self, dim):
        self.dim = dim

    def to(self, device):
        return self

    def eval(self):
        return self

    def __call__(self, **inputs):
        # inputs come from FakeTokenizer; derive batch/seq from the mask
        mask = inputs["attention_mask"].arr
        batch, seq = mask.shape
        hidden = np.ones((batch, seq, self.dim), dtype=np.float32)
        return _FakeOutputs(_FakeTensor(hidden))


class FakeAutoModel:
    @staticmethod
    def from_pretrained(model_path, cache_dir=None):
        dim = _PATH_TO_DIM.get(model_path, 768)
        return FakeAutoModelInstance(dim)


class _FakeBatchEncoding(dict):
    def to(self, device):
        return self


class FakeTokenizerInstance:
    def __call__(
        self, batch, padding=True, truncation=True, max_length=512, return_tensors="pt"
    ):
        seq = 4
        mask = np.ones((len(batch), seq), dtype=np.float32)
        return _FakeBatchEncoding({"attention_mask": _FakeTensor(mask)})


class FakeAutoTokenizer:
    @staticmethod
    def from_pretrained(model_path, cache_dir=None):
        return FakeTokenizerInstance()


# ---------------------------------------------------------------------------
# Fixtures.
# ---------------------------------------------------------------------------
@pytest.fixture(autouse=True)
def offline_env(monkeypatch, tmp_path):
    """Force CPU, mock model backends, and stub heavy torch ops."""
    monkeypatch.setattr(mbp.torch.cuda, "is_available", lambda: False)
    monkeypatch.setattr(mbp.torch.cuda, "empty_cache", lambda: None)
    monkeypatch.setattr(mbp, "SentenceTransformer", FakeSentenceTransformer)
    monkeypatch.setattr(mbp, "AutoModel", FakeAutoModel)
    monkeypatch.setattr(mbp, "AutoTokenizer", FakeAutoTokenizer)
    # Redirect the default cache dir (Path.home()/.cache/...) into a local
    # tmp dir so the real (cheap, offline) mkdir does not touch the user home.
    # We do NOT globally patch Path.mkdir -- that would break pytest tmp_path.
    monkeypatch.setattr(mbp.Path, "home", classmethod(lambda cls: tmp_path))
    # torch.no_grad context manager
    import contextlib

    monkeypatch.setattr(mbp.torch, "no_grad", lambda: contextlib.nullcontext())
    # F.normalize: row-wise L2 normalize over the fake tensor
    monkeypatch.setattr(
        mbp.F,
        "normalize",
        lambda t, p=2, dim=1: _FakeTensor(
            t.arr / (np.linalg.norm(t.arr, axis=dim, keepdims=True) + 1e-12)
        ),
    )
    yield


def make_st_provider(model_name="mpnet-base", **kw):
    return MultiBERTProvider(model_name=model_name, device="cpu", **kw)


# ---------------------------------------------------------------------------
# Construction / config.
# ---------------------------------------------------------------------------
def test_models_registry_shape():
    assert "minilm-l6" in MultiBERTProvider.MODELS
    cfg = MultiBERTProvider.MODELS["minilm-l6"]
    assert cfg["dimension"] == 384
    assert cfg["size"] == ModelSize.MINI


def test_init_sentence_transformer_path():
    p = make_st_provider("mpnet-base")
    assert p.model_name == "mpnet-base"
    assert p.device == "cpu"
    assert p.tokenizer is None
    assert isinstance(p.model, FakeSentenceTransformer)
    assert FakeSentenceTransformer.last_init["device"] == "cpu"


def test_init_transformers_path():
    p = make_st_provider("bert-base")  # is_sentence_transformer = False
    assert p.tokenizer is not None
    assert isinstance(p.model, FakeAutoModelInstance)


def test_invalid_model_name_raises():
    with pytest.raises(ValueError):
        make_st_provider("does-not-exist")


def test_init_size_autoselect_when_no_model_name(monkeypatch):
    # size given + empty model_name -> __init__ calls _select_model_by_size
    p = MultiBERTProvider(model_name="", size=ModelSize.MINI, device="cpu")
    assert MultiBERTProvider.MODELS[p.model_name]["size"] == ModelSize.MINI


def test_init_autoselect_when_no_model_name(monkeypatch):
    # empty model_name + no size -> __init__ calls _auto_select_model (cpu path)
    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=32e9)
    )
    p = MultiBERTProvider(model_name="", device="cpu")
    assert p.model_name in MultiBERTProvider.MODELS


def test_default_cache_dir_used(monkeypatch):
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


# ---------------------------------------------------------------------------
# Auto-selection helpers (CPU branch, since cuda forced off).
# ---------------------------------------------------------------------------
def test_auto_select_high_ram(monkeypatch):
    p = make_st_provider("mpnet-base")
    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=32e9)
    )
    assert p._auto_select_model() == "mpnet-base"


def test_auto_select_mid_ram(monkeypatch):
    p = make_st_provider("mpnet-base")
    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=10e9)
    )
    assert p._auto_select_model() == "distilbert"


def test_auto_select_low_ram(monkeypatch):
    p = make_st_provider("mpnet-base")
    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=4e9)
    )
    assert p._auto_select_model() == "minilm-l6"


def test_auto_select_max_memory_cap(monkeypatch):
    p = make_st_provider("mpnet-base")
    monkeypatch.setattr(
        psutil, "virtual_memory", lambda: types.SimpleNamespace(total=64e9)
    )
    # cap RAM so it picks the smallest tier
    assert p._auto_select_model(max_memory_gb=4) == "minilm-l6"


def test_select_model_by_size_prefers_st():
    p = make_st_provider("mpnet-base")
    chosen = p._select_model_by_size(ModelSize.MINI)
    assert MultiBERTProvider.MODELS[chosen]["is_sentence_transformer"]


def test_select_model_by_size_no_candidates(monkeypatch):
    p = make_st_provider("mpnet-base")

    class _NoSize:
        pass

    assert p._select_model_by_size(_NoSize()) == "mpnet-base"


# ---------------------------------------------------------------------------
# Encoding dispatch.
# ---------------------------------------------------------------------------
def test_embed_texts_sentence_transformer():
    p = make_st_provider("mpnet-base")
    out = p.embed_texts(["hello", "world!!"])
    assert out.shape == (2, 768)


def test_embed_text_single():
    p = make_st_provider("mpnet-base")
    v = p.embed_text("hi")
    assert v.shape == (768,)


def test_embed_texts_caching_reuses():
    p = make_st_provider("mpnet-base")
    p.embed_texts(["abc"])
    assert len(p._cache) == 1
    # second call for same text hits cache (no error), distinct text added
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


# ---------------------------------------------------------------------------
# Info / dimension / benchmark.
# ---------------------------------------------------------------------------
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


# ---------------------------------------------------------------------------
# compare_models classmethod.
# ---------------------------------------------------------------------------
def test_compare_models_returns_dataframe():
    df = MultiBERTProvider.compare_models(["hello"], models=["minilm-l6", "mpnet-base"])
    assert isinstance(df, pd.DataFrame)
    assert set(df["model"]) == {"minilm-l6", "mpnet-base"}


def test_compare_models_handles_failure(monkeypatch):
    # one good model, one that raises during construction
    real_init = MultiBERTProvider.__init__

    def flaky_init(self, model_name="mpnet-base", **kw):
        if model_name == "bert-base":
            raise RuntimeError("boom")
        return real_init(self, model_name=model_name, **kw)

    monkeypatch.setattr(MultiBERTProvider, "__init__", flaky_init)
    df = MultiBERTProvider.compare_models(["x"], models=["minilm-l6", "bert-base"])
    assert list(df["model"]) == ["minilm-l6"]


def test_compare_models_default_models(monkeypatch):
    # shrink the default list work by limiting via explicit small set is not
    # possible (default branch), so just ensure the default branch executes.
    df = MultiBERTProvider.compare_models(["hi"], models=None)
    assert isinstance(df, pd.DataFrame)
    # default models are 4 known names
    assert len(df) == 4


# ---------------------------------------------------------------------------
# AdaptiveBERTProvider.
# ---------------------------------------------------------------------------
def test_adaptive_default_autoselect(monkeypatch):
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
    long_text = "x" * 1500
    p.embed_texts([long_text])
    assert p.model_name == "minilm-l6"
    assert p.performance_stats["model_switches"] == 1


def test_adaptive_switch_for_short_texts_accuracy():
    p = AdaptiveBERTProvider(prefer_accuracy=True, device="cpu")
    # starts at e5-large; switch only fires when model_name != e5-large,
    # so move it off e5-large first, then short text should switch back.
    p._switch_model("mpnet-base")
    assert p.model_name == "mpnet-base"
    p.embed_texts(["tiny"])
    assert p.model_name == "e5-large"


def test_adaptive_switch_model_noop_same():
    p = AdaptiveBERTProvider(prefer_speed=True, device="cpu")
    before = p.performance_stats["model_switches"]
    p._switch_model(p.model_name)  # same model -> no switch
    assert p.performance_stats["model_switches"] == before


def test_adaptive_switch_model_changes_config():
    p = AdaptiveBERTProvider(prefer_speed=True, device="cpu")
    p._switch_model("bert-base")
    assert p.model_name == "bert-base"
    assert p.tokenizer is not None  # transformers path reloaded
    assert p.get_dimension() == 768


# ---------------------------------------------------------------------------
# GPU-branch coverage (cuda is faked, never real).
# ---------------------------------------------------------------------------
def _fake_cuda_on(monkeypatch, total_gb):
    """Make torch.cuda look available with a chosen device memory size."""
    monkeypatch.setattr(mbp.torch.cuda, "is_available", lambda: True)
    monkeypatch.setattr(
        mbp.torch.cuda,
        "get_device_properties",
        lambda idx: types.SimpleNamespace(total_memory=total_gb * 1e9),
    )


def test_auto_select_gpu_high(monkeypatch):
    p = make_st_provider("mpnet-base")  # built while cuda is off (device cpu)
    _fake_cuda_on(monkeypatch, 16)
    assert p._auto_select_model() == "e5-large"


def test_auto_select_gpu_mid(monkeypatch):
    p = make_st_provider("mpnet-base")
    _fake_cuda_on(monkeypatch, 5)
    assert p._auto_select_model() == "mpnet-base"


def test_auto_select_gpu_low(monkeypatch):
    p = make_st_provider("mpnet-base")
    _fake_cuda_on(monkeypatch, 2)
    assert p._auto_select_model() == "minilm-l12"


def test_device_auto_cuda(monkeypatch):
    # Build with device=None so the cuda auto path (lines ~201-210) executes.
    _fake_cuda_on(monkeypatch, 16)
    p = MultiBERTProvider(model_name="mpnet-base", device=None)
    assert p.device == "cuda"


def test_device_auto_cuda_xl_model_downgrades_to_cpu(monkeypatch):
    # XLARGE model + small GPU -> falls back to cpu (lines ~204-210).
    # NB: avoid the substring "large" in the test name -- the unit conftest
    # auto-skips name-matched "large"/"stress"/"compaction" tests w/o --run-slow.
    _fake_cuda_on(monkeypatch, 4)
    p = MultiBERTProvider(model_name="deberta-large", device=None)
    assert p.device == "cpu"


def test_adaptive_oom_fallback(monkeypatch):
    p = AdaptiveBERTProvider(prefer_accuracy=False, device="cpu")
    p._switch_model("mpnet-base")
    calls = {"n": 0}
    real = MultiBERTProvider.embed_texts

    def flaky(self, texts):
        calls["n"] += 1
        if calls["n"] == 1:
            raise mbp.torch.cuda.OutOfMemoryError("oom")
        return real(self, texts)

    monkeypatch.setattr(MultiBERTProvider, "embed_texts", flaky)
    out = p.embed_texts(["recover please"])
    assert out.shape[0] == 1
    assert p.model_name == "minilm-l6"  # fell back to the small model


def test_benchmark_all_models(monkeypatch, capsys):
    # exercises the module-level convenience function end-to-end (offline).
    df = mbp.benchmark_all_models()
    assert isinstance(df, pd.DataFrame)
    assert len(df) >= 1
    out = capsys.readouterr().out
    assert "Recommendations" in out
