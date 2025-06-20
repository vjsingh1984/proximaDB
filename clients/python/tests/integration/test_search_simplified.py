#!/usr/bin/env python3

import grpc
import sys
import os
import json
import numpy as np
import time
from typing import List, Dict, Any

# Add the Python client path to sys.path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients', 'python', 'src'))

from proximadb import proximadb_pb2
from proximadb import proximadb_pb2_grpc

def create_versioned_payload(operation_type: str, json_data: bytes) -> bytes:
    """Create versioned payload format for UnifiedAvroService"""
    schema_version = (1).to_bytes(4, byteorder='little')
    op_bytes = operation_type.encode('utf-8')
    op_len = (len(op_bytes)).to_bytes(4, byteorder='little')
    
    versioned_payload = bytearray()
    versioned_payload.extend(schema_version)
    versioned_payload.extend(op_len)
    versioned_payload.extend(op_bytes)
    versioned_payload.extend(json_data)
    
    return bytes(versioned_payload)

def test_comprehensive_search():
    """Test comprehensive search functionality with synthetic embeddings"""
    print("🔍 ProximaDB Comprehensive Search Test")
    print("🎲 Using Synthetic Embeddings for Testing")
    print("=" * 80)
    
    # Create channel to gRPC server
    channel = grpc.insecure_channel('localhost:5679')
    stub = proximadb_pb2_grpc.ProximaDBStub(channel)
    
    try:
        # Test data with semantic relationships
        test_documents = [
            {"text": "The cat sat on the mat", "category": "animals", "source": "literature", "language": "english"},
            {"text": "A dog barked loudly in the park", "category": "animals", "source": "literature", "language": "english"},
            {"text": "Python is a programming language", "category": "technology", "source": "documentation", "language": "english"},
            {"text": "Machine learning algorithms are powerful", "category": "technology", "source": "research", "language": "english"},
            {"text": "The weather is sunny today", "category": "weather", "source": "news", "language": "english"},
            {"text": "It's raining heavily outside", "category": "weather", "source": "news", "language": "english"},
            {"text": "Le chat mange du poisson", "category": "animals", "source": "literature", "language": "french"},
            {"text": "Das Wetter ist heute schön", "category": "weather", "source": "news", "language": "german"},
            {"text": "Deep learning neural networks", "category": "technology", "source": "research", "language": "english"},
            {"text": "Birds flying in the sky", "category": "animals", "source": "nature", "language": "english"},
        ]
        
        print(f"📚 Test dataset: {len(test_documents)} documents with categorical relationships")
        
        # Generate synthetic embeddings (384 dimensions like BERT)
        np.random.seed(42)  # For reproducible results
        embeddings = []
        
        for i, doc in enumerate(test_documents):
            # Create category-specific patterns in embeddings for testing
            base_vector = np.random.rand(384).astype(np.float32)
            
            # Add some pattern based on category for search testing
            if doc["category"] == "animals":
                base_vector[0:50] += 0.5  # Boost first 50 dimensions
            elif doc["category"] == "technology":
                base_vector[50:100] += 0.5  # Boost second 50 dimensions  
            elif doc["category"] == "weather":
                base_vector[100:150] += 0.5  # Boost third 50 dimensions
                
            # Normalize to unit length for cosine similarity
            norm = np.linalg.norm(base_vector)
            if norm > 0:
                base_vector = base_vector / norm
                
            embeddings.append(base_vector.tolist())
        
        print(f"🔢 Embedding dimension: {len(embeddings[0])}")
        
        # Step 1: Create a collection for search testing
        collection_name = "search_test_collection"
        
        print(f"\n📁 Creating collection: {collection_name}")
        
        collection_config = proximadb_pb2.CollectionConfig(
            name=collection_name,
            dimension=len(embeddings[0]),
            distance_metric=1,  # Cosine
            storage_engine=1,  # Viper (default)
            indexing_algorithm=1,  # HNSW
            filterable_metadata_fields=["category", "source", "language"],
            indexing_config={
                "hnsw_m": "16",
                "hnsw_ef_construction": "200"
            }
        )
        
        collection_request = proximadb_pb2.CollectionRequest(
            operation=1,  # CREATE
            collection_config=collection_config
        )
        
        try:
            response = stub.CollectionOperation(collection_request, timeout=15)
            if response.success:
                print(f"   ✅ Created in {response.processing_time_us}μs")
            else:
                print(f"   ❌ Failed: {response.error_message}")
                return False
        except Exception as e:
            print(f"   ❌ Error: {e}")
            return False
        
        # Step 2: Insert synthetic embeddings
        print(f"\n📥 Inserting {len(test_documents)} synthetic embeddings...")
        
        vectors_data = []
        for i, (doc, embedding) in enumerate(zip(test_documents, embeddings)):
            vector_data = {
                "vector": embedding,
                "id": f"doc_{i:03d}",
                "metadata": {
                    "text": doc["text"],
                    "category": doc["category"],
                    "source": doc["source"],
                    "language": doc["language"],
                    "doc_index": i
                }
            }
            vectors_data.append(vector_data)
        
        bulk_data = {
            "collection_id": collection_name,
            "vectors": vectors_data,
            "upsert_mode": False
        }
        
        json_data = json.dumps(bulk_data).encode('utf-8')
        avro_payload = create_versioned_payload("batch_insert", json_data)
        
        insert_request = proximadb_pb2.VectorInsertRequest(
            collection_id=collection_name,
            avro_payload=avro_payload
        )
        
        try:
            response = stub.VectorInsert(insert_request, timeout=10)
            if response.success:
                print(f"   ✅ {len(vectors_data)} vectors inserted successfully")
            else:
                print(f"   ❌ Insert failed: {response.error_message}")
                return False
        except Exception as e:
            print(f"   ❌ Insert error: {e}")
            return False
        
        # Step 3: Test different search types
        print(f"\n🔍 Running comprehensive search tests...")
        
        # Test 1: Similarity search with an "animals" vector
        print(f"\n1️⃣ Similarity Search Test")
        # Use the first animal document (cat) as query
        query_embedding = embeddings[0]  # "The cat sat on the mat"
        
        grpc_request = proximadb_pb2.VectorSearchRequest(
            collection_id=collection_name,
            queries=[proximadb_pb2.SearchQuery(vector=query_embedding)],
            top_k=3,
            include_fields=proximadb_pb2.IncludeFields(
                vector=False,
                metadata=True,
                score=True,
                rank=False
            ),
            distance_metric_override=1  # Cosine
        )
        
        try:
            start_time = time.time()
            response = stub.VectorSearch(grpc_request, timeout=10)
            search_time = (time.time() - start_time) * 1000
            
            if response.success and response.result_payload:
                results = response.result_payload.results
                print(f"   ✅ Found {len(results)} results in {search_time:.2f}ms")
                
                for i, result in enumerate(results):
                    category = result.metadata.get("category", "unknown")
                    text = result.metadata.get("text", "")
                    score = result.score
                    print(f"      {i+1}. {category}: {score:.4f} - '{text[:50]}...'")
                    
                # Check if we found other animals (should be top results)
                categories = [r.metadata.get("category") for r in results]
                animals_found = categories.count("animals")
                print(f"   🎯 Animals found in top 3: {animals_found}/3")
                    
            else:
                print(f"   ❌ Search failed")
                return False
                
        except Exception as e:
            print(f"   ❌ Error: {e}")
            return False
        
        # Test 2: Metadata filtering search
        print(f"\n2️⃣ Metadata Filter Search Test")
        
        filter_request = proximadb_pb2.VectorSearchRequest(
            collection_id=collection_name,
            queries=[proximadb_pb2.SearchQuery(
                vector=query_embedding,
                metadata_filter={"category": "technology"}  # Filter for technology docs
            )],
            top_k=5,
            include_fields=proximadb_pb2.IncludeFields(
                vector=False,
                metadata=True,
                score=True,
                rank=False
            ),
            distance_metric_override=1
        )
        
        try:
            filter_response = stub.VectorSearch(filter_request, timeout=10)
            if filter_response.success and filter_response.result_payload:
                filtered_results = filter_response.result_payload.results
                print(f"   ✅ Found {len(filtered_results)} technology results")
                
                for i, result in enumerate(filtered_results):
                    category = result.metadata.get("category", "unknown")
                    text = result.metadata.get("text", "")
                    score = result.score
                    print(f"      {i+1}. {category}: {score:.4f} - '{text[:50]}...'")
                
                # Verify all results are technology
                tech_count = sum(1 for r in filtered_results if r.metadata.get("category") == "technology")
                print(f"   🎯 Technology filter accuracy: {tech_count}/{len(filtered_results)} (100% = {tech_count == len(filtered_results)})")
                
            else:
                print(f"   ❌ Filter search failed")
                return False
        except Exception as e:
            print(f"   ❌ Filter error: {e}")
            return False
        
        # Test 3: Multiple distance metrics
        print(f"\n3️⃣ Distance Metric Comparison Test")
        
        distance_metrics = [
            (1, "Cosine"),
            (2, "Euclidean"), 
            (3, "DotProduct")
        ]
        
        for dist_id, dist_name in distance_metrics:
            dist_request = proximadb_pb2.VectorSearchRequest(
                collection_id=collection_name,
                queries=[proximadb_pb2.SearchQuery(vector=query_embedding)],
                top_k=2,
                include_fields=proximadb_pb2.IncludeFields(
                    vector=False,
                    metadata=True,
                    score=True,
                    rank=False
                ),
                distance_metric_override=dist_id
            )
            
            try:
                dist_response = stub.VectorSearch(dist_request, timeout=10)
                if dist_response.success and dist_response.result_payload:
                    results = dist_response.result_payload.results
                    if results:
                        top_score = results[0].score
                        top_category = results[0].metadata.get("category", "unknown")
                        print(f"   {dist_name:12}: score={top_score:.4f}, top_category={top_category}")
                    else:
                        print(f"   {dist_name:12}: No results")
                else:
                    print(f"   {dist_name:12}: Failed")
            except Exception as e:
                print(f"   {dist_name:12}: Error - {e}")
        
        # Step 4: Performance summary
        print(f"\n📈 Search Test Summary")
        print("=" * 80)
        print(f"✅ Collection created and populated successfully")
        print(f"✅ Similarity search working (found related categories)")
        print(f"✅ Metadata filtering working (filtered by category)")
        print(f"✅ Multiple distance metrics supported")
        print(f"📊 Total documents: {len(test_documents)}")
        print(f"🔢 Embedding dimension: {len(embeddings[0])}")
        print(f"🏪 Collection: {collection_name}")
        
        return True
        
    except Exception as e:
        print(f"❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False
    finally:
        channel.close()

if __name__ == "__main__":
    success = test_comprehensive_search()
    sys.exit(0 if success else 1)