"""Verify the Python SDK propagates server-side gRPC errors instead of
silently returning a fake-success Collection.

Pre-fix bug: `unified_client.create_collection` catches *any* exception
from the adapter and falls back to building an in-memory Collection from
the request, masking real server-side rejections (INVALID_ARGUMENT,
ALREADY_EXISTS, INTERNAL, etc.). The user gets a Collection object back
and assumes success when the catalog never wrote a row.

This test forces a server-side rejection (dimension exceeds the
1,000,000 cap enforced by CollectionService) and asserts the SDK raises
instead of pretending success.
"""

import atexit
import os
import subprocess
import time

import pytest
import requests

from proximadb_sdk import (
    CollectionConfig,
    EmbeddingPrecision,
    ProximaDBClient,
    ProximaDBError,
)


_server_process: subprocess.Popen | None = None


def _start_server() -> bool:
    global _server_process

    os.system("pkill -f proximadb-server")
    time.sleep(1)

    test_dir = os.path.dirname(os.path.abspath(__file__))
    project_root = os.path.abspath(os.path.join(test_dir, "../../../.."))
    server_binary = os.path.join(project_root, "target/release/proximadb-server")
    config_file = os.path.join(project_root, "config/simple-config.toml")

    if not os.path.exists(server_binary):
        return False

    _server_process = subprocess.Popen(
        [server_binary, "--config", config_file],
        cwd=project_root,
        env={**os.environ, "RUST_LOG": "info"},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    deadline = time.time() + 30
    while time.time() < deadline:
        try:
            r = requests.get("http://localhost:5678/health", timeout=2)
            if r.ok:
                return True
        except Exception:
            pass
        time.sleep(0.5)
    return False


def _stop_server() -> None:
    global _server_process
    if _server_process:
        _server_process.terminate()
        try:
            _server_process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            _server_process.kill()
        _server_process = None


atexit.register(_stop_server)


@pytest.fixture(scope="module", autouse=True)
def _server():
    if not _start_server():
        pytest.skip(
            "Release proximadb-server binary not built. "
            "Run: cargo build --release -p proximadb-server"
        )
    yield
    _stop_server()


@pytest.mark.server_lifecycle
def test_grpc_server_side_error_must_propagate_not_swallow() -> None:
    """Server-side INVALID_ARGUMENT on dimension must raise, not return fake.

    The server's CollectionService rejects dimension > 1,000,000 with
    INVALID_DIMENSION. The SDK MUST surface this; today it returns a
    fake locally-built Collection and the caller never sees the error.
    """
    client = ProximaDBClient("grpc://localhost:5679", protocol="grpc")

    name = f"err_propagate_{int(time.time() * 1_000_000)}"
    # model_construct bypasses pydantic's `le=65536` check on dimension so
    # we can put oversized data on the wire — what we actually want to
    # test is the server's INVALID_DIMENSION response reaching the user.
    config = CollectionConfig.model_construct(
        name=name,
        dimension=2_000_000,  # exceeds the server's 1,000,000 cap
        canonical_embedding_precision=EmbeddingPrecision.FP16,
    )

    with pytest.raises(ProximaDBError):
        client.create_collection(name=name, config=config)

    # And the catalog must NOT have a row — the SDK must not have
    # back-doored a successful response from a failed request.
    resp = requests.get(f"http://localhost:5678/api/v1/collections/{name}", timeout=5)
    body = resp.json() if resp.ok else {}
    # Server returns success=false + collection=null when not found via the
    # legacy v1 GET; either that or 404 is acceptable. The collection must
    # NOT exist.
    if resp.ok:
        assert (
            body.get("success") is False or body.get("collection") is None
        ), f"server-rejected create should leave no catalog row; got: {body}"
    else:
        assert resp.status_code in (404,), f"unexpected REST GET status: {resp.status_code}"
