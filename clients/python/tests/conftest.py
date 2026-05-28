"""
ProximaDB Test Configuration

This root conftest.py provides:
1. Embedded database fixture for isolated integration tests
2. Embedding cache warmup for performance
3. Common test markers and configuration

Tests rely on the editable install (pip install -e .) rather than
sys.path manipulation for consistent imports.
"""

# Quarantine list — files that reference deprecated `proximadb.ProximaDB`
# or `proximadb.init_logging` at module load time and therefore fail
# pytest collection before any test can run. Each entry should either
# be deleted, or updated to the current SDK API, in a follow-up
# cleanup PR. Listing them here keeps the rest of the suite collectable.
collect_ignore_glob = [
    "quick_timing_test.py",
    "test_50k_benchmark.py",
    "test_engine_index_matrix.py",
    "test_rl_planner_embedded.py",
    "utils/embedded_base_test.py",
]

import asyncio
import logging
import os
import tempfile
import time

import pytest

# Configure logging for tests
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
logger = logging.getLogger(__name__)


# =============================================================================
# Embedding Cache Warmup
# =============================================================================

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
    enabled = os.getenv("PROXIMADB_TEST_EMBED_WARMUP", "1").lower() not in {
        "0",
        "false",
        "no",
    }
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


# =============================================================================
# Embedded Database Fixtures
# =============================================================================


@pytest.fixture(scope="session")
def embedded_db_config():
    """Configuration for embedded database tests.

    Returns a config dict that can be customized via environment variables.
    """
    return {
        "data_dir": os.getenv("PROXIMADB_TEST_DATA_DIR", None),  # None = temp dir
        "rest_port": int(
            os.getenv("PROXIMADB_TEST_REST_PORT", "15678")
        ),  # Non-standard port for testing
        "grpc_port": int(os.getenv("PROXIMADB_TEST_GRPC_PORT", "15679")),
        "startup_timeout": float(os.getenv("PROXIMADB_TEST_STARTUP_TIMEOUT", "30")),
        "cleanup_on_exit": os.getenv("PROXIMADB_TEST_CLEANUP", "1").lower()
        in {"1", "true", "yes"},
    }


@pytest.fixture(scope="session")
def embedded_db(embedded_db_config):
    """Embedded ProximaDB instance for integration tests.

    This fixture starts an embedded ProximaDB server for the test session
    and stops it when tests complete. Tests can use this for full integration
    testing without needing an external server.

    Usage:
        def test_something(embedded_db):
            client = embedded_db.rest_client()
            # ... run tests ...

    To skip tests that require embedded db (if not available):
        @pytest.mark.embedded_required
        def test_requires_embedded(embedded_db):
            ...
    """
    try:
        from proximadb_sdk.embedded import EmbeddedConfig, EmbeddedProximaDB
    except ImportError:
        pytest.skip("Embedded database not available")
        return

    # Use temp directory if not specified
    data_dir = embedded_db_config["data_dir"]
    temp_dir = None
    if data_dir is None:
        temp_dir = tempfile.mkdtemp(prefix="proximadb_test_")
        data_dir = temp_dir

    try:
        config = EmbeddedConfig(
            data_dir=data_dir,
            rest_port=embedded_db_config["rest_port"],
            grpc_port=embedded_db_config["grpc_port"],
        )
        db = EmbeddedProximaDB(config=config)

        # Start database
        loop = asyncio.new_event_loop()
        try:
            loop.run_until_complete(db.start())
            logger.info(
                f"Embedded database started at REST:{config.rest_port}, gRPC:{config.grpc_port}"
            )
        except Exception as e:
            logger.warning(f"Failed to start embedded database: {e}")
            pytest.skip(f"Could not start embedded database: {e}")
            return

        yield db

        # Stop database
        try:
            loop.run_until_complete(db.stop())
            logger.info("Embedded database stopped")
        except Exception as e:
            logger.warning(f"Error stopping embedded database: {e}")
        finally:
            loop.close()

    finally:
        # Cleanup temp directory
        if temp_dir and embedded_db_config["cleanup_on_exit"]:
            import shutil

            try:
                shutil.rmtree(temp_dir)
                logger.debug(f"Cleaned up temp directory: {temp_dir}")
            except Exception as e:
                logger.warning(f"Failed to cleanup temp directory: {e}")


@pytest.fixture
def embedded_rest_client(embedded_db, embedded_db_config):
    """REST client connected to embedded database."""
    from proximadb_sdk import Protocol, ProximaDBClient
    from proximadb_sdk.config import ClientConfig

    config = ClientConfig(
        url=f"http://localhost:{embedded_db_config['rest_port']}",
        protocol=Protocol.REST,
        timeout=30.0,
    )
    client = ProximaDBClient(config=config)
    yield client
    client.close()


@pytest.fixture
def embedded_grpc_client(embedded_db, embedded_db_config):
    """gRPC client connected to embedded database."""
    from proximadb_sdk import Protocol, ProximaDBClient
    from proximadb_sdk.config import ClientConfig

    config = ClientConfig(
        url=f"grpc://localhost:{embedded_db_config['grpc_port']}",
        protocol=Protocol.GRPC,
        timeout=30.0,
    )
    client = ProximaDBClient(config=config)
    yield client
    client.close()


# =============================================================================
# Test Markers and Configuration
# =============================================================================


def pytest_configure(config):
    """Configure custom pytest markers."""
    config.addinivalue_line(
        "markers", "slow: marks tests as slow (may take > 10 seconds)"
    )
    config.addinivalue_line(
        "markers", "integration: marks tests requiring a running server"
    )
    config.addinivalue_line(
        "markers", "embedded_required: marks tests requiring embedded database"
    )
    config.addinivalue_line("markers", "unit: marks pure unit tests (no server needed)")
    config.addinivalue_line(
        "markers", "storage: marks tests related to storage functionality"
    )
    config.addinivalue_line(
        "markers", "search: marks tests related to search operations"
    )
    config.addinivalue_line("markers", "graph: marks tests related to graph operations")
    config.addinivalue_line("markers", "performance: marks performance/benchmark tests")
    config.addinivalue_line(
        "markers", "requires_models: marks tests requiring embedding models"
    )


# =============================================================================
# Test Utilities
# =============================================================================


@pytest.fixture
def unique_collection_name():
    """Generate unique collection name for each test."""
    timestamp = int(time.time() * 1000)
    test_name = (
        os.environ.get("PYTEST_CURRENT_TEST", "unknown").split("::")[-1].split("[")[0]
    )
    return f"pytest_{test_name}_{timestamp}"


@pytest.fixture
def test_vectors():
    """Generate test vectors for use in tests."""
    import numpy as np

    def generate(count: int = 100, dimension: int = 128) -> list:
        """Generate random normalized vectors."""
        vectors = np.random.randn(count, dimension).astype(np.float32)
        norms = np.linalg.norm(vectors, axis=1, keepdims=True)
        normalized = (vectors / norms).tolist()
        return normalized

    return generate


@pytest.fixture
def test_metadata():
    """Generate test metadata for use in tests."""
    categories = ["technology", "science", "healthcare", "education", "business"]

    def generate(count: int = 100) -> list:
        """Generate test metadata."""
        return [
            {
                "index": i,
                "category": categories[i % len(categories)],
                "importance": (i % 10) + 1,
                "test_generated": True,
            }
            for i in range(count)
        ]

    return generate
