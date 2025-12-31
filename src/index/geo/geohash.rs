// Geohash encoding and decoding for spatial partitioning
//
// Geohash is a hierarchical spatial indexing system that encodes
// geographic coordinates into a short string. Adjacent geohashes
// share common prefixes, enabling efficient range queries.

use super::types::{GeoPoint, GeoBoundingBox};

/// Base32 alphabet for geohash encoding
const BASE32: &[u8] = b"0123456789bcdefghjkmnpqrstuvwxyz";

/// Geohash representation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GeoHash {
    /// The geohash string
    pub hash: String,
    /// Precision (number of characters)
    pub precision: usize,
}

impl GeoHash {
    /// Create a new geohash from a string
    pub fn new(hash: &str) -> Self {
        Self {
            hash: hash.to_lowercase(),
            precision: hash.len(),
        }
    }

    /// Create a geohash from a point with specified precision
    pub fn from_point(point: &GeoPoint, precision: usize) -> Self {
        let hash = encode_geohash(point, precision);
        Self { hash, precision }
    }

    /// Get the bounding box for this geohash
    pub fn bounding_box(&self) -> GeoBoundingBox {
        decode_geohash_bounds(&self.hash)
    }

    /// Get the center point of this geohash
    pub fn center(&self) -> GeoPoint {
        decode_geohash(&self.hash)
    }

    /// Get the 8 neighboring geohashes
    pub fn neighbors(&self) -> Vec<GeoHash> {
        geohash_neighbors(&self.hash)
            .into_iter()
            .map(|h| GeoHash::new(&h))
            .collect()
    }

    /// Check if this geohash contains another (is a prefix)
    pub fn contains(&self, other: &GeoHash) -> bool {
        other.hash.starts_with(&self.hash)
    }

    /// Get parent geohash (one level less precision)
    pub fn parent(&self) -> Option<GeoHash> {
        if self.precision <= 1 {
            None
        } else {
            Some(GeoHash::new(&self.hash[..self.precision - 1]))
        }
    }

    /// Get all children geohashes (one level more precision)
    pub fn children(&self) -> Vec<GeoHash> {
        BASE32
            .iter()
            .map(|&c| {
                let mut child = self.hash.clone();
                child.push(c as char);
                GeoHash::new(&child)
            })
            .collect()
    }
}

