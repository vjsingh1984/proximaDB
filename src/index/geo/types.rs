// Geospatial types for location-based indexing
//
// Provides core geometric types:
// - GeoPoint: Latitude/longitude coordinate
// - GeoBoundingBox: Rectangular region
// - GeoCircle: Circular region
// - GeoPolygon: Arbitrary polygon

use serde::{Deserialize, Serialize};

/// Earth's radius in kilometers
pub const EARTH_RADIUS_KM: f64 = 6371.0;

/// Earth's radius in miles
pub const EARTH_RADIUS_MILES: f64 = 3959.0;

/// A geographic point with latitude and longitude
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    /// Latitude in degrees (-90 to 90)
    pub latitude: f64,
    /// Longitude in degrees (-180 to 180)
    pub longitude: f64,
}

impl GeoPoint {
    /// Create a new GeoPoint (unchecked)
    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self { latitude, longitude }
    }

    /// Create a new GeoPoint with validation
    pub fn try_new(latitude: f64, longitude: f64) -> Result<Self, GeoError> {
        if !(-90.0..=90.0).contains(&latitude) {
            return Err(GeoError::InvalidLatitude(latitude));
        }
        if !(-180.0..=180.0).contains(&longitude) {
            return Err(GeoError::InvalidLongitude(longitude));
        }
        Ok(Self { latitude, longitude })
    }

    /// Calculate the Haversine distance to another point in kilometers
    pub fn haversine_distance(&self, other: &GeoPoint) -> f64 {
        self.haversine_distance_with_radius(other, EARTH_RADIUS_KM)
    }

    /// Calculate the Haversine distance with a custom radius
    pub fn haversine_distance_with_radius(&self, other: &GeoPoint, radius: f64) -> f64 {
        let lat1_rad = self.latitude.to_radians();
        let lat2_rad = other.latitude.to_radians();
        let delta_lat = (other.latitude - self.latitude).to_radians();
        let delta_lon = (other.longitude - self.longitude).to_radians();

        let a = (delta_lat / 2.0).sin().powi(2)
            + lat1_rad.cos() * lat2_rad.cos() * (delta_lon / 2.0).sin().powi(2);
        let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

        radius * c
    }

    /// Calculate the Haversine distance in miles
    pub fn haversine_distance_miles(&self, other: &GeoPoint) -> f64 {
        self.haversine_distance_with_radius(other, EARTH_RADIUS_MILES)
    }

    /// Calculate bearing to another point in degrees (0-360)
    pub fn bearing_to(&self, other: &GeoPoint) -> f64 {
        let lat1_rad = self.latitude.to_radians();
        let lat2_rad = other.latitude.to_radians();
        let delta_lon = (other.longitude - self.longitude).to_radians();

        let x = delta_lon.sin() * lat2_rad.cos();
        let y = lat1_rad.cos() * lat2_rad.sin()
            - lat1_rad.sin() * lat2_rad.cos() * delta_lon.cos();

        let bearing = x.atan2(y).to_degrees();
        (bearing + 360.0) % 360.0
    }

    /// Calculate a destination point given bearing and distance
    pub fn destination_point(&self, bearing_degrees: f64, distance_km: f64) -> GeoPoint {
        let lat1_rad = self.latitude.to_radians();
        let lon1_rad = self.longitude.to_radians();
        let bearing_rad = bearing_degrees.to_radians();
        let angular_distance = distance_km / EARTH_RADIUS_KM;

        let lat2_rad = (lat1_rad.sin() * angular_distance.cos()
            + lat1_rad.cos() * angular_distance.sin() * bearing_rad.cos())
        .asin();

        let lon2_rad = lon1_rad
            + (bearing_rad.sin() * angular_distance.sin() * lat1_rad.cos())
                .atan2(angular_distance.cos() - lat1_rad.sin() * lat2_rad.sin());

        GeoPoint::new(lat2_rad.to_degrees(), lon2_rad.to_degrees())
    }

    /// Convert to radians (latitude, longitude)
    pub fn to_radians(&self) -> (f64, f64) {
        (self.latitude.to_radians(), self.longitude.to_radians())
    }

    /// Create from radians
    pub fn from_radians(lat_rad: f64, lon_rad: f64) -> Self {
        Self {
            latitude: lat_rad.to_degrees(),
            longitude: lon_rad.to_degrees(),
        }
    }
}

/// A rectangular bounding box defined by SW and NE corners
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoBoundingBox {
    /// Southwest corner (min lat, min lon)
    pub sw: GeoPoint,
    /// Northeast corner (max lat, max lon)
    pub ne: GeoPoint,
}

impl GeoBoundingBox {
    /// Create a new bounding box
    pub fn new(sw: GeoPoint, ne: GeoPoint) -> Self {
        Self { sw, ne }
    }

