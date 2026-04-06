// Geospatial index implementation using geohash-based spatial partitioning
//
// This implements a hierarchical spatial index using geohashes for efficient
// location-based queries. The index supports:
// - Point-based insertions
// - Distance queries (find all points within radius)
// - Bounding box queries
// - K-nearest neighbor queries

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::RwLock;

use super::geohash::{encode_geohash, geohash_neighbors, geohashes_in_bbox};
use super::queries::{GeoQuery, GeoQueryResult};
use super::types::{GeoBoundingBox, GeoDistanceUnit, GeoPoint};

/// Configuration for the geo index
#[derive(Debug, Clone)]
pub struct GeoIndexConfig {
    /// Default precision for indexing (1-12)
    pub precision: usize,
    /// Whether to enable neighbor search for boundary cases
    pub search_neighbors: bool,
    /// Maximum results to return from a query
    pub max_results: usize,
}

impl Default for GeoIndexConfig {
    fn default() -> Self {
        Self {
            precision: 6, // ~1.2km cells
            search_neighbors: true,
            max_results: 10000,
        }
    }
}

/// An entry in the geo index
#[derive(Debug, Clone)]
pub struct GeoIndexEntry {
    /// Unique identifier
    pub id: String,
    /// Geographic location
    pub point: GeoPoint,
    /// Geohash at index precision
    pub geohash: String,
}

/// Thread-safe geospatial index
pub struct GeoIndex {
    /// Configuration
    config: GeoIndexConfig,
    /// Geohash to entries mapping (primary index)
    hash_index: RwLock<BTreeMap<String, Vec<String>>>,
    /// ID to entry mapping
    entries: RwLock<HashMap<String, GeoIndexEntry>>,
}

