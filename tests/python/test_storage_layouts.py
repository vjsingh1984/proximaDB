#!/usr/bin/env python3
"""
ProximaDB Storage Layout Test Suite
Tests for LSM and VIPER storage layouts with flush, compaction, and large-scale search operations
"""

import pytest
import time
import numpy as np
from typing import List, Dict, Any
from sentence_transformers import SentenceTransformer
import logging

from proximadb import connect_rest, connect_grpc
from proximadb.models import CollectionConfig, FlushConfig, DistanceMetric
from proximadb.exceptions import ProximaDBError

logger = logging.getLogger(__name__)


class TestVIPERStorageLayout:
    """Test VIPER storage layout with flush and compaction operations"""
    
    @pytest.fixture(scope="class")
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture(scope="class")
    def grpc_client(self):
        client = connect_grpc("http://localhost:5679")
        yield client
        client.close()
    
    @pytest.fixture(scope="class")
    def bert_model(self):
        return SentenceTransformer('all-MiniLM-L6-v2')
    
    def test_viper_flush_operations(self, rest_client, bert_model):
        """Test VIPER flush operations with large data insertion"""
        collection_name = f"viper_flush_{int(time.time())}"
        
        # Configure VIPER with low flush threshold
        config = CollectionConfig(
            dimension=384,
            distance_metric=DistanceMetric.COSINE,
            description="VIPER flush test collection",
            storage_layout="viper",
            flush_config=FlushConfig(max_wal_size_mb=8.0)  # Low threshold to trigger flush
        )
        
        try:
            collection = rest_client.create_collection(collection_name, config)
            logger.info(f"Created VIPER collection: {collection_name}")
            
            # Phase 1: Insert large volume to trigger multiple flushes
            # Target: ~20MB total data to trigger multiple 8MB flushes
            # 384 dims * 4 bytes = 1536 bytes per vector
            # ~1300 vectors per MB, so ~26000 vectors for 20MB
            total_vectors = 20000
            batch_size = 500
            
            documents_data = []
            batch_count = 0
            
            logger.info(f"Phase 1: Inserting {total_vectors} vectors in batches of {batch_size}")
            
            for batch_start in range(0, total_vectors, batch_size):
                batch_end = min(batch_start + batch_size, total_vectors)
                
                # Generate realistic text data for embeddings
                texts = []
                for i in range(batch_start, batch_end):
                    category = ["technology", "science", "healthcare", "education", "business"][i % 5]
                    text = f"Document {i} about {category} research and innovation in modern applications"
                    texts.append(text)
                
                # Generate embeddings
                embeddings = bert_model.encode(texts)
                
                # Prepare batch data
                vectors = []
                vector_ids = []
                metadatas = []
                
                for idx, i in enumerate(range(batch_start, batch_end)):
                    vector = embeddings[idx].tolist()
                    vectors.append(vector)
                    vector_ids.append(f"viper_doc_{i}")
                    
                    metadata = {
                        "text": texts[idx],
                        "category": ["technology", "science", "healthcare", "education", "business"][i % 5],
                        "importance": (i % 10) + 1,
                        "batch": batch_count,
                        "vector_index": i,
                        "storage_layout": "viper",
                        "phase": "initial_load"
                    }
                    metadatas.append(metadata)
                    documents_data.append((f"viper_doc_{i}", vector, metadata))
                
                # Insert batch
                start_time = time.time()
                result = rest_client.insert_vectors(
                    collection_id=collection_name,
                    vectors=vectors,
                    ids=vector_ids,
                    metadata=metadatas
                )
                batch_time = time.time() - start_time
                
                batch_count += 1
                inserted_count = getattr(result, 'count', getattr(result, 'successful_count', len(vectors)))
                
                logger.info(f"VIPER Batch {batch_count}: {inserted_count} vectors in {batch_time:.2f}s")
                
                # Small delay to allow WAL processing
                time.sleep(0.1)
            
            logger.info(f"Phase 1 complete: {len(documents_data)} documents inserted")
            
            # Phase 2: Test search across WAL and flushed VIPER data
            logger.info("Phase 2: Testing search across WAL and VIPER storage")
            
            # Search queries that should match different categories
            search_queries = [
                ("technology and innovation", "technology"),
                ("healthcare and medical research", "healthcare"),
                ("scientific discoveries", "science"),
                ("business applications", "business"),
                ("educational technology", "education")
            ]
            
            for query_text, expected_category in search_queries:
                query_embedding = bert_model.encode([query_text])[0]
                
                start_time = time.time()
                results = rest_client.search(
                    collection_id=collection_name,
                    query=query_embedding.tolist(),
                    k=50,  # Large k to test ranking across storage layers
                    include_metadata=True,
                    include_vectors=False
                )
                search_time = time.time() - start_time
                
                assert len(results) > 0, f"No results for query: {query_text}"
                
                # Verify results span multiple batches (indicating data from both WAL and VIPER)
                batch_numbers = set()
                categories = {}
                
                for result in results:
                    batch_num = result.metadata.get('batch')
                    category = result.metadata.get('category')
                    
                    if batch_num is not None:
                        batch_numbers.add(batch_num)
                    if category:
                        categories[category] = categories.get(category, 0) + 1
                
                # Should have results from multiple batches (WAL + VIPER)
                assert len(batch_numbers) > 5, f"Results from too few batches: {len(batch_numbers)}"
                
                # Expected category should be well represented
                assert expected_category in categories, f"Expected category {expected_category} not found"
                
                logger.info(f"Query '{query_text}': {len(results)} results in {search_time:.3f}s, "
                           f"batches: {len(batch_numbers)}, top category: {max(categories, key=categories.get)}")
            
            # Phase 3: Trigger compaction with updates and deletes
            logger.info("Phase 3: Triggering compaction with updates")
            
            # Update a subset of vectors to create versioning pressure
            update_count = min(2000, len(documents_data) // 4)
            
            for i in range(0, update_count, 100):
                batch_end = min(i + 100, update_count)
                
                for j in range(i, batch_end):
                    vector_id, _, original_metadata = documents_data[j]
                    
                    # Generate new embedding for updated content
                    updated_text = f"Updated {original_metadata['text']} with new information"
                    updated_embedding = bert_model.encode([updated_text])[0]
                    
                    updated_metadata = original_metadata.copy()
                    updated_metadata.update({
                        "text": updated_text,
                        "phase": "updated",
                        "update_timestamp": time.time(),
                        "version": 2
                    })
                    
                    # Upsert updated vector
                    rest_client.insert_vector(
                        collection_id=collection_name,
                        vector_id=vector_id,
                        vector=updated_embedding.tolist(),
                        metadata=updated_metadata
                    )
                
                if i % 500 == 0:
                    logger.info(f"Updated {batch_end} vectors for compaction trigger")
            
            # Phase 4: Final verification search across all storage layers
            logger.info("Phase 4: Final verification search")
            
            # Complex query to test ranking across WAL, VIPER, and updated data
            complex_query = "advanced technology applications in healthcare and education"
            complex_embedding = bert_model.encode([complex_query])[0]
            
            start_time = time.time()
            final_results = rest_client.search(
                collection_id=collection_name,
                query=complex_embedding.tolist(),
                k=100,
                include_metadata=True,
                include_vectors=False
            )
            final_search_time = time.time() - start_time
            
            assert len(final_results) >= 50, f"Expected at least 50 results, got {len(final_results)}"
            
            # Analyze result distribution
            phases = {}
            categories = {}
            batches = set()
            
            for result in final_results:
                phase = result.metadata.get('phase', 'unknown')
                category = result.metadata.get('category', 'unknown')
                batch = result.metadata.get('batch')
                
                phases[phase] = phases.get(phase, 0) + 1
                categories[category] = categories.get(category, 0) + 1
                if batch is not None:
                    batches.add(batch)
            
            # Should have mix of original and updated data
            assert 'initial_load' in phases, "Should have results from initial data"
            assert len(batches) > 10, f"Results should span many batches, got {len(batches)}"
            
            # Technology and healthcare should be prominent given the query
            top_categories = sorted(categories.items(), key=lambda x: x[1], reverse=True)[:3]
            top_category_names = [cat for cat, count in top_categories]
            
            assert 'technology' in top_category_names or 'healthcare' in top_category_names, \
                f"Expected technology or healthcare in top categories: {top_category_names}"
            
            logger.info(f"VIPER final search: {len(final_results)} results in {final_search_time:.3f}s")
            logger.info(f"Phase distribution: {phases}")
            logger.info(f"Category distribution: {dict(top_categories)}")
            logger.info(f"Batch span: {len(batches)} batches")
            
        finally:
            try:
                rest_client.delete_collection(collection_name)
                logger.info(f"Cleaned up VIPER collection: {collection_name}")
            except:
                pass


class TestLSMStorageLayout:
    """Test LSM storage layout with flush and compaction operations"""
    
    @pytest.fixture(scope="class")
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture(scope="class")
    def grpc_client(self):
        client = connect_grpc("http://localhost:5679")
        yield client
        client.close()
    
    @pytest.fixture(scope="class")
    def bert_model(self):
        return SentenceTransformer('all-MiniLM-L6-v2')
    
    def test_lsm_compaction_operations(self, grpc_client, bert_model):
        """Test LSM compaction with write-heavy workloads"""
        collection_name = f"lsm_compaction_{int(time.time())}"
        
        # Configure LSM with aggressive compaction settings
        config = CollectionConfig(
            dimension=384,
            distance_metric=DistanceMetric.COSINE,
            description="LSM compaction test collection",
            storage_layout="lsm",  # LSM tree storage
            flush_config=FlushConfig(max_wal_size_mb=6.0)  # Lower threshold for more frequent flushes
        )
        
        try:
            collection = grpc_client.create_collection(collection_name, config)
            logger.info(f"Created LSM collection: {collection_name}")
            
            # Phase 1: Write-heavy workload to create multiple LSM levels
            total_documents = 15000
            batch_size = 300
            
            documents = []
            logger.info(f"Phase 1: LSM write-heavy workload - {total_documents} documents")
            
            for batch_start in range(0, total_documents, batch_size):
                batch_end = min(batch_start + batch_size, total_documents)
                
                # Generate diverse content for realistic workload
                texts = []
                for i in range(batch_start, batch_end):
                    domain = ["AI", "blockchain", "cloud", "data", "security"][i % 5]
                    subdomain = ["research", "development", "application", "analysis", "optimization"][i % 5]
                    text = f"Document {i}: {domain} {subdomain} for enterprise solutions and scalability"
                    texts.append(text)
                
                embeddings = grpc_client.encode(texts) if hasattr(grpc_client, 'encode') else bert_model.encode(texts)
                
                vectors = []
                vector_ids = []
                metadatas = []
                
                for idx, i in enumerate(range(batch_start, batch_end)):
                    vector = embeddings[idx].tolist() if hasattr(embeddings[idx], 'tolist') else embeddings[idx].tolist()
                    vectors.append(vector)
                    vector_ids.append(f"lsm_doc_{i}")
                    
                    metadata = {
                        "text": texts[idx],
                        "domain": ["AI", "blockchain", "cloud", "data", "security"][i % 5],
                        "subdomain": ["research", "development", "application", "analysis", "optimization"][i % 5],
                        "priority": (i % 5) + 1,
                        "doc_id": i,
                        "storage_layout": "lsm",
                        "batch": batch_start // batch_size,
                        "phase": "initial"
                    }
                    metadatas.append(metadata)
                    documents.append((f"lsm_doc_{i}", vector, metadata))
                
                # Insert via gRPC for LSM testing
                result = grpc_client.insert_vectors(
                    collection_id=collection_name,
                    vectors=vectors,
                    ids=vector_ids,
                    metadata=metadatas
                )
                
                batch_num = batch_start // batch_size + 1
                if batch_num % 10 == 0:
                    logger.info(f"LSM Batch {batch_num}: Inserted batch {batch_start}-{batch_end}")
            
            logger.info(f"Phase 1 complete: {len(documents)} documents in LSM")
            
            # Phase 2: Mixed read/write workload to trigger compaction
            logger.info("Phase 2: Mixed read/write workload for LSM compaction")
            
            # Interleave searches and updates
            for round_num in range(5):
                # Search phase
                search_queries = [
                    ("AI research and development", "AI"),
                    ("blockchain applications", "blockchain"),
                    ("cloud security optimization", "cloud"),
                    ("data analysis techniques", "data")
                ]
                
                for query_text, expected_domain in search_queries:
                    query_embedding = bert_model.encode([query_text])[0]
                    
                    results = grpc_client.search(
                        collection_id=collection_name,
                        query=query_embedding.tolist(),
                        k=30,
                        include_metadata=True
                    )
                    
                    assert len(results) > 0, f"No results for LSM query: {query_text}"
                    
                    # Verify domain relevance
                    domain_counts = {}
                    for result in results:
                        domain = result.metadata.get('domain')
                        if domain:
                            domain_counts[domain] = domain_counts.get(domain, 0) + 1
                    
                    if domain_counts:
                        top_domain = max(domain_counts, key=domain_counts.get)
                        logger.debug(f"LSM query '{query_text}': top domain = {top_domain}")
                
                # Update phase - create new versions to trigger compaction
                update_start = round_num * 1000
                update_end = min(update_start + 800, len(documents))
                
                for i in range(update_start, update_end, 50):
                    batch_end = min(i + 50, update_end)
                    
                    for j in range(i, batch_end):
                        if j < len(documents):
                            vector_id, _, original_metadata = documents[j]
                            
                            # Create updated version
                            updated_text = f"Updated v{round_num + 2}: {original_metadata['text']} with enhanced features"
                            updated_embedding = bert_model.encode([updated_text])[0]
                            
                            updated_metadata = original_metadata.copy()
                            updated_metadata.update({
                                "text": updated_text,
                                "phase": f"updated_round_{round_num}",
                                "version": round_num + 2,
                                "update_round": round_num
                            })
                            
                            grpc_client.insert_vector(
                                collection_id=collection_name,
                                vector_id=vector_id,
                                vector=updated_embedding.tolist(),
                                metadata=updated_metadata
                            )
                
                logger.info(f"LSM Round {round_num + 1}: Mixed workload complete")
                time.sleep(0.5)  # Allow compaction processing
            
            # Phase 3: Comprehensive search verification across LSM levels
            logger.info("Phase 3: LSM comprehensive search verification")
            
            # Test complex queries that should hit multiple LSM levels
            comprehensive_queries = [
                "AI and blockchain integration for enterprise security",
                "cloud data analysis with machine learning optimization",
                "research development applications in modern technology",
                "security optimization for scalable cloud infrastructure"
            ]
            
            for query_text in comprehensive_queries:
                query_embedding = bert_model.encode([query_text])[0]
                
                start_time = time.time()
                results = grpc_client.search(
                    collection_id=collection_name,
                    query=query_embedding.tolist(),
                    k=80,  # Large k to test cross-level ranking
                    include_metadata=True
                )
                search_time = time.time() - start_time
                
                assert len(results) >= 30, f"Expected at least 30 results for LSM query"
                
                # Analyze version distribution (should include original + updated)
                versions = {}
                phases = {}
                domains = {}
                
                for result in results:
                    version = result.metadata.get('version', 1)
                    phase = result.metadata.get('phase', 'initial')
                    domain = result.metadata.get('domain', 'unknown')
                    
                    versions[version] = versions.get(version, 0) + 1
                    phases[phase] = phases.get(phase, 0) + 1
                    domains[domain] = domains.get(domain, 0) + 1
                
                # Should have mix of versions (indicating search across LSM levels)
                assert len(versions) > 1, f"Should have multiple versions in results: {versions}"
                
                # Should have both original and updated data
                assert 'initial' in phases, "Should have results from initial phase"
                
                logger.info(f"LSM query: {len(results)} results in {search_time:.3f}s")
                logger.info(f"Version distribution: {versions}")
                logger.info(f"Phase distribution: {dict(list(phases.items())[:3])}")
            
            # Phase 4: Performance comparison with point queries
            logger.info("Phase 4: LSM point query performance")
            
            # Test point queries for specific documents
            test_ids = [f"lsm_doc_{i}" for i in [100, 5000, 10000, 14000]]
            
            for vector_id in test_ids:
                start_time = time.time()
                result = grpc_client.get_vector(
                    collection_id=vector_id,
                    vector_id=vector_id,
                    include_metadata=True
                )
                point_query_time = time.time() - start_time
                
                if result:
                    phase = result.get('metadata', {}).get('phase', 'unknown')
                    version = result.get('metadata', {}).get('version', 1)
                    logger.debug(f"LSM point query {vector_id}: {point_query_time:.4f}s, phase={phase}, v={version}")
            
            logger.info("LSM storage layout test completed successfully")
            
        finally:
            try:
                grpc_client.delete_collection(collection_name)
                logger.info(f"Cleaned up LSM collection: {collection_name}")
            except:
                pass


class TestStorageLayoutComparison:
    """Compare VIPER and LSM storage layouts under similar workloads"""
    
    @pytest.fixture(scope="class")
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture(scope="class")
    def grpc_client(self):
        client = connect_grpc("http://localhost:5679")
        yield client
        client.close()
    
    @pytest.fixture(scope="class")
    def bert_model(self):
        return SentenceTransformer('all-MiniLM-L6-v2')
    
    def test_storage_layout_performance_comparison(self, rest_client, grpc_client, bert_model):
        """Compare VIPER and LSM performance under identical workloads"""
        viper_collection = f"viper_perf_{int(time.time())}"
        lsm_collection = f"lsm_perf_{int(time.time())}"
        
        # Identical configurations except storage layout
        viper_config = CollectionConfig(
            dimension=384,
            distance_metric=DistanceMetric.COSINE,
            description="VIPER performance test",
            storage_layout="viper",
            flush_config=FlushConfig(max_wal_size_mb=10.0)
        )
        
        lsm_config = CollectionConfig(
            dimension=384,
            distance_metric=DistanceMetric.COSINE,
            description="LSM performance test",
            storage_layout="lsm",
            flush_config=FlushConfig(max_wal_size_mb=10.0)
        )
        
        try:
            # Create collections
            viper_coll = rest_client.create_collection(viper_collection, viper_config)
            lsm_coll = grpc_client.create_collection(lsm_collection, lsm_config)
            
            logger.info(f"Created comparison collections: {viper_collection} (VIPER), {lsm_collection} (LSM)")
            
            # Identical test dataset
            test_data_size = 8000
            batch_size = 200
            
            # Generate shared test data
            logger.info(f"Generating {test_data_size} test documents")
            all_texts = []
            all_metadatas = []
            
            for i in range(test_data_size):
                topic = ["artificial intelligence", "machine learning", "deep learning", 
                        "natural language processing", "computer vision"][i % 5]
                application = ["healthcare", "finance", "education", "entertainment", "research"][i % 5]
                text = f"Document {i}: {topic} applications in {application} with advanced algorithms"
                
                metadata = {
                    "text": text,
                    "topic": topic,
                    "application": application,
                    "doc_id": i,
                    "priority": (i % 3) + 1
                }
                
                all_texts.append(text)
                all_metadatas.append(metadata)
            
            # Generate embeddings once
            all_embeddings = bert_model.encode(all_texts)
            
            # Insert into VIPER collection
            logger.info("Loading data into VIPER collection")
            viper_insert_times = []
            
            for batch_start in range(0, test_data_size, batch_size):
                batch_end = min(batch_start + batch_size, test_data_size)
                
                vectors = [all_embeddings[i].tolist() for i in range(batch_start, batch_end)]
                vector_ids = [f"doc_{i}" for i in range(batch_start, batch_end)]
                metadatas = all_metadatas[batch_start:batch_end]
                
                start_time = time.time()
                result = rest_client.insert_vectors(
                    collection_id=viper_collection,
                    vectors=vectors,
                    ids=vector_ids,
                    metadata=metadatas
                )
                insert_time = time.time() - start_time
                viper_insert_times.append(insert_time)
            
            # Insert into LSM collection
            logger.info("Loading data into LSM collection")
            lsm_insert_times = []
            
            for batch_start in range(0, test_data_size, batch_size):
                batch_end = min(batch_start + batch_size, test_data_size)
                
                vectors = [all_embeddings[i].tolist() for i in range(batch_start, batch_end)]
                vector_ids = [f"doc_{i}" for i in range(batch_start, batch_end)]
                metadatas = all_metadatas[batch_start:batch_end]
                
                start_time = time.time()
                result = grpc_client.insert_vectors(
                    collection_id=lsm_collection,
                    vectors=vectors,
                    ids=vector_ids,
                    metadata=metadatas
                )
                insert_time = time.time() - start_time
                lsm_insert_times.append(insert_time)
            
            # Performance comparison
            avg_viper_insert = sum(viper_insert_times) / len(viper_insert_times)
            avg_lsm_insert = sum(lsm_insert_times) / len(lsm_insert_times)
            
            logger.info(f"Average insert time - VIPER: {avg_viper_insert:.3f}s, LSM: {avg_lsm_insert:.3f}s")
            
            # Search performance comparison
            test_queries = [
                "artificial intelligence in healthcare applications",
                "machine learning for financial analysis",
                "deep learning computer vision systems",
                "natural language processing research",
                "advanced algorithms for education technology"
            ]
            
            viper_search_times = []
            lsm_search_times = []
            
            logger.info("Comparing search performance")
            
            for query_text in test_queries:
                query_embedding = bert_model.encode([query_text])[0]
                
                # VIPER search
                start_time = time.time()
                viper_results = rest_client.search(
                    collection_id=viper_collection,
                    query=query_embedding.tolist(),
                    k=50,
                    include_metadata=True
                )
                viper_search_time = time.time() - start_time
                viper_search_times.append(viper_search_time)
                
                # LSM search
                start_time = time.time()
                lsm_results = grpc_client.search(
                    collection_id=lsm_collection,
                    query=query_embedding.tolist(),
                    k=50,
                    include_metadata=True
                )
                lsm_search_time = time.time() - start_time
                lsm_search_times.append(lsm_search_time)
                
                # Verify both return reasonable results
                assert len(viper_results) > 0, f"VIPER search returned no results for: {query_text}"
                assert len(lsm_results) > 0, f"LSM search returned no results for: {query_text}"
                
                logger.debug(f"Query '{query_text[:30]}...': VIPER={viper_search_time:.3f}s ({len(viper_results)} results), "
                           f"LSM={lsm_search_time:.3f}s ({len(lsm_results)} results)")
            
            # Performance summary
            avg_viper_search = sum(viper_search_times) / len(viper_search_times)
            avg_lsm_search = sum(lsm_search_times) / len(lsm_search_times)
            
            logger.info(f"Average search time - VIPER: {avg_viper_search:.3f}s, LSM: {avg_lsm_search:.3f}s")
            
            # Result quality comparison (should be similar for identical data)
            final_query = "comprehensive artificial intelligence machine learning applications"
            final_embedding = bert_model.encode([final_query])[0]
            
            viper_final = rest_client.search(
                collection_id=viper_collection,
                query=final_embedding.tolist(),
                k=20,
                include_metadata=True
            )
            
            lsm_final = grpc_client.search(
                collection_id=lsm_collection,
                query=final_embedding.tolist(),
                k=20,
                include_metadata=True
            )
            
            # Both should return high-quality results
            assert len(viper_final) >= 15, "VIPER should return sufficient results"
            assert len(lsm_final) >= 15, "LSM should return sufficient results"
            
            # Check score quality (top results should have good scores)
            viper_top_score = viper_final[0].score if viper_final else 0
            lsm_top_score = lsm_final[0].score if lsm_final else 0
            
            assert viper_top_score > 0.1, f"VIPER top score too low: {viper_top_score}"
            assert lsm_top_score > 0.1, f"LSM top score too low: {lsm_top_score}"
            
            logger.info(f"Result quality - VIPER top score: {viper_top_score:.3f}, LSM top score: {lsm_top_score:.3f}")
            logger.info("Storage layout comparison completed successfully")
            
        finally:
            try:
                rest_client.delete_collection(viper_collection)
                grpc_client.delete_collection(lsm_collection)
                logger.info("Cleaned up comparison collections")
            except:
                pass


class TestCrossStorageSearch:
    """Test search operations that span WAL, flushed data, and compacted data"""
    
    @pytest.fixture
    def rest_client(self):
        client = connect_rest("http://localhost:5678")
        yield client
        client.close()
    
    @pytest.fixture
    def bert_model(self):
        return SentenceTransformer('all-MiniLM-L6-v2')
    
    def test_unified_search_across_storage_layers(self, rest_client, bert_model):
        """Test search ranking across WAL, flushed, and compacted data"""
        collection_name = f"unified_search_{int(time.time())}"
        
        config = CollectionConfig(
            dimension=384,
            distance_metric=DistanceMetric.COSINE,
            description="Unified search test across storage layers",
            storage_layout="viper",
            flush_config=FlushConfig(max_wal_size_mb=5.0)
        )
        
        try:
            collection = rest_client.create_collection(collection_name, config)
            logger.info(f"Created unified search collection: {collection_name}")
            
            # Phase 1: Insert base data (will be flushed)
            base_documents = 3000
            logger.info(f"Phase 1: Inserting {base_documents} base documents (will trigger flush)")
            
            base_texts = []
            for i in range(base_documents):
                category = ["technology", "healthcare", "finance", "education"][i % 4]
                base_text = f"Base document {i}: comprehensive {category} solutions for enterprise applications"
                base_texts.append(base_text)
            
            base_embeddings = bert_model.encode(base_texts)
            
            # Insert in large batches to trigger flush
            batch_size = 500
            for batch_start in range(0, base_documents, batch_size):
                batch_end = min(batch_start + batch_size, base_documents)
                
                vectors = [base_embeddings[i].tolist() for i in range(batch_start, batch_end)]
                vector_ids = [f"base_doc_{i}" for i in range(batch_start, batch_end)]
                metadatas = []
                
                for i in range(batch_start, batch_end):
                    metadata = {
                        "text": base_texts[i],
                        "category": ["technology", "healthcare", "finance", "education"][i % 4],
                        "doc_type": "base",
                        "doc_id": i,
                        "phase": "base_data",
                        "relevance_score": (i % 10) + 1
                    }
                    metadatas.append(metadata)
                
                rest_client.insert_vectors(
                    collection_id=collection_name,
                    vectors=vectors,
                    ids=vector_ids,
                    metadata=metadatas
                )
            
            logger.info("Phase 1 complete - base data should be flushed to VIPER")
            time.sleep(1)  # Allow flush processing
            
            # Phase 2: Insert recent data (will stay in WAL initially)
            recent_documents = 800
            logger.info(f"Phase 2: Inserting {recent_documents} recent documents (WAL)")
            
            recent_texts = []
            for i in range(recent_documents):
                category = ["technology", "healthcare", "finance", "education"][i % 4]
                recent_text = f"Recent document {i}: latest {category} innovations and breakthrough developments"
                recent_texts.append(recent_text)
            
            recent_embeddings = bert_model.encode(recent_texts)
            
            for i in range(recent_documents):
                metadata = {
                    "text": recent_texts[i],
                    "category": ["technology", "healthcare", "finance", "education"][i % 4],
                    "doc_type": "recent",
                    "doc_id": base_documents + i,
                    "phase": "recent_data",
                    "relevance_score": 8 + (i % 3)  # Higher relevance for recent docs
                }
                
                rest_client.insert_vector(
                    collection_id=collection_name,
                    vector_id=f"recent_doc_{i}",
                    vector=recent_embeddings[i].tolist(),
                    metadata=metadata
                )
            
            logger.info("Phase 2 complete - recent data in WAL")
            
            # Phase 3: Update some base documents (creates versioned data)
            update_count = 400
            logger.info(f"Phase 3: Updating {update_count} documents (creates versioned data)")
            
            for i in range(0, update_count, 50):
                batch_end = min(i + 50, update_count)
                
                for j in range(i, batch_end):
                    original_text = base_texts[j]
                    updated_text = f"UPDATED: {original_text} with enhanced features and improved capabilities"
                    updated_embedding = bert_model.encode([updated_text])[0]
                    
                    updated_metadata = {
                        "text": updated_text,
                        "category": ["technology", "healthcare", "finance", "education"][j % 4],
                        "doc_type": "updated",
                        "doc_id": j,
                        "phase": "updated_data",
                        "relevance_score": 9,  # High relevance for updated docs
                        "version": 2,
                        "update_timestamp": time.time()
                    }
                    
                    rest_client.insert_vector(
                        collection_id=collection_name,
                        vector_id=f"base_doc_{j}",  # Same ID as original (upsert)
                        vector=updated_embedding.tolist(),
                        metadata=updated_metadata
                    )
            
            logger.info("Phase 3 complete - updated data creates versioning")
            
            # Phase 4: Comprehensive search testing across all storage layers
            logger.info("Phase 4: Testing unified search across all storage layers")
            
            # Test queries that should match different types of documents
            unified_queries = [
                ("technology solutions enterprise", ["technology"]),
                ("healthcare innovations breakthrough", ["healthcare"]),
                ("financial applications comprehensive", ["finance"]),
                ("education developments latest", ["education"]),
                ("enhanced features improved capabilities", ["updated"]),  # Should favor updated docs
                ("recent innovations breakthrough developments", ["recent"])  # Should favor recent docs
            ]
            
            for query_text, expected_matches in unified_queries:
                logger.info(f"Testing query: '{query_text}'")
                query_embedding = bert_model.encode([query_text])[0]
                
                start_time = time.time()
                results = rest_client.search(
                    collection_id=collection_name,
                    query=query_embedding.tolist(),
                    k=60,  # Large k to test ranking across storage layers
                    include_metadata=True,
                    include_vectors=False
                )
                search_time = time.time() - start_time
                
                assert len(results) >= 30, f"Expected at least 30 results for unified query"
                
                # Analyze result distribution across storage layers
                doc_types = {}
                phases = {}
                categories = {}
                relevance_scores = []
                
                for result in results:
                    doc_type = result.metadata.get('doc_type', 'unknown')
                    phase = result.metadata.get('phase', 'unknown')
                    category = result.metadata.get('category', 'unknown')
                    relevance = result.metadata.get('relevance_score', 0)
                    
                    doc_types[doc_type] = doc_types.get(doc_type, 0) + 1
                    phases[phase] = phases.get(phase, 0) + 1
                    categories[category] = categories.get(category, 0) + 1
                    relevance_scores.append(relevance)
                
                # Verify cross-storage search
                assert len(doc_types) >= 2, f"Should have results from multiple storage layers: {doc_types}"
                
                # Check if expected categories/types are prominent
                for expected in expected_matches:
                    if expected in ["updated", "recent"]:
                        assert expected in doc_types, f"Expected doc type '{expected}' not found in top results"
                    else:
                        assert expected in categories, f"Expected category '{expected}' not found in top results"
                
                # Verify ranking quality (should respect relevance scores and recency)
                top_10_scores = [results[i].score for i in range(min(10, len(results)))]
                assert all(top_10_scores[i] >= top_10_scores[i+1] for i in range(len(top_10_scores)-1)), \
                    "Results should be ranked by similarity score"
                
                logger.info(f"Query '{query_text}': {len(results)} results in {search_time:.3f}s")
                logger.info(f"  Doc types: {doc_types}")
                logger.info(f"  Top categories: {dict(sorted(categories.items(), key=lambda x: x[1], reverse=True)[:3])}")
                logger.info(f"  Score range: {min(top_10_scores):.3f} - {max(top_10_scores):.3f}")
            
            # Phase 5: Stress test with complex multi-layer query
            logger.info("Phase 5: Complex multi-layer stress test")
            
            complex_query = ("comprehensive technology healthcare finance education solutions "
                           "with enhanced features recent innovations breakthrough developments "
                           "for enterprise applications and improved capabilities")
            complex_embedding = bert_model.encode([complex_query])[0]
            
            start_time = time.time()
            complex_results = rest_client.search(
                collection_id=collection_name,
                query=complex_embedding.tolist(),
                k=100,  # Large result set
                include_metadata=True
            )
            complex_search_time = time.time() - start_time
            
            assert len(complex_results) >= 50, "Complex query should return substantial results"
            
            # Comprehensive analysis
            final_doc_types = {}
            final_categories = {}
            final_phases = {}
            score_by_type = {}
            
            for result in complex_results:
                doc_type = result.metadata.get('doc_type', 'unknown')
                category = result.metadata.get('category', 'unknown')
                phase = result.metadata.get('phase', 'unknown')
                
                final_doc_types[doc_type] = final_doc_types.get(doc_type, 0) + 1
                final_categories[category] = final_categories.get(category, 0) + 1
                final_phases[phase] = final_phases.get(phase, 0) + 1
                
                if doc_type not in score_by_type:
                    score_by_type[doc_type] = []
                score_by_type[doc_type].append(result.score)
            
            # Should have good representation from all storage layers
            assert len(final_doc_types) >= 3, f"Should have results from multiple doc types: {final_doc_types}"
            assert len(final_phases) >= 3, f"Should have results from multiple phases: {final_phases}"
            
            # All categories should be represented
            assert len(final_categories) == 4, f"Should have all 4 categories represented: {final_categories}"
            
            logger.info(f"Complex query: {len(complex_results)} results in {complex_search_time:.3f}s")
            logger.info(f"Final doc types: {final_doc_types}")
            logger.info(f"Final phases: {final_phases}")
            logger.info(f"Category distribution: {final_categories}")
            
            # Average scores by type (updated docs should score well due to query terms)
            for doc_type, scores in score_by_type.items():
                avg_score = sum(scores) / len(scores)
                logger.info(f"Average score for {doc_type}: {avg_score:.3f} ({len(scores)} docs)")
            
            logger.info("Unified search across storage layers completed successfully")
            
        finally:
            try:
                rest_client.delete_collection(collection_name)
                logger.info(f"Cleaned up unified search collection: {collection_name}")
            except:
                pass


if __name__ == "__main__":
    # Configure logging for detailed output
    logging.basicConfig(level=logging.INFO, format='%(asctime)s [%(levelname)s] %(message)s')
    pytest.main([__file__, "-v", "-s"])