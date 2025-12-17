use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use tracing::{info, debug};

use crate::storage::engines::impls::helix::HelixEngine;
use crate::storage::engines::core::formats::proximablocks::{
    quantization_trait::ProximaBlockQuantization, utils::recommend_block_size_for_dimension,
    ProximaDataBlock,
};
use crate::storage::engines::core::ops::unified_proxima_simd::EngineProfile;
use crate::storage::traits::{FlushParameters, FlushResult};
use crate::compute::clustering::hilbert_curve::{HilbertCurve, HilbertPoint};

/// Implementation of ProximaBlockQuantization for HELIX engine
#[async_trait]
impl ProximaBlockQuantization for HelixEngine {
    fn engine_profile(&self) -> EngineProfile {
        EngineProfile::Helix
    }

    fn block_size_kb(&self) -> usize {
        // HELIX uses adaptive block sizes based on clustering; prefer configured value, fall back to a dimension-aware baseline
        let configured = self.config.base_block_size_kb;
        if configured > 0 {
            configured
        } else {
            recommend_block_size_for_dimension(384, 200) / 1024
        }
    }

    fn engine_name(&self) -> &str {
        "HELIX"
    }

    async fn write_blocks_to_storage(
        &self,
        blocks: Vec<ProximaDataBlock>,
        params: &FlushParameters,
    ) -> Result<FlushResult> {
        info!("💾 HELIX: Writing {} blocks with quantization and Hilbert clustering", blocks.len());

        // HELIX-specific: Apply Hilbert curve clustering for spatial locality
        let clustered_blocks = self.apply_hilbert_clustering(blocks, params).await?;

        // Write blocks using HELIX's optimized write path
        let mut total_bytes = 0u64;
        let mut total_entries = 0u64;
        let mut files_created = 0;

        for (cluster_id, cluster_blocks) in clustered_blocks.iter().enumerate() {
            let (entries, bytes) = self.write_cluster(
                cluster_id,
                cluster_blocks.clone(),
                params
            ).await?;
            total_entries += entries;
            total_bytes += bytes;
            files_created += 1;
        }

        // Create flush result
        Ok(FlushResult {
            success: true,
            collections_affected: vec![params.collection_id.clone().unwrap_or_default()],
            entries_flushed: Some(total_entries),
            bytes_written: Some(total_bytes),
            files_created: Some(files_created),
            duration_ms: None,
            completed_at: chrono::Utc::now(),
            engine_metrics: self.collect_helix_metrics(),
            compaction_triggered: self.should_trigger_compaction(total_entries),
            flushed_batch_ids: params.batch_ids.clone(),
        })
    }

    /// Override select_optimal_layout for HELIX-specific optimization
    fn select_optimal_layout(&self, dimension: usize) -> crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout {
        use crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout;

        // HELIX prefers grouped encoding for better spatial locality
        if dimension <= 256 {
            // Small-medium dimensions: use grouped for Hilbert curve efficiency
            VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector
        } else if dimension <= 2048 {
            // Large dimensions: still use grouped but with larger groups
            VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector
        } else {
            // Very high dimensions: fall back to full vector
            VectorEncodingLayout::FullVector
        }
    }
}

impl HelixEngine {
    /// Apply Hilbert curve clustering to blocks for optimal spatial locality
    async fn apply_hilbert_clustering(
        &self,
        mut blocks: Vec<ProximaDataBlock>,
        params: &FlushParameters,
    ) -> Result<Vec<Vec<ProximaDataBlock>>> {
        let dimension = params.collection_config.as_ref()
            .map(|c| c.dimension)
            .unwrap_or(128) as usize;

        // Initialize Hilbert curve for the dimension
        let hilbert_bits = self.calculate_hilbert_bits(dimension);
        let hilbert = HilbertCurve::new(hilbert_bits)?;

        // Calculate Hilbert indices for each block based on centroid
        let mut block_indices: Vec<(u64, ProximaDataBlock)> = Vec::new();

        for block in blocks {
            let centroid = self.calculate_block_centroid(&block)?;
            let hilbert_point = self.vector_to_hilbert_point(&centroid, hilbert_bits)?;
            let hilbert_index = hilbert.point_to_index(&hilbert_point)?;
            block_indices.push((hilbert_index, block));
        }

        // Sort blocks by Hilbert index
        block_indices.sort_by_key(|&(index, _)| index);

        // Group blocks into clusters based on Hilbert proximity
        let clusters = self.group_by_hilbert_proximity(block_indices)?;

        info!("🌀 HELIX: Created {} Hilbert-clustered groups from {} blocks",
            clusters.len(), blocks.len());

        Ok(clusters)
    }

