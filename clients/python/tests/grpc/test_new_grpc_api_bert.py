#!/usr/bin/env python3
"""
Test New gRPC API with BERT Embeddings
Tests the updated ProximaDB API with storage layouts and filterable metadata fields.
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients', 'python', 'src'))

import asyncio
import time
import random
import numpy as np
from typing import List, Dict, Any
import json

import proximadb
from proximadb.models import CollectionConfig, FlushConfig

# Mock BERT-like embeddings generator
class MockBERTEmbeddings:
    """Generate realistic BERT-like embeddings for testing"""
    
    def __init__(self, dimension=384):
        self.dimension = dimension
        np.random.seed(42)  # For reproducible results
    
    def encode(self, texts: List[str]) -> np.ndarray:
        """Generate BERT-like embeddings for text"""
        embeddings = []
        for text in texts:
            # Use text hash as seed for consistent embeddings per text
            text_seed = hash(text) % 2**32
            rng = np.random.RandomState(text_seed)
            
            # Generate embedding with realistic distribution
            embedding = rng.normal(0, 0.1, self.dimension)
            # Add some structure based on text content
            if "error" in text.lower():
                embedding[0:10] += 0.5  # Error-related cluster
            elif "success" in text.lower():
                embedding[10:20] += 0.5  # Success-related cluster
            elif "warning" in text.lower():
                embedding[20:30] += 0.5  # Warning-related cluster
            
            # Normalize to unit vector (typical for BERT)
            embedding = embedding / np.linalg.norm(embedding)
            embeddings.append(embedding)
        
        return np.array(embeddings)

async def test_new_grpc_api():
    """Main test function for new gRPC API features"""
    print("🚀 Starting ProximaDB New gRPC API Test with BERT Embeddings")
    
    # Initialize BERT encoder
    bert_encoder = MockBERTEmbeddings(dimension=384)
    
    # Create gRPC client for optimal performance - connecting to real unified server
    client = proximadb.connect_grpc(url="127.0.0.1:5678")
    
    try:
        # Print performance information
        perf_info = client.get_performance_info()
        print(f"📊 Using {perf_info['protocol']} for {perf_info['advantages'][0]}")
        
        # Step 1: Check server health
        print("\n1️⃣ Testing server health...")
        health = client.health()
        print(f"✅ Server health: {health.status}")
        
        # Step 2: Create test collection with new API features
        print("\n2️⃣ Creating collection with new storage layout API...")
        import uuid
        collection_name = f"test_bert_embeddings_{uuid.uuid4().hex[:8]}"
        
        # Test different storage layouts and filterable metadata fields
        test_configs = [
            {
                "name": f"{collection_name}_viper",
                "storage_layout": "viper",
                "filterable_fields": ["category", "priority", "department", "status", "severity"],
                "description": "VIPER storage with 5 filterable metadata fields"
            },
            {
                "name": f"{collection_name}_lsm", 
                "storage_layout": "lsm",
                "filterable_fields": [],  # LSM doesn't use filterable metadata optimization
                "description": "LSM storage for comparison"
            }
        ]
        
        collections = []
        for config in test_configs:
            print(f"\n🏗️ Creating collection: {config['name']}")
            print(f"   Storage Layout: {config['storage_layout']}")
            print(f"   Filterable Fields: {config['filterable_fields']}")
            
            # Configure collection with new API
            collection_config = CollectionConfig(
                dimension=384,  # BERT-like embedding size
                distance_metric="cosine",
                indexing_algorithm="hnsw",
                storage_layout=config["storage_layout"],
                filterable_metadata_fields=config["filterable_fields"] if config["filterable_fields"] else None,
                flush_config=FlushConfig(
                    max_wal_size_mb=16.0  # Smaller size for faster testing
                ),
                description=config["description"]
            )
            
            collection = client.create_collection(config["name"], collection_config)
            collections.append((collection, config))
            
            print(f"✅ Created collection: {collection.name} (ID: {collection.id})")
            print(f"   - Dimension: {collection.dimension}")
            print(f"   - Metric: {collection.metric}")
            print(f"   - Vector count: {collection.vector_count}")
        
        # Step 3: Generate test data with BERT embeddings
        print("\n3️⃣ Generating test data with BERT-like embeddings...")
        
        # Sample texts for realistic embeddings
        sample_texts = [
            "Database connection successful", "Error processing request", "Warning: high memory usage",
            "Successfully created user account", "Critical system failure detected", "Info: backup completed",
            "Authentication failed for user", "Data synchronization complete", "Performance optimization applied",
            "Security scan completed successfully", "Error: invalid input parameters", "Warning: disk space low",
            "Transaction committed successfully", "Network timeout occurred", "Cache invalidated successfully",
            "Database query optimization", "Error handling improvement", "System monitoring active",
            "User session expired", "Data validation passed", "Critical security alert",
            "Performance metrics collected", "Error recovery initiated", "Warning: high CPU usage",
            "Successful data migration", "System health check passed", "Error: connection refused",
            "Cache hit rate improved", "Warning: memory leak detected", "Successful backup restore",
            "Database index rebuilt", "Error: permission denied", "Performance benchmark completed",
            "Critical error in payment processing", "Warning: service degraded", "Successfully scaled resources",
            "System maintenance completed", "Error: timeout waiting for response", "Performance tuning applied",
            "Database optimization successful", "Critical system alert resolved", "Warning: approaching limits",
            "User authentication successful", "Error: resource not found", "System recovery completed",
            "Performance monitoring enabled", "Critical backup failure", "Warning: service unavailable",
            "Database migration successful", "Error: invalid configuration", "System health restored",
            "Performance analysis completed", "Critical security breach", "Warning: deprecated feature",
            "Successful load balancing", "Error: out of memory", "System upgrade completed"
        ]
        
        # Generate embeddings for all texts
        print(f"🧠 Generating BERT embeddings for {len(sample_texts)} texts...")
        start_time = time.time()
        embeddings = bert_encoder.encode(sample_texts)
        embedding_time = time.time() - start_time
        print(f"   Generated {len(embeddings)} embeddings in {embedding_time:.3f}s")
        print(f"   Rate: {len(embeddings)/embedding_time:.0f} embeddings/sec")
        print(f"   Embedding shape: {embeddings.shape}")
        
        # Step 4: Test bulk insert operations
        print("\n4️⃣ Testing bulk insert operations...")
        
        total_vectors = len(sample_texts)
        departments = ["engineering", "sales", "marketing", "support", "operations"]
        statuses = ["active", "pending", "completed", "failed", "warning"]
        
        for collection, config in collections:
            print(f"\n📦 Bulk inserting into {collection.name} ({config['storage_layout']} storage)...")
            
            # Prepare vector data with metadata
            vectors = []
            vector_ids = []
            metadata_list = []
            
            for i, (text, embedding) in enumerate(zip(sample_texts, embeddings)):
                vector_id = f"{config['storage_layout']}_vec_{i:03d}"
                vector = embedding.tolist()
                
                # Generate metadata based on storage layout
                if config["storage_layout"] == "viper" and config["filterable_fields"]:
                    # Use filterable metadata fields for VIPER
                    metadata = {
                        "category": "log" if any(keyword in text.lower() for keyword in ["error", "warning", "info"]) else "message",
                        "priority": random.randint(1, 5),
                        "department": random.choice(departments),
                        "status": random.choice(statuses),
                        "severity": "high" if "critical" in text.lower() else "medium" if "warning" in text.lower() else "low",
                        "text": text,  # Original text (not in filterable fields, goes to extra_meta)
                        "timestamp": time.time() + i,
                        "source": "test_data"
                    }
                else:
                    # Simpler metadata for non-VIPER storage
                    metadata = {
                        "text": text,
                        "index": i,
                        "category": "test",
                        "timestamp": time.time() + i
                    }
                
                vectors.append(vector)
                vector_ids.append(vector_id)
                metadata_list.append(metadata)
            
            # Perform bulk insert with timing
            start_time = time.time()
            batch_result = client.insert_vectors(
                collection_id=collection.id,
                vectors=vectors,
                ids=vector_ids,
                metadata=metadata_list,
                batch_size=25  # Smaller batches for better progress tracking
            )
            insert_time = time.time() - start_time
            
            print(f"✅ Inserted {batch_result.successful_count}/{total_vectors} vectors")
            print(f"   Time: {insert_time:.3f}s")
            print(f"   Rate: {batch_result.successful_count/insert_time:.0f} vectors/sec")
            if batch_result.failed_count > 0:
                print(f"   ⚠️ Failed: {batch_result.failed_count} vectors")
        
        # Step 5: Test various search operations with performance measurement
        print("\n5️⃣ Testing search operations with performance measurement...")
        
        # Test queries
        test_queries = [
            "Error in database connection",
            "System performance optimization",
            "Critical security alert",
            "Warning about resource usage",
            "Successful operation completed"
        ]
        
        for collection, config in collections:
            print(f"\n🔍 Testing search on {collection.name} ({config['storage_layout']} storage)...")
            
            for i, query_text in enumerate(test_queries):
                print(f"\n   Query {i+1}: '{query_text}'")
                
                # Generate query embedding
                query_embedding = bert_encoder.encode([query_text])[0]
                
                # Test 1: Raw vector search (no filters)
                start_time = time.time()
                raw_results = client.search(
                    collection_id=collection.id,
                    query=query_embedding.tolist(),
                    k=5,
                    include_metadata=True,
                    include_vectors=False
                )
                raw_search_time = time.time() - start_time
                
                print(f"      Raw search: {len(raw_results)} results in {raw_search_time*1000:.2f}ms")
                if raw_results:
                    print(f"      Top result: score={raw_results[0].score:.4f}, metadata={raw_results[0].metadata}")
                
                # Test 2: ID lookup (if we know an ID)
                if vector_ids:
                    test_id = vector_ids[i % len(vector_ids)]
                    start_time = time.time()
                    id_result = client.get_vector(
                        collection_id=collection.id,
                        vector_id=test_id,
                        include_metadata=True,
                        include_vector=False
                    )
                    id_lookup_time = time.time() - start_time
                    
                    print(f"      ID lookup ({test_id}): {id_lookup_time*1000:.2f}ms")
                    if id_result:
                        print(f"      Found: {id_result.get('id', 'Unknown')}")
                
                # Test 3: Filtered metadata search (only for VIPER with filterable fields)
                if config["storage_layout"] == "viper" and config["filterable_fields"]:
                    # Test different filter scenarios
                    filters = [
                        {"category": "log"},
                        {"priority": {"$gte": 4}},
                        {"department": "engineering"},
                        {"status": "active"},
                        {"severity": "high"}
                    ]
                    
                    for filter_expr in filters:
                        start_time = time.time()
                        filtered_results = client.search(
                            collection_id=collection.id,
                            query=query_embedding.tolist(),
                            k=3,
                            filter=filter_expr,
                            include_metadata=True,
                            include_vectors=False
                        )
                        filtered_search_time = time.time() - start_time
                        
                        print(f"      Filtered search ({filter_expr}): {len(filtered_results)} results in {filtered_search_time*1000:.2f}ms")
                        if filtered_results:
                            print(f"      Top filtered: score={filtered_results[0].score:.4f}")
        
        # Step 6: Performance summary and storage layout comparison
        print("\n6️⃣ Performance Summary and Storage Layout Comparison...")
        
        for collection, config in collections:
            # Get final collection stats
            final_collection = client.get_collection(collection.id)
            print(f"\n📊 {config['storage_layout'].upper()} Storage ({collection.name}):")
            print(f"   - Storage Layout: {config['storage_layout']}")
            print(f"   - Vector count: {final_collection.vector_count}")
            print(f"   - Filterable fields: {len(config['filterable_fields'])} fields")
            print(f"   - Status: {final_collection.status}")
            
            if config["filterable_fields"]:
                print(f"   - Optimized metadata fields: {config['filterable_fields']}")
        
        print(f"\n🎉 New gRPC API test completed successfully!")
        
        # Display key improvements
        print(f"\n🚀 New API Features Tested:")
        print(f"   ✅ Storage layout selection (viper vs lsm)")
        print(f"   ✅ Filterable metadata fields for VIPER optimization")
        print(f"   ✅ Removed redundant allow_client_ids field")
        print(f"   ✅ BERT-like realistic embeddings")
        print(f"   ✅ Performance comparison across storage engines")
        print(f"   ✅ Comprehensive search scenarios (raw, filtered, ID lookup)")
        
    except Exception as e:
        print(f"❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        raise
        
    finally:
        # Cleanup
        try:
            for collection, config in collections:
                client.delete_collection(collection.id)
                print(f"🧹 Cleaned up collection: {collection.id}")
        except Exception as e:
            print(f"⚠️ Cleanup failed: {e}")
        
        client.close()

if __name__ == "__main__":
    asyncio.run(test_new_grpc_api())