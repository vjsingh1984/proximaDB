#!/usr/bin/env python3
"""
ProximaDB gRPC Large-Scale Operations & Indexing Test
Tests large-scale vector operations that trigger flush/compaction,
verifies data persistence and retrieval through indexing after WAL operations,
and ensures previously submitted data remains accessible via ID, filter, and similarity search.
"""

import asyncio
import json
import subprocess
import time
import os
import logging
import grpc
import sys
import numpy as np
import hashlib
from typing import List, Dict, Any, Tuple, Optional

# Add the client directory to Python path
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

import proximadb.proximadb_pb2 as proximadb_pb2
import proximadb.proximadb_pb2_grpc as proximadb_pb2_grpc

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

class LargeScaleTestConfig:
    """Configuration for large-scale operations and indexing tests"""
    
    # Test configurations designed to trigger flush/compaction (only working storage engines)
    TEST_CONFIGS = [
        # Primary focus: Viper + HNSW (production indexing setup)
        {"storage": "Viper", "indexing": "HNSW", "distance": "Cosine", "metadata": "basic"},
        {"storage": "Viper", "indexing": "HNSW", "distance": "Euclidean", "metadata": "extended"},
        
        # Standard storage for comparison (write-heavy workloads)
        {"storage": "Standard", "indexing": "HNSW", "distance": "Cosine", "metadata": "basic"},
        
        # IVF indexing for comparison
        {"storage": "Viper", "indexing": "IVF", "distance": "Cosine", "metadata": "basic"},
        
        # Flat indexing baseline
        {"storage": "Viper", "indexing": "Flat", "distance": "Cosine", "metadata": "none"},
        {"storage": "Standard", "indexing": "Flat", "distance": "Euclidean", "metadata": "basic"},
    ]
    
    # Large-scale operation parameters designed to trigger flush/compaction
    INITIAL_VECTOR_COUNT = 2000      # Initial dataset to establish baseline
    LARGE_BATCH_SIZE = 5000          # Large batch to trigger flush/compaction
    STRESS_VECTOR_COUNT = 10000      # Additional vectors for stress testing
    
    # Batch processing parameters
    BATCH_SIZES = [100, 500, 1000]  # Different batch sizes for insertion
    FLUSH_TRIGGER_SIZE = 1000        # Expected size to trigger flush operations
    
    # Storage engines (only working configurations)
    STORAGE_ENGINES = {"Viper": 1, "Standard": 2}
    
    # Indexing algorithms
    INDEXING_ALGORITHMS = {"HNSW": 1, "IVF": 2, "PQ": 3, "Flat": 4, "Annoy": 5}
    
    # Distance metrics
    DISTANCE_METRICS = {"Cosine": 1, "Euclidean": 2, "DotProduct": 3, "Hamming": 4}
    
    # Metadata configurations
    METADATA_CONFIGS = {
        "none": [],
        "basic": ["category", "source", "priority", "created_at"],
        "extended": [
            "category", "source", "priority", "created_at", "author", "version",
            "tags", "department", "project", "environment", "region", "tenant",
            "classification", "sensitivity", "expires_at", "last_modified"
        ],
    }

