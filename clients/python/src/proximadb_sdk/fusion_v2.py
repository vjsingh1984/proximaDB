"""
Fusion service client for ProximaDB v2.

This module provides the FusionServiceClient for interacting with the
ProximaDB Fusion v2 gRPC service — the cross-modal retrieval surface
(vector seed → graph expand → calibrated fuse-by-oid → rank).

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

from dataclasses import dataclass
from typing import Any, List, Optional


@dataclass
class FusionHit:
    """A single fused retrieval result."""

    oid: str
    score: float
    source_count: int

    @classmethod
    def from_pb(cls, pb_hit: Any) -> "FusionHit":
        return cls(
            oid=pb_hit.oid,
            score=pb_hit.score,
            source_count=pb_hit.source_count,
        )


@dataclass
class FusionStats:
    """Bookkeeping for a fusion query (observability)."""

    sources_fused: int
    sources_skipped: int
    candidates_in: int
    items_out: int

    @classmethod
    def from_pb(cls, pb_stats: Any) -> "FusionStats":
        return cls(
            sources_fused=pb_stats.sources_fused,
            sources_skipped=pb_stats.sources_skipped,
            candidates_in=pb_stats.candidates_in,
            items_out=pb_stats.items_out,
        )


@dataclass
class FusionSearchResponse:
    """Response from fusion_search."""

    results: List[FusionHit]
    stats: FusionStats


def _grain_to_enum(fusion_pb2_module: Any, grain: Optional[str]) -> int:
    """Map a grain string ("nodes"/"edges"/"both") to the FusionGrain enum value."""
    if grain is None:
        return fusion_pb2_module.FusionGrain.FUSION_GRAIN_NODES
    name = f"FUSION_GRAIN_{grain.strip().upper()}"
    return getattr(
        fusion_pb2_module.FusionGrain,
        name,
        fusion_pb2_module.FusionGrain.FUSION_GRAIN_NODES,
    )


class FusionServiceClient:
    """
    Client for the ProximaDB Fusion v2 gRPC service.

    Thin facade over the server's fusion seam (the single retrieval engine);
    see SEARCH_SURFACE_CONTRACT_2026_06_24.adoc.
    """

    def __init__(self, grpc_client: Any):
        """
        Initialize FusionServiceClient.

        Args:
            grpc_client: ProximaDBSyncGrpcClient instance
        """
        self._grpc_client = grpc_client

    def fusion_search(
        self,
        graph_id: str,
        vector_collection: str,
        query_vector: List[float],
        top_k: int = 10,
        max_depth: int = 1,
        edge_types: Optional[List[str]] = None,
        max_seeds: int = 0,
        vector_weight: Optional[float] = None,
        graph_weight: Optional[float] = None,
        rrf: bool = False,
        consensus_beta: Optional[float] = None,
        grain: Optional[str] = None,
    ) -> FusionSearchResponse:
        """
        Cross-modal fusion search: vector seed → graph expand → calibrated fuse-by-oid.

        Args:
            graph_id: Graph to traverse for expansion (required).
            vector_collection: Vector collection to seed from (required).
            query_vector: Query embedding for the ANN seed (required, non-empty).
            top_k: Max results to return.
            max_depth: k-hop expansion depth (default 1).
            edge_types: Edge types to traverse (empty = all).
            max_seeds: How many top vector seeds to expand from (0 = server default).
            vector_weight: Optional vector-source weight.
            graph_weight: Optional graph-source weight.
            rrf: Use rank-based RRF fallback instead of PIT-calibrated linear.
            consensus_beta: Consensus boost for oids in >=2 sources.
            grain: Graph contribution grain: "nodes" (default), "edges", or "both".

        Returns:
            FusionSearchResponse with ranked hits + stats.
        """
        from proximadb.v2 import fusion_pb2 as v2_fusion_pb2  # type: ignore

        request = v2_fusion_pb2.FusionSearchRequest(
            graph_id=graph_id,
            vector_collection=vector_collection,
            query_vector=query_vector,
            limit=top_k,
            max_depth=max_depth,
            edge_types=edge_types or [],
            max_seeds=max_seeds,
            rrf=rrf,
            grain=_grain_to_enum(v2_fusion_pb2, grain),
        )
        # Optional (proto3 `optional`) fields — only set when provided.
        if vector_weight is not None:
            request.vector_weight = vector_weight
        if graph_weight is not None:
            request.graph_weight = graph_weight
        if consensus_beta is not None:
            request.consensus_beta = consensus_beta

        response = self._grpc_client._execute_fusion_with_pool(
            "fusion_search",
            lambda stub: stub.FusionSearch(request, timeout=self._grpc_client.timeout),
        )

        return FusionSearchResponse(
            results=[FusionHit.from_pb(h) for h in response.results],
            stats=(
                FusionStats.from_pb(response.stats)
                if response.HasField("stats")
                else FusionStats(0, 0, 0, 0)
            ),
        )
