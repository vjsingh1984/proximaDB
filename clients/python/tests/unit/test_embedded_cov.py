"""Offline unit tests for proximadb_sdk.embedded.

Every transport is mocked. No real server/process/socket/model download is ever
touched. httpx is imported locally inside the embedded methods as ``import httpx``,
so patching ``httpx.AsyncClient`` (and the sync ``httpx.get``/``httpx.post``) at the
module level intercepts all I/O.
"""

import sys
import types

import pytest

from proximadb_sdk import embedded as emb
from proximadb_sdk.embedded import (
    BaseEmbeddingModel,
    EmbeddedCollection,
    EmbeddedConfig,
    EmbeddedMultiModalQueryExecutor,
    EmbeddedProximaDB,
    FunctionEmbeddingModel,
    OllamaEmbeddingModel,
    OpenAIEmbeddingModel,
    SentenceTransformerModel,
    create_embedding_model,
)


# ---------------------------------------------------------------------------
# Fake HTTP plumbing
# ---------------------------------------------------------------------------
class FakeResp:
    def __init__(self, payload=None, status_code=200):
        self._payload = payload if payload is not None else {}
        self.status_code = status_code
        self.headers = {}
        self.text = ""
        self.content = b""

    def json(self):
        return self._payload

    def raise_for_status(self):
        return None


class FakeAsyncClient:
    """Drop-in replacement for httpx.AsyncClient.

    Records every call and returns a programmable FakeResp. ``router`` maps
    (METHOD, path-substring) -> FakeResp or a callable(**kwargs)->FakeResp.
    """

    calls = []
    router = {}
    default = FakeResp({})

    def __init__(self, *a, **kw):
        pass

    async def __aenter__(self):
        return self

    async def __aexit__(self, *a):
        return False

    def _resolve(self, method, url, **kwargs):
        FakeAsyncClient.calls.append((method, url, kwargs))
        # Prefer the longest matching fragment so specific routes
        # (e.g. /nodes/n2/edges/outgoing) win over generic ones (/nodes/n2).
        best = None
        best_len = -1
        for (m, frag), resp in FakeAsyncClient.router.items():
            if m == method and frag in url and len(frag) > best_len:
                best, best_len = resp, len(frag)
        if best is not None:
            return best(**kwargs) if callable(best) else best
        return FakeAsyncClient.default

    async def get(self, url, **kwargs):
        return self._resolve("GET", url, **kwargs)

    async def post(self, url, **kwargs):
        return self._resolve("POST", url, **kwargs)

    async def put(self, url, **kwargs):
        return self._resolve("PUT", url, **kwargs)

    async def patch(self, url, **kwargs):
        return self._resolve("PATCH", url, **kwargs)

    async def delete(self, url, **kwargs):
        return self._resolve("DELETE", url, **kwargs)


@pytest.fixture(autouse=True)
def patch_httpx(monkeypatch):
    """Patch the httpx module so embedded's ``import httpx`` picks up our fakes."""
    fake_httpx = types.ModuleType("httpx")
    fake_httpx.AsyncClient = FakeAsyncClient
    fake_httpx.Client = FakeAsyncClient  # not awaited in tests but harmless

    def fake_get(url, **kw):
        FakeAsyncClient.calls.append(("GET", url, kw))
        return FakeResp({"status": "ok"})

    def fake_post(url, **kw):
        FakeAsyncClient.calls.append(("POST", url, kw))
        return FakeResp({"embedding": [0.1, 0.2], "data": [{"index": 0, "embedding": [0.1]}]})

    fake_httpx.get = fake_get
    fake_httpx.post = fake_post
    monkeypatch.setitem(sys.modules, "httpx", fake_httpx)
    FakeAsyncClient.calls = []
    FakeAsyncClient.router = {}
    FakeAsyncClient.default = FakeResp({})
    yield FakeAsyncClient


def started_db(**cfg_kw):
    """Build a DB that *thinks* it has already started (no subprocess)."""
    db = EmbeddedProximaDB(data_dir="/tmp/proximadb-test")
    db._started = True
    return db


# ---------------------------------------------------------------------------
# Embedding models (pure / no network in the paths we exercise)
# ---------------------------------------------------------------------------
def test_function_embedding_model():
    m = FunctionEmbeddingModel(embed_fn=lambda t: [float(len(t))], dimension=1)
    assert m.embed("abc") == [3.0]
    assert m.embed_batch(["a", "bb"]) == [[1.0], [2.0]]
    assert m.get_dimension() == 1


def test_function_embedding_model_batch_fn():
    m = FunctionEmbeddingModel(
        embed_fn=lambda t: [1.0],
        dimension=1,
        batch_fn=lambda ts: [[9.0] for _ in ts],
    )
    assert m.embed_batch(["x", "y"]) == [[9.0], [9.0]]