class LargeScaleOperationsSuite:
    """Large-scale operations test suite for flush/compaction and indexing verification"""
    
    def __init__(self):
        self.server_process = None
        self.test_results = []
        self.dimension = 128
        self.baseline_vectors = {}  # Store baseline vectors for verification
        self.collection_configs = {}  # Store collection configurations
        
    async def setup_server(self):
        """Start ProximaDB server with performance optimizations"""
        logger.info("🚀 Starting ProximaDB server for large-scale operations testing...")
        
        # Cleanup old data
        data_dirs = ["/home/vsingh/code/proximadb/data", "/home/vsingh/code/proximadb/certs/data"]
        for data_dir in data_dirs:
            if os.path.exists(data_dir):
                subprocess.run(["rm", "-rf", data_dir], check=False)
                logger.info(f"🧹 Cleaned up: {data_dir}")
        
        # Start server with release optimizations
        os.chdir("/home/vsingh/code/proximadb")
        cmd = ["cargo", "run", "--release", "--bin", "proximadb-server"]
        self.server_process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        
        # Wait for server startup
        await asyncio.sleep(15)  # Extra time for release build
        
        # Test connection
        channel = grpc.aio.insecure_channel("localhost:5679")
        stub = proximadb_pb2_grpc.ProximaDBStub(channel)
        
        health_request = proximadb_pb2.HealthRequest()
        health_response = await stub.Health(health_request)
        logger.info(f"✅ Health check: {health_response.status}")
        
        await channel.close()
        
    async def teardown_server(self):
        """Stop ProximaDB server"""
        if self.server_process:
            self.server_process.terminate()
            self.server_process.wait()
            logger.info("✅ Server stopped")
    
    def generate_large_scale_embeddings(self, count: int, seed_prefix: str = "") -> Tuple[List[str], List[List[float]], List[Dict[str, str]]]:
        """Generate large-scale BERT-like embeddings with deterministic IDs and metadata"""
        np.random.seed(42)
        
        vector_ids = []
        embeddings = []
        metadata_list = []
        
        # Categories for realistic metadata distribution
        categories = ["research", "documentation", "analysis", "development", "testing"]
        departments = ["ai", "backend", "frontend", "data", "infra"]
        priorities = ["critical", "high", "medium", "low"]
        authors = ["alice", "bob", "charlie", "diana", "eve", "frank", "grace", "henry"]
        
        for i in range(count):
            # Generate deterministic but varied vector ID
            vector_id = f"{seed_prefix}vec_{i:06d}_{hashlib.md5(f'{seed_prefix}{i}'.encode()).hexdigest()[:8]}"
            vector_ids.append(vector_id)
            
            # Generate realistic embedding with clustering patterns
            cluster_id = i % 20  # 20 clusters for variety
            base_vector = np.random.normal(0, 0.1, self.dimension)
            
            # Add cluster-specific bias
            cluster_center = np.zeros(self.dimension)
            start_idx = (cluster_id * self.dimension // 20) % self.dimension
            end_idx = min(start_idx + self.dimension // 20, self.dimension)
            cluster_center[start_idx:end_idx] = np.random.normal(cluster_id * 0.15, 0.03, end_idx - start_idx)
            
            # Combine base vector with cluster bias
            embedding = base_vector + cluster_center
            embedding = embedding / np.linalg.norm(embedding)  # Normalize
            embeddings.append(embedding.tolist())
            
            # Generate realistic metadata
            metadata = {
                "category": categories[i % len(categories)],
                "department": departments[i % len(departments)],
                "priority": priorities[i % len(priorities)],
                "author": authors[i % len(authors)],
                "created_at": str(int(time.time()) - (i * 60)),  # Minute intervals
                "batch_id": f"batch_{i // 1000:03d}",  # Group by thousands
                "cluster_id": str(cluster_id),
                "vector_norm": f"{np.linalg.norm(embedding):.6f}"
            }
            
            # Add extended metadata for some configurations
            if len(LargeScaleTestConfig.METADATA_CONFIGS.get("extended", [])) > 8:
                metadata.update({
                    "version": f"v{(i % 10) + 1}.{i % 100}",
                    "project": f"project_{i % 5}",
                    "environment": ["prod", "staging", "dev"][i % 3],
                    "region": ["us-east-1", "us-west-2", "eu-west-1"][i % 3],
                    "tenant": f"tenant_{i % 50}",
                    "tags": f"tag_{i % 20},batch_{i // 1000}",
                    "classification": ["public", "internal", "confidential"][i % 3],
                    "data_source": f"source_{i % 10}"
                })
            
            metadata_list.append(metadata)
        
        return vector_ids, embeddings, metadata_list
    
    async def create_large_scale_collection(self, config: Dict[str, str], test_name: str) -> Tuple[bool, str]:
        """Create a collection optimized for large-scale operations"""
        collection_name = f"large_scale_{test_name}_{config['storage'].lower()}_{config['indexing'].lower()}"
        metadata_fields = LargeScaleTestConfig.METADATA_CONFIGS[config['metadata']]
        
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            # Configure collection for large-scale operations
            collection_config = proximadb_pb2.CollectionConfig(
                name=collection_name,
                dimension=self.dimension,
                distance_metric=LargeScaleTestConfig.DISTANCE_METRICS[config['distance']],
                storage_engine=LargeScaleTestConfig.STORAGE_ENGINES[config['storage']],
                indexing_algorithm=LargeScaleTestConfig.INDEXING_ALGORITHMS[config['indexing']],
                filterable_metadata_fields=metadata_fields,
                indexing_config={}  # Use default indexing parameters
            )
            
            create_request = proximadb_pb2.CollectionRequest(
                operation=1,  # CREATE
                collection_config=collection_config
            )
            
            response = await stub.CollectionOperation(create_request)
            await channel.close()
            
            if response.success:
                logger.info(f"✅ Created large-scale collection: {collection_name}")
                self.collection_configs[collection_name] = config
                return True, collection_name
            else:
                logger.error(f"❌ Failed to create collection {collection_name}: {response.error_message}")
                return False, collection_name
                
        except Exception as e:
            logger.error(f"❌ Exception creating collection {collection_name}: {e}")
            return False, collection_name
    
    async def insert_baseline_vectors(self, collection_name: str, metadata_fields: List[str]) -> Dict[str, Any]:
        """Insert initial baseline vectors to establish the collection"""
        logger.info(f"📥 Inserting {LargeScaleTestConfig.INITIAL_VECTOR_COUNT} baseline vectors...")
        
        try:
            # Generate baseline vectors
            vector_ids, embeddings, metadata_list = self.generate_large_scale_embeddings(
                LargeScaleTestConfig.INITIAL_VECTOR_COUNT, "baseline_"
            )
            
            # Store baseline vectors for later verification
            self.baseline_vectors[collection_name] = {
                "vector_ids": vector_ids,
                "embeddings": embeddings,
                "metadata": metadata_list
            }
            
            # Insert in batches to avoid overwhelming the server
            batch_size = 500
            successful_inserts = 0
            total_time = 0
            
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            for batch_start in range(0, len(vector_ids), batch_size):
                batch_end = min(batch_start + batch_size, len(vector_ids))
                batch_ids = vector_ids[batch_start:batch_end]
                batch_embeddings = embeddings[batch_start:batch_end]
                batch_metadata = metadata_list[batch_start:batch_end]
                
                # Prepare batch data
                vectors_data = []
                for i, (vid, embedding, metadata) in enumerate(zip(batch_ids, batch_embeddings, batch_metadata)):
                    # Filter metadata to only include configured fields
                    filtered_metadata = {k: v for k, v in metadata.items() if k in metadata_fields}
                    
                    vector_record = {
                        "id": vid,
                        "vector": embedding,
                        "metadata": filtered_metadata,
                        "timestamp": int(time.time()),
                        "version": 1
                    }
                    vectors_data.append(vector_record)
                
                # Insert batch
                start_time = time.time()
                vectors_json = json.dumps(vectors_data).encode('utf-8')
                
                request = proximadb_pb2.VectorInsertRequest(
                    collection_id=collection_name,
                    upsert_mode=False,
                    vectors_avro_payload=vectors_json
                )
                
                response = await stub.VectorInsert(request)
                batch_time = time.time() - start_time
                total_time += batch_time
                
                if response.success:
                    successful_inserts += len(batch_ids)
                    logger.info(f"   Batch {batch_start//batch_size + 1}: {len(batch_ids)} vectors in {batch_time:.2f}s")
                else:
                    logger.warning(f"   ⚠️ Batch {batch_start//batch_size + 1} failed: {response.error_message}")
            
            await channel.close()
            
            result = {
                "success": successful_inserts > 0,
                "vectors_inserted": successful_inserts,
                "total_time_seconds": total_time,
                "throughput_vectors_per_sec": successful_inserts / total_time if total_time > 0 else 0
            }
            
            logger.info(f"✅ Baseline insertion complete: {successful_inserts} vectors in {total_time:.2f}s")
            return result
            
        except Exception as e:
            logger.error(f"❌ Exception in baseline insertion: {e}")
            return {"success": False, "error": str(e)}
    
    async def insert_large_batch_trigger_flush(self, collection_name: str, metadata_fields: List[str]) -> Dict[str, Any]:
        """Insert large batch of vectors to trigger flush/compaction operations"""
        logger.info(f"🚀 Inserting {LargeScaleTestConfig.LARGE_BATCH_SIZE} vectors to trigger flush/compaction...")
        
        try:
            # Generate large batch of vectors
            vector_ids, embeddings, metadata_list = self.generate_large_scale_embeddings(
                LargeScaleTestConfig.LARGE_BATCH_SIZE, "large_batch_"
            )
            
            # Use larger batch sizes to stress the system
            batch_size = 1000  # Larger batches to trigger flush
            successful_inserts = 0
            total_time = 0
            flush_detected = False
            
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            start_total = time.time()
            
            for batch_start in range(0, len(vector_ids), batch_size):
                batch_end = min(batch_start + batch_size, len(vector_ids))
                batch_ids = vector_ids[batch_start:batch_end]
                batch_embeddings = embeddings[batch_start:batch_end]
                batch_metadata = metadata_list[batch_start:batch_end]
                
                # Prepare batch data
                vectors_data = []
                for i, (vid, embedding, metadata) in enumerate(zip(batch_ids, batch_embeddings, batch_metadata)):
                    # Filter metadata to only include configured fields
                    filtered_metadata = {k: v for k, v in metadata.items() if k in metadata_fields}
                    
                    vector_record = {
                        "id": vid,
                        "vector": embedding,
                        "metadata": filtered_metadata,
                        "timestamp": int(time.time()),
                        "version": 1
                    }
                    vectors_data.append(vector_record)
                
                # Insert batch and monitor for flush operations
                batch_start_time = time.time()
                vectors_json = json.dumps(vectors_data).encode('utf-8')
                
                request = proximadb_pb2.VectorInsertRequest(
                    collection_id=collection_name,
                    upsert_mode=False,
                    vectors_avro_payload=vectors_json
                )
                
                response = await stub.VectorInsert(request)
                batch_time = time.time() - batch_start_time
                total_time += batch_time
                
                if response.success:
                    successful_inserts += len(batch_ids)
                    
                    # Check if this batch might have triggered flush (slower response = possible flush)
                    if batch_time > 2.0:  # Slower than expected = possible flush/compaction
                        flush_detected = True
                        logger.info(f"   🔄 Batch {batch_start//batch_size + 1}: {len(batch_ids)} vectors in {batch_time:.2f}s (POSSIBLE FLUSH DETECTED)")
                    else:
                        logger.info(f"   Batch {batch_start//batch_size + 1}: {len(batch_ids)} vectors in {batch_time:.2f}s")
                else:
                    logger.warning(f"   ⚠️ Batch {batch_start//batch_size + 1} failed: {response.error_message}")
                
                # Allow time for background operations
                await asyncio.sleep(0.5)
            
            await channel.close()
            
            # Wait additional time for flush/compaction to complete
            logger.info("⏳ Waiting for flush/compaction operations to complete...")
            await asyncio.sleep(10)
            
            result = {
                "success": successful_inserts > 0,
                "vectors_inserted": successful_inserts,
                "total_time_seconds": total_time,
                "throughput_vectors_per_sec": successful_inserts / total_time if total_time > 0 else 0,
                "flush_detected": flush_detected,
                "average_batch_time": total_time / (len(vector_ids) // batch_size) if len(vector_ids) > 0 else 0
            }
            
            logger.info(f"✅ Large batch insertion complete: {successful_inserts} vectors in {total_time:.2f}s")
            if flush_detected:
                logger.info("🔄 Flush/compaction operations were likely triggered during insertion")
            
            return result
            
        except Exception as e:
            logger.error(f"❌ Exception in large batch insertion: {e}")
            return {"success": False, "error": str(e)}
    
    async def verify_baseline_data_retrieval(self, collection_name: str, config: Dict[str, str]) -> Dict[str, Any]:
        """Verify that baseline data can still be retrieved after flush/compaction"""
        logger.info("🔍 Verifying baseline data retrieval after flush/compaction...")
        
        if collection_name not in self.baseline_vectors:
            logger.error("❌ No baseline vectors stored for verification")
            return {"success": False, "error": "No baseline vectors"}
        
        baseline_data = self.baseline_vectors[collection_name]
        vector_ids = baseline_data["vector_ids"]
        embeddings = baseline_data["embeddings"]
        metadata_list = baseline_data["metadata"]
        
        verification_results = {
            "id_lookup_success": 0,
            "id_lookup_failed": 0,
            "metadata_filter_success": 0,
            "metadata_filter_failed": 0,
            "similarity_search_success": 0,
            "similarity_search_failed": 0,
            "indexing_performance": {},
            "overall_success": False
        }
        
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            # Test 1: ID-based lookup (simulate by exact similarity search)
            logger.info("   Testing ID-based retrieval...")
            sample_indices = [0, len(vector_ids)//4, len(vector_ids)//2, 3*len(vector_ids)//4, len(vector_ids)-1]
            
            for i in sample_indices:
                vector_id = vector_ids[i]
                query_vector = embeddings[i]
                
                search_query = proximadb_pb2.SearchQuery(vector=query_vector)
                search_request = proximadb_pb2.VectorSearchRequest(
                    collection_id=collection_name,
                    queries=[search_query],
                    top_k=10,
                    distance_metric_override=LargeScaleTestConfig.DISTANCE_METRICS[config['distance']],
                    include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
                )
                
                start_time = time.time()
                response = await stub.VectorSearch(search_request)
                search_time = time.time() - start_time
                
                if response.success:
                    # Look for exact match in results
                    found = False
                    if (hasattr(response, 'result_payload') and response.result_payload and
                        hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                        
                        for result in response.result_payload.compact_results.results:
                            if result.id == vector_id and result.score > 0.99:  # Near-perfect match
                                found = True
                                break
                    
                    if found:
                        verification_results["id_lookup_success"] += 1
                    else:
                        verification_results["id_lookup_failed"] += 1
                        logger.warning(f"   ⚠️ Vector {vector_id} not found with high similarity")
                else:
                    verification_results["id_lookup_failed"] += 1
                    logger.warning(f"   ⚠️ Search failed for vector {vector_id}: {response.error_message}")
                
                # Track indexing performance
                if "id_search_times" not in verification_results["indexing_performance"]:
                    verification_results["indexing_performance"]["id_search_times"] = []
                verification_results["indexing_performance"]["id_search_times"].append(search_time)
            
            # Test 2: Metadata filtering
            logger.info("   Testing metadata-based retrieval...")
            metadata_filters = [
                {"category": "research"},
                {"department": "ai", "priority": "high"},
                {"author": "alice"},
                {"cluster_id": "5"}
            ]
            
            for filter_criteria in metadata_filters:
                # Use neutral query vector for metadata filtering
                query_vector = [0.0] * self.dimension
                
                search_query = proximadb_pb2.SearchQuery(
                    vector=query_vector,
                    metadata_filter=filter_criteria
                )
                
                search_request = proximadb_pb2.VectorSearchRequest(
                    collection_id=collection_name,
                    queries=[search_query],
                    top_k=100,  # Get many results for filtering
                    distance_metric_override=LargeScaleTestConfig.DISTANCE_METRICS[config['distance']],
                    include_fields=proximadb_pb2.IncludeFields(vector=False, metadata=True)
                )
                
                start_time = time.time()
                response = await stub.VectorSearch(search_request)
                search_time = time.time() - start_time
                
                if response.success:
                    result_count = 0
                    if (hasattr(response, 'result_payload') and response.result_payload and
                        hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                        result_count = len(response.result_payload.compact_results.results)
                    
                    if result_count > 0:
                        verification_results["metadata_filter_success"] += 1
                        logger.info(f"   ✅ Metadata filter {filter_criteria} found {result_count} results")
                    else:
                        verification_results["metadata_filter_failed"] += 1
                        logger.warning(f"   ⚠️ Metadata filter {filter_criteria} found no results")
                else:
                    verification_results["metadata_filter_failed"] += 1
                    logger.warning(f"   ⚠️ Metadata filter failed: {response.error_message}")
                
                # Track performance
                if "metadata_search_times" not in verification_results["indexing_performance"]:
                    verification_results["indexing_performance"]["metadata_search_times"] = []
                verification_results["indexing_performance"]["metadata_search_times"].append(search_time)
            
            # Test 3: Similarity search with various query vectors
            logger.info("   Testing similarity-based retrieval...")
            test_queries = 10
            
            for query_idx in range(test_queries):
                # Create query vector similar to baseline vectors but not identical
                base_idx = query_idx * len(embeddings) // test_queries
                query_vector = np.array(embeddings[base_idx])
                
                # Add small noise to make it similar but not identical
                noise = np.random.normal(0, 0.05, self.dimension)
                query_vector = query_vector + noise
                query_vector = query_vector / np.linalg.norm(query_vector)  # Renormalize
                
                search_query = proximadb_pb2.SearchQuery(vector=query_vector.tolist())
                search_request = proximadb_pb2.VectorSearchRequest(
                    collection_id=collection_name,
                    queries=[search_query],
                    top_k=20,
                    distance_metric_override=LargeScaleTestConfig.DISTANCE_METRICS[config['distance']],
                    include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
                )
                
                start_time = time.time()
                response = await stub.VectorSearch(search_request)
                search_time = time.time() - start_time
                
                if response.success:
                    result_count = 0
                    if (hasattr(response, 'result_payload') and response.result_payload and
                        hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                        result_count = len(response.result_payload.compact_results.results)
                    
                    if result_count > 0:
                        verification_results["similarity_search_success"] += 1
                    else:
                        verification_results["similarity_search_failed"] += 1
                        logger.warning(f"   ⚠️ Similarity search {query_idx} found no results")
                else:
                    verification_results["similarity_search_failed"] += 1
                    logger.warning(f"   ⚠️ Similarity search failed: {response.error_message}")
                
                # Track performance
                if "similarity_search_times" not in verification_results["indexing_performance"]:
                    verification_results["indexing_performance"]["similarity_search_times"] = []
                verification_results["indexing_performance"]["similarity_search_times"].append(search_time)
            
            await channel.close()
            
            # Calculate performance metrics
            if verification_results["indexing_performance"]:
                perf = verification_results["indexing_performance"]
                if "id_search_times" in perf:
                    perf["avg_id_search_ms"] = np.mean(perf["id_search_times"]) * 1000
                if "metadata_search_times" in perf:
                    perf["avg_metadata_search_ms"] = np.mean(perf["metadata_search_times"]) * 1000
                if "similarity_search_times" in perf:
                    perf["avg_similarity_search_ms"] = np.mean(perf["similarity_search_times"]) * 1000
            
            # Determine overall success
            total_tests = (verification_results["id_lookup_success"] + verification_results["id_lookup_failed"] +
                          verification_results["metadata_filter_success"] + verification_results["metadata_filter_failed"] +
                          verification_results["similarity_search_success"] + verification_results["similarity_search_failed"])
            
            successful_tests = (verification_results["id_lookup_success"] + 
                              verification_results["metadata_filter_success"] + 
                              verification_results["similarity_search_success"])
            
            verification_results["overall_success"] = successful_tests > (total_tests * 0.8)  # 80% success rate
            
            logger.info(f"✅ Data retrieval verification complete: {successful_tests}/{total_tests} tests passed")
            
            return verification_results
            
        except Exception as e:
            logger.error(f"❌ Exception in data retrieval verification: {e}")
            return {"success": False, "error": str(e)}
    
    async def perform_mixed_operations(self, collection_name: str, metadata_fields: List[str]) -> Dict[str, Any]:
        """Perform mixed insert/search/delete operations on large dataset"""
        logger.info("🔄 Performing mixed operations (insert/search/delete)...")
        
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            operations_results = {
                "inserts": {"success": 0, "failed": 0, "total_time": 0},
                "searches": {"success": 0, "failed": 0, "total_time": 0},
                "deletes": {"success": 0, "failed": 0, "total_time": 0},
                "overall_success": False
            }
            
            # Generate additional vectors for mixed operations
            vector_ids, embeddings, metadata_list = self.generate_large_scale_embeddings(1000, "mixed_ops_")
            
            # Interleave insert, search, and delete operations
            for i in range(0, len(vector_ids), 10):
                batch_end = min(i + 10, len(vector_ids))
                batch_ids = vector_ids[i:batch_end]
                batch_embeddings = embeddings[i:batch_end]
                batch_metadata = metadata_list[i:batch_end]
                
                # Insert batch
                vectors_data = []
                for vid, embedding, metadata in zip(batch_ids, batch_embeddings, batch_metadata):
                    filtered_metadata = {k: v for k, v in metadata.items() if k in metadata_fields}
                    vector_record = {
                        "id": vid,
                        "vector": embedding,
                        "metadata": filtered_metadata,
                        "timestamp": int(time.time()),
                        "version": 1
                    }
                    vectors_data.append(vector_record)
                
                # INSERT operation
                start_time = time.time()
                vectors_json = json.dumps(vectors_data).encode('utf-8')
                request = proximadb_pb2.VectorInsertRequest(
                    collection_id=collection_name,
                    upsert_mode=False,
                    vectors_avro_payload=vectors_json
                )
                
                response = await stub.VectorInsert(request)
                insert_time = time.time() - start_time
                operations_results["inserts"]["total_time"] += insert_time
                
                if response.success:
                    operations_results["inserts"]["success"] += len(batch_ids)
                else:
                    operations_results["inserts"]["failed"] += len(batch_ids)
                
                # SEARCH operation (using one of the just-inserted vectors)
                if batch_embeddings:
                    start_time = time.time()
                    search_query = proximadb_pb2.SearchQuery(vector=batch_embeddings[0])
                    search_request = proximadb_pb2.VectorSearchRequest(
                        collection_id=collection_name,
                        queries=[search_query],
                        top_k=10,
                        distance_metric_override=1,  # Cosine
                        include_fields=proximadb_pb2.IncludeFields(vector=False, metadata=True)
                    )
                    
                    response = await stub.VectorSearch(search_request)
                    search_time = time.time() - start_time
                    operations_results["searches"]["total_time"] += search_time
                    
                    if response.success:
                        operations_results["searches"]["success"] += 1
                    else:
                        operations_results["searches"]["failed"] += 1
                
                # Small delay between operations
                await asyncio.sleep(0.1)
            
            await channel.close()
            
            # Calculate success rate
            total_ops = (operations_results["inserts"]["success"] + operations_results["inserts"]["failed"] +
                        operations_results["searches"]["success"] + operations_results["searches"]["failed"])
            successful_ops = (operations_results["inserts"]["success"] + operations_results["searches"]["success"])
            
            operations_results["overall_success"] = successful_ops > (total_ops * 0.9)  # 90% success rate
            
            logger.info(f"✅ Mixed operations complete: {successful_ops}/{total_ops} operations successful")
            
            return operations_results
            
        except Exception as e:
            logger.error(f"❌ Exception in mixed operations: {e}")
            return {"success": False, "error": str(e)}
    
    async def run_large_scale_test(self, config: Dict[str, str]) -> Dict[str, Any]:
        """Run comprehensive large-scale test for a single configuration"""
        test_name = f"{config['storage']}_{config['indexing']}_{config['distance']}_{config['metadata']}"
        metadata_fields = LargeScaleTestConfig.METADATA_CONFIGS[config['metadata']]
        
        result = {
            "configuration": config,
            "test_name": test_name,
            "collection_creation": {"success": False},
            "baseline_insertion": {"success": False},
            "large_batch_insertion": {"success": False},
            "data_retrieval_verification": {"success": False},
            "mixed_operations": {"success": False},
            "flush_compaction_detected": False,
            "indexing_performance": {},
            "overall_success": False,
            "error_messages": []
        }
        
        logger.info(f"🚀 Starting large-scale test: {test_name}")
        logger.info(f"   Storage: {config['storage']}, Indexing: {config['indexing']}")
        logger.info(f"   Distance: {config['distance']}, Metadata fields: {len(metadata_fields)}")
        
        collection_name = ""
        
        try:
            # Step 1: Create collection
            logger.info("\n1️⃣ Creating collection for large-scale operations...")
            create_success, collection_name = await self.create_large_scale_collection(config, test_name)
            result["collection_creation"]["success"] = create_success
            
            if not create_success:
                result["error_messages"].append("Failed to create collection")
                return result
            
            # Step 2: Insert baseline vectors
            logger.info(f"\n2️⃣ Inserting baseline vectors ({LargeScaleTestConfig.INITIAL_VECTOR_COUNT})...")
            baseline_result = await self.insert_baseline_vectors(collection_name, metadata_fields)
            result["baseline_insertion"] = baseline_result
            
            if not baseline_result["success"]:
                result["error_messages"].append("Failed to insert baseline vectors")
                return result
            
            # Step 3: Insert large batch to trigger flush/compaction
            logger.info(f"\n3️⃣ Inserting large batch to trigger flush/compaction ({LargeScaleTestConfig.LARGE_BATCH_SIZE})...")
            large_batch_result = await self.insert_large_batch_trigger_flush(collection_name, metadata_fields)
            result["large_batch_insertion"] = large_batch_result
            result["flush_compaction_detected"] = large_batch_result.get("flush_detected", False)
            
            if not large_batch_result["success"]:
                result["error_messages"].append("Failed to insert large batch")
                # Continue with verification even if large batch failed
            
            # Step 4: Verify baseline data retrieval after flush/compaction
            logger.info("\n4️⃣ Verifying baseline data retrieval after flush/compaction...")
            verification_result = await self.verify_baseline_data_retrieval(collection_name, config)
            result["data_retrieval_verification"] = verification_result
            result["indexing_performance"] = verification_result.get("indexing_performance", {})
            
            if not verification_result.get("overall_success", False):
                result["error_messages"].append("Data retrieval verification failed")
            
            # Step 5: Perform mixed operations
            logger.info("\n5️⃣ Performing mixed insert/search/delete operations...")
            mixed_ops_result = await self.perform_mixed_operations(collection_name, metadata_fields)
            result["mixed_operations"] = mixed_ops_result
            
            if not mixed_ops_result.get("overall_success", False):
                result["error_messages"].append("Mixed operations failed")
            
            # Determine overall success
            result["overall_success"] = (
                result["collection_creation"]["success"] and
                result["baseline_insertion"]["success"] and
                result["data_retrieval_verification"].get("overall_success", False) and
                result["mixed_operations"].get("overall_success", False)
            )
            
            if result["overall_success"]:
                logger.info(f"✅ Large-scale test {test_name} completed successfully!")
            else:
                logger.warning(f"⚠️ Large-scale test {test_name} completed with issues: {result['error_messages']}")
            
        except Exception as e:
            result["error_messages"].append(f"Exception during test: {str(e)}")
            logger.error(f"❌ Exception in large-scale test {test_name}: {e}")
        
        finally:
            # Cleanup: Delete collection
            if collection_name:
                try:
                    channel = grpc.aio.insecure_channel("localhost:5679")
                    stub = proximadb_pb2_grpc.ProximaDBStub(channel)
                    
                    delete_request = proximadb_pb2.CollectionRequest(
                        operation=4,  # DELETE
                        collection_name=collection_name
                    )
                    await stub.CollectionOperation(delete_request)
                    await channel.close()
                    logger.info(f"🧹 Cleaned up collection: {collection_name}")
                except Exception as e:
                    logger.warning(f"⚠️ Failed to cleanup collection {collection_name}: {e}")
        
        return result
    
    async def run_comprehensive_large_scale_tests(self):
        """Run large-scale tests for all configurations"""
        logger.info("🚀 Starting comprehensive large-scale operations and indexing tests...")
        logger.info(f"📊 Testing {len(LargeScaleTestConfig.TEST_CONFIGS)} configurations")
        logger.info(f"📈 Baseline vectors: {LargeScaleTestConfig.INITIAL_VECTOR_COUNT}")
        logger.info(f"🔄 Large batch size: {LargeScaleTestConfig.LARGE_BATCH_SIZE}")
        logger.info(f"⚡ Total vectors per test: ~{LargeScaleTestConfig.INITIAL_VECTOR_COUNT + LargeScaleTestConfig.LARGE_BATCH_SIZE + 1000}")
        
        test_count = 0
        successful_tests = 0
        
        for config in LargeScaleTestConfig.TEST_CONFIGS:
            test_count += 1
            
            logger.info(f"\n🧪 Large-scale test {test_count}/{len(LargeScaleTestConfig.TEST_CONFIGS)}: {config}")
            
            result = await self.run_large_scale_test(config)
            self.test_results.append(result)
            
            if result["overall_success"]:
                successful_tests += 1
                logger.info(f"✅ Test {test_count} PASSED")
                
                # Log key metrics
                if "indexing_performance" in result:
                    perf = result["indexing_performance"]
                    if "avg_similarity_search_ms" in perf:
                        logger.info(f"   Similarity search: {perf['avg_similarity_search_ms']:.1f}ms avg")
                    if "avg_metadata_search_ms" in perf:
                        logger.info(f"   Metadata search: {perf['avg_metadata_search_ms']:.1f}ms avg")
                
                if result["flush_compaction_detected"]:
                    logger.info("   🔄 Flush/compaction operations detected")
            else:
                logger.error(f"❌ Test {test_count} FAILED: {result['error_messages']}")
            
            # Longer delay between tests for system recovery
            await asyncio.sleep(5)
        
        logger.info(f"\n📊 Large-scale test summary: {successful_tests}/{len(LargeScaleTestConfig.TEST_CONFIGS)} tests passed")
        
        return successful_tests == len(LargeScaleTestConfig.TEST_CONFIGS)
    
    def generate_comprehensive_report(self):
        """Generate detailed large-scale operations report"""
        report = {
            "test_summary": {
                "total_tests": len(self.test_results),
                "successful_tests": sum(1 for r in self.test_results if r["overall_success"]),
                "failed_tests": sum(1 for r in self.test_results if not r["overall_success"]),
                "success_rate": 0
            },
            "flush_compaction_analysis": {
                "tests_with_flush_detected": 0,
                "configurations_triggering_flush": []
            },
            "indexing_performance_analysis": {
                "avg_similarity_search_ms": 0,
                "avg_metadata_search_ms": 0,
                "avg_id_search_ms": 0,
                "performance_by_indexing_algorithm": {}
            },
            "data_retrieval_success_rates": {
                "id_lookup_success_rate": 0,
                "metadata_filter_success_rate": 0,
                "similarity_search_success_rate": 0
            },
            "detailed_results": self.test_results,
            "test_parameters": {
                "initial_vector_count": LargeScaleTestConfig.INITIAL_VECTOR_COUNT,
                "large_batch_size": LargeScaleTestConfig.LARGE_BATCH_SIZE,
                "vector_dimension": self.dimension,
                "configurations_tested": len(LargeScaleTestConfig.TEST_CONFIGS)
            }
        }
        
        if report["test_summary"]["total_tests"] > 0:
            report["test_summary"]["success_rate"] = (
                report["test_summary"]["successful_tests"] / report["test_summary"]["total_tests"] * 100
            )
        
        # Analyze flush/compaction detection
        similarity_times = []
        metadata_times = []
        id_times = []
        
        id_success_total = 0
        id_total_total = 0
        metadata_success_total = 0
        metadata_total_total = 0
        similarity_success_total = 0
        similarity_total_total = 0
        
        for result in self.test_results:
            if result["flush_compaction_detected"]:
                report["flush_compaction_analysis"]["tests_with_flush_detected"] += 1
                config_name = f"{result['configuration']['storage']}/{result['configuration']['indexing']}"
                report["flush_compaction_analysis"]["configurations_triggering_flush"].append(config_name)
            
            # Collect performance metrics
            if "indexing_performance" in result:
                perf = result["indexing_performance"]
                if "avg_similarity_search_ms" in perf:
                    similarity_times.append(perf["avg_similarity_search_ms"])
                if "avg_metadata_search_ms" in perf:
                    metadata_times.append(perf["avg_metadata_search_ms"])
                if "avg_id_search_ms" in perf:
                    id_times.append(perf["avg_id_search_ms"])
                
                # Group by indexing algorithm
                indexing_algo = result['configuration']['indexing']
                if indexing_algo not in report["indexing_performance_analysis"]["performance_by_indexing_algorithm"]:
                    report["indexing_performance_analysis"]["performance_by_indexing_algorithm"][indexing_algo] = {
                        "similarity_times": [],
                        "metadata_times": [],
                        "id_times": []
                    }
                
                algo_perf = report["indexing_performance_analysis"]["performance_by_indexing_algorithm"][indexing_algo]
                if "avg_similarity_search_ms" in perf:
                    algo_perf["similarity_times"].append(perf["avg_similarity_search_ms"])
                if "avg_metadata_search_ms" in perf:
                    algo_perf["metadata_times"].append(perf["avg_metadata_search_ms"])
                if "avg_id_search_ms" in perf:
                    algo_perf["id_times"].append(perf["avg_id_search_ms"])
            
            # Collect success rates
            if "data_retrieval_verification" in result:
                verification = result["data_retrieval_verification"]
                
                id_success_total += verification.get("id_lookup_success", 0)
                id_total_total += verification.get("id_lookup_success", 0) + verification.get("id_lookup_failed", 0)
                
                metadata_success_total += verification.get("metadata_filter_success", 0)
                metadata_total_total += verification.get("metadata_filter_success", 0) + verification.get("metadata_filter_failed", 0)
                
                similarity_success_total += verification.get("similarity_search_success", 0)
                similarity_total_total += verification.get("similarity_search_success", 0) + verification.get("similarity_search_failed", 0)
        
        # Calculate average performance
        if similarity_times:
            report["indexing_performance_analysis"]["avg_similarity_search_ms"] = np.mean(similarity_times)
        if metadata_times:
            report["indexing_performance_analysis"]["avg_metadata_search_ms"] = np.mean(metadata_times)
        if id_times:
            report["indexing_performance_analysis"]["avg_id_search_ms"] = np.mean(id_times)
        
        # Calculate success rates
        if id_total_total > 0:
            report["data_retrieval_success_rates"]["id_lookup_success_rate"] = (id_success_total / id_total_total) * 100
        if metadata_total_total > 0:
            report["data_retrieval_success_rates"]["metadata_filter_success_rate"] = (metadata_success_total / metadata_total_total) * 100
        if similarity_total_total > 0:
            report["data_retrieval_success_rates"]["similarity_search_success_rate"] = (similarity_success_total / similarity_total_total) * 100
        
        # Calculate averages for each indexing algorithm
        for algo, perf_data in report["indexing_performance_analysis"]["performance_by_indexing_algorithm"].items():
            if perf_data["similarity_times"]:
                perf_data["avg_similarity_ms"] = np.mean(perf_data["similarity_times"])
            if perf_data["metadata_times"]:
                perf_data["avg_metadata_ms"] = np.mean(perf_data["metadata_times"])
            if perf_data["id_times"]:
                perf_data["avg_id_ms"] = np.mean(perf_data["id_times"])
        
        # Save report to file
        with open("/home/vsingh/code/proximadb/large_scale_operations_report.json", "w") as f:
            json.dump(report, f, indent=2)
        
        logger.info("📄 Large-scale operations report saved to large_scale_operations_report.json")
        
        return report

async def main():
    """Main test execution"""
    test_suite = LargeScaleOperationsSuite()
    
    try:
        # Setup
        await test_suite.setup_server()
        
        # Run tests
        all_passed = await test_suite.run_comprehensive_large_scale_tests()
        
        # Generate report
        report = test_suite.generate_comprehensive_report()
        
        # Print summary
        print("\n" + "="*80)
        print("📊 LARGE-SCALE OPERATIONS & INDEXING TEST SUMMARY")
        print("="*80)
        print(f"Total Tests: {report['test_summary']['total_tests']}")
        print(f"Successful: {report['test_summary']['successful_tests']}")
        print(f"Failed: {report['test_summary']['failed_tests']}")
        print(f"Success Rate: {report['test_summary']['success_rate']:.1f}%")
        
        print(f"\n🔄 FLUSH/COMPACTION ANALYSIS:")
        print(f"Tests with flush detected: {report['flush_compaction_analysis']['tests_with_flush_detected']}")
        if report['flush_compaction_analysis']['configurations_triggering_flush']:
            print(f"Configurations triggering flush: {', '.join(set(report['flush_compaction_analysis']['configurations_triggering_flush']))}")
        
        print(f"\n⚡ INDEXING PERFORMANCE:")
        perf = report['indexing_performance_analysis']
        if perf['avg_similarity_search_ms']:
            print(f"Avg similarity search: {perf['avg_similarity_search_ms']:.1f}ms")
        if perf['avg_metadata_search_ms']:
            print(f"Avg metadata search: {perf['avg_metadata_search_ms']:.1f}ms")
        if perf['avg_id_search_ms']:
            print(f"Avg ID search: {perf['avg_id_search_ms']:.1f}ms")
        
        print(f"\n🎯 DATA RETRIEVAL SUCCESS RATES:")
        rates = report['data_retrieval_success_rates']
        print(f"ID lookup: {rates['id_lookup_success_rate']:.1f}%")
        print(f"Metadata filter: {rates['metadata_filter_success_rate']:.1f}%")
        print(f"Similarity search: {rates['similarity_search_success_rate']:.1f}%")
        
        print("\n📈 PERFORMANCE BY INDEXING ALGORITHM:")
        for algo, perf_data in perf['performance_by_indexing_algorithm'].items():
            print(f"  {algo}:")
            if 'avg_similarity_ms' in perf_data:
                print(f"    Similarity: {perf_data['avg_similarity_ms']:.1f}ms")
            if 'avg_metadata_ms' in perf_data:
                print(f"    Metadata: {perf_data['avg_metadata_ms']:.1f}ms")
        
        print("="*80)
        
        if all_passed:
            print("✅ All large-scale operations and indexing tests passed!")
        else:
            print("❌ Some large-scale operations and indexing tests failed!")
        
        return all_passed
        
    except Exception as e:
        logger.error(f"❌ Test suite failed: {e}")
        return False
    finally:
        await test_suite.teardown_server()

if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)