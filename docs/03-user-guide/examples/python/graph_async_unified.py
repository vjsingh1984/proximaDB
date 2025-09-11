#!/usr/bin/env python3
# Async unified client example for graph operations (picks gRPC or REST)

import asyncio
from proximadb.unified_client_async import ProximaDBAsyncUnified


async def main():
    client = ProximaDBAsyncUnified(url="http://localhost:5678", protocol="auto")
    await client.astart()

    # Shortest path with per-call prefetch overrides
    sp = await client.graph_shortest_path(
        start_node_id="n1",
        target_node_id="n8",
        max_depth=10,
        algorithm="DIJKSTRA",
        enable_prefetch=True,
        prefetch_budget=8,
    )
    print("ShortestPath:", getattr(sp, 'node_ids', None) or sp)

    # Traversal (REST async path under the hood)
    trav = await client.graph_traverse(
        start_node_id="n1",
        max_depth=3,
        edge_types=["REL"],
        algorithm="BFS",
        enable_prefetch=True,
        prefetch_budget=8,
    )
    print("Traverse:", trav)

    await client.aclose()


if __name__ == "__main__":
    asyncio.run(main())