@pytest.mark.asyncio
async def test_function_embedding_async_default():
    m = FunctionEmbeddingModel(embed_fn=lambda t: [7.0], dimension=1)
    assert await m.embed_async("z") == [7.0]
    # default base batch async runs sync in executor
    assert await m.embed_batch_async(["z"]) == [[7.0]]


@pytest.mark.asyncio
async def test_function_embedding_async_custom():
    async def aembed(t):
        return [42.0]

    m = FunctionEmbeddingModel(
        embed_fn=lambda t: [0.0], dimension=1, async_embed_fn=aembed
    )
    assert await m.embed_async("hi") == [42.0]


def test_ollama_model_dimension():
    m = OllamaEmbeddingModel(model_name="nomic-embed-text", dimension=768)
    assert m.get_dimension() == 768
    # embed() uses module-level httpx.post which our fake returns embedding for
    assert m.embed("hello") == [0.1, 0.2]


@pytest.mark.asyncio
async def test_ollama_async_embed_batch():
    m = OllamaEmbeddingModel(dimension=768)

    async def fake(text):
        return [float(len(text))]

    m.embed_async = fake  # type: ignore
    out = await m.embed_batch_async(["a", "bb"])
    assert out == [[1.0], [2.0]]


def test_openai_model_requires_key(monkeypatch):
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    with pytest.raises(ValueError):
        OpenAIEmbeddingModel()


def test_openai_model_dimensions():
    m = OpenAIEmbeddingModel(model_name="text-embedding-3-large", api_key="k")
    assert m.get_dimension() == 3072
    m2 = OpenAIEmbeddingModel(model_name="unknown", api_key="k")
    assert m2.get_dimension() == 1536


def test_create_embedding_model_factory():
    assert isinstance(
        create_embedding_model("sentence-transformers"), SentenceTransformerModel
    )
    assert isinstance(create_embedding_model("ollama"), OllamaEmbeddingModel)
    assert isinstance(
        create_embedding_model("openai", api_key="k"), OpenAIEmbeddingModel
    )
    with pytest.raises(ValueError):
        create_embedding_model("nope")


def test_sentence_transformer_construct_no_load():
    # Just construct; do NOT call embed (would download). Cover __init__.
    m = SentenceTransformerModel(model_name="BAAI/bge-small-en-v1.5")
    assert m.model_name == "BAAI/bge-small-en-v1.5"
    assert m._model is None


def test_sentence_transformer_import_error(monkeypatch):
    # Force the lazy import to fail so we hit the ImportError branch.
    monkeypatch.setitem(sys.modules, "sentence_transformers", None)
    m = SentenceTransformerModel()
    with pytest.raises(ImportError):
        m._ensure_loaded()


# ---------------------------------------------------------------------------
# Config / construction
# ---------------------------------------------------------------------------
def test_config_defaults_and_urls():
    db = EmbeddedProximaDB(data_dir="/tmp/x")
    assert db.rest_url.endswith(str(db.config.rest_port))
    assert db.grpc_url.startswith("localhost:")
    assert isinstance(db.config, EmbeddedConfig)


def test_config_override():
    cfg = EmbeddedConfig(data_dir="/tmp/y", rest_port=20000)
    db = EmbeddedProximaDB(config=cfg)
    assert db.config.rest_port == 20000


def test_generate_config_writes_toml(tmp_path):
    cfg = EmbeddedConfig(data_dir=str(tmp_path / "data"))
    db = EmbeddedProximaDB(config=cfg)
    path = db._generate_config()
    assert path.endswith("embedded-config.toml")
    content = (tmp_path / "data" / "embedded-config.toml").read_text()
    assert "node_id" in content
    assert "ORION" in content


def test_find_binary_explicit():
    db = EmbeddedProximaDB(data_dir="/tmp/x", binary_path="/usr/bin/proximadb-server")
    assert db._find_binary() == "/usr/bin/proximadb-server"


def test_find_binary_not_found(monkeypatch):
    db = EmbeddedProximaDB(data_dir="/tmp/x")
    monkeypatch.setattr(emb.Path, "exists", lambda self: False)
    import shutil

    monkeypatch.setattr(shutil, "which", lambda p: None)
    with pytest.raises(RuntimeError):
        db._find_binary()


# ---------------------------------------------------------------------------
# Value conversion helpers (pure)
# ---------------------------------------------------------------------------
def test_to_sql_value_variants():
    db = started_db()
    assert db._to_sql_value(None) == {"null_value": None}
    assert db._to_sql_value(True) == {"bool_value": True}
    assert db._to_sql_value(5) == {"int64_value": 5}
    assert db._to_sql_value(1.5) == {"number_value": 1.5}
    assert db._to_sql_value("s") == {"string_value": "s"}
    assert "bytes_value" in db._to_sql_value(b"ab")
    arr = db._to_sql_value([1, "x"])
    assert "array_value" in arr
    obj = db._to_sql_value({"k": 1})
    assert "object_value" in obj
    # fallback branch
    assert db._to_sql_value(object()).get("string_value")


