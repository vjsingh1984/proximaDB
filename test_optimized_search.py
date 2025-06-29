#!/usr/bin/env python3
"""
Test Storage-Aware Search Engine Optimizations via gRPC Python SDK

This script tests the newly implemented storage-aware polymorphic search
optimizations for both VIPER and LSM storage engines.
"""

import asyncio
import logging
import time
import numpy as np
import sys
import os
from typing import List, Dict, Any, Optional
from pathlib import Path

# Add the Python SDK to the path
sys.path.insert(0, str(Path(__file__).parent / "clients" / "python" / "src"))

try:
    from proximadb import ProximaDBClient
    from proximadb.models import CollectionConfig, SearchResult
    from proximadb.exceptions import ProximaDBError
    from proximadb.grpc_client import ProximaDBGrpcClient
except ImportError as e:
    print(f"❌ Failed to import ProximaDB SDK: {e}")
    print("Make sure you're running from the project root directory")
    sys.exit(1)

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

class SearchOptimizationTester:
    """Test harness for storage-aware search optimizations"""
    
    def __init__(self, grpc_host: str = "localhost", grpc_port: int = 5679):
        """Initialize the test harness"""
        self.grpc_url = f"{grpc_host}:{grpc_port}"
        self.client = None
        self.test_collections = []
        
    async def setup(self) -> bool:
        """Setup test environment and client connection"""
        try:
            logger.info("🚀 Setting up gRPC client connection...")
            self.client = ProximaDBGrpcClient(self.grpc_url)
            
            # Test basic connectivity
            health = await self.client.health_check()
            logger.info(f"✅ Server health: {health}")
            
            return True
            
        except Exception as e:
            logger.error(f"❌ Setup failed: {e}")
            return False
    
    async def cleanup(self):
        """Clean up test resources"""
        if self.client:
            for collection_name in self.test_collections:
                try:
                    await self.client.delete_collection(collection_name)
                    logger.info(f"🗑️ Cleaned up collection: {collection_name}")
                except:
                    pass
            
            await self.client.close()
    
    async def create_test_collection(
        self, 
        name: str, 
        dimension: int = 384,
        storage_engine: str = "viper",
        distance_metric: str = "cosine"
    ) -> bool:
        """Create a test collection with specified storage engine"""
        try:
            logger.info(f"📦 Creating collection '{name}' with {storage_engine} storage...")
            
            config = CollectionConfig(
                name=name,
                dimension=dimension,
                distance_metric=distance_metric,
                storage_engine=storage_engine,
                indexing_algorithm="hnsw"
            )
            
            await self.client.create_collection(config)
            self.test_collections.append(name)
            
            logger.info(f"✅ Created collection '{name}' successfully")
            return True
            
        except Exception as e:
            logger.error(f"❌ Failed to create collection '{name}': {e}")
            return False
    
    async def insert_test_vectors(
        self, 
        collection_name: str, 
        count: int = 1000,
        dimension: int = 384
    ) -> bool:
        """Insert test vectors into a collection"""
        try:
            logger.info(f"📥 Inserting {count} test vectors into '{collection_name}'...")
            
            # Generate diverse test vectors
            vectors = []
            for i in range(count):
                # Create vectors with different patterns for better testing
                if i % 3 == 0:
                    # Random vectors
                    vector = np.random.normal(0, 1, dimension).astype(np.float32)
                elif i % 3 == 1:
                    # Clustered vectors (similar to first pattern)
                    base = np.ones(dimension, dtype=np.float32) * 0.5
                    noise = np.random.normal(0, 0.1, dimension).astype(np.float32)
                    vector = base + noise
                else:
                    # Another cluster pattern
                    base = np.ones(dimension, dtype=np.float32) * -0.5
                    noise = np.random.normal(0, 0.1, dimension).astype(np.float32)
                    vector = base + noise
                
                # Normalize
                vector = vector / np.linalg.norm(vector)
                
                metadata = {
                    "id": i,
                    "category": f"cat_{i % 10}",
                    "cluster": i % 3,
                    "value": float(i * 0.1)
                }
                
                vectors.append({
                    "id": f"vec_{i:06d}",
                    "vector": vector.tolist(),
                    "metadata": metadata
                })
            
            # Insert in batches
            batch_size = 100
            for i in range(0, len(vectors), batch_size):
                batch = vectors[i:i + batch_size]
                await self.client.insert_vectors(collection_name, batch)
                logger.info(f"📥 Inserted batch {i//batch_size + 1}/{(len(vectors) + batch_size - 1)//batch_size}")
            
            logger.info(f"✅ Inserted {count} vectors successfully")
            return True
            
        except Exception as e:
            logger.error(f"❌ Failed to insert vectors: {e}")
            return False
    
    async def test_optimized_search(
        self,
        collection_name: str,
        test_name: str,
        optimization_params: Dict[str, Any]
    ) -> Dict[str, Any]:
        """Test search with specific optimization parameters"""
        logger.info(f"🔍 Testing {test_name} on collection '{collection_name}'...")
        
        # Generate a query vector
        dimension = 384
        query_vector = np.random.normal(0, 1, dimension).astype(np.float32)
        query_vector = query_vector / np.linalg.norm(query_vector)
        
        start_time = time.time()
        
        try:
            # Perform search with optimization parameters
            results = await self.client.search(
                collection_id=collection_name,
                query=query_vector,
                k=20,
                include_vectors=False,
                include_metadata=True,
                **optimization_params
            )
            
            search_time = time.time() - start_time
            
            metrics = {
                "test_name": test_name,
                "collection": collection_name,
                "search_time_ms": search_time * 1000,
                "results_count": len(results),
                "success": True,
                "optimization_params": optimization_params
            }
            
            if results:
                metrics["top_score"] = results[0].score
                metrics["avg_score"] = sum(r.score for r in results) / len(results)
                metrics["has_optimization_info"] = hasattr(results[0], 'search_engine') or \
                                                 'search_engine' in (results[0].metadata or {})
            
            logger.info(f"✅ {test_name}: {len(results)} results in {search_time*1000:.2f}ms")
            return metrics
            
        except Exception as e:
            logger.error(f"❌ {test_name} failed: {e}")
            return {
                "test_name": test_name,
                "collection": collection_name,
                "search_time_ms": (time.time() - start_time) * 1000,
                "results_count": 0,
                "success": False,
                "error": str(e),
                "optimization_params": optimization_params
            }
    
    async def run_comprehensive_tests(self) -> Dict[str, List[Dict[str, Any]]]:
        """Run comprehensive tests for both VIPER and LSM storage engines"""
        logger.info("🧪 Starting comprehensive storage-aware search optimization tests...")
        
        test_results = {
            "viper_tests": [],
            "lsm_tests": []
        }
        
        # Test configurations
        test_configs = [
            {
                "name": "High Optimization + SIMD",
                "params": {
                    "optimization_level": "high",
                    "use_storage_aware": True,
                    "quantization_level": "FP32",
                    "enable_simd": True
                }
            },
            {
                "name": "Medium Optimization",
                "params": {
                    "optimization_level": "medium", 
                    "use_storage_aware": True,
                    "quantization_level": "FP32",
                    "enable_simd": False
                }
            },
            {
                "name": "Low Optimization (Baseline)",
                "params": {
                    "optimization_level": "low",
                    "use_storage_aware": False,
                    "quantization_level": "FP32",
                    "enable_simd": False
                }
            },
            {
                "name": "PQ8 Quantization",
                "params": {
                    "optimization_level": "high",
                    "use_storage_aware": True,
                    "quantization_level": "PQ8",
                    "enable_simd": True
                }
            },
            {
                "name": "PQ4 Quantization",
                "params": {
                    "optimization_level": "high",
                    "use_storage_aware": True,
                    "quantization_level": "PQ4",
                    "enable_simd": True
                }
            }
        ]
        
        # Test VIPER storage engine
        viper_collection = "test_viper_optimized"
        if await self.create_test_collection(viper_collection, storage_engine="viper"):
            if await self.insert_test_vectors(viper_collection, count=1000):
                logger.info("🔍 Running VIPER optimization tests...")
                
                for config in test_configs:
                    result = await self.test_optimized_search(
                        viper_collection,
                        f"VIPER - {config['name']}",
                        config['params']
                    )
                    test_results["viper_tests"].append(result)
        
        # Test LSM storage engine
        lsm_collection = "test_lsm_optimized"
        if await self.create_test_collection(lsm_collection, storage_engine="lsm"):
            if await self.insert_test_vectors(lsm_collection, count=1000):
                logger.info("🔍 Running LSM optimization tests...")
                
                for config in test_configs:
                    result = await self.test_optimized_search(
                        lsm_collection,
                        f"LSM - {config['name']}",
                        config['params']
                    )
                    test_results["lsm_tests"].append(result)
        
        return test_results
    
    async def test_metadata_filtering(self, collection_name: str) -> Dict[str, Any]:
        """Test metadata filtering with storage-aware optimizations"""
        logger.info(f"🔍 Testing metadata filtering on '{collection_name}'...")
        
        query_vector = np.random.normal(0, 1, 384).astype(np.float32)
        query_vector = query_vector / np.linalg.norm(query_vector)
        
        # Test different filter scenarios
        filter_tests = [
            {
                "name": "Category Filter",
                "filter": {"category": "cat_1"},
                "params": {
                    "optimization_level": "high",
                    "use_storage_aware": True,
                    "enable_simd": True
                }
            },
            {
                "name": "Range Filter",
                "filter": {"value": {"$gte": 5.0, "$lte": 10.0}},
                "params": {
                    "optimization_level": "high",
                    "use_storage_aware": True,
                    "enable_simd": True
                }
            },
            {
                "name": "Complex Filter",
                "filter": {
                    "category": {"$in": ["cat_1", "cat_2", "cat_3"]},
                    "cluster": 1
                },
                "params": {
                    "optimization_level": "high",
                    "use_storage_aware": True,
                    "enable_simd": True
                }
            }
        ]
        
        results = []
        for test in filter_tests:
            start_time = time.time()
            try:
                search_results = await self.client.search(
                    collection_id=collection_name,
                    query=query_vector,
                    k=10,
                    filter=test["filter"],
                    include_metadata=True,
                    **test["params"]
                )
                
                search_time = time.time() - start_time
                
                result = {
                    "test_name": test["name"],
                    "filter": test["filter"],
                    "search_time_ms": search_time * 1000,
                    "results_count": len(search_results),
                    "success": True
                }
                
                # Verify filter effectiveness
                if search_results and "category" in test["filter"]:
                    filtered_correctly = all(
                        r.metadata and r.metadata.get("category") == test["filter"]["category"]
                        for r in search_results
                    )
                    result["filter_correctness"] = filtered_correctly
                
                results.append(result)
                logger.info(f"✅ {test['name']}: {len(search_results)} results in {search_time*1000:.2f}ms")
                
            except Exception as e:
                results.append({
                    "test_name": test["name"],
                    "filter": test["filter"],
                    "search_time_ms": (time.time() - start_time) * 1000,
                    "results_count": 0,
                    "success": False,
                    "error": str(e)
                })
                logger.error(f"❌ {test['name']} failed: {e}")
        
        return {"filter_tests": results}
    
    def print_test_summary(self, results: Dict[str, Any]):
        """Print a comprehensive test summary"""
        print("\n" + "="*80)
        print("🧪 STORAGE-AWARE SEARCH OPTIMIZATION TEST RESULTS")
        print("="*80)
        
        # VIPER Results
        if results.get("viper_tests"):
            print("\n📊 VIPER Storage Engine Results:")
            print("-" * 50)
            viper_times = []
            for test in results["viper_tests"]:
                status = "✅" if test["success"] else "❌"
                time_ms = test["search_time_ms"]
                viper_times.append(time_ms)
                print(f"{status} {test['test_name']:30} | {time_ms:6.2f}ms | {test['results_count']:2d} results")
            
            if viper_times:
                print(f"{'':34} | Avg: {np.mean(viper_times):6.2f}ms")
        
        # LSM Results
        if results.get("lsm_tests"):
            print("\n📊 LSM Storage Engine Results:")
            print("-" * 50)
            lsm_times = []
            for test in results["lsm_tests"]:
                status = "✅" if test["success"] else "❌"
                time_ms = test["search_time_ms"]
                lsm_times.append(time_ms)
                print(f"{status} {test['test_name']:30} | {time_ms:6.2f}ms | {test['results_count']:2d} results")
            
            if lsm_times:
                print(f"{'':34} | Avg: {np.mean(lsm_times):6.2f}ms")
        
        # Performance Analysis
        if results.get("viper_tests") and results.get("lsm_tests"):
            viper_high_opt = next((t for t in results["viper_tests"] if "High Optimization" in t["test_name"]), None)
            viper_baseline = next((t for t in results["viper_tests"] if "Low Optimization" in t["test_name"]), None)
            lsm_high_opt = next((t for t in results["lsm_tests"] if "High Optimization" in t["test_name"]), None)
            lsm_baseline = next((t for t in results["lsm_tests"] if "Low Optimization" in t["test_name"]), None)
            
            print("\n📈 Performance Improvements:")
            print("-" * 50)
            
            if viper_high_opt and viper_baseline and viper_baseline["search_time_ms"] > 0:
                viper_improvement = viper_baseline["search_time_ms"] / viper_high_opt["search_time_ms"]
                print(f"VIPER Optimization: {viper_improvement:.2f}x faster")
            
            if lsm_high_opt and lsm_baseline and lsm_baseline["search_time_ms"] > 0:
                lsm_improvement = lsm_baseline["search_time_ms"] / lsm_high_opt["search_time_ms"]
                print(f"LSM Optimization:   {lsm_improvement:.2f}x faster")
        
        # Filter Tests
        if results.get("filter_tests"):
            print("\n🔍 Metadata Filtering Results:")
            print("-" * 50)
            for test in results["filter_tests"]:
                status = "✅" if test["success"] else "❌"
                correctness = ""
                if test.get("filter_correctness") is not None:
                    correctness = " ✓" if test["filter_correctness"] else " ✗"
                print(f"{status} {test['test_name']:20} | {test['search_time_ms']:6.2f}ms | {test['results_count']:2d} results{correctness}")
        
        print("\n" + "="*80)

