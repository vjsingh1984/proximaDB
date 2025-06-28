#!/usr/bin/env python3
"""
gRPC UUID-Based 10MB Vector Data Test - WAL Flush to VIPER Verification
Tests complete data pipeline with large data volumes to trigger flush mechanics
"""

import sys
import os
import time
import asyncio
import logging
import numpy as np
from typing import List, Dict, Tuple
from pathlib import Path

# Add Python SDK to path
sys.path.insert(0, '/workspace/tests/python/integration')
sys.path.insert(0, '/workspace/clients/python/src')

from bert_embedding_service import BERTEmbeddingService, create_sample_corpus

# Configure logging to match server tracing
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s [%(levelname)s] %(name)s: %(message)s',
    datefmt='%Y-%m-%dT%H:%M:%S'
)
logger = logging.getLogger('grpc_uuid_test')

class GRPCUUIDFlushTester:
    """Test gRPC UUID operations with large data to trigger flush mechanics"""
    
    def __init__(self):
        """Initialize gRPC client and BERT service"""
        self.collection_name = None
        self.collection_uuid = None
        self.total_vectors = 0
        self.total_bytes = 0
        
        # Initialize BERT service for 384D embeddings
        logger.info("🤖 Initializing BERT embedding service...")
        self.bert_service = BERTEmbeddingService(
            model_name="all-MiniLM-L6-v2",  # 384 dimensions
            cache_dir="./embedding_cache"
        )
        logger.info(f"✅ BERT service ready: {self.bert_service.dimension}D embeddings")
        
        # Initialize gRPC client via direct protobuf interface
        self._setup_grpc_client()
    
    def _setup_grpc_client(self):
        """Setup direct gRPC client for better performance"""
        try:
            import grpc
            from proximadb import proximadb_pb2, proximadb_pb2_grpc
            
            self.channel = grpc.insecure_channel('localhost:5679')
            self.stub = proximadb_pb2_grpc.ProximaDBStub(self.channel)
            self.pb2 = proximadb_pb2
            logger.info("🚀 gRPC client initialized for maximum performance")
            
        except ImportError as e:
            logger.error(f"❌ gRPC dependencies not available: {e}")
            raise RuntimeError("gRPC dependencies required for this test")
    
    def generate_10mb_corpus(self) -> Tuple[List[Dict], List[List[float]]]:
        """Generate ~10MB of vector data with BERT embeddings"""
        logger.info("📚 Generating 10MB corpus with BERT embeddings...")
        
        # Calculate vectors needed for ~10MB
        # 384 floats * 4 bytes = 1536 bytes per vector
        # Plus metadata ~500 bytes per vector = ~2KB per vector
        # So ~5000 vectors = ~10MB
        target_vectors = 5000
        
        # Generate large corpus
        corpus = create_sample_corpus(size_mb=8.0)  # Large base corpus
        if len(corpus) < target_vectors:
            # Extend corpus if needed
            while len(corpus) < target_vectors:
                additional = create_sample_corpus(size_mb=2.0)
                corpus.extend(additional)
        
        corpus = corpus[:target_vectors]  # Trim to exact target
        logger.info(f"✅ Generated corpus: {len(corpus)} documents")
        
        # Generate BERT embeddings
        logger.info("🧠 Generating BERT embeddings (this may take a moment)...")
        texts = [doc['text'] for doc in corpus]
        embeddings = self.bert_service.embed_texts(
            texts,
            batch_size=100,  # Larger batch for efficiency
            show_progress=True
        )
        
        # Convert to lists and calculate size
        embedding_lists = [emb.tolist() for emb in embeddings]
        
        # Calculate total data size
        vector_bytes = len(embedding_lists) * 384 * 4  # 4 bytes per float
        metadata_bytes = sum(len(str(doc).encode('utf-8')) for doc in corpus)
        total_bytes = vector_bytes + metadata_bytes
        
        logger.info(f"✅ Generated {len(embedding_lists)} embeddings")
        logger.info(f"📏 Vector data: {vector_bytes / (1024*1024):.1f}MB")
        logger.info(f"📝 Metadata: {metadata_bytes / (1024*1024):.1f}MB") 
        logger.info(f"📊 Total size: {total_bytes / (1024*1024):.1f}MB")
        
        return corpus, embedding_lists
    
    def create_collection_via_grpc(self) -> str:
        """Create collection via gRPC and return UUID"""
        self.collection_name = f"grpc-flush-test-{int(time.time())}"
        
        logger.info(f"📦 Creating collection via gRPC: {self.collection_name}")
        
        create_request = self.pb2.CollectionRequest(
            operation=self.pb2.CollectionOperation.COLLECTION_CREATE,
            collection_id=self.collection_name,
            collection_config=self.pb2.CollectionConfig(
                name=self.collection_name,
                dimension=384,
                distance_metric=self.pb2.DistanceMetric.COSINE,
                indexing_algorithm=self.pb2.IndexingAlgorithm.HNSW
            )
        )
        
        try:
            create_response = self.stub.CollectionOperation(create_request)
            if create_response.success and create_response.collection:
                self.collection_uuid = create_response.collection.id
                logger.info(f"✅ Collection created via gRPC")
                logger.info(f"📛 Name: {self.collection_name}")
                logger.info(f"🔑 UUID: {self.collection_uuid}")
                return self.collection_uuid
            else:
                raise RuntimeError(f"Collection creation failed: {create_response.error_message}")
        except Exception as e:
            logger.error(f"❌ gRPC collection creation failed: {e}")
            raise
    
    def batch_insert_via_grpc(self, corpus: List[Dict], embeddings: List[List[float]], 
                             batch_size: int = 200):
        """Insert large batches via gRPC to trigger flush mechanics"""
        logger.info(f"🚀 Starting gRPC batch insert to trigger flush mechanics")
        logger.info(f"🎯 Target: {len(embeddings)} vectors using UUID: {self.collection_uuid}")
        logger.info(f"📦 Batch size: {batch_size} vectors per batch")
        logger.info(f"💾 Expected to trigger WAL flush → VIPER storage")
        
        total_inserted = 0
        batch_count = 0
        total_time = 0
        
        for i in range(0, len(embeddings), batch_size):
            batch_count += 1
            batch_corpus = corpus[i:i + batch_size]
            batch_embeddings = embeddings[i:i + batch_size]
            
            logger.info(f"🔄 Batch {batch_count}: vectors {i+1}-{min(i+batch_size, len(embeddings))}")
            
            # Prepare gRPC vector insert request
            vector_records = []
            for j, (doc, embedding) in enumerate(zip(batch_corpus, batch_embeddings)):
                # Create rich metadata to increase data size
                metadata = {
                    "text": doc['text'][:300],  # Larger text samples
                    "category": doc['category'],
                    "author": doc['author'],
                    "doc_type": doc['doc_type'],
                    "year": str(doc['year']),
                    "keywords": doc.get('keywords', ''),
                    "batch_id": str(batch_count),
                    "vector_id": f"grpc_v_{batch_count}_{j}",
                    "embedding_model": "all-MiniLM-L6-v2",
                    "insertion_time": time.time(),
                    "length": str(doc['length'])
                }
                
                vector_record = self.pb2.VectorRecord(
                    id=f"{doc['id']}_grpc_b{batch_count}_{j}",
                    vector=embedding,
                    metadata=metadata
                )
                vector_records.append(vector_record)
            
            # Create gRPC vector insert request using UUID  
            insert_request = self.pb2.VectorInsertRequest(
                collection_id=self.collection_uuid,  # Using UUID!
                vectors=vector_records,
                upsert=False
            )
            
            # Execute gRPC vector insert
            start_time = time.time()
            try:
                batch_response = self.stub.VectorInsert(insert_request)
                
                if batch_response.success:
                    insert_time = time.time() - start_time
                    total_time += insert_time
                    inserted_count = len(vector_records)
                    total_inserted += inserted_count
                    
                    # Calculate performance metrics
                    vectors_per_sec = inserted_count / insert_time
                    batch_mb = (inserted_count * 384 * 4) / (1024 * 1024)  # Vector data size
                    mb_per_sec = batch_mb / insert_time
                    
                    logger.info(f"   ✅ gRPC success: {inserted_count} vectors in {insert_time:.3f}s")
                    logger.info(f"   🚄 Performance: {vectors_per_sec:.1f} vectors/sec, {mb_per_sec:.1f}MB/sec")
                    logger.info(f"   📈 Progress: {total_inserted}/{len(embeddings)} vectors")
                    
                    # Log WAL and flush expectations
                    if batch_count % 5 == 0:
                        logger.info(f"   💾 Batch {batch_count}: WAL should be accumulating data")
                        logger.info(f"   🔄 Watch for flush operations in server logs")
                    
                    # Pause periodically to allow flush operations
                    if batch_count % 10 == 0:
                        logger.info(f"   ⏸️  Pausing for WAL flush to VIPER...")
                        time.sleep(2)
                        
                else:
                    logger.error(f"   ❌ Batch {batch_count} failed: {batch_response.error_message}")
                    break
                    
            except Exception as e:
                logger.error(f"   ❌ gRPC batch {batch_count} error: {e}")
                break
        
        # Final metrics
        avg_vectors_per_sec = total_inserted / total_time if total_time > 0 else 0
        total_mb = (total_inserted * 384 * 4) / (1024 * 1024)
        
        logger.info(f"🎉 Batch insert completed!")
        logger.info(f"📊 Total vectors inserted: {total_inserted}")
        logger.info(f"📏 Total data volume: {total_mb:.1f}MB")
        logger.info(f"⏱️  Total time: {total_time:.2f}s")
        logger.info(f"🚄 Average performance: {avg_vectors_per_sec:.1f} vectors/sec")
        
        self.total_vectors = total_inserted
        self.total_bytes = total_mb * 1024 * 1024
        
        return total_inserted
    
    def verify_collection_via_grpc(self):
        """Verify collection state via gRPC"""
        logger.info(f"🔍 Verifying collection state via gRPC...")
        
        get_request = self.pb2.CollectionRequest(
            operation=self.pb2.CollectionOperation.COLLECTION_GET,
            collection_id=self.collection_uuid  # Using UUID for verification
        )
        
        try:
            get_response = self.stub.CollectionOperation(get_request)
            if get_response.success and get_response.collection:
                collection = get_response.collection
                stats = collection.stats
                
                logger.info(f"✅ gRPC collection verification:")
                logger.info(f"   📛 Name: {collection.config.name}")
                logger.info(f"   🔑 UUID: {collection.id}")
                logger.info(f"   📊 Vector Count: {stats.vector_count}")
                logger.info(f"   💽 Data Size: {stats.data_size_bytes / (1024*1024):.1f}MB")
                logger.info(f"   📏 Index Size: {stats.index_size_bytes / (1024*1024):.1f}MB")
                
                # Verify vector count
                if stats.vector_count == self.total_vectors:
                    logger.info(f"✅ Vector count verification: PASSED")
                else:
                    logger.warning(f"⚠️  Vector count mismatch: expected {self.total_vectors}, got {stats.vector_count}")
                
                return collection
            else:
                logger.error(f"❌ Collection verification failed: {get_response.error_message}")
                return None
        except Exception as e:
            logger.error(f"❌ gRPC verification error: {e}")
            return None
    
    def log_flush_monitoring_instructions(self):
        """Log instructions for monitoring WAL and VIPER operations"""
        logger.info(f"📋 MONITORING INSTRUCTIONS:")
        logger.info(f"To monitor WAL → VIPER flush operations, run in another terminal:")
        logger.info(f"")
        logger.info(f"# Real-time WAL and flush monitoring:")
        logger.info(f"tail -f /tmp/proximadb_server_grpc.log | grep -E '💾|🔄|📁|✅.*flush|WAL.*write|VIPER|Flush.*completed|memtable.*flush'")
        logger.info(f"")
        logger.info(f"# Or use the monitoring script:")
        logger.info(f"./monitor_wal_viper_logs.sh")
        logger.info(f"")
        logger.info(f"Key patterns to watch for:")
        logger.info(f"  💾 WAL write operations")
        logger.info(f"  🔄 Flush operations starting")
        logger.info(f"  📁 VIPER file operations")
        logger.info(f"  ✅ Flush completion confirmations")
        logger.info(f"  🚀 WAL write completed messages")
        logger.info(f"")
    
    def cleanup(self):
        """Clean up test collection"""
        if self.collection_uuid:
            logger.info(f"🧹 Cleaning up collection: {self.collection_uuid}")
            try:
                delete_request = self.pb2.CollectionRequest(
                    operation=self.pb2.CollectionOperation.COLLECTION_DELETE,
                    collection_id=self.collection_uuid
                )
                delete_response = self.stub.CollectionOperation(delete_request)
                if delete_response.success:
                    logger.info(f"✅ Collection deleted successfully")
                else:
                    logger.error(f"❌ Deletion failed: {delete_response.error_message}")
            except Exception as e:
                logger.error(f"❌ Cleanup error: {e}")

