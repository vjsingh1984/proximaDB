#!/usr/bin/env python3
"""
Comprehensive test suite for storage-aware search optimizations in ProximaDB

This test validates:
1. Basic functionality of both VIPER and LSM storage engines
2. Performance improvements from optimizations
3. Accuracy of search results across different configurations
4. Edge cases and error handling
5. Metadata filtering with storage-aware optimizations
6. Quantization levels and their impact
7. Concurrent operations and stress testing
"""

import asyncio
import json
import numpy as np
import requests
import time
import concurrent.futures
from typing import Dict, List, Any, Optional
from dataclasses import dataclass
from statistics import mean, stdev
import sys

# Configuration
BASE_URL = "http://localhost:5678"
GRPC_URL = "localhost:5679"
VECTOR_DIM = 384
TEST_SIZES = [100, 1000, 5000]  # Different dataset sizes for testing

@dataclass
class TestResult:
    """Store test results for analysis"""
    test_name: str
    storage_engine: str
    optimization_level: str
    dataset_size: int
    search_time_ms: float
    results_count: int
    accuracy: Optional[float] = None
    success: bool = True
    error: Optional[str] = None
    metadata: Optional[Dict] = None

class StorageOptimizationTester:
    """Comprehensive tester for storage-aware optimizations"""
    
    def __init__(self):
        self.test_results: List[TestResult] = []
        self.collections_created: List[str] = []
        
    def cleanup(self):
        """Clean up all test collections"""
        print("\n🧹 Cleaning up test collections...")
        for collection in self.collections_created:
            try:
                requests.delete(f"{BASE_URL}/collections/{collection}")
                print(f"   ✓ Deleted {collection}")
            except:
                pass
    
    def create_test_vectors(self, count: int, dim: int = VECTOR_DIM) -> List[Dict]:
        """Create diverse test vectors with metadata"""
        vectors = []
        
        # Create vectors with different patterns for realistic testing
        for i in range(count):
            # Create clustered data (simulating real-world patterns)
            cluster_id = i % 5  # 5 clusters
            base_vector = np.zeros(dim, dtype=np.float32)
            
            # Each cluster has a different pattern
            if cluster_id == 0:
                base_vector[0:100] = 0.8
            elif cluster_id == 1:
                base_vector[100:200] = 0.8
            elif cluster_id == 2:
                base_vector[200:300] = 0.8
            elif cluster_id == 3:
                base_vector[300:384] = 0.8
            else:
                base_vector = np.ones(dim, dtype=np.float32) * 0.3
            
            # Add noise
            noise = np.random.normal(0, 0.2, dim).astype(np.float32)
            vector = base_vector + noise
            
            # Normalize
            vector = vector / (np.linalg.norm(vector) + 1e-6)
            
            vectors.append({
                "id": f"vec_{i:06d}",
                "vector": vector.tolist(),
                "metadata": {
                    "cluster_id": cluster_id,
                    "category": f"cat_{i % 10}",
                    "timestamp": int(time.time() * 1000) + i,
                    "value": float(i * 0.1),
                    "tags": [f"tag_{i % 3}", f"tag_{i % 5}"],
                    "description": f"Test vector {i} in cluster {cluster_id}"
                }
            })
        
        return vectors
    
    def run_all_tests(self):
        """Run comprehensive test suite"""
        print("🚀 ProximaDB Storage-Aware Optimization Comprehensive Test Suite")
        print("=" * 80)
        
        # Test 1: Basic functionality test
        print("\n📋 Test 1: Basic Functionality")
        self.test_basic_functionality()
        
        # Test 2: Performance comparison
        print("\n📊 Test 2: Performance Comparison")
        self.test_performance_comparison()
        
        # Test 3: Quantization levels
        print("\n🔬 Test 3: Quantization Levels Impact")
        self.test_quantization_levels()
        
        # Test 4: Metadata filtering
        print("\n🔍 Test 4: Metadata Filtering Performance")
        self.test_metadata_filtering()
        
        # Test 5: Concurrent operations
        print("\n⚡ Test 5: Concurrent Operations")
        self.test_concurrent_operations()
        
        # Test 6: Edge cases
        print("\n🔧 Test 6: Edge Cases and Error Handling")
        self.test_edge_cases()
        
        # Test 7: Accuracy validation
        print("\n✅ Test 7: Search Accuracy Validation")
        self.test_search_accuracy()
        
        # Final report
        self.print_comprehensive_report()
    
    def test_basic_functionality(self):
        """Test basic create, insert, search, delete operations"""
        print("Testing basic operations for both storage engines...")
        
        for engine in ["viper", "lsm"]:
            collection_name = f"test_basic_{engine}"
            print(f"\n  Testing {engine.upper()} engine:")
            
            # Create collection
            try:
                response = requests.post(f"{BASE_URL}/collections", json={
                    "name": collection_name,
                    "dimension": VECTOR_DIM,
                    "distance_metric": "cosine",
                    "storage_engine": engine,
                    "indexing_algorithm": "hnsw"
                })
                if response.status_code == 200:
                    print(f"    ✓ Collection created")
                    self.collections_created.append(collection_name)
                else:
                    print(f"    ✗ Collection creation failed: {response.text}")
                    continue
            except Exception as e:
                print(f"    ✗ Error creating collection: {e}")
                continue
            
            # Insert vectors
            try:
                vectors = self.create_test_vectors(10)
                response = requests.post(
                    f"{BASE_URL}/collections/{collection_name}/vectors/batch",
                    json=vectors
                )
                if response.status_code == 200:
                    print(f"    ✓ Vectors inserted")
                else:
                    print(f"    ✗ Vector insertion failed: {response.text}")
            except Exception as e:
                print(f"    ✗ Error inserting vectors: {e}")
            
            # Search vectors
            try:
                query_vector = np.random.normal(0, 1, VECTOR_DIM).astype(np.float32)
                query_vector = query_vector / np.linalg.norm(query_vector)
                
                response = requests.post(
                    f"{BASE_URL}/collections/{collection_name}/search",
                    json={
                        "vector": query_vector.tolist(),
                        "k": 5,
                        "include_metadata": True
                    }
                )
                if response.status_code == 200:
                    results = response.json()
                    result_count = len(results.get('data', []))
                    print(f"    ✓ Search completed: {result_count} results")
                else:
                    print(f"    ✗ Search failed: {response.text}")
            except Exception as e:
                print(f"    ✗ Error searching vectors: {e}")
    
    def test_performance_comparison(self):
        """Compare performance with and without optimizations"""
        print("Comparing performance across different optimization levels...")
        
        test_configs = [
            {"name": "baseline", "optimization_level": "low", "use_storage_aware": False},
            {"name": "medium_opt", "optimization_level": "medium", "use_storage_aware": True},
            {"name": "high_opt", "optimization_level": "high", "use_storage_aware": True}
        ]
        
        for size in TEST_SIZES:
            print(f"\n  Dataset size: {size} vectors")
            
            for engine in ["viper", "lsm"]:
                collection_name = f"test_perf_{engine}_{size}"
                
                # Create collection and insert data
                try:
                    response = requests.post(f"{BASE_URL}/collections", json={
                        "name": collection_name,
                        "dimension": VECTOR_DIM,
                        "distance_metric": "cosine",
                        "storage_engine": engine
                    })
                    if response.status_code != 200:
                        continue
                    self.collections_created.append(collection_name)
                    
                    # Insert vectors in batches
                    vectors = self.create_test_vectors(size)
                    batch_size = 100
                    for i in range(0, len(vectors), batch_size):
                        batch = vectors[i:i+batch_size]
                        requests.post(
                            f"{BASE_URL}/collections/{collection_name}/vectors/batch",
                            json=batch
                        )
                    
                    # Test different optimization levels
                    query_vector = np.random.normal(0, 1, VECTOR_DIM).astype(np.float32)
                    query_vector = query_vector / np.linalg.norm(query_vector)
                    
                    for config in test_configs:
                        search_times = []
                        
                        # Run multiple searches for average
                        for _ in range(5):
                            search_data = {
                                "vector": query_vector.tolist(),
                                "k": 20,
                                "include_metadata": False,
                                "search_hints": {
                                    "optimization_level": config["optimization_level"],
                                    "use_storage_aware": config["use_storage_aware"]
                                }
                            }
                            
                            start_time = time.time()
                            response = requests.post(
                                f"{BASE_URL}/collections/{collection_name}/search",
                                json=search_data
                            )
                            search_time = (time.time() - start_time) * 1000
                            
                            if response.status_code == 200:
                                search_times.append(search_time)
                        
                        if search_times:
                            avg_time = mean(search_times)
                            self.test_results.append(TestResult(
                                test_name="performance_comparison",
                                storage_engine=engine,
                                optimization_level=config["name"],
                                dataset_size=size,
                                search_time_ms=avg_time,
                                results_count=20,
                                success=True
                            ))
                            print(f"    {engine.upper():5} - {config['name']:12}: {avg_time:6.2f}ms")
                
                except Exception as e:
                    print(f"    Error testing {engine}: {e}")
    
    def test_quantization_levels(self):
        """Test different quantization levels for VIPER engine"""
        print("Testing quantization levels impact on VIPER engine...")
        
        collection_name = "test_quantization_viper"
        
        try:
            # Create VIPER collection
            response = requests.post(f"{BASE_URL}/collections", json={
                "name": collection_name,
                "dimension": VECTOR_DIM,
                "distance_metric": "cosine",
                "storage_engine": "viper"
            })
            if response.status_code != 200:
                print("  ✗ Failed to create collection")
                return
            self.collections_created.append(collection_name)
            
            # Insert test vectors
            vectors = self.create_test_vectors(1000)
            batch_size = 100
            for i in range(0, len(vectors), batch_size):
                batch = vectors[i:i+batch_size]
                requests.post(
                    f"{BASE_URL}/collections/{collection_name}/vectors/batch",
                    json=batch
                )
            
            # Test different quantization levels
            quantization_levels = ["FP32", "PQ8", "PQ4", "Binary", "INT8"]
            query_vector = vectors[0]["vector"]  # Use first vector as query for accuracy testing
            
            fp32_results = None  # Baseline for accuracy comparison
            
            for quant_level in quantization_levels:
                search_times = []
                all_results = []
                
                for _ in range(3):
                    search_data = {
                        "vector": query_vector,
                        "k": 50,
                        "include_metadata": False,
                        "search_hints": {
                            "storage_aware": True,
                            "quantization_level": quant_level
                        }
                    }
                    
                    start_time = time.time()
                    response = requests.post(
                        f"{BASE_URL}/collections/{collection_name}/search",
                        json=search_data
                    )
                    search_time = (time.time() - start_time) * 1000
                    
                    if response.status_code == 200:
                        results = response.json().get('data', [])
                        search_times.append(search_time)
                        all_results.append(results)
                        
                        # Store FP32 results as baseline
                        if quant_level == "FP32" and fp32_results is None:
                            fp32_results = [r['id'] for r in results[:10]]
                
                if search_times and all_results:
                    avg_time = mean(search_times)
                    
                    # Calculate accuracy vs FP32
                    accuracy = 100.0
                    if fp32_results and quant_level != "FP32":
                        result_ids = [r['id'] for r in all_results[0][:10]]
                        matches = sum(1 for id in result_ids if id in fp32_results)
                        accuracy = (matches / 10) * 100
                    
                    self.test_results.append(TestResult(
                        test_name="quantization_test",
                        storage_engine="viper",
                        optimization_level=quant_level,
                        dataset_size=1000,
                        search_time_ms=avg_time,
                        results_count=50,
                        accuracy=accuracy,
                        success=True
                    ))
                    
                    print(f"  {quant_level:8}: {avg_time:6.2f}ms, Accuracy: {accuracy:5.1f}%")
        
        except Exception as e:
            print(f"  ✗ Error in quantization test: {e}")
    
    def test_metadata_filtering(self):
        """Test metadata filtering with storage-aware optimizations"""
        print("Testing metadata filtering performance...")
        
        filter_tests = [
            {"name": "simple_equality", "filter": {"category": "cat_1"}},
            {"name": "range_query", "filter": {"value": {"$gte": 10.0, "$lte": 50.0}}},
            {"name": "in_operator", "filter": {"category": {"$in": ["cat_1", "cat_2", "cat_3"]}}},
            {"name": "complex_filter", "filter": {
                "$and": [
                    {"cluster_id": 1},
                    {"value": {"$gte": 20.0}},
                    {"category": {"$in": ["cat_1", "cat_2"]}}
                ]
            }}
        ]
        
        for engine in ["viper", "lsm"]:
            collection_name = f"test_filter_{engine}"
            print(f"\n  {engine.upper()} engine:")
            
            try:
                # Create collection
                response = requests.post(f"{BASE_URL}/collections", json={
                    "name": collection_name,
                    "dimension": VECTOR_DIM,
                    "distance_metric": "cosine",
                    "storage_engine": engine
                })
                if response.status_code != 200:
                    continue
                self.collections_created.append(collection_name)
                
                # Insert vectors
                vectors = self.create_test_vectors(1000)
                batch_size = 100
                for i in range(0, len(vectors), batch_size):
                    batch = vectors[i:i+batch_size]
                    requests.post(
                        f"{BASE_URL}/collections/{collection_name}/vectors/batch",
                        json=batch
                    )
                
                # Test each filter
                query_vector = np.random.normal(0, 1, VECTOR_DIM).astype(np.float32)
                query_vector = query_vector / np.linalg.norm(query_vector)
                
                for filter_test in filter_tests:
                    search_data = {
                        "vector": query_vector.tolist(),
                        "k": 20,
                        "filters": filter_test["filter"],
                        "include_metadata": True,
                        "search_hints": {
                            "storage_aware": True,
                            "predicate_pushdown": True
                        }
                    }
                    
                    start_time = time.time()
                    response = requests.post(
                        f"{BASE_URL}/collections/{collection_name}/search",
                        json=search_data
                    )
                    search_time = (time.time() - start_time) * 1000
                    
                    if response.status_code == 200:
                        results = response.json().get('data', [])
                        
                        # Verify filter correctness
                        filter_valid = True
                        if "category" in filter_test["filter"] and isinstance(filter_test["filter"]["category"], str):
                            filter_valid = all(
                                r.get('metadata', {}).get('category') == filter_test["filter"]["category"]
                                for r in results
                            )
                        
                        self.test_results.append(TestResult(
                            test_name="metadata_filtering",
                            storage_engine=engine,
                            optimization_level=filter_test["name"],
                            dataset_size=1000,
                            search_time_ms=search_time,
                            results_count=len(results),
                            accuracy=100.0 if filter_valid else 0.0,
                            success=True
                        ))
                        
                        status = "✓" if filter_valid else "✗"
                        print(f"    {filter_test['name']:20}: {search_time:6.2f}ms, {len(results):2d} results {status}")
            
            except Exception as e:
                print(f"    ✗ Error: {e}")
    
    def test_concurrent_operations(self):
        """Test concurrent search operations"""
        print("Testing concurrent search operations...")
        
        for engine in ["viper", "lsm"]:
            collection_name = f"test_concurrent_{engine}"
            print(f"\n  {engine.upper()} engine:")
            
            try:
                # Create collection
                response = requests.post(f"{BASE_URL}/collections", json={
                    "name": collection_name,
                    "dimension": VECTOR_DIM,
                    "distance_metric": "cosine",
                    "storage_engine": engine
                })
                if response.status_code != 200:
                    continue
                self.collections_created.append(collection_name)
                
                # Insert vectors
                vectors = self.create_test_vectors(1000)
                batch_size = 100
                for i in range(0, len(vectors), batch_size):
                    batch = vectors[i:i+batch_size]
                    requests.post(
                        f"{BASE_URL}/collections/{collection_name}/vectors/batch",
                        json=batch
                    )
                
                # Concurrent search test
                def search_task(task_id):
                    query_vector = np.random.normal(0, 1, VECTOR_DIM).astype(np.float32)
                    query_vector = query_vector / np.linalg.norm(query_vector)
                    
                    search_data = {
                        "vector": query_vector.tolist(),
                        "k": 10,
                        "search_hints": {
                            "storage_aware": True,
                            "optimization_level": "high"
                        }
                    }
                    
                    start_time = time.time()
                    response = requests.post(
                        f"{BASE_URL}/collections/{collection_name}/search",
                        json=search_data
                    )
                    search_time = (time.time() - start_time) * 1000
                    
                    return {
                        "task_id": task_id,
                        "success": response.status_code == 200,
                        "search_time_ms": search_time,
                        "results_count": len(response.json().get('data', [])) if response.status_code == 200 else 0
                    }
                
                # Run concurrent searches
                concurrent_tasks = 20
                with concurrent.futures.ThreadPoolExecutor(max_workers=10) as executor:
                    start_time = time.time()
                    futures = [executor.submit(search_task, i) for i in range(concurrent_tasks)]
                    results = [f.result() for f in concurrent.futures.as_completed(futures)]
                    total_time = (time.time() - start_time) * 1000
                
                # Analyze results
                successful = sum(1 for r in results if r['success'])
                avg_search_time = mean([r['search_time_ms'] for r in results if r['success']])
                
                self.test_results.append(TestResult(
                    test_name="concurrent_operations",
                    storage_engine=engine,
                    optimization_level="high",
                    dataset_size=1000,
                    search_time_ms=avg_search_time,
                    results_count=successful,
                    success=True,
                    metadata={
                        "total_tasks": concurrent_tasks,
                        "successful_tasks": successful,
                        "total_time_ms": total_time,
                        "throughput_qps": (successful / total_time) * 1000
                    }
                ))
                
                print(f"    Concurrent searches: {successful}/{concurrent_tasks} successful")
                print(f"    Average search time: {avg_search_time:.2f}ms")
                print(f"    Throughput: {(successful / total_time) * 1000:.1f} QPS")
            
            except Exception as e:
                print(f"    ✗ Error: {e}")
    
    def test_edge_cases(self):
        """Test edge cases and error handling"""
        print("Testing edge cases and error handling...")
        
        edge_cases = [
            {
                "name": "empty_collection",
                "description": "Search in empty collection",
                "setup": lambda coll: None,
                "search": {"vector": [0.1] * VECTOR_DIM, "k": 10}
            },
            {
                "name": "single_vector",
                "description": "Search with k > collection size",
                "setup": lambda coll: requests.post(
                    f"{BASE_URL}/collections/{coll}/vectors/batch",
                    json=[{"id": "single", "vector": [0.1] * VECTOR_DIM}]
                ),
                "search": {"vector": [0.1] * VECTOR_DIM, "k": 100}
            },
            {
                "name": "zero_k",
                "description": "Search with k=0",
                "setup": lambda coll: requests.post(
                    f"{BASE_URL}/collections/{coll}/vectors/batch",
                    json=self.create_test_vectors(10)
                ),
                "search": {"vector": [0.1] * VECTOR_DIM, "k": 0}
            },
            {
                "name": "invalid_dimension",
                "description": "Search with wrong vector dimension",
                "setup": lambda coll: requests.post(
                    f"{BASE_URL}/collections/{coll}/vectors/batch",
                    json=self.create_test_vectors(10)
                ),
                "search": {"vector": [0.1] * 100, "k": 10}  # Wrong dimension
            }
        ]
        
        for engine in ["viper", "lsm"]:
            print(f"\n  {engine.upper()} engine:")
            
            for edge_case in edge_cases:
                collection_name = f"test_edge_{engine}_{edge_case['name']}"
                
                try:
                    # Create collection
                    response = requests.post(f"{BASE_URL}/collections", json={
                        "name": collection_name,
                        "dimension": VECTOR_DIM,
                        "distance_metric": "cosine",
                        "storage_engine": engine
                    })
                    if response.status_code != 200:
                        continue
                    self.collections_created.append(collection_name)
                    
                    # Setup edge case
                    edge_case['setup'](collection_name)
                    
                    # Test search
                    response = requests.post(
                        f"{BASE_URL}/collections/{collection_name}/search",
                        json=edge_case['search']
                    )
                    
                    status = "✓" if response.status_code in [200, 422] else "✗"
                    print(f"    {edge_case['description']:40}: {status} ({response.status_code})")
                    
                    self.test_results.append(TestResult(
                        test_name="edge_cases",
                        storage_engine=engine,
                        optimization_level=edge_case['name'],
                        dataset_size=0,
                        search_time_ms=0,
                        results_count=0,
                        success=response.status_code in [200, 422],
                        error=response.text if response.status_code >= 400 else None
                    ))
                
                except Exception as e:
                    print(f"    {edge_case['description']:40}: ✗ (Exception: {str(e)[:30]})")
    
    def test_search_accuracy(self):
        """Validate search result accuracy"""
        print("Validating search result accuracy...")
        
        for engine in ["viper", "lsm"]:
            collection_name = f"test_accuracy_{engine}"
            print(f"\n  {engine.upper()} engine:")
            
            try:
                # Create collection
                response = requests.post(f"{BASE_URL}/collections", json={
                    "name": collection_name,
                    "dimension": VECTOR_DIM,
                    "distance_metric": "cosine",
                    "storage_engine": engine
                })
                if response.status_code != 200:
                    continue
                self.collections_created.append(collection_name)
                
                # Insert known vectors
                test_vectors = []
                for i in range(100):
                    # Create orthogonal vectors for easy verification
                    vector = np.zeros(VECTOR_DIM, dtype=np.float32)
                    vector[i % VECTOR_DIM] = 1.0
                    test_vectors.append({
                        "id": f"ortho_{i}",
                        "vector": vector.tolist(),
                        "metadata": {"index": i}
                    })
                
                requests.post(
                    f"{BASE_URL}/collections/{collection_name}/vectors/batch",
                    json=test_vectors
                )
                
                # Test 1: Exact match search
                query_vector = test_vectors[0]["vector"]
                response = requests.post(
                    f"{BASE_URL}/collections/{collection_name}/search",
                    json={
                        "vector": query_vector,
                        "k": 1,
                        "search_hints": {"storage_aware": True}
                    }
                )
                
                if response.status_code == 200:
                    results = response.json().get('data', [])
                    exact_match = len(results) > 0 and results[0]['id'] == "ortho_0"
                    print(f"    Exact match test: {'✓' if exact_match else '✗'}")
                
                # Test 2: Distance ordering
                # Create a query vector that's a mix of first two basis vectors
                query_vector = np.zeros(VECTOR_DIM, dtype=np.float32)
                query_vector[0] = 0.7
                query_vector[1] = 0.3
                query_vector = query_vector / np.linalg.norm(query_vector)
                
                response = requests.post(
                    f"{BASE_URL}/collections/{collection_name}/search",
                    json={
                        "vector": query_vector.tolist(),
                        "k": 5,
                        "search_hints": {"storage_aware": True}
                    }
                )
                
                if response.status_code == 200:
                    results = response.json().get('data', [])
                    # Check if results are properly ordered by score
                    scores = [r.get('score', 0) for r in results]
                    properly_ordered = all(scores[i] >= scores[i+1] for i in range(len(scores)-1))
                    print(f"    Distance ordering test: {'✓' if properly_ordered else '✗'}")
                    
                    self.test_results.append(TestResult(
                        test_name="accuracy_validation",
                        storage_engine=engine,
                        optimization_level="standard",
                        dataset_size=100,
                        search_time_ms=0,
                        results_count=len(results),
                        accuracy=100.0 if exact_match and properly_ordered else 50.0,
                        success=True
                    ))
            
            except Exception as e:
                print(f"    ✗ Error: {e}")
    
    def print_comprehensive_report(self):
        """Print comprehensive test report"""
        print("\n" + "=" * 80)
        print("📊 COMPREHENSIVE TEST REPORT")
        print("=" * 80)
        
        # Group results by test type
        test_groups = {}
        for result in self.test_results:
            if result.test_name not in test_groups:
                test_groups[result.test_name] = []
            test_groups[result.test_name].append(result)
        
        # Performance comparison
        if "performance_comparison" in test_groups:
            print("\n🚀 Performance Comparison Results:")
            print("-" * 60)
            
            perf_results = test_groups["performance_comparison"]
            
            # Calculate speedups
            for size in TEST_SIZES:
                size_results = [r for r in perf_results if r.dataset_size == size]
                if not size_results:
                    continue
                    
                print(f"\n  Dataset size: {size} vectors")
                
                for engine in ["viper", "lsm"]:
                    engine_results = [r for r in size_results if r.storage_engine == engine]
                    if not engine_results:
                        continue
                    
                    baseline = next((r for r in engine_results if r.optimization_level == "baseline"), None)
                    high_opt = next((r for r in engine_results if r.optimization_level == "high_opt"), None)
                    
                    if baseline and high_opt and baseline.search_time_ms > 0:
                        speedup = baseline.search_time_ms / high_opt.search_time_ms
                        improvement = ((baseline.search_time_ms - high_opt.search_time_ms) / baseline.search_time_ms) * 100
                        
                        print(f"    {engine.upper():5}: {speedup:.2f}x speedup ({improvement:.1f}% faster)")
                        print(f"           Baseline: {baseline.search_time_ms:6.2f}ms → Optimized: {high_opt.search_time_ms:6.2f}ms")
        
        # Quantization impact
        if "quantization_test" in test_groups:
            print("\n🔬 Quantization Impact (VIPER):")
            print("-" * 60)
            
            quant_results = test_groups["quantization_test"]
            fp32_time = next((r.search_time_ms for r in quant_results if r.optimization_level == "FP32"), 0)
            
            print(f"  {'Level':8} | {'Time (ms)':>10} | {'Speedup':>8} | {'Accuracy':>8}")
            print(f"  {'-'*8} | {'-'*10} | {'-'*8} | {'-'*8}")
            
            for result in quant_results:
                speedup = fp32_time / result.search_time_ms if result.search_time_ms > 0 else 0
                print(f"  {result.optimization_level:8} | {result.search_time_ms:10.2f} | {speedup:8.2f}x | {result.accuracy:7.1f}%")
        
        # Concurrent operations
        if "concurrent_operations" in test_groups:
            print("\n⚡ Concurrent Operations Performance:")
            print("-" * 60)
            
            for result in test_groups["concurrent_operations"]:
                metadata = result.metadata or {}
                print(f"  {result.storage_engine.upper():5}: {metadata.get('throughput_qps', 0):.1f} QPS")
                print(f"         {metadata.get('successful_tasks', 0)}/{metadata.get('total_tasks', 0)} successful")
                print(f"         Avg latency: {result.search_time_ms:.2f}ms")
        
        # Overall statistics
        print("\n📈 Overall Statistics:")
        print("-" * 60)
        
        total_tests = len(self.test_results)
        successful_tests = sum(1 for r in self.test_results if r.success)
        success_rate = (successful_tests / total_tests * 100) if total_tests > 0 else 0
        
        print(f"  Total tests run: {total_tests}")
        print(f"  Successful tests: {successful_tests}")
        print(f"  Success rate: {success_rate:.1f}%")
        
        # Calculate average improvements
        viper_improvements = []
        lsm_improvements = []
        
        for size in TEST_SIZES:
            size_results = [r for r in self.test_results if r.test_name == "performance_comparison" and r.dataset_size == size]
            
            for engine in ["viper", "lsm"]:
                engine_results = [r for r in size_results if r.storage_engine == engine]
                baseline = next((r for r in engine_results if r.optimization_level == "baseline"), None)
                high_opt = next((r for r in engine_results if r.optimization_level == "high_opt"), None)
                
                if baseline and high_opt and baseline.search_time_ms > 0:
                    improvement = baseline.search_time_ms / high_opt.search_time_ms
                    if engine == "viper":
                        viper_improvements.append(improvement)
                    else:
                        lsm_improvements.append(improvement)
        
        if viper_improvements:
            print(f"\n  Average VIPER speedup: {mean(viper_improvements):.2f}x")
        if lsm_improvements:
            print(f"  Average LSM speedup: {mean(lsm_improvements):.2f}x")
        
        print("\n" + "=" * 80)
        
        # Save detailed results
        with open("storage_optimization_full_test_results.json", "w") as f:
            json.dump([{
                "test_name": r.test_name,
                "storage_engine": r.storage_engine,
                "optimization_level": r.optimization_level,
                "dataset_size": r.dataset_size,
                "search_time_ms": r.search_time_ms,
                "results_count": r.results_count,
                "accuracy": r.accuracy,
                "success": r.success,
                "error": r.error,
                "metadata": r.metadata
            } for r in self.test_results], f, indent=2)
        
        print("\n📄 Detailed results saved to: storage_optimization_full_test_results.json")

def main():
    """Run the comprehensive test suite"""
    # Check server availability
    try:
        response = requests.get(f"{BASE_URL}/health", timeout=2)
        if response.status_code != 200:
            print("❌ ProximaDB server not healthy")
            return 1
    except:
        print("❌ ProximaDB server not reachable on localhost:5678")
        print("   Please start the server first: cargo run --bin proximadb-server")
        return 1
    
    tester = StorageOptimizationTester()
    
    try:
        tester.run_all_tests()
        return 0
    except KeyboardInterrupt:
        print("\n\n🛑 Test interrupted by user")
        return 1
    except Exception as e:
        print(f"\n\n❌ Test suite failed: {e}")
        import traceback
        traceback.print_exc()
        return 1
    finally:
        tester.cleanup()

if __name__ == "__main__":
    sys.exit(main())