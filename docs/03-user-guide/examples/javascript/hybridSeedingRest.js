// REST: Hybrid query with seeding strategy (JavaScript + fetch)

async function main() {
  const payload = {
    query: "-- SEEDING: NONE\nSELECT id FROM my_collection ORDER BY COSINE_DISTANCE(vector, $1) LIMIT 10",
    parameters: [{ value: { array_value: { items: [ { value: { number_value: 0.1 } } ] } } }],
    collection: "my_collection",
    seeding: "none"
  };
  const resp = await fetch("http://localhost:5678/api/v1/sql/execute", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload)
  });
  console.log(await resp.json());
}

main().catch(console.error);

