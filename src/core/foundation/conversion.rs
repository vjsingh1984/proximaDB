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

// Convenience type aliases
pub type ConversionResult<T> = Result<T, String>;
pub type ConversionError = String;