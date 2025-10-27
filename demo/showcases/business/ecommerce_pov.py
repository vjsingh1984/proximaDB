#!/usr/bin/env python3
"""
E-commerce PoV: Product Search with Business Filters

Demonstrates how product discovery uses vector similarity plus typed filters to
return only in-stock, affordable, highly-rated items in a target category.

Prerequisites
- ProximaDB server running (REST :5678)
- Python SDK installed: `pip install -e clients/python`
- From repo root: `export PYTHONPATH=./clients/python/src`

Run
  python3 demo/showcases/business/ecommerce_pov.py

Expected
- Creates collection `pov_ecommerce_products`
- Inserts synthetic products with metadata
- Searches with filters: category=electronics AND in_stock=true AND price<500 AND rating>=4.0
- Prints matched SKUs with reasons
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
COLLECTION = "pov_ecommerce_products"
DIM = _EMBED_DIM


def embed_text(text: str) -> List[float]:
    return _embed_model.encode([text], convert_to_tensor=False)[0].tolist()


def setup_collection(client: ProximaDBClient):
    # Cleanup if already present
    try:
        client.delete_collection(COLLECTION)
    except Exception:
        pass

    cfg = CollectionConfig(
        name=COLLECTION,
        dimension=DIM,
        filterable_columns=[
            FilterableColumn(name="category", data_type=FilterableDataType.STRING),
            FilterableColumn(name="brand", data_type=FilterableDataType.STRING),
            FilterableColumn(name="price", data_type=FilterableDataType.FLOAT, supports_range=True),
            FilterableColumn(name="in_stock", data_type=FilterableDataType.BOOLEAN),
            FilterableColumn(name="rating", data_type=FilterableDataType.FLOAT, supports_range=True),
        ],
        description="Business PoV: E-commerce product catalog",
    )
    client.create_collection(name=COLLECTION, config=cfg)


def seed_products(client: ProximaDBClient, n: int = 50):
    categories = ["electronics", "books", "fashion"]
    brands = ["Acme", "Globex", "Initech", "Umbrella", "Soylent"]

    records: List[VectorRecord] = []
    for i in range(n):
        category = random.choice(categories)
        brand = random.choice(brands)
        price = round(random.uniform(10, 2000), 2)
        in_stock = random.random() > 0.2  # 80% in stock
        rating = round(random.uniform(3.0, 5.0), 1)
        description = f"{brand} {category} product with features, price {price}"
        records.append(
            VectorRecord(
                id=f"sku_{i:04d}",
                vector=embed_text(description),
                metadata={
                    "category": category,
                    "brand": brand,
                    "price": price,
                    "in_stock": in_stock,
                    "rating": rating,
                    "description": description,
                },
            )
        )

    client.insert_vectors(COLLECTION, records=records)


def run_business_query(client: ProximaDBClient):
    query = embed_text("affordable electronics with good rating in stock")

    # Business constraints: in-stock electronics under $500 with good ratings
    fb = (
        FilterBuilder()
        .equals("category", "electronics")
        .equals("in_stock", True)
        .less_than("price", 500.0)
        .gte("rating", 4.0)
    )

    results = client.search(
        collection_id=COLLECTION,
        vector=query,
        top_k=5,
        metadata_filter=fb.to_dict(),
        include_metadata=True,
    )

    print("\nE-commerce PoV Results (electronics, in-stock, <$500, rating>=4.0):")
    if not results:
        print("  No matches (synthetic data is random; re-run to vary).")
        return

    for r in results:
        md = r.metadata or {}
        print(
            f"  {r.id} | {md.get('brand','?')} {md.get('category','?')} | "
            f"${md.get('price','?')} | rating {md.get('rating','?')} | stock={md.get('in_stock')}"
        )


def main():
    client = ProximaDBClient(url=SERVER_URL, protocol="rest")
    print("🔌 Connected to:", SERVER_URL)

    setup_collection(client)
    seed_products(client)
    run_business_query(client)

    # Cleanup to keep demo idempotent
    try:
        client.delete_collection(COLLECTION)
    except Exception:
        pass


if __name__ == "__main__":
    main()
