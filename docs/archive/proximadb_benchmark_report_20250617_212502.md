# ProximaDB Algorithm and Distance Metric Benchmark Report
Generated: 2025-06-17T21:25:02.533622
Total Duration: 7.05 seconds

## Test Configuration
- Sample Text Size: 100,000 bytes (~100KB)
- Embeddings Generated: 178
- Collections Created: 8
- Embedding Dimension: 768 (BERT-like)

## Algorithm and Distance Metric Combinations Tested
- hnsw_cosine: hnsw + cosine
- hnsw_euclidean: hnsw + euclidean
- hnsw_dot: hnsw + dot_product
- ivf_cosine: ivf + cosine
- ivf_euclidean: ivf + euclidean
- brute_cosine: brute_force + cosine
- brute_manhattan: brute_force + manhattan
- auto_cosine: auto + cosine

## Insertion Performance
| Configuration | Vectors | Time (s) | Vectors/sec | Status |
|---------------|---------|----------|-------------|--------|
| hnsw_cosine | - | - | - | ❌ |
| hnsw_euclidean | - | - | - | ❌ |
| hnsw_dot | - | - | - | ❌ |
| ivf_cosine | - | - | - | ❌ |
| ivf_euclidean | - | - | - | ❌ |
| brute_cosine | - | - | - | ❌ |
| brute_manhattan | - | - | - | ❌ |
| auto_cosine | - | - | - | ❌ |

## Search Performance
| Configuration | Similarity Avg (ms) | ID Search Avg (ms) | Status |
|---------------|---------------------|-------------------|--------|
| hnsw_cosine | 0.00 | 6.49 | ❌ |
| hnsw_euclidean | 0.00 | 6.96 | ❌ |
| hnsw_dot | 0.00 | 6.86 | ❌ |
| ivf_cosine | 0.00 | 6.87 | ❌ |
| ivf_euclidean | 0.00 | 5.16 | ❌ |
| brute_cosine | 0.00 | 7.00 | ❌ |
| brute_manhattan | 0.00 | 5.35 | ❌ |
| auto_cosine | 0.00 | 3.81 | ❌ |