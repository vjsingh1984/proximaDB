"""
Offline unit tests for the cloud / optional-dep embedding providers, after they
were ported onto ``core.BaseEmbeddingProvider`` + ``@ProviderRegistry.register``:

    proximadb_sdk.embedding_providers.openai_provider.OpenAIProvider
    proximadb_sdk.embedding_providers.cohere.CohereProvider
    proximadb_sdk.embedding_providers.fastembed.FastEmbedProvider
    proximadb_sdk.embedding_providers.openai_compatible.OpenAICompatibleProvider

The heavy/cloud SDKs (openai, cohere, fastembed) are stubbed via ``sys.modules``
so the providers' lazy ``_load_model`` succeeds with no real package, model
download, or network. ``requests`` is monkeypatched for the REST provider.

The legacy InstructorProvider (still on the System-B base shim) is covered
separately.
"""

import sys
import types

import numpy as np
import pytest

from proximadb_sdk.embedding_providers.core.config import ModelMetadata, ProviderConfig


# --------------------------------------------------------------------------- #
# Stub the optional heavy / cloud SDKs BEFORE importing the targets.
# --------------------------------------------------------------------------- #
def _install_optional_stubs():
    # ---- openai (>=1.0 client) ------------------------------------------- #
    openai_mod = types.ModuleType("openai")

    class _Item:
        def __init__(self, index, embedding):
            self.index = index
            self.embedding = embedding

    class _Usage:
        total_tokens = 7

    class _Resp:
        def __init__(self, n, dim=4):
            # Return out-of-order to exercise index-based reordering.
            self.data = [_Item(n - 1 - i, [0.1, 0.2, 0.3, 0.4][:dim]) for i in range(n)]
            self.usage = _Usage()

    class _Embeddings:
        def __init__(self, client):
            self._client = client

        def create(self, input=None, model=None, encoding_format=None, dimensions=None):
            self._client.last_create = {
                "input": input,
                "model": model,
                "dimensions": dimensions,
            }
            return _Resp(len(input))

    class _OpenAI:
        def __init__(self, api_key=None, organization=None, base_url=None, **kw):
            self.api_key = api_key
            self.organization = organization
            self.base_url = base_url
            self.embeddings = _Embeddings(self)
            self.last_create = None

    openai_mod.OpenAI = _OpenAI
    sys.modules["openai"] = openai_mod

    # ---- cohere (ClientV2) ----------------------------------------------- #
    cohere_mod = types.ModuleType("cohere")

    class _Embeddings2:
        def __init__(self, n):
            self.float = [[0.5, 0.6, 0.7] for _ in range(n)]

    class _CohereResp:
        def __init__(self, n):
            self.embeddings = _Embeddings2(n)

            class _Billed:
                input_tokens = 11 * n

            class _Meta:
                billed_units = _Billed()

            self.meta = _Meta()

    class _ClientV2:
        def __init__(self, api_key=None):
            self.api_key = api_key

        def embed(
            self,
            texts=None,
            model=None,
            input_type=None,
            embedding_types=None,
            truncate=None,
        ):
            self.last_embed = {"input_type": input_type, "model": model}
            return _CohereResp(len(texts))

    cohere_mod.ClientV2 = _ClientV2
    sys.modules["cohere"] = cohere_mod

    # ---- fastembed ------------------------------------------------------- #
    fastembed_mod = types.ModuleType("fastembed")

    class _TextEmbedding:
        def __init__(self, model_name=None, max_length=None, cache_dir=None):
            self.model_name = model_name

        def embed(self, texts, batch_size=None):
            for _ in texts:
                yield np.array([0.9, 0.8, 0.7], dtype=np.float32)

    fastembed_mod.TextEmbedding = _TextEmbedding
    sys.modules["fastembed"] = fastembed_mod


_install_optional_stubs()