def test_convert_metadata():
    db = started_db()
    out = db._convert_metadata({"a": 1, "b": "x"})
    assert out["a"] == {"int64_value": 1}
    assert out["b"] == {"string_value": "x"}


def test_to_proxima_value_variants():
    db = started_db()
    assert db._to_proxima_value(None) is None
    assert db._to_proxima_value(3) == 3
    assert db._to_proxima_value("s") == "s"
    assert db._to_proxima_value(b"ab")["type"] == "binary"
    assert db._to_proxima_value([1, 2])["type"] == "array"
    assert db._to_proxima_value({"k": 1})["type"] == "jsonb"
    # already-typed dict passes through
    typed = {"type": "int64", "value": 5}
    assert db._to_proxima_value(typed) == typed
    assert isinstance(db._to_proxima_value(object()), str)


def test_normalize_record_payload():
    db = started_db()
    rec = {
        "id": "r1",
        "vector": [1, 2, 3],
        "props": {"a": 1},
        "metadata": {"b": "x"},
        "text_fields": {"body": "hi"},
    }
    out = db._normalize_record_payload(rec, 0)
    assert out["id"] == "r1"
    assert out["vector"] == [1.0, 2.0, 3.0]
    assert out["props"]["a"] == 1
    assert out["text_fields"] == {"body": "hi"}


def test_normalize_record_payload_typed_fields():
    db = started_db()

    class Dumpable:
        def model_dump(self, exclude_none=True):
            return {"value_type": "int64", "value": 9}

    rec = {
        "vector": [0.0],
        "typed_fields": {
            "n": Dumpable(),
            "m": {"value": 5, "value_type": "int64"},
            "p": "plain",
        },
    }
    out = db._normalize_record_payload(rec, 7)
    assert out["id"] == "record_7"  # missing id -> generated
    assert out["props"]["n"] == {"type": "int64", "value": 9}
    assert out["props"]["m"] == {"type": "int64", "value": 5}
    assert out["props"]["p"] == "plain"


def test_normalize_record_missing_vector():
    db = started_db()
    with pytest.raises(ValueError):
        db._normalize_record_payload({"id": "x"}, 0)


# ---------------------------------------------------------------------------
# Lifecycle (mocked process)
# ---------------------------------------------------------------------------
@pytest.mark.asyncio
async def test_start_success(monkeypatch, tmp_path):
    cfg = EmbeddedConfig(data_dir=str(tmp_path / "d"))
    db = EmbeddedProximaDB(config=cfg)
    monkeypatch.setattr(db, "_find_binary", lambda: "/bin/true")

    class FakeProc:
        pid = 1234

    monkeypatch.setattr(emb.subprocess, "Popen", lambda *a, **k: FakeProc())
    # health probe returns 200 immediately via fake httpx.get
    await db.start(timeout=5.0)
    assert db._started is True
    # second start is a no-op
    await db.start()


@pytest.mark.asyncio
async def test_start_timeout(monkeypatch, tmp_path):
    cfg = EmbeddedConfig(data_dir=str(tmp_path / "d2"))
    db = EmbeddedProximaDB(config=cfg)
    monkeypatch.setattr(db, "_find_binary", lambda: "/bin/true")

    class FakeProc:
        pid = 99

    monkeypatch.setattr(emb.subprocess, "Popen", lambda *a, **k: FakeProc())
    # make the health probe always raise -> never ready
    fh = sys.modules["httpx"]
    fh.get = lambda *a, **k: (_ for _ in ()).throw(RuntimeError("down"))
    # make _kill_process a no-op so we don't touch os.killpg on a fake pid
    monkeypatch.setattr(db, "_kill_process", lambda: None)
    # collapse the sleep so the loop spins fast
    async def fast_sleep(*a, **k):
        return None

    monkeypatch.setattr(emb.asyncio, "sleep", fast_sleep)
    with pytest.raises(TimeoutError):
        await db.start(timeout=0.05)


@pytest.mark.asyncio
async def test_stop_when_not_started():
    db = EmbeddedProximaDB(data_dir="/tmp/x")
    await db.stop()  # no-op, no error


@pytest.mark.asyncio
async def test_stop_calls_kill(monkeypatch):
    db = started_db()
    killed = {"v": False}
    monkeypatch.setattr(db, "_kill_process", lambda: killed.__setitem__("v", True))
    await db.stop()
    assert killed["v"] is True
    assert db._started is False


# ---------------------------------------------------------------------------
# Collection CRUD via REST (mocked)
# ---------------------------------------------------------------------------
@pytest.mark.asyncio
async def test_create_collection_with_dimension():
    db = started_db()
    FakeAsyncClient.router = {("POST", "/collections"): FakeResp({"success": True})}
    coll = await db.create_collection("c1", dimension=8)
    assert isinstance(coll, EmbeddedCollection)
    assert coll.dimension == 8
    assert "c1" in db._collections


