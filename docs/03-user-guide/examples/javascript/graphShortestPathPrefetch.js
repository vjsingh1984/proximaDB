// REST: Graph shortest path with per-call prefetch overrides (JavaScript + fetch)

async function shortestPath() {
  const body = {
    start_node_id: "n1",
    target_node_id: "n8",
    algorithm: "DIJKSTRA",
    max_depth: 10
    // Optional JSON overrides (instead of headers):
    // enable_prefetch: true,
    // prefetch_budget: 8,
  };
  const resp = await fetch("http://localhost:5678/api/v1/graph/shortest_path", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      // Per-call overrides via headers
      "x-graph-prefetch-enabled": "true",
      "x-graph-prefetch-budget": "8",
    },
    body: JSON.stringify(body),
  });
  console.log(await resp.json());
}

shortestPath().catch(console.error);