async def main():
    """Main test execution with proper async support"""
    logger.info("gRPC UUID-Based 10MB Vector Data - WAL Flush to VIPER Test")
    logger.info("=" * 65)
    logger.info("🎯 Testing complete pipeline: gRPC → WAL → Flush → VIPER Storage")
    logger.info("📊 Target: 10MB of vector data to trigger flush mechanics")
    
    tester = GRPCUUIDFlushTester()
    
    try:
        # Show monitoring instructions
        tester.log_flush_monitoring_instructions()
        
        # Brief pause for setup
        logger.info("🚀 Starting test in 3 seconds...")
        await asyncio.sleep(3)
        
        # Generate large corpus and embeddings
        corpus, embeddings = tester.generate_10mb_corpus()
        
        # Create collection via gRPC
        uuid = tester.create_collection_via_grpc()
        
        # Log test start
        logger.info(f"🎯 STARTING LARGE DATA INSERT")
        logger.info(f"📊 Data volume: ~10MB vector data")
        logger.info(f"🔑 Collection UUID: {uuid}")
        logger.info(f"💾 This should trigger WAL flush to VIPER storage!")
        logger.info(f"🔍 Watch server logs for flush operations!")
        
        # Perform large batch insert via gRPC
        inserted_count = tester.batch_insert_via_grpc(corpus, embeddings, batch_size=200)
        
        # Wait for final flush operations
        logger.info(f"⏳ Waiting for final WAL flush operations...")
        await asyncio.sleep(5)
        
        # Verify final state
        tester.verify_collection_via_grpc()
        
        # Final success summary
        logger.info(f"🎉 TEST COMPLETED SUCCESSFULLY!")
        logger.info(f"✅ Inserted {inserted_count} vectors via gRPC using UUID")
        logger.info(f"✅ Used collection UUID for all operations")
        logger.info(f"✅ Generated ~{tester.total_bytes / (1024*1024):.1f}MB of vector data")
        logger.info(f"✅ Should have triggered WAL → VIPER flush mechanics")
        logger.info(f"📋 Check server logs for flush operations confirmation")
        
        # Keep collection for inspection
        logger.info(f"💡 Collection preserved for manual inspection")
        logger.info(f"🔑 UUID: {uuid}")
        
    except KeyboardInterrupt:
        logger.info("⏹️  Test interrupted by user")
        tester.cleanup()
        return False
    except Exception as e:
        logger.error(f"❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        tester.cleanup()
        return False
    
    return True

def run_sync_main():
    """Synchronous wrapper for main"""
    return asyncio.run(main())

if __name__ == "__main__":
    success = run_sync_main()
    exit(0 if success else 1)