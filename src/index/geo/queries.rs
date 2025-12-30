// Geospatial query types for location-based search
//
// Provides query definitions and result types for:
// - Distance-based queries (find all within radius)
// - Bounding box queries
// - Polygon containment queries
// - K-nearest neighbor queries

use serde::{Deserialize, Serialize};

use super::types::{GeoBoundingBox, GeoDistanceUnit, GeoPoint, GeoPolygon};

/// A geospatial query
#[derive(Debug, Clone)]
pub enum GeoQuery {
    /// Find all points within a distance from a center point
    WithinDistance {
        center: GeoPoint,
        radius: f64,
        unit: GeoDistanceUnit,
    },
    /// Find all points within a bounding box
    WithinBox {
        bbox: GeoBoundingBox,
    },
    /// Find all points within a polygon
    WithinPolygon {
        polygon: GeoPolygon,
    },
    /// Find the K nearest points to a center
    NearestK {
        center: GeoPoint,
        k: usize,
    },
}

impl GeoQuery {
    /// Create a distance query
    pub fn within_distance(center: GeoPoint, radius: f64, unit: GeoDistanceUnit) -> Self {
        Self::WithinDistance {
            center,
            radius,
            unit,
        }
    }

    /// Create a bounding box query
    pub fn within_box(bbox: GeoBoundingBox) -> Self {
        Self::WithinBox { bbox }
    }

    /// Create a polygon query
    pub fn within_polygon(polygon: GeoPolygon) -> Self {
        Self::WithinPolygon { polygon }
    }

    /// Create a K-nearest neighbor query
    pub fn nearest_k(center: GeoPoint, k: usize) -> Self {
        Self::NearestK { center, k }
    }

    /// Create a distance query in kilometers
    pub fn within_km(center: GeoPoint, radius_km: f64) -> Self {
        Self::within_distance(center, radius_km, GeoDistanceUnit::Kilometers)
    }

    /// Create a distance query in miles
    pub fn within_miles(center: GeoPoint, radius_miles: f64) -> Self {
        Self::within_distance(center, radius_miles, GeoDistanceUnit::Miles)
    }

    /// Create a distance query in meters
    pub fn within_meters(center: GeoPoint, radius_m: f64) -> Self {
        Self::within_distance(center, radius_m, GeoDistanceUnit::Meters)
    }
}

/// Result from a geospatial query
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoQueryResult {
    /// Unique identifier of the matched entity
    pub id: String,
    /// Geographic location of the matched entity
    pub point: GeoPoint,
    /// Distance from the query center (if applicable)
    pub distance_km: Option<f64>,
}

impl GeoQueryResult {
    /// Create a new query result
    pub fn new(id: String, point: GeoPoint) -> Self {
        Self {
            id,
            point,
            distance_km: None,
        }
    }

    /// Create a query result with distance
    pub fn with_distance(id: String, point: GeoPoint, distance_km: f64) -> Self {
        Self {
            id,
            point,
            distance_km: Some(distance_km),
        }
    }

    /// Get distance in specified unit
    pub fn distance_in(&self, unit: GeoDistanceUnit) -> Option<f64> {
        self.distance_km.map(|km| unit.from_km(km))
    }
}

/// Builder for constructing geo queries with filters
#[derive(Debug, Clone)]
pub struct GeoQueryBuilder {
    query: Option<GeoQuery>,
    limit: Option<usize>,
    offset: usize,
    min_distance_km: Option<f64>,
    max_distance_km: Option<f64>,
}

impl GeoQueryBuilder {
    /// Create a new query builder
    pub fn new() -> Self {
        Self {
            query: None,
            limit: None,
            offset: 0,
            min_distance_km: None,
            max_distance_km: None,
        }
    }

    /// Set the base query
    pub fn query(mut self, query: GeoQuery) -> Self {
        self.query = Some(query);
        self
    }

    /// Find points within a distance
    pub fn within_distance(
        mut self,
        center: GeoPoint,
        radius: f64,
        unit: GeoDistanceUnit,
    ) -> Self {
        self.query = Some(GeoQuery::within_distance(center, radius, unit));
        self
    }

    /// Find points within a bounding box
    pub fn within_box(mut self, bbox: GeoBoundingBox) -> Self {
        self.query = Some(GeoQuery::within_box(bbox));
        self
    }

    /// Find points within a polygon
    pub fn within_polygon(mut self, polygon: GeoPolygon) -> Self {
        self.query = Some(GeoQuery::within_polygon(polygon));
        self
    }

    /// Find K nearest neighbors
    pub fn nearest_k(mut self, center: GeoPoint, k: usize) -> Self {
        self.query = Some(GeoQuery::nearest_k(center, k));
        self
    }

    /// Limit the number of results
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Skip the first N results
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    /// Filter to points at least this far from center
    pub fn min_distance_km(mut self, min_km: f64) -> Self {
        self.min_distance_km = Some(min_km);
        self
    }

    /// Filter to points at most this far from center
    pub fn max_distance_km(mut self, max_km: f64) -> Self {
        self.max_distance_km = Some(max_km);
        self
    }

    /// Build the query (returns None if no query was set)
    pub fn build(self) -> Option<GeoQuery> {
        self.query
    }

    /// Get the limit
    pub fn get_limit(&self) -> Option<usize> {
        self.limit
    }

    /// Get the offset
    pub fn get_offset(&self) -> usize {
        self.offset
    }

    /// Get minimum distance filter
    pub fn get_min_distance_km(&self) -> Option<f64> {
        self.min_distance_km
    }