/// Encode a geographic point to a geohash string
pub fn encode_geohash(point: &GeoPoint, precision: usize) -> String {
    let mut lat_range = (-90.0, 90.0);
    let mut lon_range = (-180.0, 180.0);
    let mut hash = String::with_capacity(precision);
    let mut bits = 0u8;
    let mut bit_count = 0;
    let mut is_lon = true;

    while hash.len() < precision {
        if is_lon {
            let mid = (lon_range.0 + lon_range.1) / 2.0;
            if point.longitude >= mid {
                bits = (bits << 1) | 1;
                lon_range.0 = mid;
            } else {
                bits <<= 1;
                lon_range.1 = mid;
            }
        } else {
            let mid = (lat_range.0 + lat_range.1) / 2.0;
            if point.latitude >= mid {
                bits = (bits << 1) | 1;
                lat_range.0 = mid;
            } else {
                bits <<= 1;
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

/// Decode a geohash to its center point
pub fn decode_geohash(hash: &str) -> GeoPoint {
    let bounds = decode_geohash_bounds(hash);
    bounds.center()
}

/// Decode a geohash to its bounding box
pub fn decode_geohash_bounds(hash: &str) -> GeoBoundingBox {
    let mut lat_range = (-90.0, 90.0);
    let mut lon_range = (-180.0, 180.0);
    let mut is_lon = true;

    for c in hash.to_lowercase().chars() {
        let idx = BASE32.iter().position(|&b| b as char == c);
        if let Some(idx) = idx {
            for bit in (0..5).rev() {
                let mask = 1 << bit;
                if is_lon {
                    let mid = (lon_range.0 + lon_range.1) / 2.0;
                    if idx & mask != 0 {
                        lon_range.0 = mid;
                    } else {
                        lon_range.1 = mid;
                    }
                } else {
                    let mid = (lat_range.0 + lat_range.1) / 2.0;
                    if idx & mask != 0 {
                        lat_range.0 = mid;
                    } else {
                        lat_range.1 = mid;
                    }
                }
                is_lon = !is_lon;
            }
        }
    }

    GeoBoundingBox::new(
        GeoPoint::new(lat_range.0, lon_range.0),
        GeoPoint::new(lat_range.1, lon_range.1),
    )
}

/// Get the 8 neighboring geohashes
pub fn geohash_neighbors(hash: &str) -> Vec<String> {
    if hash.is_empty() {
        return Vec::new();
    }

    let bounds = decode_geohash_bounds(hash);
    let center = bounds.center();
    let width = bounds.width();
    let height = bounds.height();
    let precision = hash.len();

    // Calculate neighbor centers
    let offsets = [
        (0.0, height),      // North
        (width, height),    // Northeast
        (width, 0.0),       // East
        (width, -height),   // Southeast
        (0.0, -height),     // South
        (-width, -height),  // Southwest
        (-width, 0.0),      // West
        (-width, height),   // Northwest
    ];

    offsets
        .iter()
        .map(|(dx, dy)| {
            let neighbor_point = GeoPoint::new(center.latitude + dy, center.longitude + dx);
            encode_geohash(&neighbor_point, precision)
        })
        .collect()
}

/// Get all geohashes that cover a bounding box at a given precision
pub fn geohashes_in_bbox(bbox: &GeoBoundingBox, precision: usize) -> Vec<String> {
    let mut hashes = Vec::new();
    let mut visited = std::collections::HashSet::new();

    // Start from corners and center
    let start_points = [
        bbox.sw,
        bbox.ne,
        GeoPoint::new(bbox.sw.latitude, bbox.ne.longitude),
        GeoPoint::new(bbox.ne.latitude, bbox.sw.longitude),
        bbox.center(),
    ];

    let mut queue: std::collections::VecDeque<String> = start_points
        .iter()
        .map(|p| encode_geohash(p, precision))
        .collect();

    while let Some(hash) = queue.pop_front() {
        if visited.contains(&hash) {
            continue;
        }

        let hash_bounds = decode_geohash_bounds(&hash);
        if !bbox.intersects(&hash_bounds) {
            continue;
        }

        visited.insert(hash.clone());
        hashes.push(hash.clone());

        // Add neighbors to queue
        for neighbor in geohash_neighbors(&hash) {
            if !visited.contains(&neighbor) {
                queue.push_back(neighbor);
            }
        }
    }

    hashes
}

/// Get the geohash precision needed to cover a given area size (approximate)
pub fn precision_for_area(width_km: f64) -> usize {
    // Approximate geohash precision based on cell width
    // Precision 1: ~5000km, 2: ~1250km, 3: ~156km, 4: ~39km, 5: ~4.9km
    // 6: ~1.2km, 7: ~153m, 8: ~38m, 9: ~4.8m, 10: ~1.2m
    if width_km >= 5000.0 {
        1
    } else if width_km >= 1250.0 {
        2
    } else if width_km >= 156.0 {
        3
    } else if width_km >= 39.0 {
        4
    } else if width_km >= 4.9 {
        5
    } else if width_km >= 1.2 {
        6
    } else if width_km >= 0.153 {
        7
    } else if width_km >= 0.038 {
        8
    } else if width_km >= 0.0048 {
        9
    } else {
        10
    }
}

/// Validate a geohash string
pub fn is_valid_geohash(hash: &str) -> bool {
    !hash.is_empty()
        && hash
            .to_lowercase()
            .chars()
            .all(|c| BASE32.contains(&(c as u8)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let original = GeoPoint::new(37.7749, -122.4194); // San Francisco
        let hash = encode_geohash(&original, 10);
        let decoded = decode_geohash(&hash);

        // Should be accurate to within ~1 meter at precision 10
        assert!((original.latitude - decoded.latitude).abs() < 0.00001);
        assert!((original.longitude - decoded.longitude).abs() < 0.00001);
    }

    #[test]
    fn test_geohash_precision() {
        let point = GeoPoint::new(37.7749, -122.4194);

        let hash5 = encode_geohash(&point, 5);
        let hash8 = encode_geohash(&point, 8);

        assert_eq!(hash5.len(), 5);
        assert_eq!(hash8.len(), 8);
        assert!(hash8.starts_with(&hash5));
    }

    #[test]
    fn test_neighbors_count() {
        let hash = "9q8yy";
        let neighbors = geohash_neighbors(hash);

        assert_eq!(neighbors.len(), 8);
        assert!(!neighbors.contains(&hash.to_string()));
    }

    #[test]
    fn test_geohash_struct() {
        let gh = GeoHash::from_point(&GeoPoint::new(37.7749, -122.4194), 6);

        assert_eq!(gh.precision, 6);
        assert!(gh.parent().is_some());
        assert_eq!(gh.parent().unwrap().precision, 5);
    }

    #[test]
    fn test_geohash_contains() {
        let parent = GeoHash::new("9q8yy");
        let child = GeoHash::new("9q8yyz");
        let unrelated = GeoHash::new("dp3wt");

        assert!(parent.contains(&child));
        assert!(!parent.contains(&unrelated));
    }

    #[test]
    fn test_bbox_geohashes() {
        let bbox = GeoBoundingBox::new(
            GeoPoint::new(37.7, -122.5),
            GeoPoint::new(37.8, -122.4),
        );

        let hashes = geohashes_in_bbox(&bbox, 5);
        assert!(!hashes.is_empty());

        // All hashes should intersect the bbox
        for hash in &hashes {
            let hash_bounds = decode_geohash_bounds(hash);
            assert!(bbox.intersects(&hash_bounds));
        }
    }

    #[test]
    fn test_precision_for_area() {
        assert_eq!(precision_for_area(100.0), 4);   // 100km -> precision 4 (~39km cells)
        assert_eq!(precision_for_area(5.0), 5);    // 5km >= 4.9km -> precision 5
        assert_eq!(precision_for_area(2.0), 6);    // 2km (between 1.2 and 4.9) -> precision 6
        assert_eq!(precision_for_area(1.0), 7);    // 1km < 1.2km -> precision 7 (~153m cells)
        assert_eq!(precision_for_area(0.1), 8);    // 100m -> precision 8 (~38m cells)
    }

    #[test]
    fn test_valid_geohash() {
        assert!(is_valid_geohash("9q8yy"));
        assert!(is_valid_geohash("DPJM")); // Case insensitive
        assert!(!is_valid_geohash(""));
        assert!(!is_valid_geohash("9q8yy!")); // Invalid character
        assert!(!is_valid_geohash("9q8yya")); // 'a' is not in base32
    }

    #[test]
    fn test_geohash_children() {
        let parent = GeoHash::new("9q8");
        let children = parent.children();

        assert_eq!(children.len(), 32); // 32 base32 characters
        for child in &children {
            assert!(child.hash.starts_with("9q8"));
            assert_eq!(child.precision, 4);
        }
    }
}
