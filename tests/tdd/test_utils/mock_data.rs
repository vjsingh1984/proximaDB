//! Mock data generators for tests
//!
//! These generators create realistic test data:
//! - Random vectors (normalized and unnormalized)
//! - Text documents
//! - Time-series data (OHLCV bars)
//! - Graph nodes and edges
//! - Event sourcing events

use rand::Rng;
use std::collections::HashMap;

/// Mock data generators
pub struct MockData;

impl MockData {
    /// Generate random vector of given dimension
    ///
    /// # Arguments
    /// * `dimension` - Vector dimensionality
    ///
    /// # Returns
    /// Vector with random values in [0, 1)
    ///
    /// # Example
    /// ```no_run
    /// use proxima::tdd::test_utils::MockData;
    ///
    /// let vec = MockData::random_vector(128);
    /// assert_eq!(vec.len(), 128);
    /// ```
    pub fn random_vector(dimension: usize) -> Vec<f32> {
        let mut rng = rand::thread_rng();
        (0..dimension).map(|_| rng.gen()).collect()
    }

    /// Generate normalized random vector (L2 norm = 1.0)
    ///
    /// # Arguments
    /// * `dimension` - Vector dimensionality
    ///
    /// # Returns
    /// Unit vector with random direction
    pub fn random_normalized_vector(dimension: usize) -> Vec<f32> {
        let mut vec = Self::random_vector(dimension);
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm > 0.0 {
            vec.iter_mut().for_each(|x| *x /= norm);
        }

        vec
    }

    /// Generate random vector with given range
    ///
    /// # Arguments
    /// * `dimension` - Vector dimensionality
    /// * `min` - Minimum value (inclusive)
    /// * `max` - Maximum value (exclusive)
    pub fn random_vector_range(dimension: usize, min: f32, max: f32) -> Vec<f32> {
        let mut rng = rand::thread_rng();
        (0..dimension)
            .map(|_| rng.gen_range(min..max))
            .collect()
    }

