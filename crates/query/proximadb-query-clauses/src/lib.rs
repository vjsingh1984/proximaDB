use proximadb_query_filter::{FilterOperator, FilterValue};

/// Generic filter that can apply to any record-oriented query result.
#[derive(Debug, Clone)]
pub struct Filter {
    /// Field path.
    pub field: String,
    /// Comparison operator.
    pub operator: FilterOperator,
    /// Comparison value.
    pub value: FilterValue,
}

/// Generic order-by specification for query results.
#[derive(Debug, Clone)]
pub struct OrderBy {
    /// Field to order by.
    pub field: String,
    /// Ascending or descending.
    pub ascending: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_holds_structured_value() {
        let filter = Filter {
            field: "metadata.category".to_string(),
            operator: FilterOperator::In,
            value: FilterValue::Array(vec![
                FilterValue::String("books".to_string()),
                FilterValue::String("music".to_string()),
            ]),
        };

        assert_eq!(filter.field, "metadata.category");
        assert_eq!(filter.operator, FilterOperator::In);
        assert!(matches!(filter.value, FilterValue::Array(_)));
    }

    #[test]
    fn order_by_direction_is_explicit() {
        let ascending = OrderBy {
            field: "score".to_string(),
            ascending: true,
        };
        let descending = OrderBy {
            field: "created_at".to_string(),
            ascending: false,
        };

        assert!(ascending.ascending);
        assert!(!descending.ascending);
    }
}