@pytest.mark.asyncio
async def test_create_collection_with_model_string(monkeypatch):
    db = started_db()
    FakeAsyncClient.router = {("POST", "/collections"): FakeResp({"success": True})}
    # Avoid a real model download: stub get_dimension so the string branch
    # constructs a SentenceTransformerModel without loading it.
    monkeypatch.setattr(
        emb.SentenceTransformerModel, "get_dimension", lambda self: 384
    )
    coll = await db.create_collection("c2", embedding_model="some-model")
    assert coll.has_embedding_model
    assert coll.dimension == 384


@pytest.mark.asyncio
async def test_create_collection_with_model_instance():
    db = started_db()
    FakeAsyncClient.router = {("POST", "/collections"): FakeResp({"success": True})}
    model = FunctionEmbeddingModel(embed_fn=lambda t: [1.0] * 4, dimension=4)
    coll = await db.create_collection("c3", embedding_model=model)
    assert coll.dimension == 4
    assert coll.has_embedding_model


@pytest.mark.asyncio
async def test_create_collection_no_dim_no_model():
    db = started_db()
    with pytest.raises(ValueError):
        await db.create_collection("bad")


@pytest.mark.asyncio
async def test_create_collection_failure():
    db = started_db()
    FakeAsyncClient.router = {("POST", "/collections"): FakeResp({"success": False})}
    with pytest.raises(RuntimeError):
        await db.create_collection("c4", dimension=4)


@pytest.mark.asyncio
async def test_create_collection_already_exists():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/collections"): FakeResp({"error": "already exists"})
    }
    coll = await db.create_collection("c5", dimension=4)
    assert coll.name == "c5"


@pytest.mark.asyncio
async def test_get_collection_cached():
    db = started_db()
    db._collections["x"] = EmbeddedCollection("x", 4, db)
    got = await db.get_collection("x")
    assert got is db._collections["x"]


@pytest.mark.asyncio
async def test_get_collection_via_rest():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/collections/y"): FakeResp(
            {"collection": {"config": {"dimension": 16}}}
        )
    }
    got = await db.get_collection("y")
    assert got is not None
    assert got.dimension == 16


@pytest.mark.asyncio
async def test_get_collection_missing():
    db = started_db()
    FakeAsyncClient.router = {("GET", "/collections/z"): FakeResp({}, status_code=404)}
    assert await db.get_collection("z") is None


@pytest.mark.asyncio
async def test_delete_collection():
    db = started_db()
    db._collections["d"] = EmbeddedCollection("d", 4, db)
    FakeAsyncClient.router = {
        ("DELETE", "/collections/d"): FakeResp({}, status_code=200)
    }
    assert await db.delete_collection("d") is True
    assert "d" not in db._collections


@pytest.mark.asyncio
async def test_list_collections():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/api/v2/collections"): FakeResp(
            {"collections": [{"config": {"name": "a"}}, {"config": {"name": "b"}}]}
        )
    }
    assert await db.list_collections() == ["a", "b"]


@pytest.mark.asyncio
async def test_list_collections_empty():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/api/v2/collections"): FakeResp({}, status_code=500)
    }
    assert await db.list_collections() == []


# ---------------------------------------------------------------------------
# Records / search / stats
# ---------------------------------------------------------------------------
@pytest.mark.asyncio
async def test_insert_records():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/records/batch"): FakeResp({"success": True, "count": 1})
    }
    out = await db._insert_records("c", [{"id": "r", "vector": [1.0]}])
    assert out["success"] is True


@pytest.mark.asyncio
async def test_insert_vectors_alias():
    db = started_db()
    FakeAsyncClient.router = {("POST", "/records/batch"): FakeResp({"ok": 1})}
    out = await db._insert_vectors("c", [{"id": "r", "vector": [1.0]}])
    assert out == {"ok": 1}


@pytest.mark.asyncio
async def test_search_vectors_nested_results():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/search"): FakeResp(
            {"success": True, "results": {"results": [{"id": "1"}]}}
        )
    }
    out = await db._search_vectors("c", [1.0, 2.0], top_k=5, filters={"k": "v"})
    assert out == [{"id": "1"}]


@pytest.mark.asyncio
async def test_search_vectors_list_results():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/search"): FakeResp({"success": True, "results": [{"id": "2"}]})
    }
    out = await db._search_vectors("c", [1.0])
    assert out == [{"id": "2"}]


@pytest.mark.asyncio
async def test_search_vectors_no_results():
    db = started_db()
    FakeAsyncClient.router = {("POST", "/search"): FakeResp({"success": False})}
    assert await db._search_vectors("c", [1.0]) == []


@pytest.mark.asyncio
async def test_delete_vectors_stub():
    db = started_db()
    assert await db._delete_vectors("c", ["a"]) == 0