    /// Create a bounding box from center point and radius in km
    pub fn from_center_radius(center: GeoPoint, radius_km: f64) -> Self {
        // Calculate corners using destination point calculation
        let north = center.destination_point(0.0, radius_km);
        let south = center.destination_point(180.0, radius_km);
        let east = center.destination_point(90.0, radius_km);
        let west = center.destination_point(270.0, radius_km);

        Self {
            sw: GeoPoint::new(south.latitude, west.longitude),
            ne: GeoPoint::new(north.latitude, east.longitude),
        }
    }

    /// Check if a point is within this bounding box
    pub fn contains(&self, point: &GeoPoint) -> bool {
        point.latitude >= self.sw.latitude
            && point.latitude <= self.ne.latitude
            && point.longitude >= self.sw.longitude
            && point.longitude <= self.ne.longitude
    }

    /// Check if this box intersects with another
    pub fn intersects(&self, other: &GeoBoundingBox) -> bool {
        !(self.ne.latitude < other.sw.latitude
            || self.sw.latitude > other.ne.latitude
            || self.ne.longitude < other.sw.longitude
            || self.sw.longitude > other.ne.longitude)
    }

    /// Get the center point of the bounding box
    pub fn center(&self) -> GeoPoint {
        GeoPoint::new(
            (self.sw.latitude + self.ne.latitude) / 2.0,
            (self.sw.longitude + self.ne.longitude) / 2.0,
        )
    }

    /// Get width in degrees
    pub fn width(&self) -> f64 {
        self.ne.longitude - self.sw.longitude
    }

    /// Get height in degrees
    pub fn height(&self) -> f64 {
        self.ne.latitude - self.sw.latitude
    }

    /// Expand the bounding box by a margin in degrees
    pub fn expand(&self, margin: f64) -> Self {
        Self {
            sw: GeoPoint::new(self.sw.latitude - margin, self.sw.longitude - margin),
            ne: GeoPoint::new(self.ne.latitude + margin, self.ne.longitude + margin),
        }
    }
}

/// A circular region defined by center and radius
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoCircle {
    /// Center point
    pub center: GeoPoint,
    /// Radius in the specified unit
    pub radius: f64,
    /// Distance unit
    pub unit: GeoDistanceUnit,
}

/// Distance unit for geo calculations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeoDistanceUnit {
    Meters,
    Kilometers,
    Miles,
    Feet,
    NauticalMiles,
}

impl GeoDistanceUnit {
    /// Convert to kilometers
    pub fn to_km(&self, value: f64) -> f64 {
        match self {
            GeoDistanceUnit::Meters => value / 1000.0,
            GeoDistanceUnit::Kilometers => value,
            GeoDistanceUnit::Miles => value * 1.60934,
            GeoDistanceUnit::Feet => value * 0.0003048,
            GeoDistanceUnit::NauticalMiles => value * 1.852,
        }
    }

    /// Convert from kilometers
    pub fn from_km(&self, km: f64) -> f64 {
        match self {
            GeoDistanceUnit::Meters => km * 1000.0,
            GeoDistanceUnit::Kilometers => km,
            GeoDistanceUnit::Miles => km / 1.60934,
            GeoDistanceUnit::Feet => km / 0.0003048,
            GeoDistanceUnit::NauticalMiles => km / 1.852,
        }
    }
}

impl GeoCircle {
    /// Create a new circle
    pub fn new(center: GeoPoint, radius: f64, unit: GeoDistanceUnit) -> Self {
        Self { center, radius, unit }
    }

    /// Get radius in kilometers
    pub fn radius_km(&self) -> f64 {
        self.unit.to_km(self.radius)
    }

    /// Check if a point is within this circle
    pub fn contains(&self, point: &GeoPoint) -> bool {
        let distance = self.center.haversine_distance(point);
        distance <= self.radius_km()
    }

    /// Get the bounding box that contains this circle
    pub fn bounding_box(&self) -> GeoBoundingBox {
        GeoBoundingBox::from_center_radius(self.center, self.radius_km())
    }
}

/// A polygon defined by a list of vertices
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoPolygon {
    /// Vertices of the polygon (must have at least 3 points)
    pub vertices: Vec<GeoPoint>,
}

impl GeoPolygon {
    /// Create a new polygon
    pub fn new(vertices: Vec<GeoPoint>) -> Self {
        Self { vertices }
    }

    /// Check if a point is inside the polygon using ray casting algorithm
    pub fn contains(&self, point: &GeoPoint) -> bool {
        if self.vertices.len() < 3 {
            return false;
        }

        let mut inside = false;
        let n = self.vertices.len();

        let mut j = n - 1;
        for i in 0..n {
            let vi = &self.vertices[i];
            let vj = &self.vertices[j];

            if ((vi.latitude > point.latitude) != (vj.latitude > point.latitude))
                && (point.longitude
                    < (vj.longitude - vi.longitude) * (point.latitude - vi.latitude)
                        / (vj.latitude - vi.latitude)
                        + vi.longitude)
            {
                inside = !inside;
            }
            j = i;
        }

        inside
    }

