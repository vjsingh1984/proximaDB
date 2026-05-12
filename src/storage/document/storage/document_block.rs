// Document block format for SST storage
//
// Optimized block format for JSON documents:
// - Path-aware bloom filters for fast path existence checks
// - Nested field compression
// - Delta encoding for similar documents
// - Support for binary JSONB format (MessagePack)

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::proto::proximadb_v1::{SqlObject, SqlValue, sql_value::Value as SqlVal};
use proximadb_data_model::ProximaValue;

/// Block header for document storage
#[derive(Debug, Clone)]
pub struct DocumentBlockHeader {
    /// Block version
    pub version: u32,
    /// Number of documents in block
    pub document_count: u32,
    /// Compressed size in bytes
    pub compressed_size: u64,
    /// Uncompressed size in bytes
    pub uncompressed_size: u64,
    /// Bloom filter for path existence
    pub path_bloom: Vec<u8>,
    /// Min/max values for indexed paths
    pub path_stats: HashMap<String, PathStats>,
    /// Whether documents are stored in binary JSONB format
    pub use_jsonb: bool,
}

/// Statistics for a single path
#[derive(Debug, Clone)]
pub struct PathStats {
    /// Minimum value (for range queries)
    pub min_value: Option<SqlValue>,
    /// Maximum value (for range queries)
    pub max_value: Option<SqlValue>,
    /// Number of documents with this path
    pub count: u32,
    /// Number of null values
    pub null_count: u32,
}

/// Document block for storage
#[derive(Debug)]
pub struct DocumentBlock {
    /// Block header
    pub header: DocumentBlockHeader,
    /// Document IDs
    pub ids: Vec<String>,
    /// Document data (compressed)
    pub data: Vec<u8>,
}

impl DocumentBlock {
    /// Create a new empty document block
    pub fn new() -> Self {
        Self {
            header: DocumentBlockHeader {
                version: 1,
                document_count: 0,
                compressed_size: 0,
                uncompressed_size: 0,
                path_bloom: Vec::new(),
                path_stats: HashMap::new(),
                use_jsonb: false,
            },
            ids: Vec::new(),
            data: Vec::new(),
        }
    }

    /// Build a block from documents
    pub fn from_documents(
        documents: Vec<(String, SqlObject)>,
        indexed_paths: &[String],
        use_jsonb: bool,
    ) -> Result<Self> {
        let mut block = Self::new();
        block.header.document_count = documents.len() as u32;
        block.header.use_jsonb = use_jsonb;

        // Collect IDs
        block.ids = documents.iter().map(|(id, _)| id.clone()).collect();

        // Build path statistics
        for path in indexed_paths {
            let mut stats = PathStats {
                min_value: None,
                max_value: None,
                count: 0,
                null_count: 0,
            };

            for (_, doc) in &documents {
                if let Some(value) = Self::extract_path_value(doc, path) {
                    stats.count += 1;
                    if Self::is_null(&value) {
                        stats.null_count += 1;
                    } else {
                        // Update min/max
                        stats.min_value =
                            Some(stats.min_value.take().map_or_else(
                                || value.clone(),
                                |min| Self::min_value(&min, &value),
                            ));
                        stats.max_value =
                            Some(stats.max_value.take().map_or_else(
                                || value.clone(),
                                |max| Self::max_value(&max, &value),
                            ));
                    }
                }
            }

            if stats.count > 0 {
                block.header.path_stats.insert(path.clone(), stats);
            }
        }

        // Build bloom filter from all document paths
        let mut all_paths = HashSet::new();
        for (_, doc) in &documents {
            Self::collect_paths(doc, String::new(), &mut all_paths);
        }
        // Encode bloom filter as a simple bit vector (hash-based)
        // Use a fixed-size bit array with k=3 hash functions
        let bloom_bits = 1024usize; // 128 bytes
        let mut bloom = vec![0u8; bloom_bits / 8];
        for path in &all_paths {
            let hashes = Self::bloom_hashes(path, bloom_bits);
            for h in hashes {
                bloom[h / 8] |= 1 << (h % 8);
            }
        }
        block.header.path_bloom = bloom;

        // Serialize documents
        for (_, doc) in &documents {
            let bytes = if use_jsonb {
                // Convert SqlObject to JSON then to MessagePack
                let json = serde_json::to_value(doc).unwrap_or_default();
                ProximaValue::to_jsonb_vec(&json).unwrap_or_default()
            } else {
                serde_json::to_vec(doc).unwrap_or_default()
            };
            
            block
                .data
                .extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            block.data.extend_from_slice(&bytes);
        }
        block.header.uncompressed_size = block.data.len() as u64;
        block.header.compressed_size = block.data.len() as u64;

        Ok(block)
    }

