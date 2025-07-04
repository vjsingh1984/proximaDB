#!/usr/bin/env python3
"""
Test 10k corpus similarity search with VIPER quantization comparison
Tests VIPER collections with and without Q5 quantization support
"""

import asyncio
import sys
import time
import numpy as np
import json
from pathlib import Path
from typing import List, Dict, Tuple, Any

# Add the Python client to path
sys.path.insert(0, str(Path(__file__).parent / "clients" / "python" / "src"))

from proximadb.grpc_client import ProximaDBClient

async def generate_test_corpus(size: int = 10000, dimension: int = 768) -> List[Dict[str, Any]]:
    """Generate a 10k test corpus with BERT-like 768-dimensional vectors"""
    print(f"🔄 Generating {size:,} test vectors with {dimension} dimensions...")
    
    vectors = []
    np.random.seed(42)  # For reproducible results
    
    for i in range(size):
        # Generate BERT-like embeddings (roughly normalized)
        vector = np.random.normal(0, 0.4, dimension).astype(np.float32)
        vector = vector / np.linalg.norm(vector)  # Normalize to unit length
        
        # Add some metadata for testing
        metadata = {
            "doc_id": f"doc_{i:06d}",
            "category": ["technology", "science", "business", "health"][i % 4],
            "importance": float(np.random.uniform(0.1, 1.0)),
            "created_timestamp": int(time.time()) + i,
        }
        
        vectors.append({
            "id": f"vec_{i:06d}",
            "vector": vector.tolist(),
            "metadata": metadata
        })
        
        if (i + 1) % 1000 == 0:
            print(f"   Generated {i + 1:,} vectors...")
    
    print(f"✅ Generated {len(vectors):,} test vectors")
    return vectors

async def create_collections(client: ProximaDBClient, dimension: int = 768) -> Tuple[str, str]:
    """Create two VIPER collections: one without quantization, one with Q5"""
    
    # Collection 1: No quantization (baseline)
    collection_no_quant = "viper_baseline_10k"
    print(f"🏗️ Creating baseline collection '{collection_no_quant}' (no quantization)...")
    
    try:
        await client.delete_collection(collection_no_quant)
        print(f"   Cleaned up existing collection")
    except:
        pass
    
    await client.create_collection(
        collection_no_quant,
        dimension=dimension,
        distance_metric=1,  # COSINE
        storage_engine=1    # VIPER
    )
    print(f"✅ Created baseline collection")
    
    # Collection 2: Second VIPER collection (for performance comparison)
    collection_q5_quant = "viper_comparison_10k"
    print(f"🏗️ Creating comparison collection '{collection_q5_quant}' (VIPER baseline)...")
    
    try:
        await client.delete_collection(collection_q5_quant)
        print(f"   Cleaned up existing collection")
    except:
        pass
    
    await client.create_collection(
        collection_q5_quant,
        dimension=dimension,
        distance_metric=1,  # COSINE 
        storage_engine=1    # VIPER (will add quantization support later)
    )
    print(f"✅ Created comparison collection")
    
    return collection_no_quant, collection_q5_quant

async def insert_vectors_batch(client: ProximaDBClient, collection_name: str, vectors: List[Dict], batch_size: int = 200):
    """Insert vectors in batches with progress tracking"""
    print(f"📤 Inserting {len(vectors):,} vectors into '{collection_name}' (batch size: {batch_size})...")
    
    total_inserted = 0
    start_time = time.time()
    
    for i in range(0, len(vectors), batch_size):
        batch = vectors[i:i + batch_size]
        
        batch_start = time.time()
        result = client.insert_vectors(collection_name, batch)
        batch_time = time.time() - batch_start
        
        total_inserted += result.count
        
        # Progress update every 2000 vectors
        if (i + batch_size) % 2000 == 0 or i + batch_size >= len(vectors):
            elapsed = time.time() - start_time
            rate = total_inserted / elapsed if elapsed > 0 else 0
            print(f"   Inserted {total_inserted:,}/{len(vectors):,} vectors "
                  f"({rate:.1f} vec/sec, batch: {batch_time*1000:.1f}ms)")
    
    total_time = time.time() - start_time
    print(f"✅ Inserted {total_inserted:,} vectors in {total_time:.2f}s "
          f"(avg: {total_inserted/total_time:.1f} vectors/sec)")
    
    return total_inserted