from proximadb_sdk.embedding_providers.cohere import (  # noqa: E402
    COHERE_MODELS,
    CohereProvider,
)
from proximadb_sdk.embedding_providers.fastembed import (  # noqa: E402
    FASTEMBED_MODELS,
    FastEmbedProvider,
)
from proximadb_sdk.embedding_providers.openai_compatible import (  # noqa: E402
    OpenAICompatibleProvider,
    _embeddings_url,
)
from proximadb_sdk.embedding_providers.openai_provider import (  # noqa: E402
    OPENAI_MODELS,
    OpenAIProvider,
)


def _cfg(models, key, **overrides):
    extra = {"show_cost_warnings": False}
    extra.update(overrides.pop("extra", {}))
    return ProviderConfig(model=models[key], extra=extra, **overrides)


# =========================================================================== #
# OpenAIProvider
# =========================================================================== #
class TestOpenAIProvider:
    def test_default_config(self):
        cfg = OpenAIProvider().default_config()
        assert cfg.model.name == "text-embedding-3-small"
        assert cfg.model.dimension == 1536
        assert cfg.normalize is False

    def test_missing_key_raises_on_use(self, monkeypatch):
        monkeypatch.delenv("OPENAI_API_KEY", raising=False)
        p = OpenAIProvider(_cfg(OPENAI_MODELS, "text-embedding-3-small"))
        with pytest.raises(RuntimeError):
            p.embed(["hi"])
        assert p.is_available() is False

    def test_embed_parses_and_orders(self):
        cfg = _cfg(
            OPENAI_MODELS,
            "text-embedding-3-small",
            batch_size=2,
            normalize=False,
            extra={"api_key": "sk-key"},
        )
        p = OpenAIProvider(cfg)
        out = p.embed(["a", "b", "c"])
        assert out.shape == (3, 4)
        assert p.get_token_usage()["estimated_tokens"] > 0

    def test_embed_normalizes(self):
        cfg = _cfg(
            OPENAI_MODELS,
            "text-embedding-3-small",
            normalize=True,
            extra={"api_key": "sk-key"},
        )
        out = OpenAIProvider(cfg).embed(["hello"])
        np.testing.assert_allclose(np.linalg.norm(out[0]), 1.0, rtol=1e-5)

    def test_embed_empty(self):
        cfg = _cfg(OPENAI_MODELS, "text-embedding-3-small", extra={"api_key": "k"})
        assert OpenAIProvider(cfg).embed([]).size == 0

    def test_matryoshka_dimensions_passed(self):
        cfg = _cfg(
            OPENAI_MODELS,
            "text-embedding-3-large",
            extra={"api_key": "k", "dimensions": 256},
        )
        p = OpenAIProvider(cfg)
        assert p.get_dimension() == 256
        p.embed(["x"])
        assert p._model.last_create["dimensions"] == 256

    def test_matryoshka_ignored_for_ada(self):
        cfg = _cfg(
            OPENAI_MODELS,
            "text-embedding-ada-002",
            extra={"api_key": "k", "dimensions": 256},
        )
        p = OpenAIProvider(cfg)
        # ada-002 is not Matryoshka -> dimension unchanged + param dropped.
        assert p.get_dimension() == 1536
        p.embed(["x"])
        assert p._model.last_create["dimensions"] is None

    def test_cost_warning_emitted(self):
        cfg = ProviderConfig(
            model=OPENAI_MODELS["text-embedding-3-small"],
            extra={"api_key": "k", "show_cost_warnings": True},
        )
        p = OpenAIProvider(cfg)
        with pytest.warns(UserWarning):
            p.embed(["a"])

    def test_api_error_wrapped(self):
        cfg = _cfg(OPENAI_MODELS, "text-embedding-3-small", extra={"api_key": "k"})
        p = OpenAIProvider(cfg)
        p.ensure_initialized()

        def _boom(**kw):
            raise ValueError("api down")

        p._model.embeddings.create = _boom
        with pytest.raises(RuntimeError):
            p.embed(["a"])


