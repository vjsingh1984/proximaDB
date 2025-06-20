#!/usr/bin/env python3
"""
ProximaDB gRPC Vectors Comprehensive Test
Tests vector insert, search, and mutation operations across all configurations
with 128D BERT-like embeddings and various metadata configurations.
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
from typing import List, Dict, Any, Tuple

# Add the client directory to Python path
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

import proximadb.proximadb_pb2 as proximadb_pb2
import proximadb.proximadb_pb2_grpc as proximadb_pb2_grpc

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

class VectorTestConfig:
    """Configuration for vector test combinations"""
    
    # Test subset of configurations for comprehensive vector testing (only working storage engines)
    TEST_CONFIGS = [
        # Viper + HNSW combinations (most common)
        {"storage": "Viper", "indexing": "HNSW", "distance": "Cosine", "metadata": "none"},
        {"storage": "Viper", "indexing": "HNSW", "distance": "Cosine", "metadata": "basic"},
        {"storage": "Viper", "indexing": "HNSW", "distance": "Euclidean", "metadata": "extended"},
        {"storage": "Viper", "indexing": "HNSW", "distance": "DotProduct", "metadata": "extensive"},
        
        # Viper + Flat combinations (for exact search)
        {"storage": "Viper", "indexing": "Flat", "distance": "Cosine", "metadata": "none"},
        {"storage": "Viper", "indexing": "Flat", "distance": "Euclidean", "metadata": "basic"},
        
        # Standard storage combinations
        {"storage": "Standard", "indexing": "HNSW", "distance": "Cosine", "metadata": "none"},
        {"storage": "Standard", "indexing": "Flat", "distance": "Euclidean", "metadata": "extended"},
        
        # Other indexing algorithms with working storage
        {"storage": "Viper", "indexing": "IVF", "distance": "Cosine", "metadata": "basic"},
        {"storage": "Standard", "indexing": "Annoy", "distance": "Euclidean", "metadata": "none"},
    ]
    
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
        "extensive": [
            "category", "source", "priority", "created_at", "author", "version",
            "tags", "department", "project", "environment", "region", "tenant",
            "classification", "sensitivity", "expires_at", "last_modified",
            "owner", "cost_center", "compliance_level", "data_lineage", "quality_score",
            "retention_policy", "access_level", "encryption_key", "backup_policy",
            "monitoring_enabled", "alerting_config", "performance_tier", "geo_location",
            "business_unit", "application_id", "service_tier", "data_format"
        ]
    }

class VectorTestSuite:
    """Comprehensive vector operations test suite"""
    
    def __init__(self):
        self.stub = None
        self.server_process = None
        self.test_results = []
        self.dimension = 128
        self.vectors_per_collection = 10
        
    async def setup_server(self):
        """Start ProximaDB server"""
        logger.info("🚀 Starting ProximaDB server...")
        
        # Cleanup old data
        data_dirs = ["/home/vsingh/code/proximadb/data", "/home/vsingh/code/proximadb/certs/data"]
        for data_dir in data_dirs:
            if os.path.exists(data_dir):
                subprocess.run(["rm", "-rf", data_dir], check=False)
                logger.info(f"🧹 Cleaned up: {data_dir}")
        
        # Start server
        os.chdir("/home/vsingh/code/proximadb")
        cmd = ["cargo", "run", "--bin", "proximadb-server"]
        self.server_process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        
        # Wait for server startup
        await asyncio.sleep(12)
        
        # Connect client
        channel = grpc.aio.insecure_channel("localhost:5679")
        self.stub = proximadb_pb2_grpc.ProximaDBStub(channel)
        
        # Health check
        health_request = proximadb_pb2.HealthRequest()
        health_response = await self.stub.Health(health_request)
        logger.info(f"✅ Health check: {health_response.status}")
        
    async def teardown_server(self):
        """Stop ProximaDB server"""
        if self.server_process:
            self.server_process.terminate()
            self.server_process.wait()
            logger.info("✅ Server stopped")
    
    def generate_bert_like_vectors(self, count: int) -> List[List[float]]:
        """Generate BERT-like 128D vectors with realistic distributions"""
        np.random.seed(42)  # For reproducible tests
        
        vectors = []
        for i in range(count):
            # Generate BERT-like embeddings: mostly small values with some larger ones
            vector = np.random.normal(0, 0.1, self.dimension)  # Small standard deviation
            
            # Add some larger values in random positions (like BERT attention patterns)
            num_large = np.random.randint(5, 15)
            large_indices = np.random.choice(self.dimension, num_large, replace=False)
            vector[large_indices] += np.random.normal(0, 0.3, num_large)
            
            # Normalize to unit vector (common for embeddings)
            vector = vector / np.linalg.norm(vector)
            
            vectors.append(vector.tolist())
        
        return vectors
    
    def generate_metadata(self, vector_id: str, metadata_fields: List[str]) -> Dict[str, str]:
        """Generate realistic metadata for a vector"""
        metadata = {}
        
        for field in metadata_fields:
            if field == "category":
                metadata[field] = np.random.choice(["document", "image", "audio", "video", "text"])
            elif field == "source":
                metadata[field] = np.random.choice(["web", "database", "file", "api", "stream"])
            elif field == "priority":
                metadata[field] = str(np.random.choice([1, 2, 3, 4, 5]))
            elif field == "created_at":
                metadata[field] = str(int(time.time()) - np.random.randint(0, 86400*30))  # Last 30 days
            elif field == "author":
                metadata[field] = np.random.choice(["alice", "bob", "charlie", "diana", "eve"])
            elif field == "version":
                metadata[field] = f"v{np.random.randint(1, 10)}.{np.random.randint(0, 10)}"
            elif field == "department":
                metadata[field] = np.random.choice(["engineering", "marketing", "sales", "support"])
            elif field == "environment":
                metadata[field] = np.random.choice(["prod", "staging", "dev", "test"])
            elif field == "region":
                metadata[field] = np.random.choice(["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-1"])
            elif field == "tenant":
                metadata[field] = f"tenant_{np.random.randint(1, 100)}"
            else:
                # Default random string for other fields
                metadata[field] = f"{field}_{np.random.randint(1, 1000)}"
        
        return metadata
    
    async def create_collection(self, config: Dict[str, str]) -> Tuple[bool, str]:
        """Create a collection for testing"""
        collection_name = f"vectors_{config['storage'].lower()}_{config['indexing'].lower()}_{config['distance'].lower()}_{config['metadata']}"
        metadata_fields = VectorTestConfig.METADATA_CONFIGS[config['metadata']]
        
        try:
            collection_config = proximadb_pb2.CollectionConfig(
                name=collection_name,
                dimension=self.dimension,
                distance_metric=VectorTestConfig.DISTANCE_METRICS[config['distance']],
                storage_engine=VectorTestConfig.STORAGE_ENGINES[config['storage']],
                indexing_algorithm=VectorTestConfig.INDEXING_ALGORITHMS[config['indexing']],
                filterable_metadata_fields=metadata_fields,
                indexing_config={}
            )
            
            create_request = proximadb_pb2.CollectionRequest(
                operation=1,  # CREATE
                collection_config=collection_config
            )
            
            response = await self.stub.CollectionOperation(create_request)
            
            if response.success:
                logger.info(f"✅ Created collection: {collection_name}")
                return True, collection_name
            else:
                logger.error(f"❌ Failed to create collection {collection_name}: {response.error_message}")
                return False, collection_name
                
        except Exception as e:
            logger.error(f"❌ Exception creating collection {collection_name}: {e}")
            return False, collection_name
    
    async def insert_vectors(self, collection_name: str, metadata_fields: List[str]) -> Tuple[bool, List[str]]:
        """Insert vectors into collection using new separated schema"""
        try:
            # Generate vectors and metadata
            vectors = self.generate_bert_like_vectors(self.vectors_per_collection)
            vector_ids = []
            
            # Prepare vector data for new schema (only vector data in payload)
            vectors_data = []
            for i, vector in enumerate(vectors):
                vector_id = f"{collection_name}_vector_{i:03d}"
                vector_ids.append(vector_id)
                
                metadata = self.generate_metadata(vector_id, metadata_fields)
                
                vector_record = {
                    "id": vector_id,
                    "vector": vector,
                    "metadata": metadata,
                    "timestamp": int(time.time()),
                    "version": 1
                }
                vectors_data.append(vector_record)
            
            # Serialize vector data only (new schema format)
            vectors_json = json.dumps(vectors_data).encode('utf-8')
            
            # Use new separated schema format
            request = proximadb_pb2.VectorInsertRequest(
                collection_id=collection_name,        # gRPC field (metadata)
                upsert_mode=False,                   # gRPC field (metadata)
                vectors_avro_payload=vectors_json   # Only vector data
            )
            
            response = await self.stub.VectorInsert(request)
            
            if response.success:
                logger.info(f"✅ Inserted {len(vectors)} vectors into {collection_name}")
                return True, vector_ids
            else:
                logger.error(f"❌ Failed to insert vectors into {collection_name}: {response.error_message}")
                return False, vector_ids
                
        except Exception as e:
            logger.error(f"❌ Exception inserting vectors into {collection_name}: {e}")
            return False, []
    
    async def search_vectors(self, collection_name: str, distance_metric: str, 
                           test_metadata_filters: bool = False) -> Tuple[bool, Dict[str, Any]]:
        """Search vectors with similarity, metadata filters, and ID lookups"""
        try:
            # Generate query vector (similar to inserted vectors)
            query_vector = self.generate_bert_like_vectors(1)[0]
            
            # Test 1: Basic similarity search
            search_query = proximadb_pb2.SearchQuery(vector=query_vector)
            search_request = proximadb_pb2.VectorSearchRequest(
                collection_id=collection_name,
                queries=[search_query],
                top_k=5,
                distance_metric_override=VectorTestConfig.DISTANCE_METRICS[distance_metric],
                include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
            )
            
            response = await self.stub.VectorSearch(search_request)
            
            search_results = {
                "basic_similarity": {"success": False, "result_count": 0},
                "with_metadata_filter": {"success": False, "result_count": 0},
                "vector_quality": {"avg_score": 0.0, "score_range": [0.0, 0.0]}
            }
            
            if response.success:
                # Parse results
                result_count = 0
                scores = []
                
                if hasattr(response, 'result_payload') and response.result_payload:
                    if hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results:
                        results = response.result_payload.compact_results.results
                        result_count = len(results)
                        scores = [result.score for result in results]
                        
                        search_results["basic_similarity"] = {
                            "success": True,
                            "result_count": result_count
                        }
                        
                        if scores:
                            search_results["vector_quality"] = {
                                "avg_score": float(np.mean(scores)),
                                "score_range": [float(min(scores)), float(max(scores))]
                            }
                
                logger.info(f"✅ Basic search in {collection_name}: found {result_count} results")
                
                # Test 2: Metadata filter search (if metadata available)
                if test_metadata_filters:
                    filter_query = proximadb_pb2.SearchQuery(
                        vector=query_vector,
                        metadata_filter={"category": "document"}  # Filter for documents
                    )
                    filter_request = proximadb_pb2.VectorSearchRequest(
                        collection_id=collection_name,
                        queries=[filter_query],
                        top_k=3,
                        distance_metric_override=VectorTestConfig.DISTANCE_METRICS[distance_metric],
                        include_fields=proximadb_pb2.IncludeFields(vector=False, metadata=True)
                    )
                    
                    filter_response = await self.stub.VectorSearch(filter_request)
                    
                    if filter_response.success:
                        filter_count = 0
                        if (hasattr(filter_response, 'result_payload') and filter_response.result_payload and
                            hasattr(filter_response.result_payload, 'compact_results') and filter_response.result_payload.compact_results):
                            filter_count = len(filter_response.result_payload.compact_results.results)
                        
                        search_results["with_metadata_filter"] = {
                            "success": True,
                            "result_count": filter_count
                        }
                        logger.info(f"✅ Metadata filter search in {collection_name}: found {filter_count} results")
                
                return True, search_results
            else:
                logger.error(f"❌ Search failed in {collection_name}: {response.error_message}")
                return False, search_results
                
        except Exception as e:
            logger.error(f"❌ Exception searching in {collection_name}: {e}")
            return False, {"error": str(e)}
    
    async def test_vector_operations(self, config: Dict[str, str]) -> Dict[str, Any]:
        """Test complete vector operations lifecycle for a configuration"""
        
        metadata_fields = VectorTestConfig.METADATA_CONFIGS[config['metadata']]
        
        result = {
            "configuration": config,
            "metadata_field_count": len(metadata_fields),
            "collection_creation": {"success": False},
            "vector_insertion": {"success": False, "vector_count": 0},
            "vector_search": {"success": False},
            "performance_metrics": {},
            "error_messages": []
        }
        
        logger.info(f"🧪 Testing vectors: {config['storage']}/{config['indexing']}/{config['distance']}/{config['metadata']} ({len(metadata_fields)} fields)")
        
        collection_name = ""
        
        try:
            # Step 1: Create collection
            start_time = time.time()
            create_success, collection_name = await self.create_collection(config)
            create_time = time.time() - start_time
            
            result["collection_creation"] = {
                "success": create_success,
                "time_seconds": create_time
            }
            
            if not create_success:
                result["error_messages"].append("Collection creation failed")
                return result
            
            # Step 2: Insert vectors
            start_time = time.time()
            insert_success, vector_ids = await self.insert_vectors(collection_name, metadata_fields)
            insert_time = time.time() - start_time
            
            result["vector_insertion"] = {
                "success": insert_success,
                "vector_count": len(vector_ids),
                "time_seconds": insert_time,
                "throughput_vectors_per_sec": len(vector_ids) / insert_time if insert_time > 0 else 0
            }
            
            if not insert_success:
                result["error_messages"].append("Vector insertion failed")
                return result
            
            # Wait for vectors to be processed
            await asyncio.sleep(2)
            
            # Step 3: Search vectors
            start_time = time.time()
            search_success, search_results = await self.search_vectors(
                collection_name, config['distance'], 
                test_metadata_filters=(len(metadata_fields) > 0)
            )
            search_time = time.time() - start_time
            
            result["vector_search"] = {
                "success": search_success,
                "time_seconds": search_time,
                "results": search_results
            }
            
            if not search_success:
                result["error_messages"].append("Vector search failed")
            
            # Performance summary
            result["performance_metrics"] = {
                "total_time_seconds": create_time + insert_time + search_time,
                "insert_throughput": result["vector_insertion"]["throughput_vectors_per_sec"],
                "search_latency_ms": search_time * 1000,
                "vectors_per_collection": self.vectors_per_collection
            }
            
        except Exception as e:
            result["error_messages"].append(f"Exception during test: {str(e)}")
            logger.error(f"❌ Exception in vector test: {e}")
        
        finally:
            # Cleanup: Delete collection
            if collection_name:
                try:
                    delete_request = proximadb_pb2.CollectionRequest(
                        operation=4,  # DELETE
                        collection_name=collection_name
                    )
                    await self.stub.CollectionOperation(delete_request)
                except Exception as e:
                    logger.warning(f"⚠️ Failed to cleanup collection {collection_name}: {e}")
        
        return result
    
    async def run_comprehensive_tests(self):
        """Run vector tests for all configurations"""
        logger.info("🚀 Starting comprehensive vector tests...")
        logger.info(f"📊 Testing {len(VectorTestConfig.TEST_CONFIGS)} configurations with {self.vectors_per_collection} vectors each...")
        
        test_count = 0
        successful_tests = 0
        
        for config in VectorTestConfig.TEST_CONFIGS:
            test_count += 1
            
            logger.info(f"🧪 Test {test_count}/{len(VectorTestConfig.TEST_CONFIGS)}: {config}")
            
            result = await self.test_vector_operations(config)
            self.test_results.append(result)
            
            # Check if test passed
            if (result["collection_creation"]["success"] and 
                result["vector_insertion"]["success"] and 
                result["vector_search"]["success"] and 
                not result["error_messages"]):
                successful_tests += 1
                logger.info(f"✅ Test {test_count} PASSED")
            else:
                logger.error(f"❌ Test {test_count} FAILED: {result['error_messages']}")
            
            # Small delay between tests
            await asyncio.sleep(1)
        
        # Generate summary
        logger.info(f"📊 Test Summary: {successful_tests}/{len(VectorTestConfig.TEST_CONFIGS)} tests passed ({successful_tests/len(VectorTestConfig.TEST_CONFIGS)*100:.1f}%)")
        
        return successful_tests == len(VectorTestConfig.TEST_CONFIGS)
    
    def generate_report(self):
        """Generate detailed test report"""
        report = {
            "test_summary": {
                "total_tests": len(self.test_results),
                "successful_tests": sum(1 for r in self.test_results if not r["error_messages"]),
                "failed_tests": sum(1 for r in self.test_results if r["error_messages"]),
                "success_rate": 0
            },
            "performance_summary": {
                "avg_insert_throughput": 0,
                "avg_search_latency_ms": 0,
                "total_vectors_tested": 0
            },
            "results_by_configuration": [],
            "failure_analysis": {}
        }
        
        if report["test_summary"]["total_tests"] > 0:
            report["test_summary"]["success_rate"] = (
                report["test_summary"]["successful_tests"] / report["test_summary"]["total_tests"] * 100
            )
        
        # Calculate performance metrics
        throughputs = []
        latencies = []
        total_vectors = 0
        
        for result in self.test_results:
            if result["vector_insertion"]["success"]:
                throughputs.append(result["performance_metrics"]["insert_throughput"])
                total_vectors += result["vector_insertion"]["vector_count"]
            
            if result["vector_search"]["success"]:
                latencies.append(result["performance_metrics"]["search_latency_ms"])
            
            # Store configuration result
            config_summary = {
                "configuration": result["configuration"],
                "success": not bool(result["error_messages"]),
                "performance": result["performance_metrics"],
                "errors": result["error_messages"]
            }
            report["results_by_configuration"].append(config_summary)
            
            # Track failures
            if result["error_messages"]:
                config_key = f"{result['configuration']['storage']}/{result['configuration']['indexing']}/{result['configuration']['distance']}/{result['configuration']['metadata']}"
                report["failure_analysis"][config_key] = result["error_messages"]
        
        if throughputs:
            report["performance_summary"]["avg_insert_throughput"] = sum(throughputs) / len(throughputs)
        if latencies:
            report["performance_summary"]["avg_search_latency_ms"] = sum(latencies) / len(latencies)
        report["performance_summary"]["total_vectors_tested"] = total_vectors
        
        # Save report to file
        with open("/home/vsingh/code/proximadb/vector_test_report.json", "w") as f:
            json.dump(report, f, indent=2)
        
        logger.info("📄 Test report saved to vector_test_report.json")
        
        return report

async def main():
    """Main test execution"""
    test_suite = VectorTestSuite()
    
    try:
        # Setup
        await test_suite.setup_server()
        
        # Run tests
        all_passed = await test_suite.run_comprehensive_tests()
        
        # Generate report
        report = test_suite.generate_report()
        
        # Print summary
        print("\n" + "="*80)
        print("📊 VECTOR OPERATIONS TEST SUMMARY")
        print("="*80)
        print(f"Total Tests: {report['test_summary']['total_tests']}")
        print(f"Successful: {report['test_summary']['successful_tests']}")
        print(f"Failed: {report['test_summary']['failed_tests']}")
        print(f"Success Rate: {report['test_summary']['success_rate']:.1f}%")
        print(f"Total Vectors Tested: {report['performance_summary']['total_vectors_tested']}")
        print(f"Avg Insert Throughput: {report['performance_summary']['avg_insert_throughput']:.1f} vectors/sec")
        print(f"Avg Search Latency: {report['performance_summary']['avg_search_latency_ms']:.1f} ms")
        
        if report["failure_analysis"]:
            print("\n❌ FAILED CONFIGURATIONS:")
            for config, errors in report["failure_analysis"].items():
                print(f"  {config}: {', '.join(errors)}")
        
        print("="*80)
        
        if all_passed:
            print("✅ All vector operation tests passed!")
        else:
            print("❌ Some vector operation tests failed!")
        
        return all_passed
        
    except Exception as e:
        logger.error(f"❌ Test suite failed: {e}")
        return False
    finally:
        await test_suite.teardown_server()

if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)