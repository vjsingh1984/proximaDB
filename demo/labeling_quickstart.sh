#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:5678}"

echo "== ProximaDB labeling quickstart demo =="

echo "Creating document collections..."
curl -sS -X POST "$BASE_URL/api/v1/documents/collections" \
  -H "Content-Type: application/json" \
  -d '{"name": "labeling_assets"}'
echo
curl -sS -X POST "$BASE_URL/api/v1/documents/collections" \
  -H "Content-Type: application/json" \
  -d '{"name": "labeling_decisions"}'
echo

echo "Inserting labeling assets..."
curl -sS -X POST "$BASE_URL/api/v1/documents/collections/labeling_assets/documents" \
  -H "Content-Type: application/json" \
  -d '{"document":{"asset_id":"asset_1","uri":"s3://labels/raw/asset_1.txt","label_status":"unlabeled","dataset":"support","source":"tickets"}}'
echo
curl -sS -X POST "$BASE_URL/api/v1/documents/collections/labeling_assets/documents" \
  -H "Content-Type: application/json" \
  -d '{"document":{"asset_id":"asset_2","uri":"s3://labels/raw/asset_2.txt","label_status":"unlabeled","dataset":"support","source":"tickets"}}'
echo
curl -sS -X POST "$BASE_URL/api/v1/documents/collections/labeling_assets/documents" \
  -H "Content-Type: application/json" \
  -d '{"document":{"asset_id":"asset_3","uri":"s3://labels/raw/asset_3.txt","label_status":"unlabeled","dataset":"support","source":"tickets"}}'
echo

echo "Creating vector collection for embeddings..."
curl -sS -X POST "$BASE_URL/api/v1/collections" \
  -H "Content-Type: application/json" \
  -d '{"collection_id":"labeling_embeddings","collection_config":{"dimension":384,"storage_engine":"sst","distance_metric":"cosine"}}'
echo

echo "Inserting embeddings with metadata..."
curl -sS -X POST "$BASE_URL/api/v1/vectors/batch" \
  -H "Content-Type: application/json" \
  -d '{
    "collection_id": "labeling_embeddings",
    "operation": "insert",
    "vectors": [
      {"id": "asset_1", "vector": [0.1, 0.2], "metadata": {"asset_id": {"stringValue": "asset_1"}, "label_status": {"stringValue": "unlabeled"}, "dataset": {"stringValue": "support"}}},
      {"id": "asset_2", "vector": [0.12, 0.18], "metadata": {"asset_id": {"stringValue": "asset_2"}, "label_status": {"stringValue": "unlabeled"}, "dataset": {"stringValue": "support"}}},
      {"id": "asset_3", "vector": [0.2, 0.1], "metadata": {"asset_id": {"stringValue": "asset_3"}, "label_status": {"stringValue": "unlabeled"}, "dataset": {"stringValue": "support"}}}
    ]
  }'
echo

echo "Creating label taxonomy graph..."
curl -sS -X POST "$BASE_URL/api/v1/graph/graphs" \
  -H "Content-Type: application/json" \
  -d '{"graph_id":"label_taxonomy"}'
echo

curl -sS -X POST "$BASE_URL/api/v1/graph/graphs/label_taxonomy/nodes" \
  -H "Content-Type: application/json" \
  -d '{"node":{"id":"label:refund","labels":["Label"],"properties":{"name":"Refund"}}}'
echo
curl -sS -X POST "$BASE_URL/api/v1/graph/graphs/label_taxonomy/nodes" \
  -H "Content-Type: application/json" \
  -d '{"node":{"id":"label:billing","labels":["Label"],"properties":{"name":"Billing"}}}'
echo
curl -sS -X POST "$BASE_URL/api/v1/graph/graphs/label_taxonomy/edges" \
  -H "Content-Type: application/json" \
  -d '{"edge":{"from_node_id":"label:refund","to_node_id":"label:billing","edge_type":"RELATED_TO","weight":1.0}}'
echo

echo "Hybrid search (semantic + filters)..."
curl -sS -X POST "$BASE_URL/api/v1/search" \
  -H "Content-Type: application/json" \
  -d '{
    "collection_id": "labeling_embeddings",
    "query_vector": [0.1, 0.2],
    "top_k": 10,
    "metadata_filters": [
      {"field": "label_status", "operator": "EQUALS", "value": {"stringValue": "unlabeled"}},
      {"field": "dataset", "operator": "EQUALS", "value": {"stringValue": "support"}}
    ]
  }'
echo

echo "Labeling loop: write decision and update embedding metadata..."
curl -sS -X POST "$BASE_URL/api/v1/documents/collections/labeling_decisions/documents" \
  -H "Content-Type: application/json" \
  -d '{"document":{"asset_id":"asset_1","label":"Refund","labeler":"user_42","decision_ts":"2025-01-01T00:00:00Z"}}'
echo

curl -sS -X DELETE "$BASE_URL/api/v1/vectors/labeling_embeddings/asset_1"
echo
curl -sS -X POST "$BASE_URL/api/v1/vectors/batch" \
  -H "Content-Type: application/json" \
  -d '{
    "collection_id": "labeling_embeddings",
    "operation": "insert",
    "vectors": [
      {"id": "asset_1", "vector": [0.1, 0.2], "metadata": {"asset_id": {"stringValue": "asset_1"}, "label_status": {"stringValue": "labeled"}, "label": {"stringValue": "Refund"}, "dataset": {"stringValue": "support"}}}
    ]
  }'
echo

echo "Re-query for unlabeled items..."
curl -sS -X POST "$BASE_URL/api/v1/search" \
  -H "Content-Type: application/json" \
  -d '{
    "collection_id": "labeling_embeddings",
    "query_vector": [0.1, 0.2],
    "top_k": 10,
    "metadata_filters": [
      {"field": "label_status", "operator": "EQUALS", "value": {"stringValue": "unlabeled"}}
    ]
  }'
echo

echo "Done."
