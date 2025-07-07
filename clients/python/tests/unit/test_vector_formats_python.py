#!/usr/bin/env python3
"""
Test vector insertion with both 1D and 2D array formats using ProximaDB Python SDK
Tests single vector insertion and bulk vector insertion comprehensively
"""

import sys
import time
import random
import numpy as np
from typing import List, Dict, Any
import asyncio

# Add the Python client to path
sys.path.insert(0, '../clients/python/src')

from proximadb.rest_client import ProximaDBRestClient
from proximadb.exceptions import ProximaDBError
from proximadb.models import CollectionConfig, DistanceMetric


class VectorFormatTester:
    """Test harness for vector insertion format testing"""
    
    def __init__(self, base_url: str = "http://localhost:5678"):
        self.client = ProximaDBRestClient(url=base_url)
        self.collection_name = f"test_vec_format_{int(time.time())}"
        self.dimension = 4  # Small dimension for testing
        self.passed_tests = 0
        self.failed_tests = 0
        
    def generate_vector(self, dim: int) -> List[float]:
        """Generate random vector of given dimension"""
        return [random.random() for _ in range(dim)]
    
    def generate_vectors(self, count: int, dim: int) -> List[List[float]]:
        """Generate multiple random vectors"""
        return [self.generate_vector(dim) for _ in range(count)]
    
    def print_result(self, test_name: str, success: bool, details: str = ""):
        """Print test result with color"""
        if success:
            print(f"✅ {test_name} - PASSED {details}")
            self.passed_tests += 1
        else:
            print(f"❌ {test_name} - FAILED {details}")
            self.failed_tests += 1
    
    def setup(self):
        """Create test collection"""
        print(f"\n🔧 Setting up test collection: {self.collection_name}")
        try:
            config = CollectionConfig(
                dimension=self.dimension,
                distance_metric=DistanceMetric.COSINE,
                storage_layout="viper"
            )
            result = self.client.create_collection(self.collection_name, config)
            self.print_result("Collection creation", True)
            return True
        except Exception as e:
            self.print_result("Collection creation", False, str(e))
            return False
    
    def test_single_vector_insertion(self):
        """Test 1D array single vector insertion"""
        print("\n📝 Testing single vector insertion (1D array)...")
        
        try:
            # Test 1: Single vector with ID
            vector = self.generate_vector(self.dimension)
            result = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=[vector],
                ids=["single_001"],
                metadata=[{"type": "single", "test": "1D"}]
            )
            self.print_result("Single vector with ID", hasattr(result, 'successful_count') and result.successful_count == 1 or hasattr(result, 'count') and result.count == 1)
            
            # Test 2: Single vector without ID (auto-generate)
            vector2 = self.generate_vector(self.dimension)
            result2 = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=[vector2],
                metadata=[{"type": "single_auto", "test": "1D"}]
            )
            self.print_result("Single vector auto-ID", hasattr(result2, 'successful_count') and result2.successful_count == 1 or hasattr(result2, 'count') and result2.count == 1)
            
            # Test 3: Single vector with numpy array
            np_vector = np.random.rand(self.dimension).astype(np.float32)
            result3 = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=[np_vector.tolist()],
                ids=["single_numpy"],
                metadata=[{"type": "numpy", "test": "1D"}]
            )
            self.print_result("Single vector numpy array", hasattr(result3, 'successful_count') and result3.successful_count == 1 or hasattr(result3, 'count') and result3.count == 1)
            
        except Exception as e:
            self.print_result("Single vector insertion", False, str(e))
    
    def test_bulk_vector_insertion(self):
        """Test 2D array bulk vector insertion"""
        print("\n📦 Testing bulk vector insertion (2D array)...")
        
        try:
            # Test 1: Bulk vectors with IDs
            vectors = self.generate_vectors(5, self.dimension)
            ids = [f"bulk_{i:03d}" for i in range(5)]
            metadata = [{"type": "bulk", "index": i} for i in range(5)]
            
            result = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=vectors,
                ids=ids,
                metadata=metadata
            )
            self.print_result("Bulk vectors with IDs", hasattr(result, 'successful_count') and result.successful_count == 5 or hasattr(result, 'count') and result.count == 5)
            
            # Test 2: Bulk vectors without IDs (auto-generate)
            vectors2 = self.generate_vectors(3, self.dimension)
            result2 = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=vectors2,
                metadata=[{"type": "bulk_auto", "idx": i} for i in range(3)]
            )
            self.print_result("Bulk vectors auto-ID", hasattr(result2, 'successful_count') and result2.successful_count == 3 or hasattr(result2, 'count') and result2.count == 3)
            
            # Test 3: Bulk vectors with numpy array
            np_vectors = np.random.rand(4, self.dimension).astype(np.float32)
            ids3 = [f"bulk_np_{i}" for i in range(4)]
            result3 = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=np_vectors.tolist(),
                ids=ids3
            )
            self.print_result("Bulk vectors numpy array", hasattr(result3, 'successful_count') and result3.successful_count == 4 or hasattr(result3, 'count') and result3.count == 4)
            
            # Test 4: Large batch
            large_vectors = self.generate_vectors(100, self.dimension)
            large_ids = [f"large_{i:04d}" for i in range(100)]
            result4 = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=large_vectors,
                ids=large_ids
            )
            self.print_result("Large batch (100 vectors)", hasattr(result4, 'successful_count') and result4.successful_count == 100 or hasattr(result4, 'count') and result4.count == 100)
            
        except Exception as e:
            self.print_result("Bulk vector insertion", False, str(e))
    
    def test_edge_cases(self):
        """Test edge cases and error handling"""
        print("\n⚠️  Testing edge cases...")
        
        # Test 1: Empty vector
        try:
            result = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=[[]],
                ids=["empty_vec"]
            )
            self.print_result("Empty vector rejection", False, "Should have failed")
        except Exception:
            self.print_result("Empty vector rejection", True)
        
        # Test 2: Mismatched dimensions in bulk
        try:
            bad_vectors = [[0.1, 0.2], [0.3, 0.4, 0.5]]  # Different dimensions
            result = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=bad_vectors,
                ids=["bad1", "bad2"]
            )
            self.print_result("Mismatched dimensions", False, "Should have failed")
        except Exception:
            self.print_result("Mismatched dimensions", True)
        
        # Test 3: Wrong dimension vector
        try:
            wrong_dim_vector = [0.1, 0.2]  # Should be 4 dimensions
            result = self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=[wrong_dim_vector],
                ids=["wrong_dim"]
            )
            self.print_result("Both formats rejection", False, "Should have failed")
        except Exception as e:
            self.print_result("Both formats rejection", True, "Correctly rejected")
    
    def test_search_verification(self):
        """Verify inserted vectors through search"""
        print("\n🔍 Verifying insertions with search...")
        
        try:
            # Search with a query vector
            query_vector = self.generate_vector(self.dimension)
            results = self.client.search(
                collection_id=self.collection_name,
                query=query_vector,
                k=10,
                include_vectors=False,
                include_metadata=True
            )
            
            # The search method returns List[SearchResult] directly
            result_list = results if isinstance(results, list) else []
            
            found_single = any(hasattr(r, 'metadata') and r.metadata and r.metadata.get("type") == "single" for r in result_list)
            found_bulk = any(hasattr(r, 'metadata') and r.metadata and r.metadata.get("type") == "bulk" for r in result_list)
            
            self.print_result("Search found single vectors", found_single)
            self.print_result("Search found bulk vectors", found_bulk)
            self.print_result("Search overall", len(result_list) > 0, 
                            f"Found {len(result_list)} results")
            
        except Exception as e:
            self.print_result("Search verification", False, str(e))
    
    def test_performance(self):
        """Test performance of different insertion methods"""
        print("\n⚡ Testing performance...")
        
        # Generate test data
        single_vectors = self.generate_vectors(100, self.dimension)
        bulk_vectors = self.generate_vectors(100, self.dimension)
        
        # Test single vector insertion performance
        start_time = time.time()
        for i, vec in enumerate(single_vectors):
            self.client.insert_vector(
                collection_id=self.collection_name,
                vectors=[vec],
                ids=[f"perf_single_{i}"]
            )
        single_time = time.time() - start_time
        
        # Test bulk vector insertion performance
        start_time = time.time()
        bulk_ids = [f"perf_bulk_{i}" for i in range(100)]
        self.client.insert_vector(
            collection_id=self.collection_name,
            vectors=bulk_vectors,
            ids=bulk_ids
        )
        bulk_time = time.time() - start_time
        
        speedup = single_time / bulk_time
        self.print_result("Performance test", True, 
                        f"Single: {single_time:.2f}s, Bulk: {bulk_time:.2f}s, "
                        f"Speedup: {speedup:.1f}x")
    
    def cleanup(self):
        """Clean up test collection"""
        print("\n🧹 Cleaning up...")
        try:
            self.client.delete_collection(self.collection_name)
            self.print_result("Cleanup", True)
        except Exception as e:
            self.print_result("Cleanup", False, str(e))
    
    def run_all_tests(self):
        """Run all tests"""
        print("=" * 60)
        print(f"🚀 ProximaDB Vector Format Tests")
        print(f"📍 Server: {self.client.config.url}")
        print(f"📊 Collection: {self.collection_name}")
        print("=" * 60)
        
        if not self.setup():
            print("❌ Setup failed, cannot continue")
            return
        
        # Run all test suites
        self.test_single_vector_insertion()
        self.test_bulk_vector_insertion()
        self.test_edge_cases()
        self.test_search_verification()
        self.test_performance()
        
        # Clean up
        self.cleanup()
        
        # Summary
        print("\n" + "=" * 60)
        print(f"📊 Test Summary:")
        print(f"   ✅ Passed: {self.passed_tests}")
        print(f"   ❌ Failed: {self.failed_tests}")
        print(f"   📈 Success Rate: {self.passed_tests/(self.passed_tests+self.failed_tests)*100:.1f}%")
        print("=" * 60)
        
        # Return exit code
        return 0 if self.failed_tests == 0 else 1


def main():
    """Main entry point"""
    # Check if custom URL provided
    base_url = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:5678"
    
    tester = VectorFormatTester(base_url)
    exit_code = tester.run_all_tests()
    sys.exit(exit_code)


if __name__ == "__main__":
    main()