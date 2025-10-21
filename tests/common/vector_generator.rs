//! Test Vector Generation Utilities
//!
//! Provides various patterns for generating test vectors:
//! - Random vectors
//! - Sequential vectors
//! - Clustered vectors (for spatial algorithm testing)
//! - Vectors with metadata
//!
//! This eliminates duplicated vector generation code across 10+ test files.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::HashMap;

use proximadb::proto::proximadb_v1::{SqlValue, VectorRecord, sql_value};

/// Generate random vectors with uniform distribution
///
/// # Examples
///
/// ```
/// use tests::common::vector_generator;
///
/// let vectors = vector_generator::random("my_collection", 1000, 128);
/// assert_eq!(vectors.len(), 1000);
/// assert_eq!(vectors[0].vector.len(), 128);
/// ```
pub fn random(collection_id: &str, count: usize, dimension: usize) -> Vec<VectorRecord> {
    let mut rng = rand::thread_rng();

    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{}", i),
            vector: (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect(),
            metadata: HashMap::new(),
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        })
        .collect()
}

/// Generate random vectors with a fixed seed (deterministic)
///
/// Useful for reproducible tests
pub fn random_seeded(
    collection_id: &str,
    count: usize,
    dimension: usize,
    seed: u64,
) -> Vec<VectorRecord> {
    random_seeded_with_prefix("vec", count, dimension, seed)
}

/// Generate random vectors with a fixed seed and custom ID prefix
///
/// Useful when tests expect specific ID formats (e.g., "test_vec_0")
pub fn random_seeded_with_prefix(
    id_prefix: &str,
    count: usize,
    dimension: usize,
    seed: u64,
) -> Vec<VectorRecord> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    (0..count)
        .map(|i| VectorRecord {
            id: format!("{}_{}", id_prefix, i),
            vector: (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect(),
            metadata: HashMap::new(),
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        })
        .collect()
}

/// Generate sequential vectors (useful for debugging)
///
/// Each dimension increases linearly: [0.0, 1.0, 2.0, ...]
pub fn sequential(collection_id: &str, count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{}", i),
            vector: (0..dimension).map(|d| (i * dimension + d) as f32).collect(),
            metadata: HashMap::new(),
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        })
        .collect()
}

/// Generate normalized random vectors (unit length)
///
/// Useful for cosine similarity testing
pub fn normalized(collection_id: &str, count: usize, dimension: usize) -> Vec<VectorRecord> {
    let mut rng = rand::thread_rng();

    (0..count)
        .map(|i| {
            let mut vec: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();

            // Normalize to unit length
            let magnitude: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            if magnitude > 0.0 {
                vec.iter_mut().for_each(|x| *x /= magnitude);
            }

            VectorRecord {
                id: format!("vec_{}", i),
                vector: vec,
                metadata: HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            }
        })
        .collect()
}

/// Generate clustered vectors (for testing spatial algorithms like HELIX)
///
/// Creates `num_clusters` clusters with `count/num_clusters` vectors each.
/// Each cluster has a random center with vectors distributed around it.
pub fn clustered(
    collection_id: &str,
    count: usize,
    dimension: usize,
    num_clusters: usize,
) -> Vec<VectorRecord> {
    let mut rng = rand::thread_rng();
    let per_cluster = count / num_clusters;

    // Generate cluster centers
    let centers: Vec<Vec<f32>> = (0..num_clusters)
        .map(|_| (0..dimension).map(|_| rng.gen_range(-10.0..10.0)).collect())
        .collect();

    let mut vectors = Vec::new();

    for (cluster_idx, center) in centers.iter().enumerate() {
        for vec_idx in 0..per_cluster {
            let id = format!("vec_{}_{}", cluster_idx, vec_idx);

            // Generate vector near cluster center
            let vector: Vec<f32> = center
                .iter()
                .map(|&c| c + rng.gen_range(-1.0..1.0)) // Small deviation from center
                .collect();

            vectors.push(VectorRecord {
                id,
                vector,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert(
                        "cluster".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::Int64Value(cluster_idx as i64)),
                        },
                    );
                    m
                },
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            });
        }
    }

    vectors
}

/// Generate vectors with metadata
///
/// Provides a flexible way to create vectors with custom metadata
pub fn with_metadata<F>(
    collection_id: &str,
    count: usize,
    dimension: usize,
    metadata_fn: F,
) -> Vec<VectorRecord>
where
    F: Fn(usize) -> HashMap<String, SqlValue>,
{
    let mut rng = rand::thread_rng();

    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{}", i),
            vector: (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect(),
            metadata: metadata_fn(i),
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        })
        .collect()
}

/// Preset vector generators for common test scenarios
pub mod presets {
    use super::*;