@pytest.mark.asyncio
async def test_get_collection_stats():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/collections/c"): FakeResp(
            {"collection": {"stats": {"vector_count": 42}}}
        )
    }
    stats = await db._get_collection_stats("c")
    assert stats["vector_count"] == 42


@pytest.mark.asyncio
async def test_get_collection_stats_miss():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/collections/c"): FakeResp({}, status_code=404)
    }
    assert await db._get_collection_stats("c") == {}


@pytest.mark.asyncio
async def test_health_check_true():
    db = started_db()
    FakeAsyncClient.router = {("GET", "/health"): FakeResp({}, status_code=200)}
    assert await db.health_check() is True


@pytest.mark.asyncio
async def test_health_check_not_started():
    db = EmbeddedProximaDB(data_dir="/tmp/x")
    assert await db.health_check() is False


@pytest.mark.asyncio
async def test_health_check_exception():
    db = started_db()
    fh = sys.modules["httpx"]

    class Boom:
        def __init__(self, *a, **k):
            raise RuntimeError("x")

    fh.AsyncClient = Boom
    assert await db.health_check() is False
    fh.AsyncClient = FakeAsyncClient


# ---------------------------------------------------------------------------
# EmbeddedCollection wrapper methods
# ---------------------------------------------------------------------------
@pytest.mark.asyncio
async def test_collection_insert_and_count():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/records/batch"): FakeResp({"success": True}),
        ("GET", "/collections/c"): FakeResp(
            {"collection": {"stats": {"vector_count": 3}}}
        ),
    }
    coll = EmbeddedCollection("c", 4, db)
    await coll.insert_records([{"id": "1", "vector": [1.0]}])
    await coll.insert([{"id": "2", "vector": [1.0]}])
    assert await coll.count() == 3


@pytest.mark.asyncio
async def test_collection_search_and_delete():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/search"): FakeResp({"success": True, "results": [{"id": "9"}]})
    }
    coll = EmbeddedCollection("c", 4, db)
    out = await coll.search([1.0], top_k=3, filters={"a": 1})
    assert out == [{"id": "9"}]
    assert await coll.delete(["9"]) == 0


@pytest.mark.asyncio
async def test_collection_insert_with_embedding():
    db = started_db()
    FakeAsyncClient.router = {("POST", "/records/batch"): FakeResp({"success": True})}
    model = FunctionEmbeddingModel(embed_fn=lambda t: [1.0, 2.0], dimension=2)
    coll = EmbeddedCollection("c", 2, db, embedding_model=model)
    out = await coll.insert_with_embedding(
        [{"id": "d1", "text": "hello", "metadata": {"k": "v"}}]
    )
    assert out["success"] is True


@pytest.mark.asyncio
async def test_collection_insert_with_embedding_no_model():
    db = started_db()
    coll = EmbeddedCollection("c", 2, db)
    with pytest.raises(RuntimeError):
        await coll.insert_with_embedding([{"id": "x", "text": "y"}])


@pytest.mark.asyncio
async def test_collection_search_text():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/search"): FakeResp({"success": True, "results": [{"id": "t"}]})
    }
    model = FunctionEmbeddingModel(embed_fn=lambda t: [1.0, 2.0], dimension=2)
    coll = EmbeddedCollection("c", 2, db, embedding_model=model)
    out = await coll.search_text("query", top_k=5)
    assert out == [{"id": "t"}]


@pytest.mark.asyncio
async def test_collection_search_text_no_model():
    db = started_db()
    coll = EmbeddedCollection("c", 2, db)
    with pytest.raises(RuntimeError):
        await coll.search_text("q")


def test_collection_set_embedding_model():
    db = started_db()
    coll = EmbeddedCollection("c", 2, db)
    assert coll.has_embedding_model is False
    coll.set_embedding_model(FunctionEmbeddingModel(lambda t: [1.0], 1))
    assert coll.has_embedding_model is True


# ---------------------------------------------------------------------------
# Document API
# ---------------------------------------------------------------------------
@pytest.mark.asyncio
async def test_create_document_collection():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/document-collections"): FakeResp({"collection_id": "doc1"})
    }
    out = await db.create_document_collection(
        "doc1", indexes=[{"path": "$.x"}], enable_fulltext=True, fulltext_paths=["$.y"]
    )
    assert out["collection_id"] == "doc1"


@pytest.mark.asyncio
async def test_insert_get_document():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/documents"): FakeResp({"id": "d1", "version": 1}),
        ("GET", "/documents/d1"): FakeResp({"id": "d1", "data": {}}),
    }
    ins = await db.insert_document("coll", {"k": "v"}, id="d1")
    assert ins["id"] == "d1"
    got = await db.get_document("coll", "d1")
    assert got["id"] == "d1"


@pytest.mark.asyncio
async def test_get_document_missing():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/documents/none"): FakeResp({}, status_code=404)
    }
    assert await db.get_document("coll", "none") is None


