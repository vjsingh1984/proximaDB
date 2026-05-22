//! Conversion traits and utilities for schema type transformation

/// Trait for converting external types to unified types
pub trait ToUnified<T> {
    /// Convert to unified type
    fn to_unified(self) -> T;

    /// Try to convert to unified type with error handling
    fn try_to_unified(self) -> Result<T, String>
    where
        Self: Sized,
    {
        Ok(self.to_unified())
    }
}

/// Trait for converting unified types to external types
pub trait FromUnified<T> {
    /// Convert from unified type
    fn from_unified(unified: T) -> Self;

    /// Try to convert from unified type with error handling
    fn try_from_unified(unified: T) -> Result<Self, String>
    where
        Self: Sized,
    {
        Ok(Self::from_unified(unified))
    }
}

// Note: impl_conversion macro removed with unified_types.rs migration
// Use standard From/Into traits instead

/// Utility for batch conversion of collections
pub fn convert_vec<T, U>(items: Vec<T>) -> Vec<U>
where
    T: ToUnified<U>,
{
    items.into_iter().map(|item| item.to_unified()).collect()
}

/// Utility for converting Option types
pub fn convert_option<T, U>(item: Option<T>) -> Option<U>
where
    T: ToUnified<U>,
{
    item.map(|i| i.to_unified())
}

/// Utility for converting Result types
pub fn convert_result<T, U, E>(result: Result<T, E>) -> Result<U, E>
where
    T: ToUnified<U>,
{
    result.map(|item| item.to_unified())
}

/// Result type for type conversion operations
pub type ConversionResult<T> = Result<T, String>;
/// Error type for conversion failures (descriptive string)
pub type ConversionError = String;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct External(u32);

    #[derive(Debug, Clone, PartialEq)]
    struct Unified(u32);

    impl ToUnified<Unified> for External {
        fn to_unified(self) -> Unified {
            Unified(self.0)
        }
    }

    impl FromUnified<Unified> for External {
        fn from_unified(unified: Unified) -> Self {
            External(unified.0)
        }
    }

    #[test]
    fn conversion_traits_and_collection_helpers_use_existing_type_contracts() {
        assert_eq!(External(1).to_unified(), Unified(1));
        assert_eq!(External(2).try_to_unified().unwrap(), Unified(2));
        assert_eq!(External::from_unified(Unified(3)), External(3));
        assert_eq!(External::try_from_unified(Unified(4)).unwrap(), External(4));

        assert_eq!(
            convert_vec::<External, Unified>(vec![External(5), External(6)]),
            vec![Unified(5), Unified(6)]
        );
        assert_eq!(
            convert_option::<External, Unified>(Some(External(7))),
            Some(Unified(7))
        );
        assert_eq!(convert_option::<External, Unified>(None), None);
        assert_eq!(
            convert_result::<External, Unified, ConversionError>(Ok(External(8))).unwrap(),
            Unified(8)
        );
        let failed: ConversionResult<External> = Err("bad".to_string());
        assert_eq!(
            convert_result::<External, Unified, ConversionError>(failed).unwrap_err(),
            "bad"
        );
    }
}
