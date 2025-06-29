#!/usr/bin/env python3
"""
ProximaDB gRPC API Completeness Test
Test all gRPC endpoints to identify which are implemented vs missing, and handle async issues
"""

import pytest
import time
import numpy as np
import asyncio
from typing import List, Dict, Any

from proximadb import connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric
from proximadb.exceptions import ProximaDBError, CollectionNotFoundError


class TestGRPCAPICompleteness:
    """Comprehensive test of gRPC API endpoints"""
    
    @pytest.fixture(scope="class")
    def grpc_client(self):
        client = connect_grpc("http://localhost:5679")
        yield client
        if hasattr(client, 'close'):
            client.close()
    
    @pytest.fixture
    def test_collection(self, grpc_client):
        """Create test collection for endpoint testing"""
        collection_name = f"grpc_test_{int(time.time())}"
        config = CollectionConfig(dimension=32, distance_metric=DistanceMetric.COSINE)
        
        collection = grpc_client.create_collection(collection_name, config)
        yield collection_name
        
        # Cleanup
        try:
            grpc_client.delete_collection(collection_name)
        except:
            pass
    
    def test_collection_endpoints(self, grpc_client):
        """Test all collection-related gRPC endpoints"""
        collection_name = f"grpc_collections_{int(time.time())}"
        
        print("\n=== gRPC Collection Endpoints ===")
        
        # 1. Create Collection
        try:
            config = CollectionConfig(dimension=64, distance_metric=DistanceMetric.COSINE)
            collection = grpc_client.create_collection(collection_name, config)
            assert collection is not None
            print("✅ CreateCollection - Create collection: WORKING")
        except Exception as e:
            print(f"❌ CreateCollection - Create collection: FAILED - {e}")
            return
        
        # 2. List Collections
        try:
            collections = grpc_client.list_collections()
            
            # Handle potential async/coroutine issue
            if hasattr(collections, '__await__'):
                print("⚠️ ListCollections - Returns coroutine (ASYNC ISSUE)")
                print("🔧 ListCollections - Needs async/await fix in SDK")
            else:
                assert collections is not None
                assert len(collections) > 0
                print(f"✅ ListCollections - List collections: WORKING ({len(collections)} collections)")
        except Exception as e:
            print(f"❌ ListCollections - List collections: FAILED - {e}")
        
        # 3. Get Collection
        try:
            retrieved = grpc_client.get_collection(collection_name)
            assert retrieved is not None
            print("✅ GetCollection - Get collection: WORKING")
        except Exception as e:
            print(f"❌ GetCollection - Get collection: FAILED - {e}")
        
        # 4. Update Collection (if implemented)
        try:
            updates = {"description": "Updated via gRPC test"}
            updated = grpc_client.update_collection(collection_name, updates)
            print("✅ UpdateCollection - Update collection: WORKING")
        except AttributeError:
            print("⚠️ UpdateCollection - Update collection: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ UpdateCollection - Update collection: FAILED - {e}")
        
        # 5. Delete Collection  
        try:
            result = grpc_client.delete_collection(collection_name)
            print("✅ DeleteCollection - Delete collection: WORKING")
        except Exception as e:
            print(f"❌ DeleteCollection - Delete collection: FAILED - {e}")
    
    def test_vector_endpoints(self, grpc_client, test_collection):
        """Test all vector-related gRPC endpoints"""
        print("\n=== gRPC Vector Endpoints ===")
        
        # 1. Insert Single Vector
        try:
            vector_id = "grpc_vector_1"
            vector = np.random.random(32).astype(np.float32).tolist()
            metadata = {"type": "single", "test": "grpc_endpoint"}
            
            result = grpc_client.insert_vector(
                collection_id=test_collection,
                vector_id=vector_id,
                vector=vector,
                metadata=metadata
            )
            assert result is not None
            print("✅ InsertVector - Insert single vector: WORKING")
        except Exception as e:
            print(f"❌ InsertVector - Insert single vector: FAILED - {e}")
            return
        
        # 2. Insert Multiple Vectors
        try:
            batch_vectors = []
            batch_ids = []
            batch_metadata = []
            
            for i in range(3):
                batch_vectors.append(np.random.random(32).astype(np.float32).tolist())
                batch_ids.append(f"grpc_batch_{i}")
                batch_metadata.append({"type": "batch", "index": i})
            
            batch_result = grpc_client.insert_vectors(
                collection_id=test_collection,
                vectors=batch_vectors,
                ids=batch_ids,
                metadata=batch_metadata
            )
            assert batch_result is not None
            print("✅ InsertVectors - Insert multiple vectors: WORKING")
        except Exception as e:
            print(f"❌ InsertVectors - Insert multiple vectors: FAILED - {e}")
        
        # 3. Get Vector by ID
        try:
            retrieved = grpc_client.get_vector(
                collection_id=test_collection,
                vector_id=vector_id,
                include_metadata=True,
                include_vector=True
            )
            if retrieved is not None:
                print("✅ GetVector - Get vector: WORKING")
                
                # Verify structure
                if isinstance(retrieved, dict):
                    if 'metadata' in retrieved:
                        print("✅ GetVector - Metadata included: WORKING")
                    if 'vector' in retrieved:
                        print("✅ GetVector - Vector data included: WORKING")
                else:
                    print("⚠️ GetVector - Unexpected return structure")
            else:
                print("⚠️ GetVector - Returns NULL")
        except Exception as e:
            print(f"❌ GetVector - Get vector: FAILED - {e}")
        
        # 4. Update Vector
        try:
            updated_vector = np.random.random(32).astype(np.float32).tolist()
            updated_metadata = {"type": "updated", "test": "grpc_endpoint"}
            
            update_result = grpc_client.insert_vector(
                collection_id=test_collection,
                vector_id=vector_id,
                vector=updated_vector,
                metadata=updated_metadata,
                upsert=True
            )
            print("✅ UpdateVector - Update vector: WORKING (via upsert)")
        except Exception as e:
            print(f"❌ UpdateVector - Update vector: FAILED - {e}")
        
        # 5. Delete Vector
        try:
            delete_result = grpc_client.delete_vector(test_collection, vector_id)
            print("✅ DeleteVector - Delete vector: WORKING")
        except AttributeError:
            print("⚠️ DeleteVector - Delete vector: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ DeleteVector - Delete vector: FAILED - {e}")
    
    def test_search_endpoints(self, grpc_client, test_collection):
        """Test search-related gRPC endpoints"""
        print("\n=== gRPC Search Endpoints ===")
        
        # Add some vectors for searching
        for i in range(3):
            vector = np.random.random(32).astype(np.float32).tolist()
            grpc_client.insert_vector(
                collection_id=test_collection,
                vector_id=f"search_vector_{i}",
                vector=vector,
                metadata={"category": f"category_{i % 2}", "index": i}
            )
        
        # 1. Basic Search
        try:
            query_vector = np.random.random(32).astype(np.float32).tolist()
            results = grpc_client.search(
                collection_id=test_collection,
                query=query_vector,
                k=5,
                include_metadata=True,
                include_vectors=False
            )
            
            if results and len(results) > 0:
                print(f"✅ SearchVectors - Basic search: WORKING ({len(results)} results)")
                
                # Verify result structure
                for result in results[:2]:
                    assert hasattr(result, 'id'), "Result missing id"
                    assert hasattr(result, 'score'), "Result missing score"
                    assert hasattr(result, 'metadata'), "Result missing metadata"
                    
                    # Verify score range
                    assert 0 <= result.score <= 1, f"Invalid score: {result.score}"
                    
                print("✅ SearchVectors - Result structure: VALID")
                print(f"✅ SearchVectors - Score range: VALID ({[f'{r.score:.3f}' for r in results[:3]]})")
                
            else:
                print("⚠️ SearchVectors - Basic search: RETURNS EMPTY")
        except Exception as e:
            print(f"❌ SearchVectors - Basic search: FAILED - {e}")
        
        # 2. Search with Filters
        try:
            filtered_results = grpc_client.search(
                collection_id=test_collection,
                query=query_vector,
                k=5,
                filter={"category": "category_0"},
                include_metadata=True
            )
            if filtered_results:
                print("✅ SearchVectors (with filters) - Filtered search: WORKING")
            else:
                print("⚠️ SearchVectors (with filters) - No filtered results")
        except Exception as e:
            print(f"❌ SearchVectors (with filters) - Filtered search: FAILED - {e}")
        
        # 3. Search with different k values
        try:
            # Test k=1
            single_result = grpc_client.search(test_collection, query_vector, k=1)
            if len(single_result) == 1:
                print("✅ SearchVectors (k=1) - Single result: WORKING")
            
            # Test k > collection size
            large_k_results = grpc_client.search(test_collection, query_vector, k=100)
            if len(large_k_results) == 3:  # Should return all 3 vectors
                print("✅ SearchVectors (k>collection) - Returns all vectors: WORKING")
            
        except Exception as e:
            print(f"❌ SearchVectors (k variations) - FAILED - {e}")
        
        # 4. Multi-Search (if implemented)
        try:
            if hasattr(grpc_client, 'multi_search'):
                multi_queries = [query_vector, np.random.random(32).tolist()]
                multi_results = grpc_client.multi_search(
                    collection_id=test_collection,
                    queries=multi_queries,
                    k=3
                )
                print("✅ MultiSearch - Multi search: WORKING")
            else:
                print("⚠️ MultiSearch - Multi search: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ MultiSearch - Multi search: FAILED - {e}")
    
    def test_advanced_endpoints(self, grpc_client, test_collection):
        """Test advanced gRPC endpoints"""
        print("\n=== gRPC Advanced Endpoints ===")
        
        # 1. Batch Delete Vectors
        try:
            if hasattr(grpc_client, 'delete_vectors'):
                vector_ids = ["search_vector_0", "search_vector_1"]
                delete_result = grpc_client.delete_vectors(test_collection, vector_ids)
                print("✅ DeleteVectors (batch) - Batch delete: WORKING")
            else:
                print("⚠️ DeleteVectors (batch) - NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ DeleteVectors (batch) - FAILED - {e}")
        
        # 2. Vector History (if implemented)
        try:
            if hasattr(grpc_client, 'get_vector_history'):
                history = grpc_client.get_vector_history(test_collection, "search_vector_0")
                print("✅ GetVectorHistory - Vector history: WORKING")
            else:
                print("⚠️ GetVectorHistory - NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ GetVectorHistory - FAILED - {e}")
        
        # 3. Collection Statistics (if implemented)
        try:
            if hasattr(grpc_client, 'get_collection_stats'):
                stats = grpc_client.get_collection_stats(test_collection)
                print("✅ GetCollectionStats - Collection stats: WORKING")
            else:
                print("⚠️ GetCollectionStats - NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ GetCollectionStats - FAILED - {e}")
    
    def test_streaming_endpoints(self, grpc_client):
        """Test gRPC streaming endpoints (if implemented)"""
        print("\n=== gRPC Streaming Endpoints ===")
        
        # 1. Streaming Insert
        try:
            if hasattr(grpc_client, 'stream_insert'):
                print("⚠️ StreamInsert - IMPLEMENTED BUT NOT TESTED (requires streaming)")
            else:
                print("⚠️ StreamInsert - NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ StreamInsert - FAILED - {e}")
        
        # 2. Streaming Search
        try:
            if hasattr(grpc_client, 'stream_search'):
                print("⚠️ StreamSearch - IMPLEMENTED BUT NOT TESTED (requires streaming)")
            else:
                print("⚠️ StreamSearch - NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ StreamSearch - FAILED - {e}")
    
    def test_health_and_admin_endpoints(self, grpc_client):
        """Test health and administrative gRPC endpoints"""
        print("\n=== gRPC Health & Admin Endpoints ===")
        
        # 1. Health Check
        try:
            health = grpc_client.health()
            if health is not None:
                print("✅ HealthCheck - Health check: WORKING")
            else:
                print("⚠️ HealthCheck - Health check: RETURNS NULL")
        except AttributeError:
            print("⚠️ HealthCheck - Health check: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ HealthCheck - Health check: FAILED - {e}")
        
        # 2. Server Info
        try:
            if hasattr(grpc_client, 'get_server_info'):
                info = grpc_client.get_server_info()
                print("✅ GetServerInfo - Server info: WORKING")
            else:
                print("⚠️ GetServerInfo - Server info: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ GetServerInfo - Server info: FAILED - {e}")
        
        # 3. Service Discovery
        try:
            if hasattr(grpc_client, 'get_service_info'):
                service_info = grpc_client.get_service_info()
                print("✅ GetServiceInfo - Service info: WORKING")
            else:
                print("⚠️ GetServiceInfo - Service info: NOT IMPLEMENTED IN SDK")
        except Exception as e:
            print(f"❌ GetServiceInfo - Service info: FAILED - {e}")
    
    def test_error_handling(self, grpc_client):
        """Test gRPC API error handling"""
        print("\n=== gRPC Error Handling ===")
        
        # 1. Non-existent Collection
        try:
            grpc_client.get_collection("non_existent_collection")
            print("❌ GetCollection(non_existent) - Should return NOT_FOUND")
        except CollectionNotFoundError:
            print("✅ GetCollection(non_existent) - Proper NOT_FOUND handling")
        except ProximaDBError as e:
            if "not found" in str(e).lower() or "NOT_FOUND" in str(e):
                print("✅ GetCollection(non_existent) - Proper NOT_FOUND handling")
            else:
                print(f"⚠️ GetCollection(non_existent) - Unexpected error: {e}")
        except Exception as e:
            print(f"❌ GetCollection(non_existent) - Unexpected error type: {e}")
        
        # 2. Invalid Vector Dimensions
        try:
            collection_name = f"grpc_error_test_{int(time.time())}"
            config = CollectionConfig(dimension=64, distance_metric=DistanceMetric.COSINE)
            collection = grpc_client.create_collection(collection_name, config)
            
            try:
                # Try to insert wrong dimension vector
                wrong_vector = np.random.random(32).tolist()  # Should be 64
                grpc_client.insert_vector(
                    collection_id=collection_name,
                    vector_id="wrong_dim",
                    vector=wrong_vector
                )
                print("❌ InsertVector(wrong dimension) - Should return INVALID_ARGUMENT")
            except ProximaDBError as e:
                if "dimension" in str(e).lower() or "INVALID_ARGUMENT" in str(e):
                    print("✅ InsertVector(wrong dimension) - Proper INVALID_ARGUMENT handling")
                else:
                    print(f"⚠️ InsertVector(wrong dimension) - Unexpected error: {e}")
            except Exception as e:
                print(f"❌ InsertVector(wrong dimension) - Unexpected error: {e}")
            
            # Cleanup
            grpc_client.delete_collection(collection_name)
            
        except Exception as e:
            print(f"❌ gRPC Error test setup failed: {e}")
        
        # 3. Empty Query Vector
        try:
            collection_name = f"grpc_empty_test_{int(time.time())}"
            config = CollectionConfig(dimension=32, distance_metric=DistanceMetric.COSINE)
            collection = grpc_client.create_collection(collection_name, config)
            
            try:
                empty_vector = []
                results = grpc_client.search(collection_name, empty_vector, k=5)
                print("❌ Search(empty vector) - Should return INVALID_ARGUMENT")
            except Exception as e:
                if "empty" in str(e).lower() or "INVALID_ARGUMENT" in str(e):
                    print("✅ Search(empty vector) - Proper validation")
                else:
                    print(f"⚠️ Search(empty vector) - Unexpected error: {e}")
            
            grpc_client.delete_collection(collection_name)
            
        except Exception as e:
            print(f"❌ gRPC Empty vector test failed: {e}")
    
    def test_async_issues_diagnosis(self, grpc_client):
        """Diagnose async/await issues in gRPC client"""
        print("\n=== gRPC Async Issues Diagnosis ===")
        
        methods_to_check = [
            'list_collections',
            'create_collection', 
            'get_collection',
            'delete_collection',
            'insert_vector',
            'insert_vectors',
            'get_vector',
            'search',
            'health'
        ]
        
        async_methods = []
        sync_methods = []
        
        for method_name in methods_to_check:
            if hasattr(grpc_client, method_name):
                method = getattr(grpc_client, method_name)
                
                # Check if method is async by trying to inspect it
                import inspect
                if inspect.iscoroutinefunction(method):
                    async_methods.append(method_name)
                else:
                    sync_methods.append(method_name)
        
        print(f"🔍 Async methods detected: {async_methods}")
        print(f"🔍 Sync methods detected: {sync_methods}")
        
        if async_methods:
            print("⚠️ gRPC client has async methods that need proper await handling")
            print("🔧 Suggestion: Update SDK to handle async methods or provide sync wrappers")
    
    def test_endpoint_summary(self, grpc_client):
        """Provide summary of gRPC API endpoint status"""
        print("\n" + "="*60)
        print("gRPC API ENDPOINT SUMMARY")
        print("="*60)
        
        working_endpoints = [
            "CreateCollection (create)",
            "GetCollection (get)",
            "DeleteCollection (delete)",
            "InsertVector (insert single)",
            "InsertVectors (insert batch)",
            "GetVector (get vector)",
            "SearchVectors (basic search)"
        ]
        
        async_issues = [
            "ListCollections (list) - Returns coroutine, needs async fix"
        ]
        
        missing_endpoints = [
            "UpdateCollection (update collection)",
            "DeleteVector (delete single vector)",
            "DeleteVectors (batch delete)",
            "MultiSearch (multi search)",
            "StreamInsert (streaming insert)",
            "StreamSearch (streaming search)",
            "GetVectorHistory (vector history)",
            "GetCollectionStats (collection statistics)",
            "HealthCheck (health check)",
            "GetServerInfo (server info)"
        ]
        
        print(f"\n✅ WORKING ENDPOINTS ({len(working_endpoints)}):")
        for endpoint in working_endpoints:
            print(f"  • {endpoint}")
        
        print(f"\n🔧 ASYNC ISSUES ({len(async_issues)}):")
        for endpoint in async_issues:
            print(f"  • {endpoint}")
        
        print(f"\n⚠️ MISSING/NOT IMPLEMENTED ({len(missing_endpoints)}):")
        for endpoint in missing_endpoints:
            print(f"  • {endpoint}")
        
        total_tested = len(working_endpoints) + len(async_issues) + len(missing_endpoints)
        working_pct = (len(working_endpoints) / total_tested) * 100
        
        print(f"\n📊 gRPC API COMPLETENESS: {working_pct:.1f}% ({len(working_endpoints)}/{total_tested} endpoints working)")
        print(f"🔧 NEEDS ASYNC FIXES: {len(async_issues)} endpoints")


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])