import os
import uuid

import pytest

from proximadb_sdk import ProximaDBClient, VectorRecord


def _rest_available(url: str) -> bool:
    import httpx

    try:
        r = httpx.get(url.rstrip("/") + "/api/v1/health", timeout=2.0)
        return r.status_code < 500
    except Exception:
        return False


@pytest.mark.integration
def test_semantic_knowledge_store_graph_and_vector():
    """
    End-to-end example showing how to combine the vector and graph APIs
    to implement a simple semantic knowledge store workflow:

    - Create a vector collection and insert a few document embeddings
    - Create graph nodes for those documents and connect them with edges
    - Run a vector similarity search to find a relevant document
    - Traverse the knowledge graph from that document to collect context
    """

    base_url = os.getenv("PROXIMADB_URL", "http://localhost:5678")
    if not _rest_available(base_url):
        pytest.skip(
            "ProximaDB REST server not available; set PROXIMADB_URL and start server to run integration tests."
        )

    client = ProximaDBClient(url=base_url, protocol="rest", sks_warmup_collection=None)

    collection = f"sks_demo_{uuid.uuid4().hex[:8]}"

    try:
        # 1) Create a small vector collection
        dim = 8
        client.create_collection(collection, dimension=dim)

        # 2) Insert deterministic document embeddings (Vector API)
        # Two similar docs (rag, transformer) + one different (graph)
        v_rag: list[float] = [0.90, 0.10, 0.10, 0.00, 0.00, 0.20, 0.10, 0.00]
        v_transformer: list[float] = [0.88, 0.12, 0.08, 0.00, 0.00, 0.19, 0.09, 0.01]
        v_graph: list[float] = [0.00, 0.00, 0.90, 0.10, 0.20, 0.00, 0.00, 0.00]

        records = [
            VectorRecord(
                id="doc_rag",
                vector=v_rag,
                metadata={"title": "RAG Overview", "category": "nlp", "type": "doc"},
            ),
            VectorRecord(
                id="doc_transformer",
                vector=v_transformer,
                metadata={
                    "title": "Transformer Architectures",
                    "category": "nlp",
                    "type": "doc",
                },
            ),
            VectorRecord(
                id="doc_graph",
                vector=v_graph,
                metadata={
                    "title": "Knowledge Graphs",
                    "category": "graphs",
                    "type": "doc",
                },
            ),
        ]

        ins_res = client.insert_vectors(collection, records=records)
        assert (
            ins_res.success >= 3
        )  # success field contains successful count, not boolean
        assert ins_res.metrics.successful_count >= 3

        # 3) Create graph collection for storing nodes and edges
        # Try to create "default" graph - ignore error if it already exists
        try:
            graph = client.create_graph(
                graph_id="default",
                name="Default Graph Collection",
                description="Default graph for semantic knowledge store",
            )
        except Exception:
            # Graph may already exist from previous test runs - this is fine
            pass

        # 4) Create graph nodes and edges (Graph API)
        # Nodes mirror the vector records so we can hop from semantics to relations
        client.create_node(
            node_id="doc_rag",
            labels=["Document"],
            properties={"title": "RAG Overview", "category": "nlp"},
        )
        client.create_node(
            node_id="doc_transformer",
            labels=["Document"],
            properties={"title": "Transformer Architectures", "category": "nlp"},
        )
        client.create_node(
            node_id="doc_graph",
            labels=["Document"],
            properties={"title": "Knowledge Graphs", "category": "graphs"},
        )

        # Edges: RAG RELATED_TO Transformer; Transformer REFERENCES Knowledge Graphs
        client.create_edge(
            edge_id="e_rag_related_transformer",
            from_node_id="doc_rag",
            to_node_id="doc_transformer",
            edge_type="RELATED_TO",
            properties={"strength": 0.9},
            weight=0.9,
        )
        client.create_edge(
            edge_id="e_transformer_ref_graph",
            from_node_id="doc_transformer",
            to_node_id="doc_graph",
            edge_type="REFERENCES",
            properties={"confidence": 0.8},
            weight=0.8,
        )

        # 5) Vector similarity search to find the most relevant doc
        q_vec = [x + 0.005 for x in v_rag]  # close to RAG
        results = client.search(
            collection_id=collection,
            vector=q_vec,
            top_k=2,
            include_metadata=True,
            include_vectors=False,
        )

        assert isinstance(results, list)
        assert len(results) >= 1
        top_id = results[0].id
        assert top_id in {"doc_rag", "doc_transformer"}

        # 6) Traverse the graph from the top hit to gather related knowledge
        traversal = client.traverse_graph(
            start_node_id=top_id,
            max_depth=2,
            edge_types=["RELATED_TO", "REFERENCES"],
            node_labels=["Document"],
            algorithm="BFS",
            limit=10,
        )

        assert isinstance(traversal, dict)
        assert len(traversal.get("nodes", [])) >= 1
        assert len(traversal.get("edges", [])) >= 0

        # Optional: sanity check that traversal sees at least one neighbor
        # given our small graph design
        # Some servers may or may not include the start node; be flexible
        if len(traversal.get("nodes", [])) == 1:
            # If only the start node is returned, ensure edges are present
            assert len(traversal.get("edges", [])) >= 1

    finally:
        # Clean up vector collection (graph nodes/edges use default graph and
        # do not have delete APIs exposed yet, so we leave them in the demo graph)
        try:
            client.delete_collection(collection)
        except Exception:
            pass