@pytest.mark.asyncio
async def test_query_documents():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/documents"): FakeResp({"documents": [{"id": "1"}]})
    }
    out = await db.query_documents(
        "coll", filter={"a": 1}, projection=["a", "b"], limit=5, offset=2
    )
    assert out["documents"] == [{"id": "1"}]


@pytest.mark.asyncio
async def test_query_documents_str_filter():
    db = started_db()
    FakeAsyncClient.router = {("GET", "/documents"): FakeResp({"documents": []})}
    out = await db.query_documents("coll", filter="$.a = 1")
    assert out == {"documents": []}


@pytest.mark.asyncio
async def test_update_document():
    db = started_db()
    FakeAsyncClient.router = {
        ("PATCH", "/documents/d1"): FakeResp({"version": 2})
    }
    out = await db.update_document("coll", "d1", [{"op": "SET", "path": "x", "value": 1}])
    assert out["version"] == 2


@pytest.mark.asyncio
async def test_delete_document_and_collection():
    db = started_db()
    FakeAsyncClient.router = {
        ("DELETE", "/documents/d1"): FakeResp({}, status_code=204),
        ("DELETE", "/document-collections/coll"): FakeResp({}, status_code=200),
    }
    assert await db.delete_document("coll", "d1") is True
    assert await db.delete_document_collection("coll") is True


# ---------------------------------------------------------------------------
# Time series API
# ---------------------------------------------------------------------------
@pytest.mark.asyncio
async def test_create_timeseries_collection():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/timeseries/collections"): FakeResp({"collection_id": "ts"})
    }
    out = await db.create_timeseries_collection(
        "ts",
        value_columns=[{"name": "v", "data_type": "f64"}],
        tag_columns=["host"],
        retention_ms=1000,
    )
    assert out["collection_id"] == "ts"


@pytest.mark.asyncio
async def test_ingest_timeseries():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/timeseries/ts/ingest"): FakeResp({"ingested": 2})
    }
    out = await db.ingest_timeseries("ts", [{"timestamp": 1, "values": {"v": 1.0}}])
    assert out["ingested"] == 2


@pytest.mark.asyncio
async def test_query_timeseries():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/timeseries/ts/query"): FakeResp({"metrics": []})
    }
    out = await db.query_timeseries(
        "ts",
        "2020-01-01",
        "2020-01-02",
        aggregation="AVG",
        bucket_ms=1000,
        tag_filters={"host": "a"},
    )
    assert out == {"metrics": []}


@pytest.mark.asyncio
async def test_aggregate_timeseries():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/timeseries/ts/aggregate"): FakeResp({"result": 1})
    }
    out = await db.aggregate_timeseries("ts", "a", "b", pipeline=[{"stage": "x"}])
    assert out["result"] == 1


@pytest.mark.asyncio
async def test_delete_timeseries_collection():
    db = started_db()
    FakeAsyncClient.router = {
        ("DELETE", "/timeseries/collections/ts"): FakeResp({}, status_code=204)
    }
    assert await db.delete_timeseries_collection("ts") is True


# ---------------------------------------------------------------------------
# Hybrid API
# ---------------------------------------------------------------------------
@pytest.mark.asyncio
async def test_hybrid_search():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/hybrid/search"): FakeResp({"results": [{"id": "h"}]})
    }
    out = await db.hybrid_search(
        "vc",
        [1.0, 2.0],
        text_query="t",
        filters={"k": "v"},
        fusion_params={"w": 1},
    )
    assert out["results"] == [{"id": "h"}]


@pytest.mark.asyncio
async def test_list_fusion_strategies():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/hybrid/strategies"): FakeResp({"strategies": [{"name": "rrf"}]})
    }
    out = await db.list_fusion_strategies()
    assert out == [{"name": "rrf"}]


@pytest.mark.asyncio
async def test_list_fusion_strategies_fail():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/hybrid/strategies"): FakeResp({}, status_code=500)
    }
    assert await db.list_fusion_strategies() == []


# ---------------------------------------------------------------------------
# connect_embedded convenience
# ---------------------------------------------------------------------------
@pytest.mark.asyncio
async def test_connect_embedded(monkeypatch):
    async def fake_start(self, timeout=30.0):
        self._started = True

    monkeypatch.setattr(EmbeddedProximaDB, "start", fake_start)
    db = await emb.connect_embedded(data_dir="/tmp/x")
    assert db._started is True


# ---------------------------------------------------------------------------
# Multi-modal executor: fusion + helpers (pure, no I/O)
# ---------------------------------------------------------------------------
def _executor():
    return EmbeddedMultiModalQueryExecutor(started_db())


def test_executor_extract_field():
    ex = _executor()
    assert ex._extract_field({"a": {"b": "c"}}, "a.b") == "c"
    assert ex._extract_field({"a": 1}, "x") is None
    assert ex._extract_field({"a": None}, "a") is None


