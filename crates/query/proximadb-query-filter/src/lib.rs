/// Shared comparison operators used by cross-model query IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterOperator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    NotIn,
    Contains,
    StartsWith,
    EndsWith,
    Exists,
    Type,
}

/// Shared literal/filter values used by cross-model query IR.
#[derive(Debug, Clone)]
pub enum FilterValue {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
    Array(Vec<FilterValue>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_operator_equality_is_structural() {
        assert_eq!(FilterOperator::Eq, FilterOperator::Eq);
        assert_ne!(FilterOperator::Eq, FilterOperator::Ne);
    }

    #[test]
    fn filter_value_supports_nested_arrays() {
        let value = FilterValue::Array(vec![
            FilterValue::String("alpha".to_string()),
            FilterValue::Array(vec![FilterValue::Bool(true), FilterValue::Null]),
        ]);

        match value {
            FilterValue::Array(values) => {
                assert_eq!(values.len(), 2);
                assert!(matches!(values[0], FilterValue::String(_)));
                assert!(matches!(values[1], FilterValue::Array(_)));
            }
            other => panic!("expected array value, got {:?}", other),
        }
    }
}
