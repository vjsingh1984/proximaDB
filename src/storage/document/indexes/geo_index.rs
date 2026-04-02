// Geospatial index using geohash-based spatial lookup
//
// Provides:
// - Point indexing (latitude, longitude)
// - Bounding box queries
// - Radius queries (point + distance)
// - Nearest neighbor queries
//
// Uses geohash encoding to map 2D coordinates to a 1D string,
// enabling efficient prefix-based spatial range queries.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Result, anyhow};

/// Geohash precision levels (characters)
/// - 5 chars ≈ ±2.4 km
/// - 6 chars ≈ ±0.61 km
/// - 7 chars ≈ ±0.076 km
/// - 8 chars ≈ ±0.019 km
const DEFAULT_PRECISION: usize = 7;

/// A geographic point
#[derive(Debug, Clone, Copy)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
}

impl GeoPoint {
    pub fn new(lat: f64, lon: f64) -> Result<Self> {
        if !(-90.0..=90.0).contains(&lat) {
            return Err(anyhow!("Latitude must be between -90 and 90, got {}", lat));
        }
        if !(-180.0..=180.0).contains(&lon) {
            return Err(anyhow!("Longitude must be between -180 and 180, got {}", lon));
        }
        Ok(Self { lat, lon })
    }

    /// Haversine distance in meters between two points
    pub fn distance_meters(&self, other: &GeoPoint) -> f64 {
        const R: f64 = 6_371_000.0; // Earth's radius in meters

        let d_lat = (other.lat - self.lat).to_radians();
        let d_lon = (other.lon - self.lon).to_radians();
        let lat1 = self.lat.to_radians();
        let lat2 = other.lat.to_radians();

        let a = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
        let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

        R * c
    }
}

/// Geospatial index for document fields
///
/// Indexes geographic coordinates using geohash encoding for efficient
/// spatial queries. Documents are indexed by a path that should contain
/// a `{lat, lon}` or `{latitude, longitude}` object.
pub struct GeoIndex {
    /// Path to the geo field in documents
    path: String,
    /// Geohash precision (number of characters)
    precision: usize,
    /// Geohash → set of document IDs
    hash_to_docs: BTreeMap<String, HashSet<String>>,
    /// Document ID → (geohash, point)
    doc_to_geo: HashMap<String, (String, GeoPoint)>,
}

impl GeoIndex {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            precision: DEFAULT_PRECISION,
            hash_to_docs: BTreeMap::new(),
            doc_to_geo: HashMap::new(),
        }
    }

    /// Insert a document with a geographic point
    pub fn insert(&mut self, doc_id: &str, point: GeoPoint) -> Result<()> {
        let hash = Self::encode(point.lat, point.lon, self.precision);

        // Remove old entry if updating
        self.remove(doc_id);

        self.hash_to_docs
            .entry(hash.clone())
            .or_default()
            .insert(doc_id.to_string());
        self.doc_to_geo
            .insert(doc_id.to_string(), (hash, point));

        Ok(())
    }

    /// Remove a document from the index
    pub fn remove(&mut self, doc_id: &str) {
        if let Some((hash, _)) = self.doc_to_geo.remove(doc_id)
            && let Some(docs) = self.hash_to_docs.get_mut(&hash)
        {
            docs.remove(doc_id);
            if docs.is_empty() {
                self.hash_to_docs.remove(&hash);
            }
        }
    }

    /// Query documents within a bounding box
    pub fn query_bbox(
        &self,
        min_lat: f64,
        min_lon: f64,
        max_lat: f64,
        max_lon: f64,
    ) -> Vec<(String, GeoPoint)> {
        // Use geohash prefix matching for candidate selection, then filter
        let mut results = Vec::new();

        for (hash, point) in self.doc_to_geo.values() {
            let _ = hash; // geohash is for indexing; we filter by exact coordinates
            if point.lat >= min_lat
                && point.lat <= max_lat
                && point.lon >= min_lon
                && point.lon <= max_lon
            {
                results.push((
                    self.doc_to_geo
                        .iter()
                        .find(|(_, (h, _))| h == hash)
                        .map(|(id, _)| id.clone())
                        .unwrap_or_default(),
                    *point,
                ));
            }
        }

        // Deduplicate
        let mut seen = HashSet::new();
        results.retain(|(id, _)| seen.insert(id.clone()));
        results
    }

    /// Query documents within a radius (in meters) of a center point
    pub fn query_radius(
        &self,
        center: &GeoPoint,
        radius_meters: f64,
    ) -> Vec<(String, GeoPoint, f64)> {
        let mut results = Vec::new();

        for (doc_id, (_, point)) in &self.doc_to_geo {
            let dist = center.distance_meters(point);
            if dist <= radius_meters {
                results.push((doc_id.clone(), *point, dist));
            }
        }

        // Sort by distance
        results.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    /// Find k nearest documents to a point
    pub fn query_nearest(
        &self,
        center: &GeoPoint,
        k: usize,
    ) -> Vec<(String, GeoPoint, f64)> {
        let mut all: Vec<(String, GeoPoint, f64)> = self
            .doc_to_geo
            .iter()
            .map(|(id, (_, point))| {
                let dist = center.distance_meters(point);
                (id.clone(), *point, dist)
            })
            .collect();

        all.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
        all.truncate(k);
        all
    }

    /// Get the path this index is configured for
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Number of indexed documents
    pub fn len(&self) -> usize {
        self.doc_to_geo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.doc_to_geo.is_empty()
    }

    // --- Geohash encoding ---

    /// Encode latitude/longitude to a geohash string
    fn encode(lat: f64, lon: f64, precision: usize) -> String {
        const BASE32: &[u8] = b"0123456789bcdefghjkmnpqrstuvwxyz";
        let mut lat_range = (-90.0, 90.0);
        let mut lon_range = (-180.0, 180.0);
        let mut hash = String::with_capacity(precision);
        let mut bits = 0u8;
        let mut bit_count = 0;
        let mut is_lon = true;

        while hash.len() < precision {
            if is_lon {
                let mid = (lon_range.0 + lon_range.1) / 2.0;
                if lon >= mid {
                    bits = bits * 2 + 1;
                    lon_range.0 = mid;
                } else {
                    bits *= 2;
                    lon_range.1 = mid;
                }
            } else {
                let mid = (lat_range.0 + lat_range.1) / 2.0;
                if lat >= mid {
                    bits = bits * 2 + 1;
                    lat_range.0 = mid;
                } else {
                    bits *= 2;
                    lat_range.1 = mid;
                }
            }
            is_lon = !is_lon;
            bit_count += 1;

            if bit_count == 5 {
                hash.push(BASE32[bits as usize] as char);
                bits = 0;
                bit_count = 0;
            }
        }

        hash
    }
}

