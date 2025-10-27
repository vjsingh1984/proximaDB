"""
Pytest session config for tests: warm embedding cache to speed up runs.
"""
import os
import pytest

try:
    from .embedding_utils import warm_cache
except Exception:
    warm_cache = None


@pytest.fixture(scope="session", autouse=True)
def _warm_embeddings_once():
    """Warm the sentence-transformer embedding cache once per session.

    Controls (env vars):
    - PROXIMADB_TEST_EMBED_WARMUP: set to '0'/'false' to disable warmup
    - PROXIMADB_TEST_EMBED_DIMS: comma-separated dims (e.g., '32,64,128,384')
    """
    if warm_cache is None:
        yield
        return
    enabled = os.getenv("PROXIMADB_TEST_EMBED_WARMUP", "1").lower() not in {"0", "false", "no"}
    if not enabled:
        yield
        return
    dims_env = os.getenv("PROXIMADB_TEST_EMBED_DIMS")
    dims = None
    if dims_env:
        try:
            dims = [int(x.strip()) for x in dims_env.split(",") if x.strip()]
        except Exception:
            dims = None
    try:
        warm_cache(dims=dims)
    except Exception:
        # Best-effort cache warmup; tests will still work without it
        pass
    yield
