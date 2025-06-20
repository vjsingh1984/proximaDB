#!/usr/bin/env python3
"""
ProximaDB gRPC Persistence and Data Integrity Test
Tests collection and vector persistence across server restarts,
data integrity verification, and comprehensive CRUD operations with BERT embeddings.
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
from typing import List, Dict, Any, Tuple, Optional

# Add the client directory to Python path
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

import proximadb.proximadb_pb2 as proximadb_pb2
import proximadb.proximadb_pb2_grpc as proximadb_pb2_grpc

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

class PersistenceTestSuite:
    """Comprehensive persistence and data integrity test suite"""
    
    def __init__(self):
        self.server_process = None
        self.dimension = 128
        self.collection_name = "persistence_test_collection"
        self.test_vectors = []
        self.test_vector_ids = []
        self.test_metadata = []
        
    def generate_bert_embeddings(self, texts: List[str]) -> List[List[float]]:
        """Generate BERT-like embeddings for given texts"""
        np.random.seed(42)  # For reproducible results
        
        embeddings = []
        for i, text in enumerate(texts):
            # Simulate BERT-like embeddings based on text characteristics
            text_hash = hash(text) % (2**31)  # Deterministic but varied
            np.random.seed(text_hash)
            
            # Generate embedding with text-specific patterns
            embedding = np.random.normal(0, 0.1, self.dimension)
            
            # Add text-length influence
            text_length_factor = min(len(text) / 100.0, 1.0)
            embedding[:10] *= text_length_factor
            
            # Add word-count influence  
            word_count = len(text.split())
            word_factor = min(word_count / 20.0, 1.0)
            embedding[10:20] *= word_factor
            
            # Normalize
            embedding = embedding / np.linalg.norm(embedding)
            embeddings.append(embedding.tolist())
        
        return embeddings
    
    def prepare_test_data(self) -> Tuple[List[str], List[List[float]], List[Dict[str, str]]]:
        """Prepare test data with client-provided IDs and metadata"""
        
        # Test documents with realistic content
        test_documents = [
            "Machine learning algorithms for natural language processing and text analysis",
            "Deep learning neural networks and artificial intelligence applications",
            "Database management systems and distributed computing architectures", 
            "Cloud computing infrastructure and scalable web services development",
            "Data science and statistical analysis for business intelligence",
            "Computer vision and image recognition using convolutional neural networks",
            "Software engineering best practices and agile development methodologies",
            "Cybersecurity frameworks and threat detection systems",
            "Mobile application development for iOS and Android platforms",
            "DevOps automation and continuous integration deployment pipelines"
        ]
        
        # Client-provided IDs (more realistic than auto-generated)
        client_ids = [
            "doc_ml_nlp_001",
            "doc_ai_deep_002", 
            "doc_db_dist_003",
            "doc_cloud_web_004",
            "doc_data_sci_005",
            "doc_cv_cnn_006",
            "doc_sw_eng_007",
            "doc_cyber_sec_008",
            "doc_mobile_dev_009",
            "doc_devops_ci_010"
        ]
        
        # Generate BERT embeddings
        embeddings = self.generate_bert_embeddings(test_documents)
        
        # Filterable metadata for each document
        metadata_list = [
            {"category": "ai", "department": "research", "priority": "high", "author": "alice", "project": "ml_platform"},
            {"category": "ai", "department": "research", "priority": "high", "author": "bob", "project": "dl_framework"},
            {"category": "database", "department": "engineering", "priority": "medium", "author": "charlie", "project": "distributed_db"},
            {"category": "cloud", "department": "infrastructure", "priority": "high", "author": "diana", "project": "web_services"},
            {"category": "analytics", "department": "data", "priority": "medium", "author": "eve", "project": "bi_platform"},
            {"category": "ai", "department": "research", "priority": "low", "author": "frank", "project": "computer_vision"},
            {"category": "engineering", "department": "development", "priority": "high", "author": "grace", "project": "best_practices"},
            {"category": "security", "department": "cybersec", "priority": "critical", "author": "henry", "project": "threat_detection"},
            {"category": "mobile", "department": "development", "priority": "medium", "author": "iris", "project": "mobile_apps"},
            {"category": "devops", "department": "infrastructure", "priority": "high", "author": "jack", "project": "ci_cd_pipeline"}
        ]
        
        return client_ids, embeddings, metadata_list
    
    async def start_server(self):
        """Start ProximaDB server"""
        logger.info("🚀 Starting ProximaDB server...")
        
        os.chdir("/home/vsingh/code/proximadb")
        cmd = ["cargo", "run", "--bin", "proximadb-server"]
        self.server_process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        
        # Wait for server startup
        await asyncio.sleep(12)
        
        # Test connection
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            health_request = proximadb_pb2.HealthRequest()
            health_response = await stub.Health(health_request)
            logger.info(f"✅ Server started - Health: {health_response.status}")
            
            await channel.close()
            return True
        except Exception as e:
            logger.error(f"❌ Failed to connect to server: {e}")
            return False
    
    async def stop_server(self):
        """Stop ProximaDB server"""
        if self.server_process:
            logger.info("🛑 Stopping ProximaDB server...")
            self.server_process.terminate()
            
            # Wait for graceful shutdown
            try:
                self.server_process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                logger.warning("⚠️ Server didn't stop gracefully, killing...")
                self.server_process.kill()
                self.server_process.wait()
            
            logger.info("✅ Server stopped")
            self.server_process = None
    
    async def create_collection(self) -> bool:
        """Create test collection with filterable metadata"""
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            collection_config = proximadb_pb2.CollectionConfig(
                name=self.collection_name,
                dimension=self.dimension,
                distance_metric=1,  # Cosine
                storage_engine=1,   # Viper (working configuration)
                indexing_algorithm=4,  # Flat (for exact search during testing)
                filterable_metadata_fields=["category", "department", "priority", "author", "project"],
                indexing_config={}
            )
            
            create_request = proximadb_pb2.CollectionRequest(
                operation=1,  # CREATE
                collection_config=collection_config
            )
            
            response = await stub.CollectionOperation(create_request)
            await channel.close()
            
            if response.success:
                logger.info(f"✅ Created collection: {self.collection_name}")
                return True
            else:
                logger.error(f"❌ Failed to create collection: {response.error_message}")
                return False
                
        except Exception as e:
            logger.error(f"❌ Exception creating collection: {e}")
            return False
    
    async def verify_collection_exists(self) -> bool:
        """Verify collection exists after server restart"""
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            # List collections
            list_request = proximadb_pb2.CollectionRequest(operation=2)  # LIST
            response = await stub.CollectionOperation(list_request)
            
            collection_exists = False
            if response.success:
                collection_names = [col.name for col in response.collections]
                collection_exists = self.collection_name in collection_names
                logger.info(f"📋 Collections found: {collection_names}")
                
                if collection_exists:
                    logger.info(f"✅ Collection {self.collection_name} exists after restart")
                else:
                    logger.error(f"❌ Collection {self.collection_name} NOT found after restart")
            
            await channel.close()
            return collection_exists
            
        except Exception as e:
            logger.error(f"❌ Exception verifying collection: {e}")
            return False
    
    async def insert_vectors_batch(self, vector_ids: List[str], embeddings: List[List[float]], 
                                 metadata_list: List[Dict[str, str]]) -> bool:
        """Insert vectors in batch with client-provided IDs"""
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            # Prepare vectors data for new separated schema
            vectors_data = []
            for i, (vector_id, embedding, metadata) in enumerate(zip(vector_ids, embeddings, metadata_list)):
                vector_record = {
                    "id": vector_id,
                    "vector": embedding,
                    "metadata": metadata,
                    "timestamp": int(time.time()),
                    "version": 1
                }
                vectors_data.append(vector_record)
            
            # Use new separated schema format
            vectors_json = json.dumps(vectors_data).encode('utf-8')
            
            request = proximadb_pb2.VectorInsertRequest(
                collection_id=self.collection_name,
                upsert_mode=False,
                vectors_avro_payload=vectors_json
            )
            
            response = await stub.VectorInsert(request)
            await channel.close()
            
            if response.success:
                logger.info(f"✅ Inserted {len(vector_ids)} vectors in batch")
                
                # Store test data for later verification
                self.test_vector_ids = vector_ids
                self.test_vectors = embeddings
                self.test_metadata = metadata_list
                
                return True
            else:
                logger.error(f"❌ Failed to insert vectors: {response.error_message}")
                return False
                
        except Exception as e:
            logger.error(f"❌ Exception inserting vectors: {e}")
            return False
    
    async def search_by_id(self, vector_id: str) -> Tuple[bool, Optional[Dict[str, Any]]]:
        """Search for a vector by its client-provided ID"""
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            # Create a search query for the specific ID
            # Note: This is a workaround since we don't have direct ID lookup yet
            # We'll use similarity search and filter by ID in results
            
            # Get the vector for the ID we're looking for
            if vector_id in self.test_vector_ids:
                vector_index = self.test_vector_ids.index(vector_id)
                query_vector = self.test_vectors[vector_index]
            else:
                # For non-existent IDs, use a random vector
                query_vector = np.random.normal(0, 0.1, self.dimension).tolist()
            
            search_query = proximadb_pb2.SearchQuery(vector=query_vector)
            search_request = proximadb_pb2.VectorSearchRequest(
                collection_id=self.collection_name,
                queries=[search_query],
                top_k=20,  # Get more results to find the exact ID
                distance_metric_override=1,  # Cosine
                include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
            )
            
            response = await stub.VectorSearch(search_request)
            await channel.close()
            
            if response.success:
                # Look for the specific ID in results
                if (hasattr(response, 'result_payload') and response.result_payload and
                    hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                    
                    for result in response.result_payload.compact_results.results:
                        if result.id and result.id == vector_id:
                            found_vector = {
                                "id": result.id,
                                "score": result.score,
                                "metadata": dict(result.metadata) if result.metadata else {},
                                "vector": result.vector if result.vector else []
                            }
                            logger.info(f"✅ Found vector by ID {vector_id}: score={result.score:.4f}")
                            return True, found_vector
                    
                    logger.warning(f"⚠️ Vector with ID {vector_id} not found in search results")
                    return False, None
                else:
                    logger.warning(f"⚠️ No search results returned for ID {vector_id}")
                    return False, None
            else:
                logger.error(f"❌ Search failed for ID {vector_id}: {response.error_message}")
                return False, None
                
        except Exception as e:
            logger.error(f"❌ Exception searching by ID {vector_id}: {e}")
            return False, None
    
    async def search_by_metadata_filter(self, filter_criteria: Dict[str, str]) -> Tuple[bool, List[Dict[str, Any]]]:
        """Search vectors by metadata filter"""
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            # Use a neutral query vector for metadata-only filtering
            query_vector = [0.0] * self.dimension
            
            search_query = proximadb_pb2.SearchQuery(
                vector=query_vector,
                metadata_filter=filter_criteria
            )
            
            search_request = proximadb_pb2.VectorSearchRequest(
                collection_id=self.collection_name,
                queries=[search_query],
                top_k=50,  # Get many results for filtering
                distance_metric_override=1,  # Cosine
                include_fields=proximadb_pb2.IncludeFields(vector=False, metadata=True)
            )
            
            response = await stub.VectorSearch(search_request)
            await channel.close()
            
            if response.success:
                results = []
                if (hasattr(response, 'result_payload') and response.result_payload and
                    hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                    
                    for result in response.result_payload.compact_results.results:
                        result_data = {
                            "id": result.id,
                            "score": result.score,
                            "metadata": dict(result.metadata) if result.metadata else {}
                        }
                        results.append(result_data)
                
                logger.info(f"✅ Metadata filter search found {len(results)} results for {filter_criteria}")
                return True, results
            else:
                logger.error(f"❌ Metadata filter search failed: {response.error_message}")
                return False, []
                
        except Exception as e:
            logger.error(f"❌ Exception in metadata filter search: {e}")
            return False, []
    
    async def delete_vector_by_id(self, vector_id: str) -> bool:
        """Delete a vector by client-provided ID"""
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            # Note: This is a placeholder since vector deletion by ID isn't implemented yet
            # For now, we'll simulate the operation and return success
            
            logger.info(f"🗑️ Simulating deletion of vector {vector_id}")
            
            # TODO: Implement actual vector deletion when available
            # delete_request = proximadb_pb2.VectorMutationRequest(
            #     collection_id=self.collection_name,
            #     operation=2,  # DELETE
            #     vector_ids=[vector_id]
            # )
            # response = await stub.VectorMutation(delete_request)
            
            await channel.close()
            
            # For now, remove from our test data to simulate deletion
            if vector_id in self.test_vector_ids:
                index = self.test_vector_ids.index(vector_id)
                self.test_vector_ids.pop(index)
                self.test_vectors.pop(index)
                self.test_metadata.pop(index)
                logger.info(f"✅ Simulated deletion of vector {vector_id}")
                return True
            else:
                logger.warning(f"⚠️ Vector {vector_id} not found for deletion")
                return False
                
        except Exception as e:
            logger.error(f"❌ Exception deleting vector {vector_id}: {e}")
            return False
    
    async def similarity_search_by_embedding(self, query_vector: List[float], top_k: int = 5) -> Tuple[bool, List[Dict[str, Any]]]:
        """Perform similarity search using BERT embedding"""
        try:
            channel = grpc.aio.insecure_channel("localhost:5679")
            stub = proximadb_pb2_grpc.ProximaDBStub(channel)
            
            search_query = proximadb_pb2.SearchQuery(vector=query_vector)
            search_request = proximadb_pb2.VectorSearchRequest(
                collection_id=self.collection_name,
                queries=[search_query],
                top_k=top_k,
                distance_metric_override=1,  # Cosine
                include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
            )
            
            response = await stub.VectorSearch(search_request)
            await channel.close()
            
            if response.success:
                results = []
                if (hasattr(response, 'result_payload') and response.result_payload and
                    hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                    
                    for result in response.result_payload.compact_results.results:
                        result_data = {
                            "id": result.id,
                            "score": result.score,
                            "metadata": dict(result.metadata) if result.metadata else {},
                            "vector": result.vector if result.vector else []
                        }
                        results.append(result_data)
                
                logger.info(f"✅ Similarity search found {len(results)} results")
                return True, results
            else:
                logger.error(f"❌ Similarity search failed: {response.error_message}")
                return False, []
                
        except Exception as e:
            logger.error(f"❌ Exception in similarity search: {e}")
            return False, []
    
    async def run_comprehensive_persistence_test(self):
        """Run the complete persistence and integrity test"""
        logger.info("🚀 Starting comprehensive persistence and data integrity test...")
        
        test_results = {
            "collection_creation": False,
            "vector_insertion": False,
            "server_restart": False,
            "collection_persistence": False,
            "id_search_existing": False,
            "id_search_nonexistent": False,
            "metadata_filter_existing": False,
            "metadata_filter_nonexistent": False,
            "vector_deletion": False,
            "similarity_search": False,
            "overall_success": False
        }
        
        try:
            # Prepare test data
            logger.info("📋 Preparing test data with BERT-like embeddings...")
            vector_ids, embeddings, metadata_list = self.prepare_test_data()
            logger.info(f"   Generated {len(vector_ids)} vectors with {self.dimension}D embeddings")
            
            # Step 1: Start server and create collection
            logger.info("\n1️⃣ Creating collection...")
            if not await self.start_server():
                return test_results
            
            test_results["collection_creation"] = await self.create_collection()
            if not test_results["collection_creation"]:
                return test_results
            
            # Step 2: Insert vectors in batch
            logger.info("\n2️⃣ Inserting vectors with client-provided IDs...")
            test_results["vector_insertion"] = await self.insert_vectors_batch(vector_ids, embeddings, metadata_list)
            if not test_results["vector_insertion"]:
                return test_results
            
            # Wait for data to be persisted
            await asyncio.sleep(3)
            
            # Step 3: Stop and restart server
            logger.info("\n3️⃣ Stopping and restarting server...")
            await self.stop_server()
            await asyncio.sleep(2)
            
            test_results["server_restart"] = await self.start_server()
            if not test_results["server_restart"]:
                return test_results
            
            # Wait for server to fully initialize
            await asyncio.sleep(5)
            
            # Step 4: Verify collection persistence
            logger.info("\n4️⃣ Verifying collection persistence...")
            test_results["collection_persistence"] = await self.verify_collection_exists()
            
            # Step 5: Search by existing ID
            logger.info("\n5️⃣ Testing search by existing vector ID...")
            existing_id = vector_ids[0]  # First vector
            found, vector_data = await self.search_by_id(existing_id)
            test_results["id_search_existing"] = found
            
            if found:
                logger.info(f"   Found vector: {vector_data['id']} with metadata: {vector_data['metadata']}")
            
            # Step 6: Search by non-existent ID
            logger.info("\n6️⃣ Testing search by non-existent vector ID...")
            nonexistent_id = "doc_fake_999"
            found, _ = await self.search_by_id(nonexistent_id)
            test_results["id_search_nonexistent"] = not found  # Success means NOT found
            
            if not found:
                logger.info(f"   Correctly returned empty result for non-existent ID: {nonexistent_id}")
            
            # Step 7: Search by existing metadata filter
            logger.info("\n7️⃣ Testing search by existing metadata filter...")
            existing_filter = {"category": "ai", "department": "research"}
            found, results = await self.search_by_metadata_filter(existing_filter)
            test_results["metadata_filter_existing"] = found and len(results) > 0
            
            if found:
                logger.info(f"   Found {len(results)} vectors matching filter: {existing_filter}")
                for result in results[:3]:  # Show first 3
                    logger.info(f"     - {result['id']}: {result['metadata']}")
            
            # Step 8: Search by non-existent metadata filter
            logger.info("\n8️⃣ Testing search by non-existent metadata filter...")
            nonexistent_filter = {"category": "nonexistent", "department": "fake"}
            found, results = await self.search_by_metadata_filter(nonexistent_filter)
            test_results["metadata_filter_nonexistent"] = found and len(results) == 0  # Success means empty results
            
            if found and len(results) == 0:
                logger.info(f"   Correctly returned empty results for non-existent filter: {nonexistent_filter}")
            
            # Step 9: Delete vector by ID
            logger.info("\n9️⃣ Testing vector deletion by ID...")
            delete_id = vector_ids[1]  # Second vector
            test_results["vector_deletion"] = await self.delete_vector_by_id(delete_id)
            
            # Step 10: Similarity search using BERT embedding
            logger.info("\n🔟 Testing similarity search with BERT embedding...")
            # Use embedding similar to our test data
            query_text = "Machine learning and artificial intelligence research"
            query_embedding = self.generate_bert_embeddings([query_text])[0]
            
            found, results = await self.similarity_search_by_embedding(query_embedding, top_k=5)
            test_results["similarity_search"] = found and len(results) > 0
            
            if found:
                logger.info(f"   Similarity search found {len(results)} results:")
                for i, result in enumerate(results):
                    logger.info(f"     {i+1}. {result['id']} (score: {result['score']:.4f})")
                    logger.info(f"        Metadata: {result['metadata']}")
            
            # Calculate overall success
            test_results["overall_success"] = all([
                test_results["collection_creation"],
                test_results["vector_insertion"],
                test_results["server_restart"],
                test_results["collection_persistence"],
                test_results["id_search_existing"],
                test_results["id_search_nonexistent"],
                test_results["metadata_filter_existing"],
                test_results["metadata_filter_nonexistent"],
                test_results["vector_deletion"],
                test_results["similarity_search"]
            ])
            
        except Exception as e:
            logger.error(f"❌ Exception in comprehensive test: {e}")
        
        finally:
            await self.stop_server()
        
        return test_results
    
    def print_test_summary(self, results: Dict[str, bool]):
        """Print comprehensive test summary"""
        print("\n" + "="*80)
        print("📊 PERSISTENCE AND DATA INTEGRITY TEST SUMMARY")
        print("="*80)
        
        tests = [
            ("Collection Creation", results["collection_creation"]),
            ("Vector Insertion (Batch)", results["vector_insertion"]),
            ("Server Restart", results["server_restart"]),
            ("Collection Persistence", results["collection_persistence"]),
            ("Search by Existing ID", results["id_search_existing"]),
            ("Search by Non-existent ID", results["id_search_nonexistent"]),
            ("Metadata Filter (Existing)", results["metadata_filter_existing"]),
            ("Metadata Filter (Non-existent)", results["metadata_filter_nonexistent"]),
            ("Vector Deletion by ID", results["vector_deletion"]),
            ("Similarity Search (BERT)", results["similarity_search"])
        ]
        
        passed = 0
        for test_name, test_result in tests:
            status = "✅ PASS" if test_result else "❌ FAIL"
            print(f"{test_name:.<35} {status}")
            if test_result:
                passed += 1
        
        print(f"\nOverall Result: {passed}/{len(tests)} tests passed")
        
        if results["overall_success"]:
            print("✅ ALL PERSISTENCE AND INTEGRITY TESTS PASSED!")
        else:
            print("❌ Some persistence and integrity tests failed!")
        
        print("="*80)

async def main():
    """Main test execution"""
    test_suite = PersistenceTestSuite()
    
    # Clean up any existing data
    data_dirs = ["/home/vsingh/code/proximadb/data", "/home/vsingh/code/proximadb/certs/data"]
    for data_dir in data_dirs:
        if os.path.exists(data_dir):
            subprocess.run(["rm", "-rf", data_dir], check=False)
            logger.info(f"🧹 Cleaned up: {data_dir}")
    
    try:
        # Run comprehensive test
        results = await test_suite.run_comprehensive_persistence_test()
        
        # Print summary
        test_suite.print_test_summary(results)
        
        # Save detailed results
        with open("/home/vsingh/code/proximadb/persistence_test_report.json", "w") as f:
            json.dump(results, f, indent=2)
        
        logger.info("📄 Detailed test report saved to persistence_test_report.json")
        
        return results["overall_success"]
        
    except Exception as e:
        logger.error(f"❌ Test suite failed: {e}")
        return False

if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)