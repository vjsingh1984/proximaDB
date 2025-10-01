// # Vector Encoding Types
//
// Helper types for encoding collections of vectors in columnar or row-wise layouts.

use super::types::EncodedDimension;

/// **DimensionGroup** - Group of dimensions encoded together
///
/// Used in columnar encoding to group dimensions with similar characteristics
/// for more efficient compression.
///
/// # Fields
/// - `start_dim`: Starting dimension index (inclusive)
/// - `end_dim`: Ending dimension index (exclusive)
/// - `dimensions`: Encoded data for each dimension in the group
#[derive(Debug, Clone)]
pub struct DimensionGroup {
    pub start_dim: usize,
    pub end_dim: usize,
    pub dimensions: Vec<EncodedDimension>,
}

/// **ColumnarEncodedVectors** - Vectors encoded in columnar layout
///
/// Columnar layout stores all values for dimension 0, then all values for dimension 1, etc.
/// This enables better compression when dimensions have different characteristics.
///
/// # Example
/// ```
/// // 3 vectors of dimension 4:
/// // [1.0, 2.0, 3.0, 4.0]
/// // [1.1, 2.1, 3.1, 4.1]
/// // [1.2, 2.2, 3.2, 4.2]
///
/// // Columnar layout:
/// // Dim 0: [1.0, 1.1, 1.2]  <- can use Delta encoding
/// // Dim 1: [2.0, 2.1, 2.2]  <- can use Delta encoding
/// // Dim 2: [3.0, 3.1, 3.2]  <- can use Delta encoding
/// // Dim 3: [4.0, 4.1, 4.2]  <- can use Delta encoding
/// ```
///
/// # Fields
/// - `num_vectors`: Number of vectors encoded
/// - `dimension`: Dimension of each vector
/// - `dimension_groups`: Groups of encoded dimensions
#[derive(Debug, Clone)]
pub struct ColumnarEncodedVectors {
    pub num_vectors: usize,
    pub dimension: usize,
    pub dimension_groups: Vec<DimensionGroup>,
}

/// **RowWiseEncodedVectors** - Vectors encoded in row-wise layout
///
/// Row-wise layout stores complete vectors sequentially.
/// This is simpler but may have worse compression.
///
/// # Example
/// ```
/// // 3 vectors of dimension 4:
/// // Row 0: [1.0, 2.0, 3.0, 4.0]
/// // Row 1: [1.1, 2.1, 3.1, 4.1]
/// // Row 2: [1.2, 2.2, 3.2, 4.2]
/// ```
///
/// # Fields
/// - `num_vectors`: Number of vectors encoded
/// - `dimension`: Original dimension of each vector
/// - `padded_dimension`: Dimension after padding (for alignment)
/// - `encoded_vectors`: Encoded bytes for each vector
#[derive(Debug, Clone)]
pub struct RowWiseEncodedVectors {
    pub num_vectors: usize,
    pub dimension: usize,
    pub padded_dimension: usize,
    pub encoded_vectors: Vec<Vec<u8>>,
}

/// **EncodedVectors** - Unified output for vector encoding
///
/// Represents encoded vectors in either columnar or row-wise layout.
///
/// # Usage
/// ```
/// use proximadb::storage::engines::core::ops::proximaencoder::*;
///
/// let vectors = vec![
///     vec![1.0, 2.0, 3.0],
///     vec![1.1, 2.1, 3.1],
/// ];
///
/// let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 0 });
/// let encoded = encoder.encode_vectors_auto(&vectors)?;
///
/// match encoded {
///     EncodedVectors::Columnar(col) => {
///         println!("Encoded {} vectors in columnar layout", col.num_vectors);
///     }
///     EncodedVectors::RowWise(row) => {
///         println!("Encoded {} vectors in row-wise layout", row.num_vectors);
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub enum EncodedVectors {
    /// Columnar layout (dimension-major)
    Columnar(ColumnarEncodedVectors),
    /// Row-wise layout (vector-major)
    RowWise(RowWiseEncodedVectors),
}

impl EncodedVectors {
    /// Get number of vectors
    pub fn num_vectors(&self) -> usize {
        match self {
            EncodedVectors::Columnar(c) => c.num_vectors,
            EncodedVectors::RowWise(r) => r.num_vectors,
        }
    }

    /// Get dimension
    pub fn dimension(&self) -> usize {
        match self {
            EncodedVectors::Columnar(c) => c.dimension,
            EncodedVectors::RowWise(r) => r.dimension,
        }
    }

    /// Check if columnar layout
    pub fn is_columnar(&self) -> bool {
        matches!(self, EncodedVectors::Columnar(_))
    }

    /// Check if row-wise layout
    pub fn is_rowwise(&self) -> bool {
        matches!(self, EncodedVectors::RowWise(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dimension_group() {
        let group = DimensionGroup {
            start_dim: 0,
            end_dim: 10,
            dimensions: vec![],
        };
        assert_eq!(group.start_dim, 0);
        assert_eq!(group.end_dim, 10);
    }

    #[test]
    fn test_encoded_vectors_helpers() {
        let columnar = EncodedVectors::Columnar(ColumnarEncodedVectors {
            num_vectors: 100,
            dimension: 768,
            dimension_groups: vec![],
        });

        assert_eq!(columnar.num_vectors(), 100);
        assert_eq!(columnar.dimension(), 768);
        assert!(columnar.is_columnar());
        assert!(!columnar.is_rowwise());

        let rowwise = EncodedVectors::RowWise(RowWiseEncodedVectors {
            num_vectors: 50,
            dimension: 128,
            padded_dimension: 128,
            encoded_vectors: vec![],
        });

        assert_eq!(rowwise.num_vectors(), 50);
        assert_eq!(rowwise.dimension(), 128);
        assert!(!rowwise.is_columnar());
        assert!(rowwise.is_rowwise());
    }
}
