#!/usr/bin/env python3
# REST: Graph shortest path with per-call prefetch overrides (Python + requests)

import requests

BASE = "http://localhost:5678"

body = {
    "start_node_id": "n1",
    "target_node_id": "n8",
    "algorithm": "DIJKSTRA",
    "max_depth": 10,
}

headers = {
    "Content-Type": "application/json",
    "x-graph-prefetch-enabled": "true",
    "x-graph-prefetch-budget": "8",
}

r = requests.post(f"{BASE}/api/v1/graph/shortest_path", json=body, headers=headers, timeout=10)
print(r.status_code)
print(r.json())