def test_executor_fuse_single_and_empty():
    ex = _executor()
    assert ex._fuse_results([], "rrf", {}) == []
    assert ex._fuse_results([[{"id": "1"}]], "rrf", {}) == [{"id": "1"}]


def test_executor_fuse_rrf():
    ex = _executor()
    out = ex._fuse_results(
        [[{"id": "1", "_source_type": "vector"}], [{"id": "1"}, {"id": "2"}]],
        "rrf",
        {"vector_1": 2.0},
    )
    assert any("_rrf_score" in r for r in out)
    ids = [r["id"] for r in out]
    assert "1" in ids and "2" in ids


def test_executor_fuse_weighted():
    ex = _executor()
    out = ex._fuse_results(
        [[{"id": "1", "score": 0.5}], [{"id": "1", "score": 0.5}, {"id": "2", "score": 0.9}]],
        "weighted",
        {"component_0": 2.0},
    )
    assert all("_weighted_score" in r for r in out)


def test_executor_fuse_intersection():
    ex = _executor()
    out = ex._fuse_results(
        [[{"id": "1", "a": 1}, {"id": "2"}], [{"id": "1", "b": 2}]],
        "intersection",
        {},
    )
    assert len(out) == 1
    assert out[0]["id"] == "1"
    assert out[0].get("a") == 1 and out[0].get("b") == 2


def test_executor_fuse_union():
    ex = _executor()
    out = ex._fuse_results(
        [[{"id": "1"}, {"x": "noid"}], [{"id": "1"}, {"id": "2"}]],
        "union",
        {},
    )
    ids = [r.get("id") for r in out]
    assert ids.count("1") == 1
    assert "2" in ids


def test_executor_fuse_default_falls_to_rrf():
    ex = _executor()
    out = ex._fuse_results([[{"id": "1"}], [{"id": "2"}]], "unknown", {})
    assert any("_rrf_score" in r for r in out)


def test_executor_apply_joins():
    ex = _executor()
    comps = [
        [{"id": "1", "l": 1}, {"id": "2"}],
        [{"id": "1", "r": 9}],
    ]
    out = ex._apply_joins(comps, [{"join_type": "inner"}])
    assert len(out[0]) == 1
    assert out[0][0]["r"] == 9
    assert out[0][0]["_join_type"] == "inner"


def test_executor_apply_joins_too_few():
    ex = _executor()
    comps = [[{"id": "1"}]]
    assert ex._apply_joins(comps, [{"join_type": "inner"}]) == comps


def test_executor_apply_time_decay():
    ex = _executor()
    now = 1_000_000_000_000_000_000
    records = [
        {"id": "1", "timestamp": now, "score": 1.0},
        {"id": "2", "timestamp": now - 10**18, "score": 1.0},
        {"id": "3"},  # no timestamp -> skipped
    ]

    class F:
        value = "exponential"

    out = ex._apply_time_decay(
        list(records), (F(), {"reference_time": now, "halflife_hours": 24})
    )
    decayed = [r for r in out if "_decayed_score" in r]
    assert len(decayed) == 2


def test_executor_apply_time_decay_linear_and_gaussian():
    ex = _executor()
    now = 1_000_000_000_000_000_000
    for fn in ("linear", "gaussian", "other"):
        recs = [{"id": "1", "timestamp": now - 10**17, "_rrf_score": 1.0}]
        out = ex._apply_time_decay(recs, (fn, {"reference_time": now, "halflife_hours": 24}))
        assert "_time_decay" in out[0]


# ---------------------------------------------------------------------------
# Multi-modal executor: component execution (mocked REST) + execute()
# ---------------------------------------------------------------------------
class FakeQuery:
    """Duck-typed MultiModalQuery for the executor."""

    def __init__(self, components, **kw):
        self.components = components
        self.joins = kw.get("joins", [])
        self.fusion_strategy = kw.get("fusion_strategy", "rrf")
        self.fusion_weights = kw.get("fusion_weights", {})
        self.time_decay = kw.get("time_decay")
        self.custom_scorer = kw.get("custom_scorer")
        self.offset = kw.get("offset", 0)
        self.limit = kw.get("limit", 10)


@pytest.mark.asyncio
async def test_executor_execute_vector():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/search"): FakeResp(
            {"success": True, "results": [{"id": "v1", "score": 0.9}]}
        )
    }
    ex = EmbeddedMultiModalQueryExecutor(db)
    q = FakeQuery(
        [{"type": "vector", "collection": "c", "query_vector": [1.0], "top_k": 5}]
    )
    res = await ex.execute(q)
    assert res.total_count >= 1
    assert res.fusion_strategy == "rrf"
    assert res.metadata["executor"] == "embedded"


