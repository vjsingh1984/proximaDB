"""
Offline unit tests for the (core-based, registered) FinBERT / SEC-BERT providers.

TD-126 System-B collapse: FinBERTProvider + SECBERTProvider were ported onto
``core.BaseEmbeddingProvider`` and registered (``finbert`` / ``sec-bert``). The
heavy ``torch`` / ``transformers`` / ``sentence_transformers`` deps are imported
lazily inside the model-loading / encoding paths, so we stub them via
``sys.modules`` and exercise both backends fully offline (no model download).
"""

import contextlib
import sys
import types

import numpy as np
import pytest


class _FakeTensor:
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
_torch.cuda = types.SimpleNamespace(
    is_available=lambda: False, empty_cache=lambda: None
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
_torch.nn.functional = _F  # support `torch.nn.functional.normalize`
sys.modules["torch"] = _torch
sys.modules["torch.nn"] = _nn
sys.modules["torch.nn.functional"] = _F


class FakeSentenceTransformer:
    def __init__(self, model_path, device=None, cache_folder=None):
        self.model_path = model_path

    def encode(
        self,
        texts,
        batch_size=32,
        normalize_embeddings=True,
        show_progress_bar=False,
        convert_to_numpy=True,
    ):
        return np.ones((len(texts), 768), dtype=np.float32)


class FakeAutoModelInstance:
    def to(self, device):
        return self

    def eval(self):
        return self

    def __call__(self, **inputs):
        batch, seq = inputs["attention_mask"].arr.shape
        return _FakeOutputs(_FakeTensor(np.ones((batch, seq, 768), dtype=np.float32)))


class FakeAutoModel:
    @staticmethod
    def from_pretrained(model_path, cache_dir=None):
        return FakeAutoModelInstance()


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


_st = types.ModuleType("sentence_transformers")
_st.SentenceTransformer = FakeSentenceTransformer
sys.modules["sentence_transformers"] = _st

_tf = types.ModuleType("transformers")
_tf.AutoModel = FakeAutoModel
_tf.AutoTokenizer = FakeAutoTokenizer
sys.modules["transformers"] = _tf

from proximadb_sdk.embedding_providers import get_provider  # noqa: E402
from proximadb_sdk.embedding_providers.finbert_provider import (  # noqa: E402
    FINBERT_MODELS,
    SECBERT_MODELS,
    FinBERTProvider,
    SECBERTProvider,
)


# -- registration ------------------------------------------------------------
def test_registered_resolves():
    assert type(get_provider("finbert")) is FinBERTProvider
    assert type(get_provider("sec-bert")) is SECBERTProvider
    assert type(get_provider("financial-bert")) is FinBERTProvider  # alias
    assert type(get_provider("legal-bert")) is SECBERTProvider  # alias


# -- FinBERT -----------------------------------------------------------------
def test_finbert_default_config():
    p = FinBERTProvider()
    cfg = p.default_config()
    assert cfg.model.name == "ProsusAI/finbert"
    assert cfg.model.dimension == 768
    assert p.pooling_strategy == "mean"


def test_finbert_transformers_backend():
    p = FinBERTProvider()  # ProsusAI/finbert -> transformers
    out = p.embed(["The company revenue grew 12% in Q3 2024."])
    assert out.shape == (1, 768)
    assert p.get_dimension() == 768


def test_finbert_empty():
    assert FinBERTProvider().embed([]).size == 0


def test_finbert_sentence_transformer_backend():
    from proximadb_sdk.embedding_providers.core import ProviderConfig

    p = FinBERTProvider(
        ProviderConfig(
            model=FINBERT_MODELS["sentence-transformers/paraphrase-mpnet-base-v2"]
        )
    )
    out = p.embed(["sentence one", "sentence two"])
    assert out.shape == (2, 768)


@pytest.mark.parametrize("pooling", ["mean", "max", "cls"])
def test_finbert_pooling_strategies(pooling):
    from proximadb_sdk.embedding_providers.core import ProviderConfig

    p = FinBERTProvider(
        ProviderConfig(
            model=FINBERT_MODELS["ProsusAI/finbert"],
            extra={"pooling_strategy": pooling},
        )
    )
    out = p.embed(["alpha", "beta"])
    assert out.shape == (2, 768)


def test_finbert_embed_documents():
    p = FinBERTProvider()
    out = p.embed_documents([{"text": "a"}, {"text": "b"}])
    assert out.shape == (2, 768)


def test_finbert_preprocess():
    p = FinBERTProvider()
    out = p.preprocess_financial_text("Revenue of $1,200M grew 12% on 2024-01-01")
    assert "[MONEY]" in out
    assert "[PERCENT]" in out
    assert "[DATE]" in out
    assert "REVENUE" in out


# -- SEC-BERT ----------------------------------------------------------------
def test_secbert_default_config():
    p = SECBERTProvider()
    assert p.default_config().model.name == "nlpaueb/sec-bert-base"
    assert p.config.model.name in SECBERT_MODELS


def test_secbert_embed():
    p = SECBERTProvider()
    out = p.embed(["Item 1A. Risk Factors in this Form 10-K."])
    assert out.shape == (1, 768)


def test_secbert_preprocess_accession_before_cik():
    """Accession numbers must be normalised before the 10-digit CIK pattern
    (the 10-digit prefix of an accession would otherwise be mis-tagged)."""
    p = SECBERTProvider()
    out = p.preprocess_financial_text(
        "Form 10-K CIK 0000320193 accession 0000320193-24-000123"
    )
    assert "[ACCESSION]" in out
    assert "[CIK]" in out
    assert "FORM_10K" in out