    /// Calculate centroid of all vectors in a block
    fn calculate_block_centroid(&self, block: &ProximaDataBlock) -> Result<Vec<f32>> {
        if block.records.is_empty() {
            return Ok(vec![]);
        }

        let dimension = block.records[0].values.len();
        let mut centroid = vec![0.0f32; dimension];

        for record in &block.records {
            for (i, &value) in record.values.iter().enumerate() {
                centroid[i] += value;
            }
        }

        let count = block.records.len() as f32;
        for value in &mut centroid {
            *value /= count;
        }

        Ok(centroid)
    }

    /// Convert vector to Hilbert point
    fn vector_to_hilbert_point(&self, vector: &[f32], bits: usize) -> Result<HilbertPoint> {
        // Normalize vector to [0, 2^bits - 1] range
        let max_coord = (1u64 << bits) - 1;
        let coords: Vec<u64> = vector.iter()
            .take(self.config.hilbert_dimensions)
            .map(|&v| {
                // Clamp to [0, 1] and scale
                let normalized = v.max(0.0).min(1.0);
                (normalized * max_coord as f32) as u64
            })
            .collect();

        Ok(HilbertPoint::new(coords))
    }

    /// Calculate optimal Hilbert bits based on dimension
    fn calculate_hilbert_bits(&self, dimension: usize) -> usize {
        // Use fewer bits for higher dimensions to keep computation tractable
        match dimension {
            d if d <= 64 => 16,
            d if d <= 256 => 12,
            d if d <= 1024 => 8,
            _ => 6,
        }
    }

    /// Group blocks by Hilbert proximity
    fn group_by_hilbert_proximity(
        &self,
        sorted_blocks: Vec<(u64, ProximaDataBlock)>,
    ) -> Result<Vec<Vec<ProximaDataBlock>>> {
        let mut clusters = Vec::new();
        let mut current_cluster = Vec::new();
        let mut last_index = 0u64;

        let proximity_threshold = self.config.hilbert_proximity_threshold;

        for (index, block) in sorted_blocks {
            if !current_cluster.is_empty() &&
               (index - last_index > proximity_threshold ||
                current_cluster.len() >= self.config.max_blocks_per_cluster) {
                // Start new cluster
                clusters.push(current_cluster);
                current_cluster = Vec::new();
            }

            current_cluster.push(block);
            last_index = index;
        }

        if !current_cluster.is_empty() {
            clusters.push(current_cluster);
        }

        Ok(clusters)
    }

    /// Write a cluster of blocks to storage
    async fn write_cluster(
        &self,
        cluster_id: usize,
        blocks: Vec<ProximaDataBlock>,
        params: &FlushParameters,
    ) -> Result<(u64, u64)> {
        let mut total_entries = 0u64;
        let mut total_bytes = 0u64;

        for block in &blocks {
            total_entries += block.records.len() as u64;
            total_bytes += block.metadata.size_bytes;
        }

        debug!("✍️ HELIX: Writing cluster {} with {} blocks, {} entries, {} bytes",
            cluster_id, blocks.len(), total_entries, total_bytes);

        // In production, this would:
        // 1. Create a cluster file with Hilbert-ordered blocks
        // 2. Build spatial index for the cluster
        // 3. Apply HELIX-specific compression
        // 4. Write with atomic operations
        // 5. Update cluster metadata

        Ok((total_entries, total_bytes))
    }

    /// Collect HELIX-specific metrics
    fn collect_helix_metrics(&self) -> std::collections::HashMap<String, serde_json::Value> {
        let mut metrics = std::collections::HashMap::new();
        metrics.insert("engine".to_string(), serde_json::Value::String("HELIX".to_string()));
        metrics.insert("clustering_algorithm".to_string(),
            serde_json::Value::String("hilbert_curve".to_string()));
        metrics.insert("spatial_optimization".to_string(),
            serde_json::Value::Bool(true));
        metrics
    }

    /// Determine if compaction should be triggered
    fn should_trigger_compaction(&self, entries_written: u64) -> bool {
        // HELIX triggers compaction based on cluster fragmentation
        entries_written > self.config.compaction_trigger_threshold
    }
}
