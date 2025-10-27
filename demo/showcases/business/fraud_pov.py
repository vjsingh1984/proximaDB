#!/usr/bin/env python3
"""
Fraud Detection PoV: Surface Risky Patterns with Vector + Graph

Demonstrates how ProximaDB can flag likely-fraud transactions via vector similarity
and (when available) simple graph context from account relationships.

Prerequisites
- ProximaDB server running (REST :5678)
- Python SDK installed: `pip install -e clients/python`
- From repo root: `export PYTHONPATH=./clients/python/src`

Run
  python3 demo/showcases/business/fraud_pov.py

Expected
- Creates collection `pov_transactions`
- Inserts synthetic transactions with metadata (amount, merchant, country, risk_score)
- Searches near a known-fraud vector with filters: amount>500 AND country in [US, CA]
- If graph API available, creates small account graph and traverses from a suspicious node
"""

import os
import random
from typing import Any, Dict, List

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
COLLECTION = "pov_transactions"
GRAPH_ID = "fraud"
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
            FilterableColumn(name="amount", data_type=FilterableDataType.FLOAT, supports_range=True),
            FilterableColumn(name="merchant", data_type=FilterableDataType.STRING),
            FilterableColumn(name="country", data_type=FilterableDataType.STRING),
            FilterableColumn(name="is_chargeback", data_type=FilterableDataType.BOOLEAN),
            FilterableColumn(name="risk_score", data_type=FilterableDataType.FLOAT, supports_range=True),
        ],
        description="Business PoV: Fraud detection on transactions",
    )
    client.create_collection(name=COLLECTION, config=cfg)


def seed_transactions(client: ProximaDBClient, n: int = 100) -> str:
    merchants = ["Uber", "Amazon", "Acme Air", "Globex Retail", "Initech Cloud"]
    countries = ["US", "CA", "GB", "DE", "IN", "SG"]

    # One synthetic "known fraud" seed we’ll query around
    known_fraud_id = "txn_fraud_seed"
    records: List[VectorRecord] = [
        VectorRecord(
            id=known_fraud_id,
            vector=embed_text("Fraudulent transaction at Globex Retail in US amount 799.0"),
            metadata={
                "amount": 799.0,
                "merchant": "Globex Retail",
                "country": "US",
                "is_chargeback": True,
                "risk_score": 0.98,
            },
        )
    ]

    for i in range(n - 1):
        amount = round(random.uniform(5, 2000), 2)
        risk = round(random.uniform(0.0, 1.0), 2)
        is_cb = risk > 0.9 and amount > 500
        desc = f"Transaction at {random.choice(merchants)} in {random.choice(countries)} amount {amount}"
        records.append(
            VectorRecord(
                id=f"txn_{i:05d}",
                vector=embed_text(desc),
                metadata={
                    "amount": amount,
                    "merchant": desc.split(" at ")[1].split(" in ")[0],
                    "country": desc.split(" in ")[1].split(" amount ")[0],
                    "is_chargeback": is_cb,
                    "risk_score": risk,
                    "description": desc,
                },
            )
        )

    client.insert_vectors(COLLECTION, records=records)
    return known_fraud_id


def vector_risk_surfacing(client: ProximaDBClient, seed_id: str):
    # Query embedding from a descriptive risk prompt
    query = embed_text("high amount transaction in US or CA with risk of chargeback")

    fb = (
        FilterBuilder()
        .greater_than("amount", 500.0)
        .or_()
        .in_("country", ["US", "CA"])
    )

    results = client.search(
        collection_id=COLLECTION,
        vector=query,
        top_k=8,
        metadata_filter=fb.to_dict(),
        include_metadata=True,
    )

    print("\nFraud PoV Results (amount>500 OR country in [US,CA]):")
    risky = 0
    for r in results or []:
        md = r.metadata or {}
        flag = "⚠" if (md.get("risk_score", 0) >= 0.85 or md.get("is_chargeback")) else " "
        if flag == "⚠":
            risky += 1
        print(
            f"  {flag} {r.id} | amount ${md.get('amount')} | merchant {md.get('merchant')} | "
            f"country {md.get('country')} | risk {md.get('risk_score')} | cb={md.get('is_chargeback')}"
        )

    print(f"\nSummary: {risky}/{len(results or [])} flagged as high risk by metadata.")


def simple_graph_context():
    # Use direct REST calls (httpx) to avoid SDK internals
    import httpx

    try:
        with httpx.Client(base_url=SERVER_URL, timeout=5.0) as http:
            # Delete if exists (best-effort)
            try:
                http.delete(f"/api/v1/graph/graphs/{GRAPH_ID}")
            except Exception:
                pass

            # Create graph
            http.post(
                "/api/v1/graph/graphs",
                json={"graph_id": GRAPH_ID, "name": "Fraud Graph", "description": "Accounts and transfers"},
            ).raise_for_status()

            # Create nodes
            accounts = ["acct_A", "acct_B", "acct_C", "acct_D"]
            for a in accounts:
                http.post(
                    f"/api/v1/graph/graphs/{GRAPH_ID}/nodes",
                    json={"node": {"id": a, "labels": ["Account"], "properties": {"type": "user"}}},
                ).raise_for_status()

            # Create edges A->B, B->C, A->D
            edges = [("acct_A", "acct_B"), ("acct_B", "acct_C"), ("acct_A", "acct_D")]
            for i, (u, v) in enumerate(edges):
                http.post(
                    f"/api/v1/graph/graphs/{GRAPH_ID}/edges",
                    json={
                        "edge": {
                            "id": f"e{i}",
                            "from_node_id": u,
                            "to_node_id": v,
                            "edge_type": "TRANSFER",
                            "properties": {"amount": 250.0},
                        }
                    },
                ).raise_for_status()

            # Traverse from acct_A up to 2 hops
            resp = http.post(
                f"/api/v1/graph/graphs/{GRAPH_ID}/traverse",
                json={
                    "start_node_id": "acct_A",
                    "max_depth": 2,
                    "edge_types": [],
                    "node_labels": [],
                    "return_path": True,
                    "algorithm": "BFS",
                },
            )
            resp.raise_for_status()
            data = resp.json()
            payload = data.get("data", data)
            visited = {n.get("id") for n in payload.get("nodes", [])}
            print(f"\nGraph Context: From acct_A within 2 hops reached: {sorted(list(visited))}")
    except Exception as e:
        msg = str(e).splitlines()[0]
        print(f"\nGraph Context: Skipped (graph API not available: {msg})")


def main():
    client = ProximaDBClient(url=SERVER_URL, protocol="rest")
    print("🔌 Connected to:", SERVER_URL)

    setup_collection(client)
    seed = seed_transactions(client)
    vector_risk_surfacing(client, seed)
    simple_graph_context()

    # Cleanup
    try:
        client.delete_collection(COLLECTION)
    except Exception:
        pass


if __name__ == "__main__":
    main()
