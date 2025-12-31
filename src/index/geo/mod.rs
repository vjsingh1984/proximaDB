// Geospatial indexing for location-based queries
//
// Provides:
// - GeoPoint and GeoBoundingBox types
// - Geohash encoding/decoding for spatial partitioning
// - R-tree style spatial index for efficient queries
// - Query operators: distance, within, intersects

pub mod types;
pub mod geohash;
pub mod index;
pub mod queries;

pub use types::{GeoPoint, GeoBoundingBox, GeoPolygon, GeoCircle, GeoDistanceUnit};
pub use geohash::{GeoHash, encode_geohash, decode_geohash, geohash_neighbors};
pub use index::{GeoIndex, GeoIndexConfig, GeoIndexEntry};
pub use queries::{GeoQuery, GeoQueryResult, GeoQueryBuilder};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_geo_point_creation() {
        let point = GeoPoint::new(37.7749, -122.4194); // San Francisco
        assert!((point.latitude - 37.7749).abs() < 0.0001);
        assert!((point.longitude - (-122.4194)).abs() < 0.0001);
    }

    #[test]
    fn test_geo_point_validation() {
        assert!(GeoPoint::try_new(37.7749, -122.4194).is_ok());
        assert!(GeoPoint::try_new(91.0, 0.0).is_err()); // Invalid latitude
        assert!(GeoPoint::try_new(0.0, 181.0).is_err()); // Invalid longitude
    }

    #[test]
    fn test_geo_distance() {
        let sf = GeoPoint::new(37.7749, -122.4194); // San Francisco
        let la = GeoPoint::new(34.0522, -118.2437); // Los Angeles

        let distance_km = sf.haversine_distance(&la);
        // SF to LA is approximately 559 km
        assert!(distance_km > 550.0 && distance_km < 570.0);
    }

    #[test]
    fn test_bounding_box() {
        let bbox = GeoBoundingBox::new(
            GeoPoint::new(37.0, -123.0), // SW corner
            GeoPoint::new(38.0, -122.0), // NE corner
        );

        let inside = GeoPoint::new(37.5, -122.5);
        let outside = GeoPoint::new(36.0, -122.5);

        assert!(bbox.contains(&inside));
        assert!(!bbox.contains(&outside));
    }

    #[test]
    fn test_geohash_encode_decode() {
        let point = GeoPoint::new(37.7749, -122.4194);
        let hash = encode_geohash(&point, 8);

        assert_eq!(hash.len(), 8);

        let decoded = decode_geohash(&hash);
        // Decoded should be within geohash cell
        assert!((decoded.latitude - point.latitude).abs() < 0.01);
        assert!((decoded.longitude - point.longitude).abs() < 0.01);
    }

    #[test]
    fn test_geohash_neighbors() {
        let hash = "9q8yy";
        let neighbors = geohash_neighbors(hash);

        assert_eq!(neighbors.len(), 8); // 8 surrounding cells
    }

    #[test]
    fn test_geo_index_insert_search() {
        let index = GeoIndex::new(GeoIndexConfig::default());

        // Insert some points
        index.insert("sf".to_string(), GeoPoint::new(37.7749, -122.4194));
        index.insert("la".to_string(), GeoPoint::new(34.0522, -118.2437));
        index.insert("nyc".to_string(), GeoPoint::new(40.7128, -74.0060));

        // Search within 100km of SF
        let query = GeoQuery::within_distance(
            GeoPoint::new(37.7749, -122.4194),
            100.0,
            GeoDistanceUnit::Kilometers,
        );

        let results = index.search(&query);
        assert_eq!(results.len(), 1); // Only SF within 100km
        assert_eq!(results[0].id, "sf");
    }

    #[test]
    fn test_geo_index_bounding_box_search() {
        let index = GeoIndex::new(GeoIndexConfig::default());

        index.insert("sf".to_string(), GeoPoint::new(37.7749, -122.4194));
        index.insert("oakland".to_string(), GeoPoint::new(37.8044, -122.2712));
        index.insert("la".to_string(), GeoPoint::new(34.0522, -118.2437));

        // Bay Area bounding box
        let query = GeoQuery::within_box(GeoBoundingBox::new(
            GeoPoint::new(37.0, -123.0),
            GeoPoint::new(38.0, -122.0),
        ));

        let results = index.search(&query);
        assert_eq!(results.len(), 2); // SF and Oakland
    }

    #[test]
    fn test_geo_circle_contains() {
        let circle = GeoCircle::new(
            GeoPoint::new(37.7749, -122.4194),
            10.0, // 10 km radius
            GeoDistanceUnit::Kilometers,
        );

        let inside = GeoPoint::new(37.78, -122.41);
        let outside = GeoPoint::new(37.9, -122.4);

        assert!(circle.contains(&inside));
        assert!(!circle.contains(&outside));
    }

    #[test]
    fn test_geo_index_delete() {
        let index = GeoIndex::new(GeoIndexConfig::default());

        index.insert("sf".to_string(), GeoPoint::new(37.7749, -122.4194));
        index.insert("la".to_string(), GeoPoint::new(34.0522, -118.2437));

        assert_eq!(index.len(), 2);

        index.delete("sf");
        assert_eq!(index.len(), 1);

        let query = GeoQuery::within_distance(
            GeoPoint::new(37.7749, -122.4194),
            100.0,
            GeoDistanceUnit::Kilometers,
        );
        let results = index.search(&query);
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_geo_polygon_contains() {
        // Triangle around SF
        let polygon = GeoPolygon::new(vec![
            GeoPoint::new(37.0, -123.0),
            GeoPoint::new(38.5, -122.0),
            GeoPoint::new(37.0, -121.0),
        ]);

        let inside = GeoPoint::new(37.5, -122.0);
        let outside = GeoPoint::new(36.0, -122.0);

        assert!(polygon.contains(&inside));
        assert!(!polygon.contains(&outside));
    }
}