    /// Get the bounding box of this polygon
    pub fn bounding_box(&self) -> GeoBoundingBox {
        if self.vertices.is_empty() {
            return GeoBoundingBox::new(GeoPoint::new(0.0, 0.0), GeoPoint::new(0.0, 0.0));
        }

        let mut min_lat = f64::MAX;
        let mut max_lat = f64::MIN;
        let mut min_lon = f64::MAX;
        let mut max_lon = f64::MIN;

        for v in &self.vertices {
            min_lat = min_lat.min(v.latitude);
            max_lat = max_lat.max(v.latitude);
            min_lon = min_lon.min(v.longitude);
            max_lon = max_lon.max(v.longitude);
        }

        GeoBoundingBox::new(
            GeoPoint::new(min_lat, min_lon),
            GeoPoint::new(max_lat, max_lon),
        )
    }

    /// Calculate the centroid of the polygon
    pub fn centroid(&self) -> GeoPoint {
        if self.vertices.is_empty() {
            return GeoPoint::new(0.0, 0.0);
        }

        let sum_lat: f64 = self.vertices.iter().map(|v| v.latitude).sum();
        let sum_lon: f64 = self.vertices.iter().map(|v| v.longitude).sum();
        let n = self.vertices.len() as f64;

        GeoPoint::new(sum_lat / n, sum_lon / n)
    }

    /// Calculate approximate area in square kilometers using Shoelace formula
    /// Note: This is an approximation that works best for small polygons
    pub fn approximate_area_sq_km(&self) -> f64 {
        if self.vertices.len() < 3 {
            return 0.0;
        }

        let mut area = 0.0;
        let n = self.vertices.len();

        for i in 0..n {
            let j = (i + 1) % n;
            let vi = &self.vertices[i];
            let vj = &self.vertices[j];

            area += vi.longitude.to_radians() * vj.latitude.to_radians();
            area -= vj.longitude.to_radians() * vi.latitude.to_radians();
        }

        (area.abs() / 2.0) * EARTH_RADIUS_KM * EARTH_RADIUS_KM
    }
}

/// Errors that can occur with geo operations
#[derive(Debug, Clone, PartialEq)]
pub enum GeoError {
    /// Invalid latitude (must be -90 to 90)
    InvalidLatitude(f64),
    /// Invalid longitude (must be -180 to 180)
    InvalidLongitude(f64),
    /// Invalid polygon (less than 3 vertices)
    InvalidPolygon,
    /// Geohash error
    InvalidGeohash(String),
}

impl std::fmt::Display for GeoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeoError::InvalidLatitude(lat) => {
                write!(f, "Invalid latitude: {}. Must be between -90 and 90", lat)
            }
            GeoError::InvalidLongitude(lon) => {
                write!(f, "Invalid longitude: {}. Must be between -180 and 180", lon)
            }
            GeoError::InvalidPolygon => {
                write!(f, "Invalid polygon: must have at least 3 vertices")
            }
            GeoError::InvalidGeohash(hash) => {
                write!(f, "Invalid geohash: {}", hash)
            }
        }
    }
}

impl std::error::Error for GeoError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distance_unit_conversion() {
        let km = GeoDistanceUnit::Kilometers;
        let miles = GeoDistanceUnit::Miles;

        assert!((km.to_km(100.0) - 100.0).abs() < 0.001);
        assert!((miles.to_km(100.0) - 160.934).abs() < 0.1);
        assert!((km.from_km(160.934) - 160.934).abs() < 0.001);
    }

    #[test]
    fn test_bearing_calculation() {
        let london = GeoPoint::new(51.5074, -0.1278);
        let paris = GeoPoint::new(48.8566, 2.3522);

        let bearing = london.bearing_to(&paris);
        // London to Paris is roughly southeast (~156 degrees)
        assert!(bearing > 140.0 && bearing < 170.0);
    }

    #[test]
    fn test_destination_point() {
        let start = GeoPoint::new(0.0, 0.0);
        let dest = start.destination_point(90.0, 111.32); // ~1 degree of longitude at equator

        assert!((dest.latitude - 0.0).abs() < 0.1);
        assert!((dest.longitude - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_polygon_area() {
        // Approximately 1km x 1km square at equator
        let polygon = GeoPolygon::new(vec![
            GeoPoint::new(0.0, 0.0),
            GeoPoint::new(0.0, 0.009), // ~1km at equator
            GeoPoint::new(0.009, 0.009),
            GeoPoint::new(0.009, 0.0),
        ]);

        let area = polygon.approximate_area_sq_km();
        // Should be roughly 1 sq km (very approximate due to projection)
        assert!(area > 0.5 && area < 2.0);
    }
}
