"""
ProximaDB Unified Async Python Client

Picks between gRPC async client (if available) and REST async client for graph operations.
"""

import logging

from .config import ClientConfig, Protocol, load_config

logger = logging.getLogger(__name__)

try:
    from .protocols.grpc_async import ProximaDBClient as GrpcAsyncClient  # type: ignore

    GRPC_OK = True
except Exception:
    GRPC_OK = False
from .protocols.rest_async import ProximaDBAsyncClient as RestAsyncClient


class ProximaDBAsyncUnified:
    def __init__(
        self,
        url: str | None = None,
        protocol: Protocol | str = Protocol.AUTO,
        config: ClientConfig | None = None,
        grpc_endpoint: str | None = None,
        rest_url: str | None = None,
        timeout: float = 60.0,
    ):
        self.config = config or load_config(url=url)
        self.protocol = Protocol(protocol) if isinstance(protocol, str) else protocol
        self.grpc_endpoint = grpc_endpoint or "localhost:5679"
        self.rest_url = rest_url or (url or self.config.url)
        self.timeout = timeout

        self._grpc = None
        self._rest = None

    async def astart(self):
        if self.protocol == Protocol.GRPC or (
            self.protocol == Protocol.AUTO and GRPC_OK
        ):
            try:
                self._grpc = GrpcAsyncClient(
                    endpoint=self.grpc_endpoint, timeout=self.timeout
                )
                logger.info("Using async gRPC client")
            except Exception as e:
                logger.warning(f"gRPC async init failed: {e}; falling back to REST")
                self._rest = RestAsyncClient(url=self.rest_url, timeout=self.timeout)
        else:
            self._rest = RestAsyncClient(url=self.rest_url, timeout=self.timeout)
            logger.info("Using async REST client")

    async def aclose(self):
        if self._rest:
            await self._rest.aclose()

    # Graph operations
    async def graph_shortest_path(
        self,
        start_node_id: str,
        target_node_id: str,
        max_depth: int | None = None,
        edge_types: list[str] | None = None,
        algorithm: str = "DIJKSTRA",
        k: int | None = None,
        enable_prefetch: bool | None = None,
        prefetch_budget: int | None = None,
    ):
        if self._grpc and hasattr(self._grpc, "shortest_path"):
            return self._grpc.shortest_path(
                start_node_id,
                target_node_id,
                max_depth,
                edge_types,
                algorithm,
                k,
                enable_prefetch,
                prefetch_budget,
            )
        if self._rest:
            return await self._rest.graph_shortest_path(
                start_node_id,
                target_node_id,
                max_depth,
                edge_types,
                algorithm,
                k,
                enable_prefetch,
                prefetch_budget,
            )
        raise RuntimeError("Client not started; call astart() first")

    async def graph_traverse(
        self,
        start_node_id: str,
        max_depth: int = 3,
        edge_types: list[str] | None = None,
        algorithm: str = "BFS",
        limit: int | None = None,
        timeout_ms: int | None = None,
        max_frontier: int | None = None,
        enable_prefetch: bool | None = None,
        prefetch_budget: int | None = None,
    ):
        # REST path for traversal (gRPC streaming traversal not exposed here)
        if self._rest:
            return await self._rest.graph_traverse(
                start_node_id,
                max_depth,
                edge_types,
                algorithm,
                limit,
                timeout_ms,
                max_frontier,
                enable_prefetch,
                prefetch_budget,
            )
        raise RuntimeError("Client not started; call astart() first")
