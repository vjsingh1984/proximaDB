#!/usr/bin/env python3
"""
ProximaDB gRPC Collections Comprehensive Test
Tests all combinations of storage engines, indexing algorithms, distance metrics,
and metadata configurations for collection management.
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
from typing import List, Dict, Any

# Add the client directory to Python path
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

import proximadb.proximadb_pb2 as proximadb_pb2
import proximadb.proximadb_pb2_grpc as proximadb_pb2_grpc

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

class CollectionTestConfig:
    """Configuration for collection test combinations"""
    
    # Storage engines (only working configurations)
    STORAGE_ENGINES = {
        "Viper": 1,
        "Standard": 2
    }
    
    # Indexing algorithms
    INDEXING_ALGORITHMS = {
        "HNSW": 1,
        "IVF": 2,
        "PQ": 3,
        "Flat": 4,
        "Annoy": 5
    }
    
    # Distance metrics
    DISTANCE_METRICS = {
        "Cosine": 1,
        "Euclidean": 2,
        "DotProduct": 3,
        "Hamming": 4
    }
    
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

class CollectionTestSuite:
    """Comprehensive collection test suite"""
    
    def __init__(self):
        self.stub = None
        self.server_process = None
        self.test_results = []
        self.dimension = 128
        
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
        
        # Wait for server startup (longer for compilation)
        await asyncio.sleep(20)
        
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
    
    async def create_collection(self, name: str, storage_engine: str, indexing_algorithm: str, 
                              distance_metric: str, metadata_fields: List[str]) -> bool:
        """Create a collection with specified configuration"""
        try:
            collection_config = proximadb_pb2.CollectionConfig(
                name=name,
                dimension=self.dimension,
                distance_metric=CollectionTestConfig.DISTANCE_METRICS[distance_metric],
                storage_engine=CollectionTestConfig.STORAGE_ENGINES[storage_engine],
                indexing_algorithm=CollectionTestConfig.INDEXING_ALGORITHMS[indexing_algorithm],
                filterable_metadata_fields=metadata_fields,
                indexing_config={}
            )
            
            create_request = proximadb_pb2.CollectionRequest(
                operation=1,  # CREATE
                collection_config=collection_config
            )
            
            response = await self.stub.CollectionOperation(create_request)
            
            if response.success:
                logger.info(f"✅ Created collection: {name}")
                return True
            else:
                logger.error(f"❌ Failed to create collection {name}: {response.error_message}")
                return False
                
        except Exception as e:
            logger.error(f"❌ Exception creating collection {name}: {e}")
            return False
    
    async def list_collections(self) -> List[str]:
        """List all collections"""
        try:
            list_request = proximadb_pb2.CollectionRequest(
                operation=2  # LIST
            )
            
            response = await self.stub.CollectionOperation(list_request)
            
            if response.success:
                collection_names = [col.name for col in response.collections]
                logger.info(f"📋 Listed {len(collection_names)} collections")
                return collection_names
            else:
                logger.error(f"❌ Failed to list collections: {response.error_message}")
                return []
                
        except Exception as e:
            logger.error(f"❌ Exception listing collections: {e}")
            return []
    
    async def get_collection_info(self, name: str) -> Dict[str, Any]:
        """Get collection information by name"""
        try:
            get_request = proximadb_pb2.CollectionRequest(
                operation=3,  # GET
                collection_name=name
            )
            
            response = await self.stub.CollectionOperation(get_request)
            
            if response.success and response.collection:
                collection = response.collection
                info = {
                    "name": collection.name,
                    "dimension": collection.dimension,
                    "storage_engine": collection.storage_engine,
                    "indexing_algorithm": collection.indexing_algorithm,
                    "distance_metric": collection.distance_metric,
                    "filterable_metadata_fields": list(collection.filterable_metadata_fields),
                    "vector_count": collection.vector_count,
                    "created_at": collection.created_at
                }
                logger.info(f"📊 Got info for collection: {name}")
                return info
            else:
                logger.error(f"❌ Failed to get collection info {name}: {response.error_message}")
                return {}
                
        except Exception as e:
            logger.error(f"❌ Exception getting collection info {name}: {e}")
            return {}
    
    async def delete_collection(self, name: str) -> bool:
        """Delete collection by name"""
        try:
            delete_request = proximadb_pb2.CollectionRequest(
                operation=4,  # DELETE
                collection_name=name
            )
            
            response = await self.stub.CollectionOperation(delete_request)
            
            if response.success:
                logger.info(f"🗑️ Deleted collection: {name}")
                return True
            else:
                logger.error(f"❌ Failed to delete collection {name}: {response.error_message}")
                return False
                
        except Exception as e:
            logger.error(f"❌ Exception deleting collection {name}: {e}")
            return False
    
    async def test_collection_lifecycle(self, storage_engine: str, indexing_algorithm: str, 
                                      distance_metric: str, metadata_config: str) -> Dict[str, Any]:
        """Test complete collection lifecycle for a configuration"""
        
        metadata_fields = CollectionTestConfig.METADATA_CONFIGS[metadata_config]
        collection_name = f"test_{storage_engine.lower()}_{indexing_algorithm.lower()}_{distance_metric.lower()}_{metadata_config}"
        
        result = {
            "storage_engine": storage_engine,
            "indexing_algorithm": indexing_algorithm,
            "distance_metric": distance_metric,
            "metadata_config": metadata_config,
            "metadata_field_count": len(metadata_fields),
            "collection_name": collection_name,
            "create_success": False,
            "list_success": False,
            "get_info_success": False,
            "delete_success": False,
            "error_messages": []
        }
        
        logger.info(f"🧪 Testing: {storage_engine}/{indexing_algorithm}/{distance_metric}/{metadata_config} ({len(metadata_fields)} fields)")
        
        try:
            # Step 1: Create collection
            result["create_success"] = await self.create_collection(
                collection_name, storage_engine, indexing_algorithm, distance_metric, metadata_fields
            )
            
            if not result["create_success"]:
                result["error_messages"].append("Collection creation failed")
                return result
            
            # Step 2: List collections (verify it appears)
            collections = await self.list_collections()
            result["list_success"] = collection_name in collections
            
            if not result["list_success"]:
                result["error_messages"].append(f"Collection {collection_name} not found in list")
            
            # Step 3: Get collection info
            info = await self.get_collection_info(collection_name)
            result["get_info_success"] = bool(info)
            result["collection_info"] = info
            
            if not result["get_info_success"]:
                result["error_messages"].append("Failed to get collection info")
            else:
                # Verify configuration matches
                if info.get("dimension") != self.dimension:
                    result["error_messages"].append(f"Dimension mismatch: expected {self.dimension}, got {info.get('dimension')}")
                if len(info.get("filterable_metadata_fields", [])) != len(metadata_fields):
                    result["error_messages"].append(f"Metadata fields mismatch: expected {len(metadata_fields)}, got {len(info.get('filterable_metadata_fields', []))}")
            
            # Step 4: Delete collection
            result["delete_success"] = await self.delete_collection(collection_name)
            
            if not result["delete_success"]:
                result["error_messages"].append("Collection deletion failed")
            
            # Step 5: Verify deletion (list again)
            collections_after = await self.list_collections()
            if collection_name in collections_after:
                result["error_messages"].append(f"Collection {collection_name} still exists after deletion")
                result["delete_success"] = False
            
        except Exception as e:
            result["error_messages"].append(f"Exception during test: {str(e)}")
            logger.error(f"❌ Exception in test: {e}")
        
        return result
    
    async def run_comprehensive_tests(self):
        """Run all combinations of collection configurations"""
        logger.info("🚀 Starting comprehensive collection tests...")
        
        total_combinations = (
            len(CollectionTestConfig.STORAGE_ENGINES) * 
            len(CollectionTestConfig.INDEXING_ALGORITHMS) * 
            len(CollectionTestConfig.DISTANCE_METRICS) * 
            len(CollectionTestConfig.METADATA_CONFIGS)
        )
        
        logger.info(f"📊 Testing {total_combinations} combinations...")
        
        test_count = 0
        successful_tests = 0
        
        for storage_engine in CollectionTestConfig.STORAGE_ENGINES:
            for indexing_algorithm in CollectionTestConfig.INDEXING_ALGORITHMS:
                for distance_metric in CollectionTestConfig.DISTANCE_METRICS:
                    for metadata_config in CollectionTestConfig.METADATA_CONFIGS:
                        test_count += 1
                        
                        logger.info(f"🧪 Test {test_count}/{total_combinations}: {storage_engine}/{indexing_algorithm}/{distance_metric}/{metadata_config}")
                        
                        result = await self.test_collection_lifecycle(
                            storage_engine, indexing_algorithm, distance_metric, metadata_config
                        )
                        
                        self.test_results.append(result)
                        
                        # Check if test passed (all operations successful)
                        if (result["create_success"] and result["list_success"] and 
                            result["get_info_success"] and result["delete_success"] and 
                            not result["error_messages"]):
                            successful_tests += 1
                            logger.info(f"✅ Test {test_count} PASSED")
                        else:
                            logger.error(f"❌ Test {test_count} FAILED: {result['error_messages']}")
                        
                        # Small delay between tests
                        await asyncio.sleep(0.5)
        
        # Generate summary
        logger.info(f"📊 Test Summary: {successful_tests}/{total_combinations} tests passed ({successful_tests/total_combinations*100:.1f}%)")
        
        return successful_tests == total_combinations
    
    def generate_report(self):
        """Generate detailed test report"""
        report = {
            "test_summary": {
                "total_tests": len(self.test_results),
                "successful_tests": sum(1 for r in self.test_results if not r["error_messages"]),
                "failed_tests": sum(1 for r in self.test_results if r["error_messages"]),
                "success_rate": 0
            },
            "results_by_configuration": {},
            "failure_analysis": {}
        }
        
        if report["test_summary"]["total_tests"] > 0:
            report["test_summary"]["success_rate"] = (
                report["test_summary"]["successful_tests"] / report["test_summary"]["total_tests"] * 100
            )
        
        # Group results by configuration type
        for result in self.test_results:
            storage = result["storage_engine"]
            indexing = result["indexing_algorithm"]
            distance = result["distance_metric"]
            metadata = result["metadata_config"]
            
            # Initialize nested structure
            if storage not in report["results_by_configuration"]:
                report["results_by_configuration"][storage] = {}
            if indexing not in report["results_by_configuration"][storage]:
                report["results_by_configuration"][storage][indexing] = {}
            if distance not in report["results_by_configuration"][storage][indexing]:
                report["results_by_configuration"][storage][indexing][distance] = {}
            
            # Store result
            report["results_by_configuration"][storage][indexing][distance][metadata] = {
                "success": not bool(result["error_messages"]),
                "errors": result["error_messages"]
            }
            
            # Track failures
            if result["error_messages"]:
                config_key = f"{storage}/{indexing}/{distance}/{metadata}"
                report["failure_analysis"][config_key] = result["error_messages"]
        
        # Save report to file
        with open("/home/vsingh/code/proximadb/collection_test_report.json", "w") as f:
            json.dump(report, f, indent=2)
        
        logger.info("📄 Test report saved to collection_test_report.json")
        
        return report

async def main():
    """Main test execution"""
    test_suite = CollectionTestSuite()
    
    try:
        # Setup
        await test_suite.setup_server()
        
        # Run tests
        all_passed = await test_suite.run_comprehensive_tests()
        
        # Generate report
        report = test_suite.generate_report()
        
        # Print summary
        print("\n" + "="*80)
        print("📊 COLLECTION TEST SUMMARY")
        print("="*80)
        print(f"Total Tests: {report['test_summary']['total_tests']}")
        print(f"Successful: {report['test_summary']['successful_tests']}")
        print(f"Failed: {report['test_summary']['failed_tests']}")
        print(f"Success Rate: {report['test_summary']['success_rate']:.1f}%")
        
        if report["failure_analysis"]:
            print("\n❌ FAILED CONFIGURATIONS:")
            for config, errors in report["failure_analysis"].items():
                print(f"  {config}: {', '.join(errors)}")
        
        print("="*80)
        
        if all_passed:
            print("✅ All collection lifecycle tests passed!")
        else:
            print("❌ Some collection lifecycle tests failed!")
        
        return all_passed
        
    except Exception as e:
        logger.error(f"❌ Test suite failed: {e}")
        return False
    finally:
        await test_suite.teardown_server()

if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)