    /// Get maximum distance filter
    pub fn get_max_distance_km(&self) -> Option<f64> {
        self.max_distance_km
    }

    /// Apply filters to results
    pub fn filter_results(&self, results: Vec<GeoQueryResult>) -> Vec<GeoQueryResult> {
        let mut filtered: Vec<GeoQueryResult> = results
            .into_iter()
            .filter(|r| {
                // Apply min distance filter
                if let Some(min_km) = self.min_distance_km {
                    if let Some(dist) = r.distance_km {
                        if dist < min_km {
                            return false;
                        }
                    }
                }
                // Apply max distance filter
                if let Some(max_km) = self.max_distance_km {
                    if let Some(dist) = r.distance_km {
                        if dist > max_km {
                            return false;
                        }
                    }
                }
                true
            })
            .collect();

        // Apply offset
        if self.offset > 0 {
            filtered = filtered.into_iter().skip(self.offset).collect();
        }

        // Apply limit
        if let Some(limit) = self.limit {
            filtered.truncate(limit);
        }

        filtered
    }
}

impl Default for GeoQueryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_within_distance_query() {
        let query = GeoQuery::within_km(GeoPoint::new(37.7749, -122.4194), 10.0);

        match query {
            GeoQuery::WithinDistance {
                center,
                radius,
                unit,
            } => {
                assert!((center.latitude - 37.7749).abs() < 0.001);
                assert!((radius - 10.0).abs() < 0.001);
                assert!(matches!(unit, GeoDistanceUnit::Kilometers));
            }
            _ => panic!("Expected WithinDistance query"),
        }
    }

    #[test]
    fn test_within_box_query() {
        let bbox = GeoBoundingBox::new(
            GeoPoint::new(37.0, -123.0),
            GeoPoint::new(38.0, -122.0),
        );
        let query = GeoQuery::within_box(bbox);

        match query {
            GeoQuery::WithinBox { bbox } => {
                assert!((bbox.sw.latitude - 37.0).abs() < 0.001);
                assert!((bbox.ne.longitude - (-122.0)).abs() < 0.001);
            }
            _ => panic!("Expected WithinBox query"),
        }
    }

    #[test]
    fn test_nearest_k_query() {
        let query = GeoQuery::nearest_k(GeoPoint::new(37.7749, -122.4194), 5);

        match query {
            GeoQuery::NearestK { center, k } => {
                assert!((center.latitude - 37.7749).abs() < 0.001);
                assert_eq!(k, 5);
            }
            _ => panic!("Expected NearestK query"),
        }
    }

    #[test]
    fn test_query_result() {
        let result = GeoQueryResult::with_distance(
            "test_id".to_string(),
            GeoPoint::new(37.7749, -122.4194),
            10.5,
        );

        assert_eq!(result.id, "test_id");
        assert!(result.distance_km.is_some());
        assert!((result.distance_km.unwrap() - 10.5).abs() < 0.001);

        let miles = result.distance_in(GeoDistanceUnit::Miles);
        assert!(miles.is_some());
        assert!((miles.unwrap() - 6.524).abs() < 0.1);
    }

    #[test]
    fn test_query_builder() {
        let builder = GeoQueryBuilder::new()
            .within_distance(
                GeoPoint::new(37.7749, -122.4194),
                50.0,
                GeoDistanceUnit::Kilometers,
            )
            .limit(10)
            .offset(5)
            .min_distance_km(1.0)
            .max_distance_km(100.0);

        // Check accessors before consuming with build()
        assert_eq!(builder.get_limit(), Some(10));
        assert_eq!(builder.get_offset(), 5);
        assert_eq!(builder.get_min_distance_km(), Some(1.0));
        assert_eq!(builder.get_max_distance_km(), Some(100.0));
        assert!(builder.build().is_some());
    }

    #[test]
    fn test_filter_results() {
        let builder = GeoQueryBuilder::new()
            .min_distance_km(5.0)
            .max_distance_km(15.0)
            .limit(2);

        let results = vec![
            GeoQueryResult::with_distance("a".into(), GeoPoint::new(0.0, 0.0), 3.0),
            GeoQueryResult::with_distance("b".into(), GeoPoint::new(0.0, 0.0), 7.0),
            GeoQueryResult::with_distance("c".into(), GeoPoint::new(0.0, 0.0), 12.0),
            GeoQueryResult::with_distance("d".into(), GeoPoint::new(0.0, 0.0), 20.0),
            GeoQueryResult::with_distance("e".into(), GeoPoint::new(0.0, 0.0), 8.0),
        ];

        let filtered = builder.filter_results(results);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "b");
        assert_eq!(filtered[1].id, "c");
    }

    #[test]
    fn test_within_miles_helper() {
        let query = GeoQuery::within_miles(GeoPoint::new(40.7128, -74.0060), 5.0);

        match query {
            GeoQuery::WithinDistance { unit, radius, .. } => {
                assert!(matches!(unit, GeoDistanceUnit::Miles));
                assert!((radius - 5.0).abs() < 0.001);
            }
            _ => panic!("Expected WithinDistance query"),
        }
    }

    #[test]
    fn test_within_meters_helper() {
        let query = GeoQuery::within_meters(GeoPoint::new(51.5074, -0.1278), 500.0);

        match query {
            GeoQuery::WithinDistance { unit, radius, .. } => {
                assert!(matches!(unit, GeoDistanceUnit::Meters));
                assert!((radius - 500.0).abs() < 0.001);
            }
            _ => panic!("Expected WithinDistance query"),
        }
    }
}