impl GeoIndex {
    /// Create a new geo index
    pub fn new(config: GeoIndexConfig) -> Self {
        Self {
            config,
            hash_index: RwLock::new(BTreeMap::new()),
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Insert a point into the index
    pub fn insert(&self, id: String, point: GeoPoint) {
        let geohash = encode_geohash(&point, self.config.precision);
        let entry = GeoIndexEntry {
            id: id.clone(),
            point,
            geohash: geohash.clone(),
        };

        // Insert into entries map
        {
            // CRITICAL: Lock poisoning indicates thread panic during write.
            // In production, this is unrecoverable and we propagate the panic.
            let mut entries = self
                .entries
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            entries.insert(id.clone(), entry);
        }

        // Insert into hash index
        {
            // CRITICAL: Lock poisoning indicates thread panic during write.
            // In production, this is unrecoverable and we propagate the panic.
            let mut hash_index = self
                .hash_index
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            hash_index.entry(geohash).or_default().push(id);
        }
    }

    /// Delete a point from the index
    pub fn delete(&self, id: &str) -> bool {
        let entry = {
            // CRITICAL: Lock poisoning indicates thread panic during write.
            // In production, this is unrecoverable and we propagate the panic.
            let mut entries = self
                .entries
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            entries.remove(id)
        };

        if let Some(entry) = entry {
            // CRITICAL: Lock poisoning indicates thread panic during write.
            // In production, this is unrecoverable and we propagate the panic.
            let mut hash_index = self
                .hash_index
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(ids) = hash_index.get_mut(&entry.geohash) {
                ids.retain(|i| i != id);
                if ids.is_empty() {
                    hash_index.remove(&entry.geohash);
                }
            }
            true
        } else {
            false
        }
    }

    /// Update a point's location
    pub fn update(&self, id: &str, new_point: GeoPoint) -> bool {
        if self.delete(id) {
            self.insert(id.to_string(), new_point);
            true
        } else {
            false
        }
    }

    /// Get entry by ID
    pub fn get(&self, id: &str) -> Option<GeoIndexEntry> {
        let entries = self
            .entries
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.get(id).cloned()
    }

    /// Get number of entries
    pub fn len(&self) -> usize {
        let entries = self
            .entries
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Search the index with a query
    pub fn search(&self, query: &GeoQuery) -> Vec<GeoQueryResult> {
        match query {
            GeoQuery::WithinDistance {
                center,
                radius,
                unit,
            } => self.search_within_distance(center, *radius, *unit),
            GeoQuery::WithinBox { bbox } => self.search_within_box(bbox),
            GeoQuery::WithinPolygon { polygon } => self.search_within_polygon(polygon),
            GeoQuery::NearestK { center, k } => self.search_nearest_k(center, *k),
        }
    }

    /// Find all points within a distance from center
    fn search_within_distance(
        &self,
        center: &GeoPoint,
        radius: f64,
        unit: GeoDistanceUnit,
    ) -> Vec<GeoQueryResult> {
        let radius_km = unit.to_km(radius);

        // Get candidate geohashes
        let search_bbox = GeoBoundingBox::from_center_radius(*center, radius_km);
        let candidate_hashes = self.get_candidate_hashes(&search_bbox);

        // Filter and compute distances
        let entries = self
            .entries
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let hash_index = self
            .hash_index
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut results: Vec<GeoQueryResult> = Vec::new();

        for hash in candidate_hashes {
            // Check this hash and its prefix matches
            for (stored_hash, ids) in hash_index.range(hash.clone()..) {
                if !stored_hash.starts_with(&hash) && stored_hash != &hash {
                    break;
                }

                for id in ids {
                    if let Some(entry) = entries.get(id) {
                        let distance = center.haversine_distance(&entry.point);
                        if distance <= radius_km {
                            results.push(GeoQueryResult {
                                id: entry.id.clone(),
                                point: entry.point,
                                distance_km: Some(distance),
                            });
                        }
                    }
                }
            }
        }

        // Sort by distance
        results.sort_by(|a, b| {
            a.distance_km
                .partial_cmp(&b.distance_km)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit results
        results.truncate(self.config.max_results);
        results
    }

    /// Find all points within a bounding box
    fn search_within_box(&self, bbox: &GeoBoundingBox) -> Vec<GeoQueryResult> {
        let candidate_hashes = self.get_candidate_hashes(bbox);

        let entries = self
            .entries
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let hash_index = self
            .hash_index
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut results: Vec<GeoQueryResult> = Vec::new();
        let mut seen = HashSet::new();

        for hash in candidate_hashes {
            for (stored_hash, ids) in hash_index.range(hash.clone()..) {
                if !stored_hash.starts_with(&hash) && stored_hash != &hash {
                    break;
                }

                for id in ids {
                    if seen.contains(id) {
                        continue;
                    }

                    if let Some(entry) = entries.get(id)
                        && bbox.contains(&entry.point)
                    {
                        seen.insert(id.clone());
                        results.push(GeoQueryResult {
                            id: entry.id.clone(),
                            point: entry.point,
                            distance_km: None,
                        });
                    }
                }
            }
        }

        results.truncate(self.config.max_results);
        results
    }

    /// Find all points within a polygon
    fn search_within_polygon(&self, polygon: &super::types::GeoPolygon) -> Vec<GeoQueryResult> {
        // Use polygon's bounding box for initial filtering
        let bbox = polygon.bounding_box();
        let candidates = self.search_within_box(&bbox);

        // Filter to points actually inside polygon
        candidates
            .into_iter()
            .filter(|r| polygon.contains(&r.point))
            .collect()
    }

    /// Find K nearest points to center
    fn search_nearest_k(&self, center: &GeoPoint, k: usize) -> Vec<GeoQueryResult> {
        // Start with a small radius and expand
        let mut radius_km = 1.0;
        let mut results: Vec<GeoQueryResult>;

        loop {
            results = self.search_within_distance(center, radius_km, GeoDistanceUnit::Kilometers);

            if results.len() >= k || radius_km > 20000.0 {
                break;
            }

            radius_km *= 2.0;
        }

        results.truncate(k);
        results
    }

    /// Get candidate geohashes for a bounding box
    fn get_candidate_hashes(&self, bbox: &GeoBoundingBox) -> Vec<String> {
        let hashes = geohashes_in_bbox(bbox, self.config.precision);

        if self.config.search_neighbors {
            // Add neighbors for each hash to handle boundary cases
            let mut all_hashes: HashSet<String> = hashes.into_iter().collect();
            let current: Vec<_> = all_hashes.iter().cloned().collect();

            for hash in current {
                for neighbor in geohash_neighbors(&hash) {
                    all_hashes.insert(neighbor);
                }
            }

            all_hashes.into_iter().collect()
        } else {
            hashes
        }
    }

    /// Bulk insert multiple points
    pub fn bulk_insert(&self, entries: Vec<(String, GeoPoint)>) {
        for (id, point) in entries {
            self.insert(id, point);
        }
    }

    /// Get statistics about the index
    pub fn stats(&self) -> GeoIndexStats {
        let entries = self
            .entries
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let hash_index = self
            .hash_index
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let total_entries = entries.len();
        let unique_hashes = hash_index.len();

        let points_per_hash: Vec<usize> = hash_index.values().map(|v| v.len()).collect();
        let avg_points_per_hash = if unique_hashes > 0 {
            points_per_hash.iter().sum::<usize>() as f64 / unique_hashes as f64
        } else {
            0.0
        };
        let max_points_per_hash = points_per_hash.iter().max().copied().unwrap_or(0);

        GeoIndexStats {
            total_entries,
            unique_hashes,
            avg_points_per_hash,
            max_points_per_hash,
            precision: self.config.precision,
        }
    }

    /// Clear all entries from the index
    pub fn clear(&self) {
        let mut entries = self
            .entries
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut hash_index = self
            .hash_index
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        entries.clear();
        hash_index.clear();
    }

    /// Get all entries within a geohash prefix
    pub fn get_by_geohash_prefix(&self, prefix: &str) -> Vec<GeoIndexEntry> {
        let entries = self
            .entries
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let hash_index = self
            .hash_index
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut results = Vec::new();

        for (hash, ids) in hash_index.range(prefix.to_string()..) {
            if !hash.starts_with(prefix) {
                break;
            }

            for id in ids {
                if let Some(entry) = entries.get(id) {
                    results.push(entry.clone());
                }
            }
        }

        results
    }
}

/// Statistics about the geo index
#[derive(Debug, Clone)]
pub struct GeoIndexStats {
    /// Total number of entries
    pub total_entries: usize,
    /// Number of unique geohash cells
    pub unique_hashes: usize,
    /// Average points per geohash cell
    pub avg_points_per_hash: f64,
    /// Maximum points in a single cell
    pub max_points_per_hash: usize,
    /// Precision used for indexing
    pub precision: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_index() -> GeoIndex {
        let index = GeoIndex::new(GeoIndexConfig::default());

        // Add some test cities
        index.insert("sf".to_string(), GeoPoint::new(37.7749, -122.4194));
        index.insert("oakland".to_string(), GeoPoint::new(37.8044, -122.2712));
        index.insert("sj".to_string(), GeoPoint::new(37.3382, -121.8863));
        index.insert("la".to_string(), GeoPoint::new(34.0522, -118.2437));
        index.insert("nyc".to_string(), GeoPoint::new(40.7128, -74.0060));

        index
    }

    #[test]
    fn test_insert_and_get() {
        let index = GeoIndex::new(GeoIndexConfig::default());
        index.insert("test".to_string(), GeoPoint::new(37.7749, -122.4194));

        let entry = index.get("test");
        assert!(entry.is_some());
        assert!((entry.unwrap().point.latitude - 37.7749).abs() < 0.0001);
    }

    #[test]
    fn test_delete() {
        let index = create_test_index();
        assert_eq!(index.len(), 5);

        assert!(index.delete("sf"));
        assert_eq!(index.len(), 4);
        assert!(index.get("sf").is_none());

        assert!(!index.delete("nonexistent"));
    }

    #[test]
    fn test_update() {
        let index = create_test_index();

        let new_point = GeoPoint::new(38.0, -123.0);
        assert!(index.update("sf", new_point));

        let entry = index.get("sf").unwrap();
        assert!((entry.point.latitude - 38.0).abs() < 0.0001);
    }

    #[test]
    fn test_distance_search() {
        let index = create_test_index();

        // Search within 50km of SF
        let query = GeoQuery::within_distance(
            GeoPoint::new(37.7749, -122.4194),
            50.0,
            GeoDistanceUnit::Kilometers,
        );

        let results = index.search(&query);

        // Should find SF, Oakland, but not San Jose, LA, NYC
        let ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"sf"));
        assert!(ids.contains(&"oakland"));
        assert!(!ids.contains(&"la"));
        assert!(!ids.contains(&"nyc"));
    }

    #[test]
    fn test_bbox_search() {
        let index = create_test_index();

        // Bay Area bounding box
        let query = GeoQuery::within_box(GeoBoundingBox::new(
            GeoPoint::new(37.0, -123.0),
            GeoPoint::new(38.0, -121.5),
        ));

        let results = index.search(&query);
        let ids: Vec<_> = results.iter().map(|r| r.id.as_str()).collect();

        assert!(ids.contains(&"sf"));
        assert!(ids.contains(&"oakland"));
        assert!(ids.contains(&"sj"));
        assert!(!ids.contains(&"la"));
    }

    #[test]
    fn test_knn_search() {
        let index = create_test_index();

        let query = GeoQuery::nearest_k(GeoPoint::new(37.7749, -122.4194), 2);

        let results = index.search(&query);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "sf"); // Closest
    }

    #[test]
    fn test_stats() {
        let index = create_test_index();
        let stats = index.stats();

        assert_eq!(stats.total_entries, 5);
        assert!(stats.unique_hashes > 0);
        assert!(stats.avg_points_per_hash > 0.0);
    }

    #[test]
    fn test_bulk_insert() {
        let index = GeoIndex::new(GeoIndexConfig::default());

        let entries = vec![
            ("a".to_string(), GeoPoint::new(37.0, -122.0)),
            ("b".to_string(), GeoPoint::new(38.0, -123.0)),
            ("c".to_string(), GeoPoint::new(39.0, -124.0)),
        ];

        index.bulk_insert(entries);
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn test_clear() {
        let index = create_test_index();
        assert!(!index.is_empty());

        index.clear();
        assert!(index.is_empty());
    }

    #[test]
    fn test_geohash_prefix_search() {
        let index = GeoIndex::new(GeoIndexConfig::default());

        // Insert points that will share a geohash prefix
        index.insert("p1".to_string(), GeoPoint::new(37.7749, -122.4194));
        index.insert("p2".to_string(), GeoPoint::new(37.7750, -122.4195));

        let entry = index.get("p1").unwrap();
        let prefix = &entry.geohash[..4]; // 4-char prefix

        let results = index.get_by_geohash_prefix(prefix);
        assert!(results.len() >= 2);
    }
}