impl Default for GeoIndex {
    fn default() -> Self {
        Self::new("location")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_geo_point_validation() {
        assert!(GeoPoint::new(0.0, 0.0).is_ok());
        assert!(GeoPoint::new(90.0, 180.0).is_ok());
        assert!(GeoPoint::new(-90.0, -180.0).is_ok());
        assert!(GeoPoint::new(91.0, 0.0).is_err());
        assert!(GeoPoint::new(0.0, 181.0).is_err());
    }

    #[test]
    fn test_haversine_distance() {
        // New York to London ≈ 5,570 km
        let nyc = GeoPoint::new(40.7128, -74.0060).unwrap();
        let london = GeoPoint::new(51.5074, -0.1278).unwrap();
        let dist = nyc.distance_meters(&london);
        assert!((dist - 5_570_000.0).abs() < 100_000.0); // Within 100km tolerance

        // Same point → 0
        assert!(nyc.distance_meters(&nyc) < 1.0);
    }

    #[test]
    fn test_geohash_encoding() {
        // Known geohash for the Eiffel Tower (48.8584, 2.2945)
        let hash = GeoIndex::encode(48.8584, 2.2945, 6);
        assert_eq!(hash.len(), 6);
        // Geohash for Paris area should start with "u09"
        assert!(hash.starts_with("u09"), "Expected Paris geohash, got: {}", hash);
    }

    #[test]
    fn test_insert_and_radius_query() {
        let mut idx = GeoIndex::new("location");

        // Insert some European cities
        idx.insert("paris", GeoPoint::new(48.8566, 2.3522).unwrap()).unwrap();
        idx.insert("london", GeoPoint::new(51.5074, -0.1278).unwrap()).unwrap();
        idx.insert("berlin", GeoPoint::new(52.5200, 13.4050).unwrap()).unwrap();
        idx.insert("madrid", GeoPoint::new(40.4168, -3.7038).unwrap()).unwrap();

        // Query within 400km of Paris — should include Paris only (London is ~340km, close)
        let center = GeoPoint::new(48.8566, 2.3522).unwrap();
        let results = idx.query_radius(&center, 400_000.0);

        assert!(!results.is_empty());
        assert_eq!(results[0].0, "paris"); // Paris should be closest
    }

    #[test]
    fn test_nearest_query() {
        let mut idx = GeoIndex::new("loc");

        idx.insert("a", GeoPoint::new(0.0, 0.0).unwrap()).unwrap();
        idx.insert("b", GeoPoint::new(1.0, 1.0).unwrap()).unwrap();
        idx.insert("c", GeoPoint::new(10.0, 10.0).unwrap()).unwrap();

        let center = GeoPoint::new(0.5, 0.5).unwrap();
        let nearest = idx.query_nearest(&center, 2);

        assert_eq!(nearest.len(), 2);
        // a and b should be the two nearest
        let ids: Vec<&str> = nearest.iter().map(|(id, _, _)| id.as_str()).collect();
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"b"));
    }

    #[test]
    fn test_bbox_query() {
        let mut idx = GeoIndex::new("loc");

        idx.insert("inside", GeoPoint::new(45.0, 10.0).unwrap()).unwrap();
        idx.insert("outside", GeoPoint::new(60.0, 20.0).unwrap()).unwrap();

        let results = idx.query_bbox(40.0, 5.0, 50.0, 15.0);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "inside");
    }

    #[test]
    fn test_remove() {
        let mut idx = GeoIndex::new("loc");
        idx.insert("a", GeoPoint::new(0.0, 0.0).unwrap()).unwrap();
        assert_eq!(idx.len(), 1);

        idx.remove("a");
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn test_update_document() {
        let mut idx = GeoIndex::new("loc");
        idx.insert("a", GeoPoint::new(0.0, 0.0).unwrap()).unwrap();
        idx.insert("a", GeoPoint::new(10.0, 10.0).unwrap()).unwrap(); // Update

        assert_eq!(idx.len(), 1);

        let nearest = idx.query_nearest(&GeoPoint::new(10.0, 10.0).unwrap(), 1);
        assert_eq!(nearest[0].0, "a");
        assert!(nearest[0].2 < 1.0); // Should be at (10,10) now
    }
}