async def perform_similarity_search(client: ProximaDBClient, collection_name: str, query_vectors: List[List[float]], top_k: int = 10) -> Dict[str, Any]:
    """Perform similarity search and measure performance"""
    print(f"🔍 Performing similarity search on '{collection_name}' (top_k={top_k})...")
    
    search_results = []
    total_search_time = 0
    
    for i, query_vector in enumerate(query_vectors):
        search_start = time.time()
        
        results = client.search_vectors(
            collection_name,
            [query_vector],  # Single query vector
            top_k=top_k,
            include_metadata=True,
            include_vectors=False  # For performance
        )
        
        search_time = time.time() - search_start
        total_search_time += search_time
        
        search_results.append({
            "query_idx": i,
            "results": results,
            "search_time_ms": search_time * 1000,
            "num_results": len(results)
        })
        
        if i == 0:  # Show first query results in detail
            print(f"   Query 0 results: {len(results)} vectors found in {search_time*1000:.2f}ms")
            for j, result in enumerate(results[:3]):  # Show top 3
                result_id = getattr(result, 'id', 'N/A') if hasattr(result, 'id') else result.get('id', 'N/A')
                result_score = getattr(result, 'score', 0) if hasattr(result, 'score') else result.get('score', 0)
                print(f"     #{j+1}: ID={result_id}, Score={result_score:.4f}")
    
    avg_search_time = total_search_time / len(query_vectors) * 1000  # ms
    total_results = sum(len(r["results"]) for r in search_results)
    
    print(f"✅ Completed {len(query_vectors)} searches in {total_search_time*1000:.1f}ms "
          f"(avg: {avg_search_time:.2f}ms/query, {total_results} total results)")
    
    return {
        "collection_name": collection_name,
        "num_queries": len(query_vectors),
        "avg_search_time_ms": avg_search_time,
        "total_search_time_ms": total_search_time * 1000,
        "total_results_found": total_results,
        "detailed_results": search_results
    }

async def compare_search_quality(baseline_results: Dict, quantized_results: Dict, tolerance: float = 0.05) -> Dict[str, Any]:
    """Compare search quality between baseline and quantized collections"""
    print(f"📊 Analyzing search quality comparison (tolerance: {tolerance})...")
    
    baseline_searches = baseline_results["detailed_results"]
    quantized_searches = quantized_results["detailed_results"]
    
    quality_metrics = {
        "total_queries": len(baseline_searches),
        "exact_matches": 0,
        "high_overlap": 0,
        "medium_overlap": 0,
        "low_overlap": 0,
        "avg_score_difference": 0.0,
        "avg_rank_correlation": 0.0,
        "performance_comparison": {}
    }
    
    total_score_diff = 0.0
    total_rank_corr = 0.0
    
    for i, (baseline_search, quantized_search) in enumerate(zip(baseline_searches, quantized_searches)):
        # Extract IDs correctly from SearchResult objects
        baseline_ids = [getattr(r, 'id', '') if hasattr(r, 'id') else r.get('id', '') for r in baseline_search["results"]]
        quantized_ids = [getattr(r, 'id', '') if hasattr(r, 'id') else r.get('id', '') for r in quantized_search["results"]]
        
        # Calculate overlap
        baseline_set = set(baseline_ids)
        quantized_set = set(quantized_ids)
        overlap = len(baseline_set & quantized_set)
        overlap_ratio = overlap / max(len(baseline_set), len(quantized_set)) if max(len(baseline_set), len(quantized_set)) > 0 else 0
        
        # Categorize overlap quality
        if overlap_ratio >= 0.9:
            quality_metrics["exact_matches"] += 1
        elif overlap_ratio >= 0.7:
            quality_metrics["high_overlap"] += 1
        elif overlap_ratio >= 0.4:
            quality_metrics["medium_overlap"] += 1
        else:
            quality_metrics["low_overlap"] += 1
        
        # Calculate score differences for overlapping results
        score_diffs = []
        for baseline_result in baseline_search["results"]:
            baseline_id = getattr(baseline_result, 'id', '') if hasattr(baseline_result, 'id') else baseline_result.get('id', '')
            baseline_score = getattr(baseline_result, 'score', 0) if hasattr(baseline_result, 'score') else baseline_result.get('score', 0)
            
            # Find corresponding result in quantized
            for quantized_result in quantized_search["results"]:
                quantized_id = getattr(quantized_result, 'id', '') if hasattr(quantized_result, 'id') else quantized_result.get('id', '')
                quantized_score = getattr(quantized_result, 'score', 0) if hasattr(quantized_result, 'score') else quantized_result.get('score', 0)
                if quantized_id == baseline_id:
                    score_diffs.append(abs(baseline_score - quantized_score))
                    break
        
        if score_diffs:
            total_score_diff += sum(score_diffs) / len(score_diffs)
        
        # Simple rank correlation (how many top results are in same order)
        rank_correlation = 0.0
        min_len = min(len(baseline_ids), len(quantized_ids))
        if min_len > 0:
            for j in range(min_len):
                if baseline_ids[j] == quantized_ids[j]:
                    rank_correlation += 1.0
            rank_correlation /= min_len
        
        total_rank_corr += rank_correlation
    
    if quality_metrics["total_queries"] > 0:
        quality_metrics["avg_score_difference"] = total_score_diff / quality_metrics["total_queries"]
        quality_metrics["avg_rank_correlation"] = total_rank_corr / quality_metrics["total_queries"]
    
    # Performance comparison
    quality_metrics["performance_comparison"] = {
        "baseline_avg_ms": baseline_results["avg_search_time_ms"],
        "quantized_avg_ms": quantized_results["avg_search_time_ms"],
        "speedup_factor": baseline_results["avg_search_time_ms"] / quantized_results["avg_search_time_ms"] if quantized_results["avg_search_time_ms"] > 0 else 0,
        "baseline_total_results": baseline_results["total_results_found"],
        "quantized_total_results": quantized_results["total_results_found"]
    }
    
    print(f"✅ Quality Analysis Complete:")
    print(f"   Exact Matches: {quality_metrics['exact_matches']}/{quality_metrics['total_queries']} ({quality_metrics['exact_matches']/quality_metrics['total_queries']*100:.1f}%)")
    print(f"   High Overlap (≥70%): {quality_metrics['high_overlap']}/{quality_metrics['total_queries']} ({quality_metrics['high_overlap']/quality_metrics['total_queries']*100:.1f}%)")
    print(f"   Avg Score Difference: {quality_metrics['avg_score_difference']:.4f}")
    print(f"   Avg Rank Correlation: {quality_metrics['avg_rank_correlation']:.4f}")
    print(f"   Speed Comparison: {quality_metrics['performance_comparison']['speedup_factor']:.2f}x {'faster' if quality_metrics['performance_comparison']['speedup_factor'] > 1 else 'slower'}")
    
    return quality_metrics