    /// Generate vectors for filter testing (with various metadata types)
    pub fn for_filter_tests(
        collection_id: &str,
        count: usize,
        dimension: usize,
    ) -> Vec<VectorRecord> {
        with_metadata(collection_id, count, dimension, |i| {
            let mut metadata = HashMap::new();

            // String metadata
            metadata.insert(
                "category".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::StringValue(format!("category_{}", i % 5))),
                },
            );

            // Float metadata
            metadata.insert(
                "price".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::NumberValue((i as f64) * 10.0)),
                },
            );

            // Boolean metadata
            metadata.insert(
                "in_stock".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::BoolValue(i % 2 == 0)),
                },
            );

            // Integer metadata
            metadata.insert(
                "created_at".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::Int64Value(1600000000 + (i as i64))),
                },
            );

            metadata
        })
    }

    /// Generate e-commerce product vectors
    pub fn ecommerce_products(
        collection_id: &str,
        count: usize,
        dimension: usize,
    ) -> Vec<VectorRecord> {
        with_metadata(collection_id, count, dimension, |i| {
            let mut metadata = HashMap::new();
            let categories = ["electronics", "clothing", "home", "toys", "books"];
            let brands = ["BrandA", "BrandB", "BrandC", "BrandD", "BrandE"];

            metadata.insert(
                "category".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::StringValue(
                        categories[i % categories.len()].to_string(),
                    )),
                },
            );

            metadata.insert(
                "brand".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::StringValue(
                        brands[i % brands.len()].to_string(),
                    )),
                },
            );

            metadata.insert(
                "price".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::NumberValue(
                        10.0 + (i % 1000) as f64 * 0.99,
                    )),
                },
            );

            metadata.insert(
                "rating".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::NumberValue(1.0 + (i % 5) as f64 * 0.8)),
                },
            );

            metadata.insert(
                "in_stock".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::BoolValue(i % 3 != 0)),
                },
            );

            metadata
        })
    }

    /// Generate document vectors for RAG testing
    pub fn rag_documents(collection_id: &str, count: usize, dimension: usize) -> Vec<VectorRecord> {
        with_metadata(collection_id, count, dimension, |i| {
            let mut metadata = HashMap::new();
            let doc_types = ["article", "paper", "blog", "documentation"];

            metadata.insert(
                "doc_type".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::StringValue(
                        doc_types[i % doc_types.len()].to_string(),
                    )),
                },
            );

            metadata.insert(
                "word_count".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::Int64Value(100 + (i % 2000) as i64)),
                },
            );

            metadata.insert(
                "published_date".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::Int64Value(
                        1640000000 + (i as i64 * 86400),
                    )),
                },
            );

            metadata.insert(
                "verified".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::BoolValue(i % 4 != 0)),
                },
            );

            metadata
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_generation() {
        let vectors = random("test_coll", 100, 128);
        assert_eq!(vectors.len(), 100);
        assert_eq!(vectors[0].vector.len(), 128);
        assert!(vectors[0].timestamp.is_some());
    }

    #[test]
    fn test_random_seeded_deterministic() {
        let vectors1 = random_seeded("test_coll", 10, 64, 42);
        let vectors2 = random_seeded("test_coll", 10, 64, 42);

        // Same seed should produce identical vectors
        for (v1, v2) in vectors1.iter().zip(vectors2.iter()) {
            assert_eq!(v1.vector, v2.vector);
        }
    }

    #[test]
    fn test_sequential_generation() {
        let vectors = sequential("test_coll", 10, 8);
        assert_eq!(vectors[0].vector[0], 0.0);
        assert_eq!(vectors[0].vector[1], 1.0);
        assert_eq!(vectors[1].vector[0], 8.0);
    }

    #[test]
    fn test_normalized_generation() {
        let vectors = normalized("test_coll", 10, 128);

        for vec in vectors {
            let magnitude: f32 = vec.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!(
                (magnitude - 1.0).abs() < 0.001,
                "Vector not normalized: {}",
                magnitude
            );
        }
    }

    #[test]
    fn test_clustered_generation() {
        let vectors = clustered("test_coll", 100, 64, 5);
        assert_eq!(vectors.len(), 100);

        // Verify cluster metadata
        for (i, vec) in vectors.iter().enumerate() {
            assert!(vec.metadata.contains_key("cluster"));
            let cluster_id = match &vec.metadata["cluster"].value {
                Some(sql_value::Value::Int64Value(id)) => *id,
                _ => panic!("Invalid cluster metadata"),
            };
            assert_eq!(cluster_id, (i / 20) as i64);
        }
    }

    #[test]
    fn test_with_metadata() {
        let vectors = with_metadata("test_coll", 10, 64, |i| {
            let mut m = HashMap::new();
            m.insert(
                "index".to_string(),
                SqlValue {
                    value: Some(sql_value::Value::Int64Value(i as i64)),
                },
            );
            m
        });

        for (i, vec) in vectors.iter().enumerate() {
            assert!(vec.metadata.contains_key("index"));
            let idx = match &vec.metadata["index"].value {
                Some(sql_value::Value::Int64Value(id)) => *id,
                _ => panic!("Invalid index metadata"),
            };
            assert_eq!(idx, i as i64);
        }
    }

    #[test]
    fn test_preset_filter_tests() {
        let vectors = presets::for_filter_tests("test_coll", 20, 128);

        for vec in vectors {
            assert!(vec.metadata.contains_key("category"));
            assert!(vec.metadata.contains_key("price"));
            assert!(vec.metadata.contains_key("in_stock"));
            assert!(vec.metadata.contains_key("created_at"));
        }
    }

    #[test]
    fn test_preset_ecommerce() {
        let vectors = presets::ecommerce_products("test_coll", 20, 128);

        for vec in vectors {
            assert!(vec.metadata.contains_key("category"));
            assert!(vec.metadata.contains_key("brand"));
            assert!(vec.metadata.contains_key("price"));
            assert!(vec.metadata.contains_key("rating"));
            assert!(vec.metadata.contains_key("in_stock"));
        }
    }
}
