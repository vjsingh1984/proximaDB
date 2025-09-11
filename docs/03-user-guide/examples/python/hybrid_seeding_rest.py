#!/usr/bin/env python3
# REST: Hybrid query with seeding strategy (AVERAGE | PER_SEED | NONE)

import json
import requests

BASE = "http://localhost:5678"

payload = {
    "query": "-- SEEDING: PER_SEED\nSELECT id FROM my_collection ORDER BY COSINE_DISTANCE(vector, $1) LIMIT 10",
    "parameters": [ { "value": { "array_value": { "items": [ {"value": {"number_value": 0.1}}, {"value": {"number_value": 0.2}} ] } } } ],
    "collection": "my_collection",
    "seeding": "per_seed"
}

r = requests.post(f"{BASE}/api/v1/sql/execute", json=payload, timeout=10)
print("Status:", r.status_code)
print(json.dumps(r.json(), indent=2))