    /// Check if a path might exist in this block (using bloom filter)
    pub fn might_contain_path(&self, path: &str) -> bool {
        if self.header.path_bloom.is_empty() {
            return true; // No bloom filter built — assume it may exist
        }
        let bloom_bits = self.header.path_bloom.len() * 8;
        let hashes = Self::bloom_hashes(path, bloom_bits);
        hashes
            .iter()
            .all(|&h| (self.header.path_bloom[h / 8] & (1 << (h % 8))) != 0)
    }

    /// Compute k=3 bloom filter hash positions for a string
    fn bloom_hashes(s: &str, num_bits: usize) -> [usize; 3] {
        // Use two independent hashes and derive a third (Kirsch-Mitzenmacher)
        let mut h1: u64 = 0;
        let mut h2: u64 = 0;
        for (i, b) in s.bytes().enumerate() {
            h1 = h1.wrapping_mul(31).wrapping_add(b as u64);
            h2 = h2
                .wrapping_mul(37)
                .wrapping_add((b as u64).wrapping_add(i as u64));
        }
        let h3 = h1.wrapping_add(h2);
        [
            (h1 as usize) % num_bits,
            (h2 as usize) % num_bits,
            (h3 as usize) % num_bits,
        ]
    }

    /// Recursively collect all dotted field paths from a SqlObject
    fn collect_paths(obj: &SqlObject, prefix: String, paths: &mut HashSet<String>) {
        for (key, value) in &obj.fields {
            let full_path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", prefix, key)
            };
            paths.insert(full_path.clone());
            if let Some(SqlVal::ObjectValue(nested)) = &value.value {
                Self::collect_paths(nested, full_path, paths);
            }
        }
    }

    /// Check if a range query might match documents in this block
    pub fn might_match_range(
        &self,
        path: &str,
        min: Option<&SqlValue>,
        max: Option<&SqlValue>,
    ) -> bool {
        if let Some(stats) = self.header.path_stats.get(path) {
            // Check if ranges overlap
            if let (Some(block_min), Some(block_max)) = (&stats.min_value, &stats.max_value) {
                if let Some(query_max) = max
                    && Self::compare_values(block_min, query_max) > 0
                {
                    return false;
                }
                if let Some(query_min) = min
                    && Self::compare_values(block_max, query_min) < 0
                {
                    return false;
                }
            }
            true
        } else {
            // Path not indexed, assume it might match
            true
        }
    }

    /// Extract value at a dotted path (e.g. "address.city")
    fn extract_path_value(doc: &SqlObject, path: &str) -> Option<SqlValue> {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current_obj = doc;

        for (i, part) in parts.iter().enumerate() {
            if let Some(val) = current_obj.fields.get(*part) {
                if i == parts.len() - 1 {
                    return Some(val.clone());
                }
                // Navigate into nested object
                if let Some(SqlVal::ObjectValue(nested)) = &val.value {
                    current_obj = nested;
                } else {
                    return None; // Path continues but value is not an object
                }
            } else {
                return None; // Path segment not found
            }
        }
        None
    }

    /// Check if a value is null
    fn is_null(value: &SqlValue) -> bool {
        matches!(
            &value.value,
            Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(_))
        )
    }

    /// Compare two values, returning -1, 0, or 1
    fn compare_values(a: &SqlValue, b: &SqlValue) -> i32 {
        match (&a.value, &b.value) {
            (Some(SqlVal::Int64Value(ai)), Some(SqlVal::Int64Value(bi))) => ai.cmp(bi) as i32,
            (Some(SqlVal::NumberValue(af)), Some(SqlVal::NumberValue(bf))) => {
                af.partial_cmp(bf).map_or(0, |o| o as i32)
            }
            (Some(SqlVal::StringValue(sa)), Some(SqlVal::StringValue(sb))) => sa.cmp(sb) as i32,
            (Some(SqlVal::BoolValue(ba)), Some(SqlVal::BoolValue(bb))) => ba.cmp(bb) as i32,
            // Cross-type: int vs float
            (Some(SqlVal::Int64Value(ai)), Some(SqlVal::NumberValue(bf))) => {
                (*ai as f64).partial_cmp(bf).map_or(0, |o| o as i32)
            }
            (Some(SqlVal::NumberValue(af)), Some(SqlVal::Int64Value(bi))) => {
                af.partial_cmp(&(*bi as f64)).map_or(0, |o| o as i32)
            }
            // Nulls compare less than non-nulls
            (Some(SqlVal::NullValue(_)), Some(SqlVal::NullValue(_))) => 0,
            (Some(SqlVal::NullValue(_)), _) => -1,
            (_, Some(SqlVal::NullValue(_))) => 1,
            _ => 0, // Incomparable types treated as equal
        }
    }

    /// Get minimum of two values
    fn min_value(a: &SqlValue, b: &SqlValue) -> SqlValue {
        if Self::compare_values(a, b) <= 0 {
            a.clone()
        } else {
            b.clone()
        }
    }

    /// Get maximum of two values
    fn max_value(a: &SqlValue, b: &SqlValue) -> SqlValue {
        if Self::compare_values(a, b) >= 0 {
            a.clone()
        } else {
            b.clone()
        }
    }
}

