#!/usr/bin/env python3
"""
Customer 360 PoV: Similar Customers for Retention/Upsell

Shows how to surface customers similar to a target profile while applying
business constraints (segment, churn risk, region) for actionable insights.

Prerequisites
- ProximaDB server running (REST :5678)
- Python SDK installed: `pip install -e clients/python`
- From repo root: `export PYTHONPATH=./clients/python/src`

Run
  python3 demo/showcases/business/customer360_pov.py

Expected
- Creates collection `pov_customers`
- Inserts synthetic customers with metadata (segment, churn_risk, region, plan, age)
- Finds top similar customers with: segment='pro' AND churn_risk>0.7
"""

import os
import random
from typing import List

import numpy as np

try:
    from sentence_transformers import SentenceTransformer
    _embed_model = SentenceTransformer('all-MiniLM-L6-v2')
    _EMBED_DIM = _embed_model.get_sentence_embedding_dimension()
except Exception as e:
    raise SystemExit("Please install sentence-transformers: pip install sentence-transformers")

from proximadb import (
    ProximaDBClient,
    CollectionConfig,
    VectorRecord,
    FilterableColumn,
    FilterableDataType,
)
from proximadb.filters import FilterBuilder


SERVER_URL = os.environ.get("PROXIMADB_URL", "http://localhost:5678")
COLLECTION = "pov_customers"
DIM = _EMBED_DIM


def embed_text(text: str) -> List[float]:
    return _embed_model.encode([text], convert_to_tensor=False)[0].tolist()


def setup_collection(client: ProximaDBClient):
    try:
        client.delete_collection(COLLECTION)
    except Exception:
        pass

    cfg = CollectionConfig(
        name=COLLECTION,
        dimension=DIM,
        filterable_columns=[
            FilterableColumn(name="segment", data_type=FilterableDataType.STRING),
            FilterableColumn(name="churn_risk", data_type=FilterableDataType.FLOAT, supports_range=True),
            FilterableColumn(name="region", data_type=FilterableDataType.STRING),
            FilterableColumn(name="plan", data_type=FilterableDataType.STRING),
            FilterableColumn(name="age", data_type=FilterableDataType.INTEGER, supports_range=True),
        ],
        description="Business PoV: Customer 360 similarity for retention/upsell",
    )
    client.create_collection(name=COLLECTION, config=cfg)


def seed_customers(client: ProximaDBClient, n: int = 60) -> str:
    segments = ["free", "starter", "pro", "enterprise"]
    regions = ["NA", "EU", "APAC", "LATAM"]
    plans = ["basic", "plus", "premium"]

    target_id = "cust_target"
    recs: List[VectorRecord] = [
        VectorRecord(
            id=target_id,
            vector=embed_text("pro segment customer in NA on plus plan age 34"),
            metadata={
                "segment": "pro",
                "churn_risk": 0.82,
                "region": "NA",
                "plan": "plus",
                "age": 34,
            },
        )
    ]

    for i in range(n - 1):
        desc = f"{segments[i % len(segments)]} customer in {regions[i % len(regions)]} on {plans[i % len(plans)]} plan age {random.randint(18,75)}"
        recs.append(
            VectorRecord(
                id=f"cust_{i:04d}",
                vector=embed_text(desc),
                metadata={
                    "segment": segments[i % len(segments)],
                    "churn_risk": round(random.uniform(0.0, 1.0), 2),
                    "region": regions[i % len(regions)],
                    "plan": plans[i % len(plans)],
                    "age": int(desc.split(" age ")[1]),
                    "description": desc,
                },
            )
        )

    client.insert_vectors(COLLECTION, records=recs)
    return target_id


def run_business_query(client: ProximaDBClient, target_id: str):
    # Query from a descriptive retention/upsell prompt
    query = embed_text("find pro customers with high churn risk for retention offers")

    fb = FilterBuilder().equals("segment", "pro").greater_than("churn_risk", 0.7)

    results = client.search(
        collection_id=COLLECTION,
        vector=query,
        top_k=5,
        metadata_filter=fb.to_dict(),
        include_metadata=True,
    )

    print("\nCustomer 360 PoV Results (segment=pro AND churn_risk>0.7):")
    if not results:
        print("  No matches this run (synthetic data varies).")
        return

    for r in results:
        md = r.metadata or {}
        print(
            f"  {r.id} | region {md.get('region')} | plan {md.get('plan')} | "
            f"churn_risk {md.get('churn_risk')} | age {md.get('age')}"
        )


def main():
    client = ProximaDBClient(url=SERVER_URL, protocol="rest")
    print("🔌 Connected to:", SERVER_URL)

    setup_collection(client)
    target_id = seed_customers(client)
    run_business_query(client, target_id)

    # Cleanup
    try:
        client.delete_collection(COLLECTION)
    except Exception:
        pass


if __name__ == "__main__":
    main()