    /// Generate multiple random vectors
    ///
    /// # Arguments
    /// * `count` - Number of vectors to generate
    /// * `dimension` - Vector dimensionality
    pub fn random_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
        (0..count)
            .map(|_| Self::random_vector(dimension))
            .collect()
    }

    /// Generate multiple normalized random vectors
    pub fn random_normalized_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
        (0..count)
            .map(|_| Self::random_normalized_vector(dimension))
            .collect()
    }

    /// Generate random text document
    ///
    /// # Arguments
    /// * `word_count` - Number of words in document
    ///
    /// # Returns
    /// Random text using common ML/search terminology
    pub fn random_text(word_count: usize) -> String {
        let words = vec![
            "machine",
            "learning",
            "database",
            "vector",
            "search",
            "embedding",
            "semantic",
            "retrieval",
            "hybrid",
            "fusion",
            "neural",
            "network",
            "deep",
            "learning",
            "natural",
            "language",
            "processing",
            "computer",
            "vision",
            "graph",
            "traversal",
            "time",
            "series",
            "analytics",
            "query",
            "optimization",
            "index",
            "clustering",
        ];

        let mut rng = rand::thread_rng();
        (0..word_count)
            .map(|_| words[rng.gen_range(0..words.len())])
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Generate random text document with topic
    ///
    /// # Arguments
    /// * `word_count` - Number of words
    /// * `topic` - Topic word to include multiple times
    pub fn random_text_with_topic(word_count: usize, topic: &str) -> String {
        let mut text = Self::random_text(word_count);

        // Insert topic at random positions
        let mut rng = rand::thread_rng();
        let insertions = rng.gen_range(1..4);

        for _ in 0..insertions {
            let pos = rng.gen_range(0..word_count);
            let words: Vec<&str> = text.split_whitespace().collect();
            if pos < words.len() {
                words[pos] = topic;
            }
        }

        text
    }

    /// Generate random metadata JSON
    pub fn random_metadata() -> serde_json::Value {
        let mut rng = rand::thread_rng();
        let mut map = serde_json::Map::new();

        map.insert(
            "id".to_string(),
            serde_json::json!(format!("id_{}", rng.gen::<u64>())),
        );

        map.insert(
            "timestamp".to_string(),
            serde_json::json!(chrono::Utc::now().to_rfc3339()),
        );

        map.insert("score".to_string(), serde_json::json!(rng.gen::<f64>()));

        if rng.gen_bool(0.5) {
            map.insert(
                "category".to_string(),
                serde_json::json!(["tech", "finance", "health", "sports"]
                    [rng.gen_range(0..4)]),
            );
        }

        serde_json::Value::Object(map)
    }

    /// Generate OHLCV bar data
    ///
    /// # Arguments
    /// * `timestamp_ns` - Timestamp in nanoseconds
    ///
    /// # Returns
    /// Realistic OHLCV bar with random but consistent data
    pub fn random_ohlcv_bar(timestamp_ns: i64) -> crate::storage::engines::impls::tst::OHLCVBar {
        let mut rng = rand::thread_rng();

        let open = 100.0 + rng.gen::<f64>() * 50.0;
        let close = open + (rng.gen::<f64>() - 0.5) * 10.0;
        let high = open.max(close) + rng.gen::<f64>() * 5.0;
        let low = open.min(close) - rng.gen::<f64>() * 5.0;

        crate::storage::engines::impls::tst::OHLCVBar {
            timestamp_ns,
            symbol: "TEST".to_string(),
            granularity_ns: 86_400_000_000_000, // 1 day
            open,
            high,
            low,
            close,
            volume: rng.gen::<u64>() % 1_000_000,
            trade_count: rng.gen::<u64>() % 10_000,
            vwap: Some((open + close) / 2.0),
            twap: Some((open + close) / 2.0),
        }
    }

    /// Generate time-series point
    pub fn random_timeseries_point(timestamp_ns: i64) -> crate::storage::engines::impls::tst::TimeSeriesPoint {
        let mut rng = rand::thread_rng();

        crate::storage::engines::impls::tst::TimeSeriesPoint {
            timestamp_ns,
            symbol: "AAPL".to_string(),
            value: 100.0 + rng.gen::<f64>() * 50.0,
            volume: Some(rng.gen::<u64>() % 10_000),
            metadata: {
                let mut map = HashMap::new();
                map.insert("source".to_string(), serde_json::json!("test"));
                Some(map)
            },
        }
    }

    /// Generate sequence of OHLCV bars
    ///
    /// # Arguments
    /// * `count` - Number of bars to generate
    /// * `start_timestamp_ns` - Starting timestamp
    /// * `interval_ns` - Time between bars
    ///
    /// # Returns
    /// Vec of OHLCV bars with consecutive timestamps
    pub fn random_ohlcv_bars(
        count: usize,
        start_timestamp_ns: i64,
        interval_ns: i64,
    ) -> Vec<crate::storage::engines::impls::tst::OHLCVBar> {
        (0..count)
            .map(|i| {
                let timestamp = start_timestamp_ns + (i as i64 * interval_ns);
                Self::random_ohlcv_bar(timestamp)
            })
            .collect()
    }

    /// Generate graph node
    pub fn random_graph_node(id: &str) -> crate::graph::Node {
        let mut rng = rand::thread_rng();
        let mut properties = HashMap::new();

        properties.insert(
            "label".to_string(),
            serde_json::json!(["Person", "Organization", "Location"]
                [rng.gen_range(0..3)]),
        );

        properties.insert("score".to_string(), serde_json::json!(rng.gen::<f64>()));

        crate::graph::Node {
            id: id.to_string(),
            label: Some(properties["label"].as_str().unwrap().to_string()),
            properties,
        }
    }

    /// Generate graph edge
    pub fn random_graph_edge(from: &str, to: &str) -> crate::graph::Edge {
        let mut rng = rand::thread_rng();

        crate::graph::Edge {
            id: format!("edge_{}", uuid::Uuid::new_v4()),
            from_node: from.to_string(),
            to_node: to.to_string(),
            edge_type: ["KNOWS", "LIKES", "FOLLOWS", "WORKS_AT"]
                [rng.gen_range(0..4)]
                .to_string(),
            weight: Some(rng.gen::<f64>()),
            properties: HashMap::new(),
        }
    }

    /// Generate event sourcing event
    pub fn random_event(event_type: &str, timestamp_ns: i64) -> crate::storage::engines::impls::event_source::Event {
        let mut rng = rand::thread_rng();

        let mut data = serde_json::Map::new();
        data.insert(
            "value".to_string(),
            serde_json::json!(rng.gen::<f64>()),
        );
        data.insert("description".to_string(), serde_json::json!("Test event"));

        crate::storage::engines::impls::event_source::Event {
            id: format!("evt_{}", uuid::Uuid::new_v4()),
            event_type: event_type.to_string(),
            timestamp_ns,
            entity_id: format!("entity_{}", rng.gen::<u64>()),
            correlation_id: None,
            causation_id: None,
            data: serde_json::Value::Object(data),
            prev_hash: String::new(),
            hash: String::new(),
            signature: None,
            metadata: HashMap::new(),
        }
    }

    /// Generate log entry
    pub fn random_log_entry() -> crate::observability::LogEntry {
        let mut rng = rand::thread_rng();

        crate::observability::LogEntry {
            timestamp_ns: chrono::Utc::now().timestamp_nanos_opt().unwrap(),
            message: Self::random_text(10),
            severity: [
                crate::observability::Severity::Trace,
                crate::observability::Severity::Debug,
                crate::observability::Severity::Info,
                crate::observability::Severity::Warn,
                crate::observability::Severity::Error,
            ][rng.gen_range(0..5)],
            service: ["api", "worker", "scheduler"][rng.gen_range(0..3)].to_string(),
            source: "test".to_string(),
            fields: {
                let mut fields = HashMap::new();
                fields.insert("host".to_string(), serde_json::json!("localhost"));
                fields.insert("pid".to_string(), serde_json::json!(rng.gen::<u32>()));
                fields
            },
        }
    }

    /// Generate correlated documents (for testing recall)
    ///
    /// Creates documents with known similarity structure
    ///
    /// # Arguments
    /// * `num_clusters` - Number of topic clusters
    /// * `docs_per_cluster` - Documents per cluster
    /// * `dimension` - Vector dimension
    pub fn clustered_documents(
        num_clusters: usize,
        docs_per_cluster: usize,
        dimension: usize,
    ) -> (Vec<Vec<f32>>, Vec<String>) {
        let mut vectors = Vec::new();
        let mut texts = Vec::new();

        // Generate cluster centers
        let cluster_centers: Vec<Vec<f32>> =
            Self::random_normalized_vectors(num_clusters, dimension);

        for cluster_idx in 0..num_clusters {
            let center = &cluster_centers[cluster_idx];
            let topic = format!("topic_{}", cluster_idx);

            for _ in 0..docs_per_cluster {
                // Generate document vector near cluster center
                let mut vec = center.clone();
                // Add small random noise
                for v in vec.iter_mut() {
                    *v += (rand::random::<f32>() - 0.5) * 0.1;
                }
                // Renormalize
                let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                vec.iter_mut().for_each(|x| *x /= norm);

                vectors.push(vec);
                texts.push(Self::random_text_with_topic(15, &topic));
            }
        }

        (vectors, texts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_vector_dimension() {
        let vec = MockData::random_vector(128);
        assert_eq!(vec.len(), 128);
    }

    #[test]
    fn test_random_normalized_vector_norm() {
        let vec = MockData::random_normalized_vector(128);
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        AssertApprox::assert_close_f32(norm, 1.0, 0.0001);
    }

    #[test]
    fn test_random_text_word_count() {
        let text = MockData::random_text(10);
        let word_count = text.split_whitespace().count();
        assert_eq!(word_count, 10);
    }

    #[test]
    fn test_random_text_with_topic() {
        let text = MockData::random_text_with_topic(20, "neural");
        assert!(text.contains("neural"));
    }

    #[test]
    fn test_random_ohlcv_bar_consistency() {
        let bar = MockData::random_ohlcv_bar(1_600_000_000_000_000_000);

        // High should be >= open and close
        assert!(bar.high >= bar.open);
        assert!(bar.high >= bar.close);

        // Low should be <= open and close
        assert!(bar.low <= bar.open);
        assert!(bar.low <= bar.close);

        // Volume should be non-negative
        assert!(bar.volume >= 0);
    }

    #[test]
    fn test_random_ohlcv_bars_sequence() {
        let bars = MockData::random_ohlcv_bars(5, 1_600_000_000_000_000_000, 86_400_000_000_000);

        assert_eq!(bars.len(), 5);

        // Check timestamps are sequential
        for i in 1..bars.len() {
            assert!(bars[i].timestamp_ns > bars[i - 1].timestamp_ns);
        }
    }

    #[test]
    fn test_clustered_documents() {
        let (vectors, texts) = MockData::clustered_documents(3, 10, 128);

        assert_eq!(vectors.len(), 30);
        assert_eq!(texts.len(), 30);

        // All vectors should be normalized
        for vec in &vectors {
            let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            AssertApprox::assert_close_f32(norm, 1.0, 0.001);
        }
    }
}