impl Default for DocumentBlock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sql_string(s: &str) -> SqlValue {
        SqlValue {
            value: Some(SqlVal::StringValue(s.to_string())),
        }
    }

    fn make_sql_int(i: i64) -> SqlValue {
        SqlValue {
            value: Some(SqlVal::Int64Value(i)),
        }
    }

    fn make_doc(fields: Vec<(&str, SqlValue)>) -> SqlObject {
        SqlObject {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }

    #[test]
    fn test_document_block_new() {
        let block = DocumentBlock::new();
        assert_eq!(block.header.document_count, 0);
        assert!(block.ids.is_empty());
    }

    #[test]
    fn test_bloom_filter_positive() {
        let docs = vec![
            (
                "d1".to_string(),
                make_doc(vec![
                    ("name", make_sql_string("Alice")),
                    ("age", make_sql_int(30)),
                ]),
            ),
            (
                "d2".to_string(),
                make_doc(vec![
                    ("name", make_sql_string("Bob")),
                    ("age", make_sql_int(25)),
                ]),
            ),
        ];
        let block =
            DocumentBlock::from_documents(docs, &["name".to_string(), "age".to_string()], false).unwrap();

        assert!(block.might_contain_path("name"));
        assert!(block.might_contain_path("age"));
    }

    #[test]
    fn test_bloom_filter_negative() {
        let docs = vec![(
            "d1".to_string(),
            make_doc(vec![("name", make_sql_string("Alice"))]),
        )];
        let block = DocumentBlock::from_documents(docs, &["name".to_string()], false).unwrap();

        // "zzz_nonexistent_field" should almost certainly be negative
        // (false positive rate is very low for 1024-bit filter with few entries)
        assert!(!block.might_contain_path("zzz_nonexistent_field_12345"));
    }

    #[test]
    fn test_extract_path_value() {
        let doc = make_doc(vec![
            ("city", make_sql_string("London")),
            ("pop", make_sql_int(9000000)),
        ]);
        let val = DocumentBlock::extract_path_value(&doc, "city").unwrap();
        assert_eq!(val.value, Some(SqlVal::StringValue("London".to_string())));
    }

    #[test]
    fn test_compare_values_int() {
        let a = make_sql_int(10);
        let b = make_sql_int(20);
        assert!(DocumentBlock::compare_values(&a, &b) < 0);
        assert!(DocumentBlock::compare_values(&b, &a) > 0);
        assert_eq!(DocumentBlock::compare_values(&a, &a), 0);
    }

    #[test]
    fn test_path_stats_populated() {
        let docs = vec![
            (
                "d1".to_string(),
                make_doc(vec![("score", make_sql_int(10))]),
            ),
            (
                "d2".to_string(),
                make_doc(vec![("score", make_sql_int(50))]),
            ),
            (
                "d3".to_string(),
                make_doc(vec![("score", make_sql_int(30))]),
            ),
        ];
        let block = DocumentBlock::from_documents(docs, &["score".to_string()], false).unwrap();
        let stats = block.header.path_stats.get("score").unwrap();
        assert_eq!(stats.count, 3);
    }

    #[test]
    fn test_data_serialized() {
        let docs = vec![(
            "d1".to_string(),
            make_doc(vec![("k", make_sql_string("v"))]),
        )];
        let block = DocumentBlock::from_documents(docs, &[], false).unwrap();
        assert!(
            !block.data.is_empty(),
            "documents should be serialized into data"
        );
    }

    #[test]
    fn test_jsonb_data_serialized() {
        let docs = vec![(
            "d1".to_string(),
            make_doc(vec![("k", make_sql_string("v"))]),
        )];
        let block = DocumentBlock::from_documents(docs, &[], true).unwrap();
        assert!(block.header.use_jsonb);
        assert!(
            !block.data.is_empty(),
            "documents should be serialized into data (jsonb)"
        );
    }
}
