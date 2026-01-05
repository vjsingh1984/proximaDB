"""
ProximaDB Unified Async Python Client

Picks between gRPC async client (if available) and REST async client for graph operations.
"""

from typing import Optional, List
from enum import Enum
import logging

from .config import Protocol, load_config, ClientConfig

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
        url: Optional[str] = None,
        protocol: Protocol | str = Protocol.AUTO,
        config: Optional[ClientConfig] = None,
        grpc_endpoint: Optional[str] = None,
        rest_url: Optional[str] = None,
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
        max_depth: Optional[int] = None,
        edge_types: Optional[List[str]] = None,
        algorithm: str = "DIJKSTRA",
        k: Optional[int] = None,
        enable_prefetch: Optional[bool] = None,
        prefetch_budget: Optional[int] = None,
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
        edge_types: Optional[List[str]] = None,
        algorithm: str = "BFS",
        limit: Optional[int] = None,
        timeout_ms: Optional[int] = None,
        max_frontier: Optional[int] = None,
        enable_prefetch: Optional[bool] = None,
        prefetch_budget: Optional[int] = None,
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