@pytest.mark.asyncio
async def test_executor_execute_vector_error_returns_empty():
    db = started_db()
    ex = EmbeddedMultiModalQueryExecutor(db)

    async def boom(*a, **k):
        raise RuntimeError("x")

    db._search_vectors = boom
    out = await ex._execute_vector({"collection": "c", "query_vector": [1.0]})
    assert out == []


@pytest.mark.asyncio
async def test_executor_execute_logs_metrics_empty():
    ex = _executor()
    assert await ex._execute_logs({}) == []
    assert await ex._execute_metrics({}) == []


@pytest.mark.asyncio
async def test_executor_execute_document():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/document-collections/c/query"): FakeResp(
            {"documents": [{"id": "d1", "name": "x"}]}
        )
    }
    ex = EmbeddedMultiModalQueryExecutor(db)
    out = await ex._execute_document(
        {"collection": "c", "filter": {"name": "x", "age": 5}, "limit": 10}
    )
    assert out[0]["id"] == "d1"
    assert out[0]["_source_type"] == "document"


@pytest.mark.asyncio
async def test_executor_execute_document_str_filter():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/document-collections/c/query"): FakeResp({"documents": []})
    }
    ex = EmbeddedMultiModalQueryExecutor(db)
    out = await ex._execute_document({"collection": "c", "filter": "raw expr"})
    assert out == []


@pytest.mark.asyncio
async def test_executor_execute_graph_with_start_nodes():
    db = started_db()
    FakeAsyncClient.router = {
        ("GET", "/nodes/n1"): FakeResp({"node": {"labels": ["L"], "properties": {}}}),
        ("GET", "/nodes/n1/edges/outgoing"): FakeResp({"edges": []}),
    }
    ex = EmbeddedMultiModalQueryExecutor(db)
    out = await ex._execute_graph(
        {"graph_id": "g", "start_nodes": ["n1"], "max_depth": 1, "limit": 10}
    )
    assert out[0]["id"] == "n1"
    assert out[0]["_source_type"] == "graph"


@pytest.mark.asyncio
async def test_executor_execute_graph_by_label():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/nodes/query"): FakeResp({"nodes": [{"id": "n2"}]}),
        ("GET", "/nodes/n2"): FakeResp({"node": {"labels": ["L"]}}),
        ("GET", "/nodes/n2/edges/outgoing"): FakeResp(
            {"edges": [{"edge_type": "REL", "to_node_id": "n3"}]}
        ),
        ("GET", "/nodes/n3"): FakeResp({"node": {}}),
        ("GET", "/nodes/n3/edges/outgoing"): FakeResp({"edges": []}),
    }
    ex = EmbeddedMultiModalQueryExecutor(db)
    out = await ex._execute_graph(
        {
            "graph_id": "g",
            "start_label": "L",
            "edge_types": ["REL"],
            "max_depth": 2,
            "limit": 10,
        }
    )
    ids = [r["id"] for r in out]
    assert "n2" in ids and "n3" in ids


@pytest.mark.asyncio
async def test_executor_execute_graph_no_nodes():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/nodes/query"): FakeResp({"nodes": []})
    }
    ex = EmbeddedMultiModalQueryExecutor(db)
    out = await ex._execute_graph({"graph_id": "g", "start_label": "L"})
    assert out == []


@pytest.mark.asyncio
async def test_executor_execute_full_with_custom_scorer_and_decay():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/search"): FakeResp(
            {"success": True, "results": [{"id": "v1", "score": 0.5}]}
        )
    }
    ex = EmbeddedMultiModalQueryExecutor(db)
    now = 1_000_000_000_000_000_000
    q = FakeQuery(
        [{"type": "vector", "collection": "c", "query_vector": [1.0]}],
        custom_scorer=lambda r: 1.0,
        time_decay=("linear", {"reference_time": now, "halflife_hours": 1}),
    )
    res = await ex.execute(q)
    assert res.total_count >= 0


@pytest.mark.asyncio
async def test_executor_execute_graph_from_previous():
    db = started_db()
    FakeAsyncClient.router = {
        ("POST", "/search"): FakeResp(
            {"success": True, "results": [{"id": "seed", "score": 1.0}]}
        ),
        ("GET", "/nodes/seed"): FakeResp({"node": {}}),
        ("GET", "/nodes/seed/edges/outgoing"): FakeResp({"edges": []}),
    }
    ex = EmbeddedMultiModalQueryExecutor(db)
    q = FakeQuery(
        [
            {"type": "vector", "collection": "c", "query_vector": [1.0]},
            {"type": "graph", "graph_id": "g", "_from_previous": True, "_id_field": "id"},
        ],
        joins=[{"join_type": "inner"}],
    )
    res = await ex.execute(q)
    assert isinstance(res.records, list)


@pytest.mark.asyncio
async def test_executor_execute_unknown_component():
    ex = _executor()
    q = FakeQuery([{"type": "mystery"}])
    res = await ex.execute(q)
    assert res.total_count == 0