# =========================================================================== #
# CohereProvider
# =========================================================================== #
class TestCohereProvider:
    def test_default_config(self):
        cfg = CohereProvider().default_config()
        assert cfg.model.name == "embed-english-light-v3.0"
        assert cfg.extra["input_type"] == "search_document"

    def test_missing_key_raises(self, monkeypatch):
        monkeypatch.delenv("COHERE_API_KEY", raising=False)
        p = CohereProvider(_cfg(COHERE_MODELS, "embed-english-v3.0"))
        with pytest.raises(RuntimeError):
            p.embed(["a"])

    def test_embed_dispatch(self):
        cfg = _cfg(
            COHERE_MODELS,
            "embed-english-light-v3.0",
            batch_size=2,
            normalize=False,
            extra={"api_key": "co-key"},
        )
        out = CohereProvider(cfg).embed(["a b c", "d e", "f"])
        assert out.shape == (3, 3)

    def test_embed_query_uses_search_query(self):
        cfg = _cfg(
            COHERE_MODELS,
            "embed-english-light-v3.0",
            normalize=False,
            extra={"api_key": "co-key"},
        )
        p = CohereProvider(cfg)
        p.embed_query("a query")
        assert p._model.last_embed["input_type"] == "search_query"

    def test_embed_passages_uses_search_document(self):
        cfg = _cfg(
            COHERE_MODELS,
            "embed-english-light-v3.0",
            normalize=False,
            extra={"api_key": "co-key"},
        )
        p = CohereProvider(cfg)
        p.embed_passages(["a doc"])
        assert p._model.last_embed["input_type"] == "search_document"

    def test_invalid_input_type(self):
        cfg = _cfg(
            COHERE_MODELS, "embed-english-light-v3.0", extra={"api_key": "co-key"}
        )
        with pytest.raises(ValueError):
            CohereProvider(cfg).embed(["a"], input_type="nonsense")

    def test_token_usage(self):
        cfg = _cfg(
            COHERE_MODELS, "embed-english-light-v3.0", extra={"api_key": "co-key"}
        )
        p = CohereProvider(cfg)
        p.embed(["hello world"])
        assert p.get_token_usage()["model"] == "embed-english-light-v3.0"

    def test_api_error_wrapped(self):
        cfg = _cfg(
            COHERE_MODELS, "embed-english-light-v3.0", extra={"api_key": "co-key"}
        )
        p = CohereProvider(cfg)
        p.ensure_initialized()

        def _boom(**kw):
            raise ValueError("cohere down")

        p._model.embed = _boom
        with pytest.raises(RuntimeError):
            p.embed(["a"])


# =========================================================================== #
# FastEmbedProvider
# =========================================================================== #
class TestFastEmbedProvider:
    def test_default_config(self):
        cfg = FastEmbedProvider().default_config()
        assert cfg.model.name == "BAAI/bge-small-en-v1.5"
        assert cfg.model.dimension == 384

    def test_embed_dispatch(self):
        out = FastEmbedProvider().embed(["a", "b", "c"])
        assert out.shape == (3, 3)

    def test_embed_empty(self):
        assert FastEmbedProvider().embed([]).size == 0

    def test_dimension_lookup(self):
        cfg = _cfg(FASTEMBED_MODELS, "BAAI/bge-base-en-v1.5")
        assert FastEmbedProvider(cfg).get_dimension() == 768


# =========================================================================== #
# OpenAICompatibleProvider (REST via mocked `requests`)
# =========================================================================== #
class _FakeResp:
    def __init__(self, status_code=200, payload=None, text=""):
        self.status_code = status_code
        self._payload = payload if payload is not None else {}
        self.text = text

    def json(self):
        return self._payload