async def main():
    """Main test execution"""
    print("🚀 ProximaDB Storage-Aware Search Optimization Test Suite")
    print("=" * 80)
    
    tester = SearchOptimizationTester()
    
    try:
        # Setup
        if not await tester.setup():
            print("❌ Failed to setup test environment")
            return 1
        
        # Run comprehensive tests
        results = await tester.run_comprehensive_tests()
        
        # Test metadata filtering on one collection
        if tester.test_collections:
            filter_results = await tester.test_metadata_filtering(tester.test_collections[0])
            results.update(filter_results)
        
        # Print summary
        tester.print_test_summary(results)
        
        # Save detailed results
        import json
        with open("search_optimization_test_results.json", "w") as f:
            json.dump(results, f, indent=2)
        print(f"\n📄 Detailed results saved to: search_optimization_test_results.json")
        
        return 0
        
    except KeyboardInterrupt:
        print("\n🛑 Test interrupted by user")
        return 1
    except Exception as e:
        logger.error(f"❌ Test execution failed: {e}")
        return 1
    finally:
        await tester.cleanup()

if __name__ == "__main__":
    # Check if server is likely running
    import socket
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    result = sock.connect_ex(('localhost', 5679))
    sock.close()
    
    if result != 0:
        print("❌ ProximaDB gRPC server not reachable on localhost:5679")
        print("   Please start the server first: cargo run --bin proximadb-server")
        sys.exit(1)
    
    exit_code = asyncio.run(main())
    sys.exit(exit_code)