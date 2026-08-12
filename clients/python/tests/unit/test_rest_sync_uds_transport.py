"""UDS transport for the sync REST client.

An embedded server started with ``transport="uds"`` (the default) binds a
Unix-domain socket and no TCP port, and its ``rest_url`` degrades to the host
header sentinel ``http://localhost``. A client built from that URL alone
silently targets port 80, so the socket has to be plumbed through explicitly.
"""

import httpx

from proximadb_sdk.config import ClientConfig
from proximadb_sdk.protocols.rest_sync import ProximaDBClient as RestClient
from proximadb_sdk.unified_client import ProximaDBClient


def _uds_transport(client: httpx.Client):
    """Return the httpx transport's configured UDS path, or None."""
    transport = client._transport
    pool = getattr(transport, "_pool", None)
    return getattr(pool, "_uds", None)


def test_uds_path_produces_a_unix_socket_transport(tmp_path):
    socket = tmp_path / "proximadb.rest.sock"
    client = RestClient(
        config=ClientConfig(url="http://localhost", uds_path=str(socket))
    )

    http_client = client._create_http_client()
    try:
        assert _uds_transport(http_client) == str(socket)
        # base_url is still needed — it supplies the Host header on every request.
        assert str(http_client.base_url) == "http://localhost"
    finally:
        http_client.close()


def test_without_uds_path_the_tcp_client_is_unchanged():
    client = RestClient(config=ClientConfig(url="http://localhost:15678"))

    http_client = client._create_http_client()
    try:
        assert _uds_transport(http_client) is None
        assert str(http_client.base_url) == "http://localhost:15678"
    finally:
        http_client.close()


def test_unified_client_forwards_uds_path_to_config(tmp_path):
    socket = tmp_path / "proximadb.rest.sock"

    client = ProximaDBClient(
        url="http://localhost", protocol="rest", uds_path=str(socket)
    )

    assert client.config.uds_path == str(socket)


def test_uds_path_defaults_to_none():
    assert ClientConfig(url="http://localhost:15678").uds_path is None