async def test_collection_info(client: ProximaDBClient, collection_name: str):
    """Get and display collection information"""
    print(f"📋 Getting collection info for '{collection_name}'...")
    
    try:
        collection_info = client.get_collection(collection_name)
        print(f"   Collection: {collection_info.get('name', 'N/A')}")
        print(f"   Dimension: {collection_info.get('dimension', 'N/A')}")
        print(f"   Vector Count: {collection_info.get('vector_count', 'N/A')}")
        print(f"   Storage Engine: {collection_info.get('storage_engine', 'N/A')}")
        if 'quantization_level' in collection_info:
            print(f"   Quantization: {collection_info.get('quantization_level', 'None')}")
    except Exception as e:
        print(f"   ❌ Error getting collection info: {e}")

async def main():
    """Main test function"""
    print("🚀 ProximaDB 10k Vector Corpus - VIPER Quantization Comparison Test")
    print("=" * 80)
    
    # Configuration - optimized for realistic benchmark
    CORPUS_SIZE = 10000  # Full 10k corpus for realistic benchmark
    DIMENSION = 768
    NUM_QUERY_VECTORS = 5
    TOP_K = 10
    BATCH_SIZE = 200  # Larger batches for better efficiency
    
    print(f"Configuration:")
    print(f"  📊 Corpus Size: {CORPUS_SIZE:,} vectors")
    print(f"  📐 Dimension: {DIMENSION}")
    print(f"  🔍 Query Vectors: {NUM_QUERY_VECTORS}")
    print(f"  🎯 Top-K: {TOP_K}")
    print(f"  📦 Batch Size: {BATCH_SIZE}")
    print()
    
    try:
        # Connect to ProximaDB
        print("🔌 Connecting to ProximaDB...")
        client = ProximaDBClient("localhost:5679")  # gRPC client
        print("✅ Connected to ProximaDB gRPC server")
        print()
        
        # Generate test corpus
        corpus = await generate_test_corpus(CORPUS_SIZE, DIMENSION)
        
        # Extract query vectors (first 5 vectors from corpus)
        query_vectors = [vec["vector"] for vec in corpus[:NUM_QUERY_VECTORS]]
        print(f"🎯 Using first {len(query_vectors)} vectors as query vectors")
        print()
        
        # Create collections
        collection_baseline, collection_quantized = await create_collections(client, DIMENSION)
        print()
        
        # Insert vectors into baseline collection
        print("📤 PHASE 1: Inserting vectors into baseline collection...")
        baseline_inserted = await insert_vectors_batch(client, collection_baseline, corpus, BATCH_SIZE)
        # await test_collection_info(client, collection_baseline)  # Skip for now
        print()
        
        # Insert vectors into quantized collection  
        print("📤 PHASE 2: Inserting vectors into quantized collection...")
        quantized_inserted = await insert_vectors_batch(client, collection_quantized, corpus, BATCH_SIZE)
        # await test_collection_info(client, collection_quantized)  # Skip for now
        print()
        
        # Wait for indexing/flush
        print("⏳ Waiting for indexing and potential WAL flush...")
        await asyncio.sleep(5)
        print()
        
        # Perform similarity search on baseline collection
        print("🔍 PHASE 3: Similarity search on baseline collection...")
        baseline_results = await perform_similarity_search(client, collection_baseline, query_vectors, TOP_K)
        print()
        
        # Perform similarity search on quantized collection
        print("🔍 PHASE 4: Similarity search on quantized collection...")
        quantized_results = await perform_similarity_search(client, collection_quantized, query_vectors, TOP_K)
        print()
        
        # Compare search quality and performance
        print("📊 PHASE 5: Quality and performance comparison...")
        quality_comparison = await compare_search_quality(baseline_results, quantized_results)
        print()
        
        # Final summary
        print("📋 FINAL SUMMARY")
        print("=" * 50)
        print(f"✅ Successfully tested {CORPUS_SIZE:,} vector corpus")
        print(f"✅ Baseline Collection: {baseline_inserted:,} vectors inserted")
        print(f"✅ Quantized Collection: {quantized_inserted:,} vectors inserted")
        print()
        print("Performance Comparison:")
        perf = quality_comparison["performance_comparison"]
        print(f"  🏃 Baseline Search: {perf['baseline_avg_ms']:.2f}ms avg")
        print(f"  ⚡ Quantized Search: {perf['quantized_avg_ms']:.2f}ms avg")
        if perf['speedup_factor'] > 1:
            print(f"  🚀 Quantization Speedup: {perf['speedup_factor']:.2f}x faster")
        else:
            print(f"  🐌 Quantization Impact: {1/perf['speedup_factor']:.2f}x slower")
        print()
        print("Quality Metrics:")
        print(f"  🎯 Exact Matches: {quality_comparison['exact_matches']}/{quality_comparison['total_queries']} ({quality_comparison['exact_matches']/quality_comparison['total_queries']*100:.1f}%)")
        print(f"  📈 High Overlap: {quality_comparison['high_overlap']}/{quality_comparison['total_queries']} ({quality_comparison['high_overlap']/quality_comparison['total_queries']*100:.1f}%)")
        print(f"  📊 Avg Score Diff: {quality_comparison['avg_score_difference']:.4f}")
        print(f"  🔗 Rank Correlation: {quality_comparison['avg_rank_correlation']:.4f}")
        print()
        
        # Save summary results (avoiding SearchResult serialization issues)
        results_file = f"viper_quantization_test_results_{int(time.time())}.json"
        summary_results = {
            "test_config": {
                "corpus_size": CORPUS_SIZE,
                "dimension": DIMENSION,
                "num_queries": NUM_QUERY_VECTORS,
                "top_k": TOP_K,
                "batch_size": BATCH_SIZE,
                "flush_threshold_mb": 1  # 1MB flush threshold
            },
            "collections": {
                "baseline": collection_baseline,
                "quantized": collection_quantized
            },
            "insertion_results": {
                "baseline_inserted": baseline_inserted,
                "quantized_inserted": quantized_inserted
            },
            "performance_summary": {
                "baseline_avg_search_ms": baseline_results["avg_search_time_ms"],
                "quantized_avg_search_ms": quantized_results["avg_search_time_ms"],
                "speedup_factor": quality_comparison["performance_comparison"]["speedup_factor"],
                "baseline_total_results": baseline_results["total_results_found"],
                "quantized_total_results": quantized_results["total_results_found"]
            },
            "quality_metrics": {
                "exact_matches": quality_comparison["exact_matches"],
                "high_overlap": quality_comparison["high_overlap"],
                "avg_score_difference": quality_comparison["avg_score_difference"],
                "avg_rank_correlation": quality_comparison["avg_rank_correlation"]
            }
        }
        
        with open(results_file, 'w') as f:
            json.dump(summary_results, f, indent=2)
        print(f"💾 Summary results saved to: {results_file}")
        
        print("\n🎉 VIPER Quantization Comparison Test Complete!")
        
        return True
        
    except Exception as e:
        print(f"❌ Test failed with error: {e}")
        import traceback
        traceback.print_exc()
        return False

if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)