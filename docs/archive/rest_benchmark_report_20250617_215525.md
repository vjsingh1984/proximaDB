# ProximaDB REST API Algorithm Benchmark Report
Generated: 2025-06-17T21:55:25.267392
Total Duration: 0.63 seconds
Base URL: http://localhost:5678/api/v1

## Test Configuration
- Collections Created: 5
- Embeddings Generated: 20
- Embedding Dimension: 128

## Distance Metric and Storage Layout Combinations
- cosine_standard: cosine + standard
- euclidean_standard: euclidean + standard
- dot_standard: dot_product + standard
- manhattan_viper: manhattan + viper
- cosine_viper: cosine + viper

## Insertion Performance (REST API)
| Configuration | Vectors | Success Rate | Time (s) | Vectors/sec |
|---------------|---------|--------------|----------|-------------|
| cosine_standard | 0/20 | 0.0% | 0.07 | 0.0 |
| euclidean_standard | 0/20 | 0.0% | 0.09 | 0.0 |
| dot_standard | 0/20 | 0.0% | 0.08 | 0.0 |
| manhattan_viper | 0/20 | 0.0% | 0.08 | 0.0 |
| cosine_viper | 0/20 | 0.0% | 0.11 | 0.0 |

## Search Performance (REST API)
| Configuration | Similarity Avg (ms) | ID Search Avg (ms) | Success Rate |
|---------------|---------------------|-------------------|--------------|
| cosine_standard | 6.79 | 3.74 | 100.0% |
| euclidean_standard | 5.79 | 3.66 | 100.0% |
| dot_standard | 5.60 | 3.40 | 100.0% |
| manhattan_viper | 4.92 | 3.27 | 100.0% |
| cosine_viper | 5.07 | 3.55 | 100.0% |