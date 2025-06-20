use crate::core::VectorRecord;
use crate::storage::encoding::{ColumnFamilyConfig, ColumnStorageFormat, CompressionType};
use std::collections::HashMap;

/// Column Family storage similar to HBase/Cassandra
/// Allows different storage strategies per column family
pub struct ColumnFamilyStorage {
    families: HashMap<String, ColumnFamilyConfig>,
}

impl ColumnFamilyStorage {
    pub fn new() -> Self {
        Self {
            families: HashMap::new(),
        }
    }

    pub fn add_family(&mut self, config: ColumnFamilyConfig) {
        self.families.insert(config.name.clone(), config);
    }

    /// Get suggested column families for vector database workloads
    pub fn default_families() -> Vec<ColumnFamilyConfig> {
        vec![
            // Vector data - optimized for similarity search
            ColumnFamilyConfig {
                name: "vectors".to_string(),
                compression: CompressionType::Lz4, // Fast decompression for searches
                ttl_seconds: None,
                storage_format: ColumnStorageFormat::Vector { dimension: 0 }, // Will be set per collection
            },
            // Metadata - optimized for filtering
            ColumnFamilyConfig {
                name: "metadata".to_string(),
                compression: CompressionType::Zstd { level: 3 }, // Better compression for text
                ttl_seconds: None,
                storage_format: ColumnStorageFormat::Document {
                    max_document_size_kb: 64,
                },
            },
            // Analytics data - optimized for OLAP queries
            ColumnFamilyConfig {
                name: "analytics".to_string(),
                compression: CompressionType::Zstd { level: 6 }, // High compression
                ttl_seconds: Some(86400 * 30),                   // 30 days TTL
                storage_format: ColumnStorageFormat::ParquetRowGroup { batch_size: 1000 },
            },
            // Hot cache - frequently accessed data
            ColumnFamilyConfig {
                name: "hot_cache".to_string(),
                compression: CompressionType::None, // No compression for speed
                ttl_seconds: Some(3600),            // 1 hour TTL
                storage_format: ColumnStorageFormat::KeyValue,
            },
        ]
    }

    pub fn route_record(&self, record: &VectorRecord) -> Vec<(String, Vec<u8>)> {
        let mut routed_data = Vec::new();

        for (family_name, config) in &self.families {
            match &config.storage_format {
                ColumnStorageFormat::Vector { .. } => {
                    // Store vector data
                    if let Ok(encoded) = self.encode_vector_data(record, config) {
                        routed_data.push((family_name.clone(), encoded));
                    }
                }
                ColumnStorageFormat::Document { .. } => {
                    // Store metadata
                    if let Ok(encoded) = self.encode_metadata(record, config) {
                        routed_data.push((family_name.clone(), encoded));
                    }
                }
                ColumnStorageFormat::ParquetRowGroup { .. } => {
                    // Store for analytics (batched)
                    if let Ok(encoded) = self.encode_for_analytics(record, config) {
                        routed_data.push((family_name.clone(), encoded));
                    }
                }
                ColumnStorageFormat::KeyValue => {
                    // Store as simple key-value
                    if let Ok(encoded) = bincode::serialize(record) {
                        routed_data.push((family_name.clone(), encoded));
                    }
                }
            }
        }

        routed_data
    }

    fn encode_vector_data(
        &self,
        record: &VectorRecord,
        config: &ColumnFamilyConfig,
    ) -> crate::Result<Vec<u8>> {
        // Encode just the vector with metadata for reconstruction
        let vector_data = serde_json::json!({
            "id": record.id,
            "vector": record.vector,
            "timestamp": record.timestamp,
        });

        let encoded = serde_json::to_vec(&vector_data)?;
        self.apply_compression(&encoded, &config.compression)
    }

    fn encode_metadata(
        &self,
        record: &VectorRecord,
        config: &ColumnFamilyConfig,
    ) -> crate::Result<Vec<u8>> {
        // Encode metadata with ID for joins
        let metadata_record = serde_json::json!({
            "id": record.id,
            "collection_id": record.collection_id,
            "metadata": record.metadata,
            "timestamp": record.timestamp,
        });

        let encoded = serde_json::to_vec(&metadata_record)?;
        self.apply_compression(&encoded, &config.compression)
    }

    fn encode_for_analytics(
        &self,
        record: &VectorRecord,
        _config: &ColumnFamilyConfig,
    ) -> crate::Result<Vec<u8>> {
        // This would typically be batched and stored as Parquet
        // For now, just serialize the record
        let encoded = bincode::serialize(record)?;
        Ok(encoded)
    }

    fn apply_compression(
        &self,
        data: &[u8],
        compression: &CompressionType,
    ) -> crate::Result<Vec<u8>> {
        match compression {
            CompressionType::None => Ok(data.to_vec()),
            CompressionType::Lz4 => {
                // lz4_flex::compress_prepend_size returns Vec<u8> directly
                Ok(lz4_flex::compress_prepend_size(data))
            }
            CompressionType::Zstd { level } => Ok(zstd::encode_all(data, *level)?),
            CompressionType::Gzip => {
                use std::io::Write;
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(data)?;
                Ok(encoder.finish()?)
            }
            CompressionType::Snappy => {
                // Would need snappy crate
                unimplemented!("Snappy compression not yet implemented")
            }
        }
    }
}

impl Default for ColumnFamilyStorage {
    fn default() -> Self {
        let mut storage = Self::new();
        for family in Self::default_families() {
            storage.add_family(family);
        }
        storage
    }
}
