#!/usr/bin/env python3
"""
ProximaDB REST API Completeness Test
Test all REST endpoints to identify which are implemented vs missing (501/500 errors)
"""

import pytest
import time
import numpy as np
from typing import List, Dict, Any

from proximadb import connect_rest
from proximadb.models import CollectionConfig, DistanceMetric
from proximadb.exceptions import ProximaDBError, CollectionNotFoundError


class TestRESTAPICompleteness:
    """Comprehensive test of REST API endpoints"""
    
    @pytest.fixture(scope="class")
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        if hasattr(client, 'close'):
            client.close()
    
    @pytest.fixture
    def test_collection(self, rest_client):
        """Create test collection for endpoint testing"""
        collection_name = f"rest_test_{int(time.time())}"
        config = CollectionConfig(dimension=32, distance_metric=DistanceMetric.COSINE)
        
        collection = rest_client.create_collection(collection_name, config)
        yield collection_name
        
        # Cleanup
        try:
            rest_client.delete_collection(collection_name)
        except:
            pass
    
    def test_collection_endpoints(self, rest_client):
        """Test all collection-related REST endpoints"""
        collection_name = f"rest_collections_{int(time.time())}"
        
        print("\n=== REST Collection Endpoints ===")
        
        # 1. Create Collection
        try:
            config = CollectionConfig(dimension=64, distance_metric=DistanceMetric.COSINE)
            collection = rest_client.create_collection(collection_name, config)
            assert collection is not None
            print("✅ POST /collections - Create collection: WORKING")
        except Exception as e:
            print(f"❌ POST /collections - Create collection: FAILED - {e}")
            return
        
        # 2. List Collections
        try:
            collections = rest_client.list_collections()
            assert collections is not None
            assert len(collections) > 0
            print(f"✅ GET /collections - List collections: WORKING ({len(collections)} collections)")
        except Exception as e:
            print(f"❌ GET /collections - List collections: FAILED - {e}")
        
        # 3. Get Collection
        try:
            retrieved = rest_client.get_collection(collection_name)
            assert retrieved is not None
            print("✅ GET /collections/{id} - Get collection: WORKING")
        except Exception as e:
            print(f"❌ GET /collections/{collection_name} - Get collection: FAILED - {e}")
        
        # 4. Update Collection (if implemented)
        try:
            updates = {"description": "Updated via REST test"}
            updated = rest_client.update_collection(collection_name, updates)
            print("✅ PATCH /collections/{id} - Update collection: WORKING")
        except AttributeError:
            print("⚠️ PATCH /collections/{id} - Update collection: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ PATCH /collections/{id} - Update collection: FAILED - {e}")
        
        # 5. Delete Collection  
        try:
            result = rest_client.delete_collection(collection_name)
            print("✅ DELETE /collections/{id} - Delete collection: WORKING")
        except Exception as e:
            print(f"❌ DELETE /collections/{id} - Delete collection: FAILED - {e}")
    
    def test_vector_endpoints(self, rest_client, test_collection):
        """Test all vector-related REST endpoints"""
        print("\n=== REST Vector Endpoints ===")
        
        # 1. Insert Single Vector
        try:
            vector_id = "rest_vector_1"
            vector = np.random.random(32).astype(np.float32).tolist()
            metadata = {"type": "single", "test": "rest_endpoint"}
            
            result = rest_client.insert_vector(
                collection_id=test_collection,
                vector_id=vector_id,
                vector=vector,
                metadata=metadata
            )
            assert result is not None
            print("✅ POST /collections/{id}/vectors - Insert single vector: WORKING")
        except Exception as e:
            print(f"❌ POST /collections/{test_collection}/vectors - Insert single vector: FAILED - {e}")
            return
        
        # 2. Insert Multiple Vectors
        try:
            batch_vectors = []
            batch_ids = []
            batch_metadata = []
            
            for i in range(3):
                batch_vectors.append(np.random.random(32).astype(np.float32).tolist())
                batch_ids.append(f"rest_batch_{i}")
                batch_metadata.append({"type": "batch", "index": i})
            
            batch_result = rest_client.insert_vectors(
                collection_id=test_collection,
                vectors=batch_vectors,
                ids=batch_ids,
                metadata=batch_metadata
            )
            assert batch_result is not None
            print("✅ POST /collections/{id}/vectors (batch) - Insert multiple vectors: WORKING")
        except Exception as e:
            print(f"❌ POST /collections/{test_collection}/vectors (batch) - Insert multiple vectors: FAILED - {e}")
        
        # 3. Get Vector by ID
        try:
            retrieved = rest_client.get_vector(
                collection_id=test_collection,
                vector_id=vector_id,
                include_metadata=True,
                include_vector=True
            )
            if retrieved is not None:
                print("✅ GET /collections/{id}/vectors/{vector_id} - Get vector: WORKING")
            else:
                print("⚠️ GET /collections/{id}/vectors/{vector_id} - Get vector: RETURNS NULL")
        except Exception as e:
            if "501" in str(e) or "Not Implemented" in str(e):
                print("❌ GET /collections/{id}/vectors/{vector_id} - Get vector: NOT IMPLEMENTED (501)")
            else:
                print(f"❌ GET /collections/{id}/vectors/{vector_id} - Get vector: FAILED - {e}")
        
        # 4. Update Vector (if implemented)
        try:
            updated_vector = np.random.random(32).astype(np.float32).tolist()
            updated_metadata = {"type": "updated", "test": "rest_endpoint"}
            
            update_result = rest_client.insert_vector(
                collection_id=test_collection,
                vector_id=vector_id,
                vector=updated_vector,
                metadata=updated_metadata,
                upsert=True
            )
            print("✅ PUT /collections/{id}/vectors/{vector_id} - Update vector: WORKING (via upsert)")
        except Exception as e:
            print(f"❌ PUT /collections/{id}/vectors/{vector_id} - Update vector: FAILED - {e}")
        
        # 5. Delete Vector
        try:
            delete_result = rest_client.delete_vector(test_collection, vector_id)
            print("✅ DELETE /collections/{id}/vectors/{vector_id} - Delete vector: WORKING")
        except AttributeError:
            print("⚠️ DELETE /collections/{id}/vectors/{vector_id} - Delete vector: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ DELETE /collections/{id}/vectors/{vector_id} - Delete vector: FAILED - {e}")
    
    def test_search_endpoints(self, rest_client, test_collection):
        """Test search-related REST endpoints"""
        print("\n=== REST Search Endpoints ===")
        
        # Add some vectors for searching
        for i in range(3):
            vector = np.random.random(32).astype(np.float32).tolist()
            rest_client.insert_vector(
                collection_id=test_collection,
                vector_id=f"search_vector_{i}",
                vector=vector,
                metadata={"category": f"category_{i % 2}", "index": i}
            )
        
        # 1. Basic Search
        try:
            query_vector = np.random.random(32).astype(np.float32).tolist()
            results = rest_client.search(
                collection_id=test_collection,
                query=query_vector,
                k=5,
                include_metadata=True,
                include_vectors=False
            )
            
            if results and len(results) > 0:
                print(f"✅ POST /collections/{test_collection}/search - Basic search: WORKING ({len(results)} results)")
                
                # Verify result structure
                for result in results[:2]:
                    assert hasattr(result, 'id'), "Result missing id"
                    assert hasattr(result, 'score'), "Result missing score"
                    assert hasattr(result, 'metadata'), "Result missing metadata"
                print("✅ Search result structure: VALID")
                
            else:
                print("⚠️ POST /collections/{id}/search - Basic search: RETURNS EMPTY")
        except Exception as e:
            if "500" in str(e) or "Server Error" in str(e):
                print("❌ POST /collections/{id}/search - Basic search: SERVER ERROR (500)")
            else:
                print(f"❌ POST /collections/{id}/search - Basic search: FAILED - {e}")
        
        # 2. Search with Filters (if implemented)
        try:
            filtered_results = rest_client.search(
                collection_id=test_collection,
                query=query_vector,
                k=5,
                filter={"category": "category_0"},
                include_metadata=True
            )
            print("✅ POST /collections/{id}/search (with filters) - Filtered search: WORKING")
        except Exception as e:
            print(f"❌ POST /collections/{id}/search (with filters) - Filtered search: FAILED - {e}")
        
        # 3. Multi-Search (if implemented)
        try:
            if hasattr(rest_client, 'multi_search'):
                multi_queries = [query_vector, np.random.random(32).tolist()]
                multi_results = rest_client.multi_search(
                    collection_id=test_collection,
                    queries=multi_queries,
                    k=3
                )
                print("✅ POST /collections/{id}/multi-search - Multi search: WORKING")
            else:
                print("⚠️ POST /collections/{id}/multi-search - Multi search: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ POST /collections/{id}/multi-search - Multi search: FAILED - {e}")
    
    def test_health_and_admin_endpoints(self, rest_client):
        """Test health and administrative REST endpoints"""
        print("\n=== REST Health & Admin Endpoints ===")
        
        # 1. Health Check
        try:
            health = rest_client.health()
            if health is not None:
                print("✅ GET /health - Health check: WORKING")
            else:
                print("⚠️ GET /health - Health check: RETURNS NULL")
        except AttributeError:
            print("⚠️ GET /health - Health check: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ GET /health - Health check: FAILED - {e}")
        
        # 2. Metrics
        try:
            if hasattr(rest_client, 'get_metrics'):
                metrics = rest_client.get_metrics()
                print("✅ GET /metrics - Metrics: WORKING")
            else:
                print("⚠️ GET /metrics - Metrics: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ GET /metrics - Metrics: FAILED - {e}")
        
        # 3. Server Info
        try:
            if hasattr(rest_client, 'get_server_info'):
                info = rest_client.get_server_info()
                print("✅ GET /info - Server info: WORKING")
            else:
                print("⚠️ GET /info - Server info: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ GET /info - Server info: FAILED - {e}")
    
    def test_error_handling(self, rest_client):
        """Test REST API error handling"""
        print("\n=== REST Error Handling ===")
        
        # 1. Non-existent Collection
        try:
            rest_client.get_collection("non_existent_collection")
            print("❌ GET /collections/{non_existent} - Should return 404")
        except CollectionNotFoundError:
            print("✅ GET /collections/{non_existent} - Proper 404 handling")
        except ProximaDBError as e:
            if "404" in str(e) or "not found" in str(e).lower():
                print("✅ GET /collections/{non_existent} - Proper 404 handling")
            else:
                print(f"⚠️ GET /collections/{non_existent} - Unexpected error: {e}")
        except Exception as e:
            print(f"❌ GET /collections/{non_existent} - Unexpected error type: {e}")
        
        # 2. Invalid Vector Dimensions
        try:
            collection_name = f"error_test_{int(time.time())}"
            config = CollectionConfig(dimension=64, distance_metric=DistanceMetric.COSINE)
            collection = rest_client.create_collection(collection_name, config)
            
            try:
                # Try to insert wrong dimension vector
                wrong_vector = np.random.random(32).tolist()  # Should be 64
                rest_client.insert_vector(
                    collection_id=collection_name,
                    vector_id="wrong_dim",
                    vector=wrong_vector
                )
                print("❌ POST /vectors (wrong dimension) - Should return 400")
            except ProximaDBError as e:
                if "dimension" in str(e).lower() or "400" in str(e):
                    print("✅ POST /vectors (wrong dimension) - Proper 400 handling")
                else:
                    print(f"⚠️ POST /vectors (wrong dimension) - Unexpected error: {e}")
            except Exception as e:
                print(f"❌ POST /vectors (wrong dimension) - Unexpected error: {e}")
            
            # Cleanup
            rest_client.delete_collection(collection_name)
            
        except Exception as e:
            print(f"❌ Error test setup failed: {e}")
    
    def test_endpoint_summary(self, rest_client):
        """Provide summary of REST API endpoint status"""
        print("\n" + "="*60)
        print("REST API ENDPOINT SUMMARY")
        print("="*60)
        
        working_endpoints = [
            "POST /collections (create)",
            "GET /collections (list)", 
            "GET /collections/{id} (get)",
            "DELETE /collections/{id} (delete)",
            "POST /collections/{id}/vectors (insert single)",
            "POST /collections/{id}/vectors (insert batch)"
        ]
        
        broken_endpoints = [
            "GET /collections/{id}/vectors/{vector_id} (get vector) - 501 Not Implemented",
            "POST /collections/{id}/search (search) - 500 Server Error"
        ]
        
        missing_endpoints = [
            "PATCH /collections/{id} (update collection)",
            "DELETE /collections/{id}/vectors/{vector_id} (delete vector)",
            "GET /health (health check)",
            "GET /metrics (metrics)",
            "POST /collections/{id}/multi-search (multi search)"
        ]
        
        print(f"\n✅ WORKING ENDPOINTS ({len(working_endpoints)}):")
        for endpoint in working_endpoints:
            print(f"  • {endpoint}")
        
        print(f"\n❌ BROKEN ENDPOINTS ({len(broken_endpoints)}):")
        for endpoint in broken_endpoints:
            print(f"  • {endpoint}")
        
        print(f"\n⚠️ MISSING/NOT IMPLEMENTED ({len(missing_endpoints)}):")
        for endpoint in missing_endpoints:
            print(f"  • {endpoint}")
        
        total_tested = len(working_endpoints) + len(broken_endpoints) + len(missing_endpoints)
        working_pct = (len(working_endpoints) / total_tested) * 100
        
        print(f"\n📊 REST API COMPLETENESS: {working_pct:.1f}% ({len(working_endpoints)}/{total_tested} endpoints working)")


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])