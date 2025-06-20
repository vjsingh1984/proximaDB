#!/usr/bin/env python3

import grpc
import sys
import os
import json
import numpy as np
import time
from typing import List, Dict, Any
from sentence_transformers import SentenceTransformer

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

def generate_bert_embeddings(texts: List[str]) -> List[List[float]]:
    """Generate BERT embeddings for text data"""
    print("🔄 Loading BERT model (sentence-transformers/all-MiniLM-L6-v2)...")
    
    try:
        # Use a lightweight but effective BERT model
        model = SentenceTransformer('sentence-transformers/all-MiniLM-L6-v2')
        print("✅ BERT model loaded successfully")
        
        print(f"🔄 Generating embeddings for {len(texts)} texts...")
        embeddings = model.encode(texts, convert_to_tensor=False, normalize_embeddings=True)
        
        # Convert to list of lists
        return [embedding.tolist() for embedding in embeddings]
        
    except Exception as e:
        print(f"❌ Failed to load BERT model: {e}")
        print("📝 Using random embeddings as fallback...")
        # Fallback to random embeddings with same dimension as MiniLM (384)
        return [np.random.rand(384).tolist() for _ in texts]

def test_comprehensive_search():
    """Test comprehensive search functionality with BERT embeddings"""
    print("🔍 ProximaDB Comprehensive Search Test")
    print("🤖 Using BERT Embeddings for Semantic Similarity")
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
        
        print(f"📚 Test dataset: {len(test_documents)} documents with semantic relationships")
        
        # Generate BERT embeddings
        texts = [doc["text"] for doc in test_documents]
        embeddings = generate_bert_embeddings(texts)
        
        print(f"🔢 Embedding dimension: {len(embeddings[0])}")
        
        # Step 1: Create collection for each distance metric and indexing algorithm
        distance_metrics = [
            (1, "Cosine"),
            (2, "Euclidean"), 
            (3, "DotProduct"),
            (4, "Hamming")
        ]
        
        indexing_algorithms = [
            (1, "HNSW"),
            (2, "IVF"),
            (3, "PQ"),
            (4, "Flat"),
            (5, "Annoy")
        ]
        
        collections_created = []
        
        for dist_id, dist_name in distance_metrics:
            for idx_id, idx_name in indexing_algorithms:
                collection_name = f"bert_search_{dist_name.lower()}_{idx_name.lower()}"
                
                print(f"\n📁 Creating collection: {collection_name}")
                print(f"   Distance: {dist_name}, Index: {idx_name}")
                
                collection_config = proximadb_pb2.CollectionConfig(
                    name=collection_name,
                    dimension=len(embeddings[0]),
                    distance_metric=dist_id,
                    storage_engine=1,  # Viper
                    indexing_algorithm=idx_id,
                    filterable_metadata_fields=["category", "source", "language"],
                    indexing_config={
                        "hnsw_m": "16" if idx_name == "HNSW" else "0",
                        "hnsw_ef_construction": "200" if idx_name == "HNSW" else "0",
                        "ivf_nlist": "100" if idx_name == "IVF" else "0",
                        "pq_m": "8" if idx_name == "PQ" else "0"
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
                        collections_created.append(collection_name)
                    else:
                        print(f"   ❌ Failed: {response.error_message}")
                except Exception as e:
                    print(f"   ❌ Error: {e}")
        
        # Step 2: Insert BERT embeddings into all collections
        print(f"\n📥 Inserting {len(test_documents)} BERT embeddings into {len(collections_created)} collections...")
        
        for collection_name in collections_created:
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
                    print(f"   ✅ {collection_name}: {len(vectors_data)} vectors inserted")
                else:
                    print(f"   ❌ {collection_name}: Insert failed")
            except Exception as e:
                print(f"   ❌ {collection_name}: Error {e}")
        
        # Step 3: Comprehensive search tests
        print(f"\n🔍 Running comprehensive search tests...")
        
        # Test queries with semantic meaning
        test_queries = [
            {
                "text": "cat and dog pets", 
                "expected_category": "animals",
                "description": "Animal-related semantic search"
            },
            {
                "text": "artificial intelligence programming", 
                "expected_category": "technology",
                "description": "Technology semantic search"
            },
            {
                "text": "sunny rainy climate", 
                "expected_category": "weather",
                "description": "Weather semantic search"
            }
        ]
        
        # Generate query embeddings
        query_texts = [q["text"] for q in test_queries]
        query_embeddings = generate_bert_embeddings(query_texts)
        
        search_results = {}
        
        for collection_name in collections_created[:4]:  # Test first 4 collections to save time
            collection_results = {}
            
            print(f"\n📊 Testing collection: {collection_name}")
            
            for query_idx, (query_info, query_embedding) in enumerate(zip(test_queries, query_embeddings)):
                print(f"   🔎 Query {query_idx + 1}: {query_info['description']}")
                print(f"      Text: '{query_info['text']}'")
                
                # Test 1: Semantic similarity search
                search_request = json.dumps({
                    "collection_id": collection_name,
                    "queries": [query_embedding],
                    "top_k": 3,
                    "include_vectors": False,
                    "include_metadata": True,
                    "distance_metric": 1,  # Will be overridden by collection
                    "index_algorithm": 1,  # Will be overridden by collection
                    "search_params": {"ef_search": 50}
                }).encode('utf-8')
                
                avro_payload = create_versioned_payload("vector_search", search_request)
                
                grpc_request = proximadb_pb2.VectorSearchRequest(
                    collection_id=collection_name,
                    queries=[proximadb_pb2.VectorQuery(vector=query_embedding)],
                    top_k=3,
                    include_vectors=False,
                    include_metadata=True,
                    distance_metric=1,
                    index_algorithm=1,
                    search_params={"ef_search": "50"}
                )
                
                try:
                    start_time = time.time()
                    response = stub.VectorSearch(grpc_request, timeout=10)
                    search_time = (time.time() - start_time) * 1000
                    
                    if response.success and response.result_payload:
                        results = response.result_payload.results
                        print(f"      ✅ Found {len(results)} results in {search_time:.2f}ms")
                        
                        # Analyze semantic relevance
                        categories_found = []
                        for i, result in enumerate(results):
                            category = result.metadata.get("category", "unknown")
                            text = result.metadata.get("text", "")
                            score = result.score
                            categories_found.append(category)
                            print(f"         {i+1}. {category}: {score:.4f} - '{text[:50]}...'")
                        
                        # Check if expected category is in top results
                        expected_found = query_info["expected_category"] in categories_found
                        print(f"      🎯 Expected category '{query_info['expected_category']}' found: {expected_found}")
                        
                        collection_results[f"query_{query_idx + 1}"] = {
                            "success": True,
                            "search_time_ms": search_time,
                            "results_count": len(results),
                            "expected_found": expected_found,
                            "categories": categories_found
                        }
                    else:
                        print(f"      ❌ Search failed")
                        collection_results[f"query_{query_idx + 1}"] = {"success": False}
                        
                except Exception as e:
                    print(f"      ❌ Error: {e}")
                    collection_results[f"query_{query_idx + 1}"] = {"success": False, "error": str(e)}
                
                # Test 2: Metadata filtering
                print(f"      🏷️  Testing metadata filter: category='{query_info['expected_category']}'")
                
                filter_request = proximadb_pb2.VectorSearchRequest(
                    collection_id=collection_name,
                    queries=[proximadb_pb2.VectorQuery(vector=query_embedding)],
                    top_k=5,
                    include_vectors=False,
                    include_metadata=True,
                    metadata_filters={query_info["expected_category"]: query_info["expected_category"]},
                    distance_metric=1,
                    index_algorithm=1
                )
                
                try:
                    filter_response = stub.VectorSearch(filter_request, timeout=10)
                    if filter_response.success and filter_response.result_payload:
                        filtered_results = filter_response.result_payload.results
                        print(f"         ✅ Filtered to {len(filtered_results)} results")
                        
                        # Verify all results match the filter
                        all_match = all(
                            result.metadata.get("category") == query_info["expected_category"] 
                            for result in filtered_results
                        )
                        print(f"         🎯 All results match filter: {all_match}")
                    else:
                        print(f"         ❌ Filter search failed")
                except Exception as e:
                    print(f"         ❌ Filter error: {e}")
            
            search_results[collection_name] = collection_results
        
        # Step 4: Performance summary
        print(f"\n📈 Search Performance Summary")
        print("=" * 80)
        
        total_searches = 0
        successful_searches = 0
        total_time = 0
        semantic_accuracy = 0
        
        for collection_name, results in search_results.items():
            print(f"\n🏪 Collection: {collection_name}")
            collection_searches = len(results)
            collection_success = sum(1 for r in results.values() if r.get("success", False))
            collection_time = sum(r.get("search_time_ms", 0) for r in results.values() if r.get("success", False))
            collection_accuracy = sum(1 for r in results.values() if r.get("expected_found", False))
            
            print(f"   📊 Searches: {collection_success}/{collection_searches} successful")
            print(f"   ⚡ Avg latency: {collection_time/max(collection_success, 1):.2f}ms")
            print(f"   🎯 Semantic accuracy: {collection_accuracy}/{collection_success} ({collection_accuracy/max(collection_success, 1)*100:.1f}%)")
            
            total_searches += collection_searches
            successful_searches += collection_success
            total_time += collection_time
            semantic_accuracy += collection_accuracy
        
        print(f"\n🏆 OVERALL RESULTS:")
        print(f"   📊 Total searches: {successful_searches}/{total_searches}")
        print(f"   ⚡ Average latency: {total_time/max(successful_searches, 1):.2f}ms")
        print(f"   🎯 Semantic accuracy: {semantic_accuracy}/{successful_searches} ({semantic_accuracy/max(successful_searches, 1)*100:.1f}%)")
        print(f"   🤖 BERT embedding dimension: {len(embeddings[0])}")
        print(f"   📚 Test documents: {len(test_documents)}")
        print(f"   🏪 Collections tested: {len(search_results)}")
        
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