def _fake_post_factory(embedding=(0.1, 0.2, 0.3, 0.4), status=200):
    calls = []

    def _post(url, json=None, headers=None, timeout=None):
        calls.append({"url": url, "json": json, "headers": headers})
        if status != 200:
            return _FakeResp(status_code=status, text="boom")
        data = [
            {"index": i, "embedding": list(embedding)}
            for i, _ in enumerate(json["input"])
        ]
        return _FakeResp(status_code=200, payload={"data": data})

    _post.calls = calls
    return _post


class TestOpenAICompatibleProvider:
    def test_embeddings_url_preserves_v1(self):
        # Regression: urljoin(".../v1", "/embeddings") dropped "/v1".
        assert _embeddings_url("http://x/v1") == "http://x/v1/embeddings"
        assert _embeddings_url("http://x/v1/") == "http://x/v1/embeddings"
        assert _embeddings_url("http://x") == "http://x/embeddings"

    def test_default_config(self):
        cfg = OpenAICompatibleProvider().default_config()
        assert cfg.model.name == "nomic-embed-text"
        assert cfg.extra["api_base"].endswith("/v1")

    def test_embed_dispatch_and_url(self, monkeypatch):
        import proximadb_sdk.embedding_providers.openai_compatible as mod

        post = _fake_post_factory(embedding=(1.0, 0.0, 0.0))
        monkeypatch.setattr(mod.requests, "post", post)
        cfg = ProviderConfig(
            model=OPENAI_COMPATIBLE_MODEL(),
            batch_size=2,
            normalize=False,
            extra={"api_base": "http://x/v1", "api_key": "k"},
        )
        p = OpenAICompatibleProvider(cfg)
        out = p.embed(["a", "b", "c"])
        assert out.shape == (3, 3)
        # /v1 preserved on the request URL.
        assert post.calls[0]["url"] == "http://x/v1/embeddings"
        assert post.calls[0]["headers"]["Authorization"] == "Bearer k"

    def test_embed_no_key_no_auth(self, monkeypatch):
        import proximadb_sdk.embedding_providers.openai_compatible as mod

        post = _fake_post_factory(embedding=(0.1, 0.2))
        monkeypatch.setattr(mod.requests, "post", post)
        cfg = ProviderConfig(
            model=OPENAI_COMPATIBLE_MODEL(),
            normalize=False,
            extra={"api_base": "http://x/v1", "api_key": None},
        )
        p = OpenAICompatibleProvider(cfg)
        p.embed(["a"])
        assert "Authorization" not in post.calls[0]["headers"]

    def test_embed_failure_raises(self, monkeypatch):
        import proximadb_sdk.embedding_providers.openai_compatible as mod

        post = _fake_post_factory(status=503)
        monkeypatch.setattr(mod.requests, "post", post)
        cfg = ProviderConfig(
            model=OPENAI_COMPATIBLE_MODEL(),
            extra={"api_base": "http://x/v1", "api_key": None},
        )
        with pytest.raises(RuntimeError):
            OpenAICompatibleProvider(cfg).embed(["a"])

    def test_embed_empty(self):
        assert OpenAICompatibleProvider().embed([]).size == 0

    def test_create_ollama_provider(self):
        p = OpenAICompatibleProvider.create_ollama_provider(
            model_name="all-minilm", host="h", port=1234
        )
        assert p.config.extra["api_base"] == "http://h:1234/v1"
        assert p.get_dimension() == 384

    def test_create_vllm_provider(self):
        p = OpenAICompatibleProvider.create_vllm_provider(
            model_name="BAAI/bge-base-en-v1.5", host="h", port=8001
        )
        assert p.config.extra["api_base"] == "http://h:8001/v1"
        assert p.get_dimension() == 768


def OPENAI_COMPATIBLE_MODEL():
    from proximadb_sdk.embedding_providers.openai_compatible import (
        OPENAI_COMPATIBLE_MODELS,
    )

    return OPENAI_COMPATIBLE_MODELS["nomic-embed-text"]
