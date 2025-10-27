#!/usr/bin/env python3
"""
Hybrid PoV: Unified Entity Store (Vector + Relations)

Demonstrates ProximaDB's hybrid entity store: upsert entities with embeddings,
typed metadata, and relations; then run an entity search (vector + filters) and
retrieve entities including relations.

Prerequisites
- ProximaDB server running (REST :5678)
- Python SDK installed: `pip install -e clients/python`
- From repo root: `export PYTHONPATH=./clients/python/src`

Run
  python3 demo/showcases/business/hybrid_pov.py

Expected
- Creates logical collection `pov_entities`
- Upserts 3 entities (A, B, C) with embeddings and relations (A→B, B→C)
- Searches for similar entities with filters (segment='pro' AND score>0.6)
- Fetches an entity with relations for inspection
"""

import os
from typing import Dict, List

import httpx
import numpy as np

try:
    from sentence_transformers import SentenceTransformer
    _embed_model = SentenceTransformer('all-MiniLM-L6-v2')
    _EMBED_DIM = _embed_model.get_sentence_embedding_dimension()
except Exception as e:
    raise SystemExit("Please install sentence-transformers: pip install sentence-transformers")


SERVER_URL = os.environ.get("PROXIMADB_URL", "http://localhost:5678")
COLLECTION = "pov_entities"
DIM = _EMBED_DIM


def embed_text(text: str) -> List[float]:
    return _embed_model.encode([text], convert_to_tensor=False)[0].tolist()


def upsert_entity(http: httpx.Client, entity: Dict) -> str:
    resp = http.post(
        f"/api/v1/collections/{COLLECTION}/entities",
        json={
            "entity": entity,
            "create_collection_if_missing": True,
        },
        headers={"Content-Type": "application/json"},
        timeout=10.0,
    )
    resp.raise_for_status()
    data = resp.json()
    return data.get("entity_id", entity.get("id", ""))


def search_entities(http: httpx.Client, query_vec: List[float]):
    # Filters follow proto: MetadataFilter { clauses[], op }
    filters = {
        "clauses": [
            {"field": "segment", "op": "EQ", "string_value": "pro"},
            {"field": "score", "op": "GT", "double_value": 0.6},
        ],
        "op": "AND",
    }
    resp = http.post(
        f"/api/v1/collections/{COLLECTION}/entities/search",
        json={
            "query_vector": query_vec,
            "filters": filters,
            "top_k": 5,
            "progressive": False,
        },
        headers={"Content-Type": "application/json"},
        timeout=10.0,
    )
    resp.raise_for_status()
    return resp.json()


def get_entity(http: httpx.Client, entity_id: str):
    resp = http.get(
        f"/api/v1/collections/{COLLECTION}/entities/{entity_id}",
        params={"include_embeddings": True, "include_relations": True},
        timeout=10.0,
    )
    resp.raise_for_status()
    return resp.json()


def delete_entity(http: httpx.Client, entity_id: str):
    try:
        http.delete(
            f"/api/v1/collections/{COLLECTION}/entities/{entity_id}",
            params={"hard_delete": True},
            timeout=5.0,
        )
    except Exception:
        pass


def main():
    print("🔌 Connected to:", SERVER_URL)
    with httpx.Client(base_url=SERVER_URL) as http:
        # Create three entities: A (pro), B (starter), C (pro)
        entA = {
            "id": "ent_A",
            "collection_id": COLLECTION,
            "embeddings": [
                {
                    "model_id": "demo",
                    "model_version": "v1",
                    "vector": embed_text("pro segment entity in NA with score 0.85 referring B"),
                    "dimension": DIM,
                }
            ],
            "typed_metadata": {
                "fields": {
                    "segment": {"string_value": "pro", "indexed": True, "filterable": True},
                    "score": {"double_value": 0.85, "indexed": True, "filterable": True},
                    "region": {"string_value": "NA", "indexed": True, "filterable": True},
                }
            },
            "relations": [
                {"source_entity_id": "ent_A", "target_entity_id": "ent_B", "relation_type": "REFERS", "weight": 0.7}
            ],
        }

        entB = {
            "id": "ent_B",
            "collection_id": COLLECTION,
            "embeddings": [
                {"model_id": "demo", "model_version": "v1", "vector": embed_text("starter segment entity in EU score 0.55 referring C"), "dimension": DIM}
            ],
            "typed_metadata": {
                "fields": {
                    "segment": {"string_value": "starter", "indexed": True, "filterable": True},
                    "score": {"double_value": 0.55, "indexed": True, "filterable": True},
                    "region": {"string_value": "EU", "indexed": True, "filterable": True},
                }
            },
            "relations": [
                {"source_entity_id": "ent_B", "target_entity_id": "ent_C", "relation_type": "REFERS", "weight": 0.6}
            ],
        }

        entC = {
            "id": "ent_C",
            "collection_id": COLLECTION,
            "embeddings": [
                {"model_id": "demo", "model_version": "v1", "vector": embed_text("pro segment entity in APAC score 0.72"), "dimension": DIM}
            ],
            "typed_metadata": {
                "fields": {
                    "segment": {"string_value": "pro", "indexed": True, "filterable": True},
                    "score": {"double_value": 0.72, "indexed": True, "filterable": True},
                    "region": {"string_value": "APAC", "indexed": True, "filterable": True},
                }
            },
            "relations": [],
        }

        # Upsert (auto-create collection on first call)
        a_id = upsert_entity(http, entA)
        b_id = upsert_entity(http, entB)
        c_id = upsert_entity(http, entC)
        print(f"✅ Upserted entities: {a_id}, {b_id}, {c_id}")

        # Search for similar entities (vector + filters)
        query_vec = embed_text("find pro entities with score over point six")
        result = search_entities(http, query_vec)
        hits = result.get("results", [])
        print("\nHybrid Entity Search (segment='pro' AND score>0.6):")
        if not hits:
            print("  No matches (synthetic data can vary).")
        else:
            for item in hits:
                ent = item.get("entity", {})
                score = item.get("score")
                md = ent.get("typed_metadata", {}).get("fields", {})
                print(
                    f"  {ent.get('id')} | segment={md.get('segment',{}).get('string_value')} "
                    f"| score={md.get('score',{}).get('double_value')} | region={md.get('region',{}).get('string_value')} "
                    f"| sim={score:.4f}"
                )

        # Fetch an entity with relations
        ent = get_entity(http, "ent_A")
        rels = ent.get("relations", [])
        rel_pairs = [(r.get("source_entity_id"), r.get("target_entity_id")) for r in rels]
        print(f"\nRelations for ent_A: {rel_pairs}")

        # Cleanup
        delete_entity(http, "ent_A")
        delete_entity(http, "ent_B")
        delete_entity(http, "ent_C")


if __name__ == "__main__":
    main()
