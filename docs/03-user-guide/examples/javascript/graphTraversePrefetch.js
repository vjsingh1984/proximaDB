// REST: Graph traversal with per-call prefetch overrides (JavaScript + fetch)

async function traverse() {
  const body = {
    start_node_id: "n1",
    max_depth: 3,
    edge_types: ["REL"],
    algorithm: "BFS",
    // Optional JSON overrides (instead of headers):
    // enable_prefetch: true,
    // prefetch_budget: 8,
  };
  const resp = await fetch("http://localhost:5678/api/v1/graph/traverse", {
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

traverse().catch(console.error);

